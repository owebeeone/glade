//! The glade node server (P1) — ties the store, router, and echo provider over
//! the websocket carrier. One connection per session; frames dispatched:
//! `Subscribe` registers interest and ships the resume gap, `Ops` appends +
//! fans out (minus origin) and answers each op with its status, and the directed
//! exchange/channel frames hit the echo provider. The resume/convergence and
//! verification logic all live in the carrier-free modules; this is the glue.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use glade_wire::cbor;
use glade_wire::generated::{Error, ErrorCode, Head, Heads, Op, Ops, StreamHeads, Welcome};

use crate::echo::Echo;
use crate::frame::Frame;
use crate::mesh::Mesh;
use crate::registry::HOME;
use crate::router::{Router, SessionId};
use crate::session::{error_frame, heads_map, missing_for, op_status};
use crate::store::{Append, Store, StoreError};
use crate::sysdata::SystemSnapshot;
use crate::tasks::{Owners, Site, Tasks};
use crate::ws::{self, Msg};

pub(crate) struct Shared {
    pub(crate) store: Mutex<Store>,
    pub(crate) router: Mutex<Router>,
    pub(crate) out: Mutex<BTreeMap<SessionId, mpsc::UnboundedSender<Vec<u8>>>>,
    pub(crate) next: AtomicU64,
    /// The peer mesh (accept loop + links + claim routing), set once by
    /// `enable_mesh`. `None` = the legacy client-serve node: every subscribe is
    /// served locally, exactly the pre-mesh contract.
    pub(crate) mesh: OnceLock<Arc<Mesh>>,
    /// Attached authority providers for DECLARED exchange surfaces:
    /// `(share, glade_id) -> session`. Exchanges route here, never to a replica
    /// (the fan-out asymmetry, `exchange.rs`).
    pub(crate) providers: Mutex<BTreeMap<(String, String), SessionId>>,
    /// In-flight exchanges: correlation id -> the requesting session, so a
    /// provider's `ExchangeRes` routes back 1:1 (never folded, never fanned).
    pub(crate) pending: Mutex<BTreeMap<String, SessionId>>,
    /// The adopted directory-write authority (`claims.rs`), set once by
    /// `adopt_boot`. `None` = a store-only node: it serves and replicates but
    /// never mints directory records of its own.
    pub(crate) dir: OnceLock<crate::claims::DirState>,
    /// Session -> bound principal (GLP-0006 P0.S7): a session that sent a
    /// Hello naming a principal is BOUND to it — the attribution seam
    /// suppliers read (P1). Sessions absent here keep origin-as-identity.
    pub(crate) principals: Mutex<BTreeMap<SessionId, String>>,
    /// Where every task this node spawns goes (plan Step 3.3, `tasks.rs`):
    /// detached, unless the assembled root's lifecycle owns them.
    pub(crate) tasks: Tasks,
}

/// A glade node bound to a store directory.
pub struct Server {
    pub(crate) shared: Arc<Shared>,
}

impl Server {
    pub fn open(root: impl Into<PathBuf>) -> std::io::Result<Server> {
        let store = Store::open(root).map_err(to_io)?;
        Ok(Server {
            shared: Arc::new(Shared {
                store: Mutex::new(store),
                router: Mutex::new(Router::new()),
                out: Mutex::new(BTreeMap::new()),
                next: AtomicU64::new(1),
                mesh: OnceLock::new(),
                providers: Mutex::new(BTreeMap::new()),
                pending: Mutex::new(BTreeMap::new()),
                dir: OnceLock::new(),
                principals: Mutex::new(BTreeMap::new()),
                tasks: Tasks::unowned(),
            }),
        })
    }

    /// Hand every task this node spawns from here on to `owners` (plan Step
    /// 3.3): the assembled root's lifecycle, before anything is spawned.
    pub(crate) fn own_tasks(&self, owners: Owners) -> std::io::Result<()> {
        self.shared.tasks.own(owners)
    }

    /// Seed the live replica with a boot registry snapshot: the home-share
    /// records land in the SAME store the subscribe path serves, so
    /// `dir.workspaces` is an ORDINARY share read the ordinary way (GDL-038) —
    /// no privileged read path, no registry RPC. Idempotent (re-seeding the
    /// same ops is a no-op); returns how many ops were newly appended.
    pub async fn seed_registry(&self, snap: &SystemSnapshot) -> usize {
        let mut store = self.shared.store.lock().await;
        let mut appended = 0usize;
        for bytes in &snap.records {
            let op = Op::from_cbor(&cbor::decode(bytes));
            if matches!(store.append(op), Ok(Append::Appended)) {
                appended += 1;
            }
        }
        appended
    }

    /// Accept connections until the listener errors.
    pub async fn run(self, listener: TcpListener) -> std::io::Result<()> {
        accept_clients(&self.shared, &listener).await
    }
}

/// `Server::run`'s loop: one client session per accepted connection, until
/// the listener errors. The assembled root's `Sessions` runs it too.
pub(crate) async fn accept_clients(
    shared: &Arc<Shared>,
    listener: &TcpListener,
) -> std::io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let session = shared.clone();
        shared.tasks.spawn(Site::ClientSession, async move {
            let _ = handle(session, stream).await;
        });
    }
}

pub(crate) async fn send(shared: &Arc<Shared>, sid: SessionId, frame: &Frame) {
    let tx = shared.out.lock().await.get(&sid).cloned();
    if let Some(tx) = tx {
        let _ = tx.send(frame.to_bytes());
    }
}

/// The answer to a client's op on the home share (ruling H-R3, plan Step 4.3):
/// the node's answer to any refused op, the op's status (R1), here under the
/// wire's `Unauthorized` code.
fn home_refused(op: &Op) -> Frame {
    let message = format!("refused: only the node writes the {HOME} share (H-R3)");
    op_status(op, ErrorCode::Unauthorized, message)
}

async fn handle(shared: Arc<Shared>, stream: TcpStream) -> std::io::Result<()> {
    let (mut reader, writer) = ws::accept(stream).await?;
    let sid = shared.next.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    shared.out.lock().await.insert(sid, tx);

    // writer task: drain this session's outbound onto the socket.
    let wtask = shared.tasks.spawn(Site::ClientWriter, async move {
        while let Some(bytes) = rx.recv().await {
            if writer.send_binary(&bytes).await.is_err() {
                break;
            }
        }
    });

    let mut echo = Echo::new();
    // resume vectors the client has announced, or sent and the node holds
    // (R3), per zone-surface (share, glade_id, key) -> origin -> seq.
    let mut client_heads: BTreeMap<(String, String, Vec<u8>), BTreeMap<String, i64>> = BTreeMap::new();

    loop {
        let bytes = match reader.read().await {
            Ok(Msg::Binary(b)) => b,
            _ => break, // close or error
        };
        let frame = match Frame::from_bytes(&bytes) {
            Ok(f) => f,
            Err(_) => continue,
        };

        // directed channel frames -> echo provider (not replicated). Exchange
        // frames route through `exchange.rs` (declared surfaces reach the
        // authority; undeclared ids keep this same echo, inside handle_request).
        if matches!(
            frame,
            Frame::ChannelOpen(_) | Frame::ChannelData(_) | Frame::ChannelClose(_)
        ) {
            for out in echo.handle(&frame) {
                send(&shared, sid, &out).await;
            }
            continue;
        }

        match frame {
            Frame::ExchangeReq(req) => {
                crate::exchange::handle_request(&shared, sid, req, &mut echo).await;
            }
            Frame::ExchangeRes(res) => {
                // an attached authority provider answering: 1:1 by corr.
                crate::exchange::handle_response(&shared, res).await;
            }
            Frame::Hello(h) => {
                for sh in &h.heads {
                    let m = client_heads.entry((sh.share.clone(), sh.glade_id.clone(), sh.key.clone())).or_default();
                    for hd in &sh.heads {
                        m.insert(hd.origin.clone(), hd.seq);
                    }
                }
                // Principals minimal (P0.S7): a Hello naming a principal BINDS
                // the session to it, and an unknown principal auto-appends a
                // minimal dir.principals record — identity as data, nothing
                // enforced. No principal = origin-as-identity, unchanged.
                if let Some(p) = h.principal.as_deref().filter(|p| !p.is_empty()) {
                    shared.principals.lock().await.insert(sid, p.to_string());
                    crate::claims::note_principal(&shared, p).await;
                }
                send(&shared, sid, &Frame::Welcome(Welcome { session: h.session, protocol: 1, heads: vec![] })).await;
            }
            Frame::Subscribe(s) => {
                // A subscription is to one zone-surface (share, glade_id, key);
                // absent key = the commons zone. The C2 routing decision picks
                // where it is served (mesh-less nodes are always Local — the
                // legacy contract, unchanged).
                let key = s.key.clone().unwrap_or_default();
                // A SUBSCRIBE to a DECLARED exchange surface is an authority
                // provider attaching (trace C4) — directed routing, no replica.
                let declared = {
                    let st = shared.store.lock().await;
                    crate::exchange::declared_exchange(&st, &s.glade_id)
                };
                if declared {
                    crate::exchange::attach_provider(&shared, sid, &s.share, &s.glade_id, key).await;
                    continue;
                }
                match crate::mesh::route_subscribe(&shared, &s.share).await {
                    crate::mesh::Route::Absent(reason) => {
                        // The trace's STATUS step (E5): absence is data with a
                        // reason, never silence — and the session stays usable.
                        let status = Frame::Error(Error {
                            code: ErrorCode::UnknownShare,
                            message: reason,
                            share: Some(s.share.clone()),
                            glade_id: Some(s.glade_id.clone()),
                            corr: None,
                        });
                        send(&shared, sid, &status).await;
                    }
                    route => {
                        // Local AND Forward both register + ack + ship the gap
                        // from the LOCAL replica (the replica serves the reads);
                        // Forward additionally routes the interest to the
                        // claim holder, whose ops arrive and fan out here.
                        shared.router.lock().await.subscribe(sid, &s.share, &s.glade_id, &key);
                        let their = client_heads.get(&(s.share.clone(), s.glade_id.clone(), key.clone())).cloned().unwrap_or_default();
                        let (server_heads, gap) = {
                            let st = shared.store.lock().await;
                            (heads_map(&st, &s.share, &s.glade_id, &key), missing_for(&st, &s.share, &s.glade_id, &key, &their))
                        };
                        let ack = Frame::Heads(Heads {
                            streams: vec![StreamHeads {
                                share: s.share.clone(),
                                glade_id: s.glade_id.clone(),
                                key: key.clone(),
                                heads: server_heads
                                    .iter()
                                    .map(|(o, sq)| Head { origin: o.clone(), seq: *sq, hash: None })
                                    .collect(),
                            }],
                        });
                        send(&shared, sid, &ack).await;
                        if !gap.is_empty() {
                            send(&shared, sid, &Frame::Ops(Ops { ops: gap, pri: None })).await;
                        }
                        if let crate::mesh::Route::Forward(peer) = route {
                            crate::mesh::forward_interest(&shared, peer, s.share.clone(), s.glade_id.clone(), key.clone()).await;
                        }
                    }
                }
            }
            Frame::Ops(ops) => {
                // Each op gets one status, in order (R1): `Ok` once the node
                // holds it, and for an appended op only once its fan-out is
                // queued (R2); otherwise the refusal. Only an op the node
                // holds joins the session's heads (R3). Client-writes plan
                // Step 2.1; GladeSubstrateV1 §6, "Session answers".
                for op in ops.ops {
                    // H-R3: a client submits intent, and appends no record with
                    // a privileged effect. Every home record kind has one, and
                    // the node writes its own (`claims::publish`), so a
                    // client's op on home is refused before any of it is kept
                    // (plan Step 4.3, part 1). The frame's other ops go on.
                    if op.share == HOME {
                        send(&shared, sid, &home_refused(&op)).await;
                        continue;
                    }
                    let res = shared.store.lock().await.append(op.clone());
                    let status = match res {
                        Ok(Append::Appended) => {
                            hold(&mut client_heads, &op);
                            let status = op_status(&op, ErrorCode::Ok, "appended".into());
                            let targets = shared.router.lock().await.route(
                                sid,
                                &op.share,
                                &op.glade_id,
                                &op.key,
                            );
                            let frame = Frame::Ops(Ops {
                                ops: vec![op],
                                pri: None,
                            });
                            for t in targets {
                                send(&shared, t, &frame).await;
                            }
                            status
                        }
                        Ok(Append::Duplicate) => {
                            hold(&mut client_heads, &op);
                            op_status(&op, ErrorCode::Ok, "already held".into())
                        }
                        // Taken as seen, not held (R2): the session's heads
                        // stay, and every op of its chain is above it.
                        Ok(Append::BelowRetained) => {
                            let message = format!(
                                "({},{}) is below the first seq its chain holds",
                                op.origin, op.seq
                            );
                            op_status(&op, ErrorCode::Retention, message)
                        }
                        Err(e) => error_frame(&e, &op),
                    };
                    send(&shared, sid, &status).await;
                }
            }
            _ => {}
        }
    }

    shared.out.lock().await.remove(&sid);
    shared.router.lock().await.unsubscribe_all(sid);
    // a departing authority provider releases its exchange surfaces.
    shared.providers.lock().await.retain(|_, v| *v != sid);
    // the principal binding is session-scoped; the RECORD it minted stays.
    shared.principals.lock().await.remove(&sid);
    wtask.abort();
    Ok(())
}

/// R3: the session's heads take the seq of an op the node holds, and never
/// fall, so a repeat of a lower seq leaves them where they are.
fn hold(heads: &mut BTreeMap<(String, String, Vec<u8>), BTreeMap<String, i64>>, op: &Op) {
    let zone = (op.share.clone(), op.glade_id.clone(), op.key.clone());
    let head = heads
        .entry(zone)
        .or_default()
        .entry(op.origin.clone())
        .or_insert(op.seq);
    *head = (*head).max(op.seq);
}

fn to_io(e: StoreError) -> std::io::Error {
    match e {
        StoreError::Io(e) => e,
        other => std::io::Error::new(std::io::ErrorKind::Other, format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{G_GRANTS, HOME};
    use crate::sysdata::CapabilityGrant;
    use glade_wire::generated::{ExchangeReq, Op, Ops, Shape, Subscribe};

    fn op(origin: &str, seq: i64, payload: &[u8]) -> Op {
        Op {
            share: "sh".into(),
            glade_id: "g".into(),
            key: vec![],
            origin: origin.into(),
            seq,
            prev: None,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }
    fn subscribe() -> Vec<u8> {
        Frame::Subscribe(Subscribe { share: "sh".into(), glade_id: "g".into(), key: None, from: None }).to_bytes()
    }
    fn subscribe_key(key: Option<Vec<u8>>) -> Vec<u8> {
        Frame::Subscribe(Subscribe { share: "sh".into(), glade_id: "g".into(), key, from: None }).to_bytes()
    }
    fn keyed_op(origin: &str, seq: i64, key: &[u8], payload: &[u8]) -> Op {
        Op { key: key.to_vec(), ..op(origin, seq, payload) }
    }
    fn ops_frame(o: Op) -> Vec<u8> {
        Frame::Ops(Ops { ops: vec![o], pri: None }).to_bytes()
    }
    async fn recv(r: &mut ws::WsReader) -> Frame {
        match r.read().await.unwrap() {
            Msg::Binary(b) => Frame::from_bytes(&b).unwrap(),
            Msg::Close => panic!("unexpected close"),
        }
    }
    /// The next frame within 5 s: a status that never comes fails the test
    /// rather than hanging it.
    async fn next(r: &mut ws::WsReader, what: &str) -> Frame {
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), r.read());
        match read
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .unwrap()
        {
            Msg::Binary(b) => Frame::from_bytes(&b).unwrap(),
            Msg::Close => panic!("unexpected close waiting for {what}"),
        }
    }
    /// The op's hash in lower-case hex, the `corr` R1 promises, written out
    /// here rather than taken from the code under test.
    fn corr(op: &Op) -> String {
        crate::chain::op_hash(op)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    /// What a status says: its code, its `corr`, its share and its stream.
    type Said = (ErrorCode, Option<String>, Option<String>, Option<String>);
    fn said(e: &Error) -> Said {
        (e.code, e.corr.clone(), e.share.clone(), e.glade_id.clone())
    }
    /// What the status of `op` must say under `code`: R1 names the op.
    fn status_for(op: &Op, code: ErrorCode) -> Said {
        (
            code,
            Some(corr(op)),
            Some(op.share.clone()),
            Some(op.glade_id.clone()),
        )
    }

    /// §11 localhost role end-to-end over a real websocket: two clients exchange
    /// an op (routing), a late joiner resumes the op (gap-ship), and the echo
    /// provider round-trips an exchange.
    #[tokio::test]
    async fn end_to_end_over_websocket() {
        let dir = std::env::temp_dir().join("glade-server-e2e");
        let _ = std::fs::remove_dir_all(&dir);
        let server = Server::open(&dir).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));

        let (mut r1, w1) = ws::connect("127.0.0.1", port).await.unwrap();
        let (mut r2, w2) = ws::connect("127.0.0.1", port).await.unwrap();

        // both subscribe; the Heads ack confirms registration (ordering point)
        w1.send_binary(&subscribe()).await.unwrap();
        assert!(matches!(recv(&mut r1).await, Frame::Heads(_)));
        w2.send_binary(&subscribe()).await.unwrap();
        assert!(matches!(recv(&mut r2).await, Frame::Heads(_)));

        // client 1 writes an op; client 2 receives it (fan-out minus origin)
        let hello = op("a", 0, b"hello");
        w1.send_binary(&ops_frame(hello.clone())).await.unwrap();
        // client 1 is answered: its op is held (R1)
        match next(&mut r1, "client 1's status").await {
            Frame::Error(e) => assert_eq!(said(&e), status_for(&hello, ErrorCode::Ok)),
            other => panic!("client 1 expected its op's status, got {other:?}"),
        }
        match recv(&mut r2).await {
            Frame::Ops(o) => assert_eq!(o.ops[0].payload, b"hello"),
            other => panic!("client 2 expected Ops, got {other:?}"),
        }

        // a late joiner subscribes and resumes the op from history (gap-ship)
        let (mut r3, w3) = ws::connect("127.0.0.1", port).await.unwrap();
        w3.send_binary(&subscribe()).await.unwrap();
        assert!(matches!(recv(&mut r3).await, Frame::Heads(_)));
        match recv(&mut r3).await {
            Frame::Ops(o) => assert_eq!(o.ops[0].payload, b"hello"),
            other => panic!("client 3 expected resume Ops, got {other:?}"),
        }

        // echo provider: exchange round-trips with its correlation id
        w1.send_binary(
            &Frame::ExchangeReq(ExchangeReq {
                share: "sh".into(),
                glade_id: "echo".into(),
                corr: "x1".into(),
                payload: b"ping".to_vec(),
            })
            .to_bytes(),
        )
        .await
        .unwrap();
        match recv(&mut r1).await {
            Frame::ExchangeRes(res) => {
                assert_eq!(res.corr, "x1");
                assert_eq!(res.payload.as_deref(), Some(b"ping".as_slice()));
            }
            other => panic!("client 1 expected ExchangeRes, got {other:?}"),
        }
    }

    /// Privacy by keying, end-to-end: a private-zone op (key `self:p`) is fanned
    /// out only to that zone's subscriber; the commons subscriber receives just
    /// the commons op, never the private one (GladeZones.md).
    #[tokio::test]
    async fn private_zone_isolated_from_commons() {
        let dir = std::env::temp_dir().join("glade-server-zones");
        let _ = std::fs::remove_dir_all(&dir);
        let server = Server::open(&dir).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));

        let (mut rc, wc) = ws::connect("127.0.0.1", port).await.unwrap(); // commons subscriber
        let (mut rp, wp) = ws::connect("127.0.0.1", port).await.unwrap(); // private subscriber
        let (_rw, ww) = ws::connect("127.0.0.1", port).await.unwrap(); // writer

        wc.send_binary(&subscribe_key(None)).await.unwrap(); // commons
        assert!(matches!(recv(&mut rc).await, Frame::Heads(_)));
        wp.send_binary(&subscribe_key(Some(b"self:p".to_vec()))).await.unwrap();
        assert!(matches!(recv(&mut rp).await, Frame::Heads(_)));

        // writer emits a private op then a commons op (independent chains, both seq 0)
        ww.send_binary(&ops_frame(keyed_op("w", 0, b"self:p", b"secret"))).await.unwrap();
        ww.send_binary(&ops_frame(keyed_op("w", 0, b"", b"public"))).await.unwrap();

        // the private subscriber sees the secret; the commons subscriber's only
        // delivered op is the public one — the secret never crosses the zone.
        match recv(&mut rp).await {
            Frame::Ops(o) => assert_eq!(o.ops[0].payload, b"secret"),
            other => panic!("private subscriber expected the private op, got {other:?}"),
        }
        match recv(&mut rc).await {
            Frame::Ops(o) => assert_eq!(o.ops[0].payload, b"public"),
            other => panic!("commons subscriber expected only the commons op, got {other:?}"),
        }
    }

    /// A node serving websockets on an OS-assigned port over a fresh store
    /// named `name`: its shared state, to read back what it stored, and its
    /// port.
    async fn serving(name: &str) -> (Arc<Shared>, u16) {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let server = Server::open(&dir).unwrap();
        let shared = server.shared.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));
        (shared, port)
    }

    /// Subscribe to `sh/g` and read up to its ack. A session handles its
    /// frames in order, so every op sent before the subscribe has been handled
    /// by then. Returns the `Error` frames, the ops' statuses, that arrived
    /// before the ack.
    async fn errors_before_ack(r: &mut ws::WsReader, w: &ws::WsWriter) -> Vec<Error> {
        w.send_binary(&subscribe()).await.unwrap();
        let mut errors = Vec::new();
        loop {
            match recv(r).await {
                Frame::Error(e) => errors.push(e),
                Frame::Heads(_) => return errors,
                other => panic!("expected errors, then the ack, got {other:?}"),
            }
        }
    }

    /// Ruling H-R3 (plan Step 4.3, part 1): a client submits intent, and it
    /// never appends a record with a privileged effect. Every kind the home
    /// share holds has one: grants, claims, declarations, identity. A client's
    /// op on `home`, here a forged grant, is refused with `Error{Unauthorized}`
    /// naming the share, the stream and, as its `corr`, the op's hash (R1),
    /// and it is never stored, so nothing that reads the served store folds
    /// it. Proves the websocket client path only: a peer's push and pull
    /// still ingest home ops until Step 4.1b verifies directory records, and
    /// no grant is checked anywhere.
    #[tokio::test]
    async fn a_client_op_on_home_is_refused_and_never_stored() {
        let (shared, port) = serving("glade-server-home-refused").await;
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();

        let grant = CapabilityGrant {
            principal: "mallory".into(),
            share: "ws-razel".into(),
            verbs: vec!["read.*".into()],
        };
        let forged = Op {
            share: HOME.into(),
            glade_id: G_GRANTS.into(),
            shape: Shape::Log,
            payload: cbor::encode(&grant.to_cbor()),
            ..op("mallory", 0, b"")
        };
        w.send_binary(&ops_frame(forged.clone())).await.unwrap();
        let errors = errors_before_ack(&mut r, &w).await;

        let st = shared.store.lock().await;
        let home_zones: Vec<_> = st
            .zones()
            .into_iter()
            .filter(|zone| zone.0 == HOME)
            .collect();
        assert!(
            home_zones.is_empty(),
            "the client's op on home was stored: {home_zones:?}"
        );
        assert_eq!(errors.len(), 1, "one refusal, for the one op: {errors:?}");
        let refusal = &errors[0];
        assert_eq!(refusal.code, ErrorCode::Unauthorized);
        assert_eq!(refusal.share.as_deref(), Some(HOME));
        assert_eq!(refusal.glade_id.as_deref(), Some(G_GRANTS));
        assert_eq!(
            refusal.corr,
            Some(corr(&forged)),
            "the refusal names the op by its hash"
        );
    }

    /// The refusal names the home share exactly, and no other. A client's ops
    /// on an ordinary share, and on a share whose name only begins with
    /// `home`, are stored as before, and each is answered `Ok` (R1). Proves
    /// the append on the websocket client path only.
    #[tokio::test]
    async fn a_client_op_on_any_other_share_still_lands() {
        let (shared, port) = serving("glade-server-other-share").await;
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();

        let plain = op("w", 0, b"plain");
        let lookalike = Op {
            share: "home-notes".into(),
            ..op("w", 0, b"lookalike")
        };
        let frame = Frame::Ops(Ops {
            ops: vec![plain.clone(), lookalike.clone()],
            pri: None,
        });
        w.send_binary(&frame.to_bytes()).await.unwrap();
        let statuses = errors_before_ack(&mut r, &w).await;

        let answers: Vec<Said> = statuses.iter().map(said).collect();
        let held = [
            status_for(&plain, ErrorCode::Ok),
            status_for(&lookalike, ErrorCode::Ok),
        ];
        assert_eq!(
            answers, held,
            "no op was refused, and each is answered in order"
        );
        let st = shared.store.lock().await;
        let stored = |share: &str| st.scan(share, "g", &[], "w", i64::MIN).len();
        assert_eq!(stored("sh"), 1, "the op on an ordinary share is stored");
        assert_eq!(
            stored("home-notes"),
            1,
            "the op on a share named like home is stored"
        );
    }

    /// R1 (client-writes plan Step 2.1): each op in a client's `Ops` frame
    /// gets one status, an `Error` frame, in the order of the ops. Its `corr`
    /// is the op's hash in lower-case hex, and its share and stream are the
    /// op's. The code is `Ok` when the node holds the op, appended now or
    /// held byte for byte, and otherwise the refusal's. Proves the websocket
    /// client path only: the peer paths answer nothing.
    #[tokio::test]
    async fn every_client_op_gets_one_status_named_by_its_hash() {
        let (_shared, port) = serving("glade-server-statuses").await;
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();

        let new = op("w", 0, b"zero");
        let fork = op("w", 0, b"another zero");
        let past_gap = op("w", 5, b"five");
        let on_home = Op {
            share: HOME.into(),
            glade_id: G_GRANTS.into(),
            ..op("w", 0, b"home")
        };
        let ops = vec![
            new.clone(),
            new.clone(),
            fork.clone(),
            past_gap.clone(),
            on_home.clone(),
        ];
        w.send_binary(&Frame::Ops(Ops { ops, pri: None }).to_bytes())
            .await
            .unwrap();
        let statuses = errors_before_ack(&mut r, &w).await;

        let answers: Vec<Said> = statuses.iter().map(said).collect();
        let want = [
            status_for(&new, ErrorCode::Ok),
            status_for(&new, ErrorCode::Ok),
            status_for(&fork, ErrorCode::Equivocation),
            status_for(&past_gap, ErrorCode::Protocol),
            status_for(&on_home, ErrorCode::Unauthorized),
        ];
        assert_eq!(
            answers, want,
            "one status per op, in order, each naming its op"
        );
    }

    /// Subscribe to `sh/g`: the statuses that arrived before its ack, and the
    /// gap after it. A second subscribe, to a zone with no ops, bounds the
    /// gap, since its ack follows the gap on the session's one queue.
    async fn statuses_and_gap(r: &mut ws::WsReader, w: &ws::WsWriter) -> (Vec<Error>, Vec<Op>) {
        let statuses = errors_before_ack(r, w).await;
        let bound = Subscribe {
            share: "sh".into(),
            glade_id: "bound".into(),
            key: None,
            from: None,
        };
        w.send_binary(&Frame::Subscribe(bound).to_bytes())
            .await
            .unwrap();
        let mut gap = Vec::new();
        loop {
            match next(r, "the gap, then the bound's ack").await {
                Frame::Ops(o) => gap.extend(o.ops),
                Frame::Heads(_) => return (statuses, gap),
                other => panic!("expected the gap, then an ack, got {other:?}"),
            }
        }
    }

    /// R3 (client-writes plan Step 2.1, its finding F1): the node adds an
    /// op's seq to the session's heads only once it holds the op, and keeps
    /// the highest. So a refused op is not held by its sender: session 2's
    /// different `(w, 0)` is refused, and its subscribe ships the node's own
    /// `(w, 0)`, the op it needs in order to recover. And a session that
    /// repeats a lower seq keeps its head, so its own later op is not shipped
    /// back to it.
    #[tokio::test]
    async fn a_refused_op_is_not_held_by_its_sender() {
        let (_shared, port) = serving("glade-server-refused-not-held").await;
        let (mut r1, w1) = ws::connect("127.0.0.1", port).await.unwrap();
        let (mut r2, w2) = ws::connect("127.0.0.1", port).await.unwrap();

        let held = op("w", 0, b"one");
        w1.send_binary(&ops_frame(held.clone())).await.unwrap();
        errors_before_ack(&mut r1, &w1).await; // session 1's op is handled
        w2.send_binary(&ops_frame(op("w", 0, b"two")))
            .await
            .unwrap();
        let (statuses, gap) = statuses_and_gap(&mut r2, &w2).await;
        let codes: Vec<ErrorCode> = statuses.iter().map(|e| e.code).collect();
        assert_eq!(
            codes,
            [ErrorCode::Equivocation],
            "session 2's op is refused"
        );
        assert_eq!(
            gap,
            std::slice::from_ref(&held),
            "session 2's gap carries the op its refused op contested"
        );

        let later = Op {
            prev: Some(crate::chain::op_hash(&held).to_vec()),
            ..op("w", 1, b"one more")
        };
        let frame = Frame::Ops(Ops {
            ops: vec![later, held],
            pri: None,
        });
        w1.send_binary(&frame.to_bytes()).await.unwrap();
        let (statuses, gap) = statuses_and_gap(&mut r1, &w1).await;
        let codes: Vec<ErrorCode> = statuses.iter().map(|e| e.code).collect();
        assert_eq!(codes, [ErrorCode::Ok, ErrorCode::Ok], "both ops are held");
        assert!(
            gap.is_empty(),
            "session 1's own op came back after it repeated a lower seq: {gap:?}"
        );
    }

    /// R2's `Retention` point (owner, 2026-09-24): an op below the first seq
    /// its chain holds is taken as seen without being held, so it is answered
    /// `Retention`, not `Ok`, and it is not stored. A chain may start at any
    /// seq; this one starts at 5. A repeat of seq 5, which the node holds, is
    /// still `Ok`. Proves the websocket client path only.
    #[tokio::test]
    async fn an_op_below_the_first_seq_held_is_answered_retention() {
        let (shared, port) = serving("glade-server-retention").await;
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();

        let first = op("w", 5, b"five");
        let below = op("w", 3, b"three");
        let ops = vec![first.clone(), below.clone(), first.clone()];
        w.send_binary(&Frame::Ops(Ops { ops, pri: None }).to_bytes())
            .await
            .unwrap();
        let statuses = errors_before_ack(&mut r, &w).await;

        let answers: Vec<Said> = statuses.iter().map(said).collect();
        let want = [
            status_for(&first, ErrorCode::Ok),
            status_for(&below, ErrorCode::Retention),
            status_for(&first, ErrorCode::Ok),
        ];
        assert_eq!(
            answers, want,
            "the op below the chain's first held seq is answered Retention"
        );
        let st = shared.store.lock().await;
        let seqs: Vec<i64> = st
            .scan("sh", "g", &[], "w", i64::MIN)
            .iter()
            .map(|o| o.seq)
            .collect();
        assert_eq!(
            seqs,
            [5],
            "the op below the chain's first held seq is not stored"
        );
    }
}
