//! The glade node server (P1) — ties the store, router, and echo provider over
//! the websocket carrier. One connection per session; frames dispatched:
//! `Subscribe` registers interest and ships the resume gap, `Ops` goes to the
//! acceptance path (`accept.rs`), which appends + fans out (minus origin) and
//! answers each op with its status (refusing a client's op on `home`, and a
//! `stream` op, which has no op path), and the directed exchange/channel
//! frames hit the echo provider. The resume/convergence and verification
//! logic all live in the carrier-free modules; this is the glue.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use glade_wire::generated::{ErrorCode, Ops, Welcome};

use glade_grant_api::{GrantPort, Holder};

use crate::accept::{accept_ops, SessionHeads};
use crate::echo::Echo;
use crate::envelope;
use crate::exchange::Pending;
use crate::frame::Frame;
use crate::grants::{names_a_node, no_principal, refusal, Policy, PolicyView, READ_SUBSCRIBE};
use crate::mesh::Mesh;
use crate::registry::HOME;
use crate::router::{Router, SessionId, Zone};
use crate::session::{ack, missing_for, refusal_frame, refused_subscribe};
use crate::store::{Append, Store, StoreError};
use crate::sysdata::SystemSnapshot;
use crate::tasks::{Owners, Site, Tasks};
use crate::ws::{self, Msg};

pub(crate) struct Shared {
    pub(crate) store: Mutex<Store>,
    /// The cut (GladeSubstrateV1 §6, R4): every fan-out holds it from its
    /// append until its ops are queued, and a subscribe holds it while it
    /// registers the session and queues its ack and gap. So each op of a zone
    /// reaches a subscriber once, after the ack. It is taken before the
    /// store's, the router's or the session table's lock, never after them.
    pub(crate) cut: Mutex<()>,
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
    /// In-flight exchanges, each under a correlation this node mints, so a
    /// provider's `ExchangeRes` routes back 1:1 to its caller (never folded,
    /// never fanned; F16, `exchange.rs`).
    pub(crate) pending: Mutex<Pending>,
    /// The adopted directory-write authority (`claims.rs`), set once by
    /// `adopt_boot`. `None` = a store-only node: it serves and replicates but
    /// never mints directory records of its own.
    pub(crate) dir: OnceLock<crate::claims::DirState>,
    /// Session -> bound principal (GLP-0006 P0.S7): a session that sent a
    /// Hello naming a principal is BOUND to it — the attribution seam
    /// suppliers read (P1). Sessions absent here keep origin-as-identity.
    pub(crate) principals: Mutex<BTreeMap<SessionId, String>>,
    /// The grant fold the serve paths check (plan Step 4.3, `grants.rs`):
    /// this node's own grants and revocations, which adoption fills from the
    /// instance's registry. Until then, and in the legacy form, it holds no
    /// fold, and every check fails closed.
    pub(crate) policy: PolicyView,
    /// The admission table (plan Step 4.3): each peer subscription stream this
    /// node serves, by the session id it is routed under, with the node it
    /// serves. The re-check pass reads it whenever the fold is replaced.
    pub(crate) admitted: Mutex<BTreeMap<SessionId, [u8; 32]>>,
    /// Whether client sessions are checked too (plan Step 4.3): the websocket
    /// path's switch, off unless [`Server::enforce_client_grants`] turns it on.
    pub(crate) client_grants: AtomicBool,
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
                cut: Mutex::new(()),
                router: Mutex::new(Router::new()),
                out: Mutex::new(BTreeMap::new()),
                next: AtomicU64::new(1),
                mesh: OnceLock::new(),
                providers: Mutex::new(BTreeMap::new()),
                pending: Mutex::new(Pending::default()),
                dir: OnceLock::new(),
                principals: Mutex::new(BTreeMap::new()),
                policy: PolicyView::unavailable(),
                admitted: Mutex::new(BTreeMap::new()),
                client_grants: AtomicBool::new(false),
                tasks: Tasks::unowned(),
            }),
        })
    }

    /// Hand every task this node spawns from here on to `owners` (plan Step
    /// 3.3): the assembled root's lifecycle, before anything is spawned.
    pub(crate) fn own_tasks(&self, owners: Owners) -> std::io::Result<()> {
        self.shared.tasks.own(owners)
    }

    /// Check client sessions against the grant fold too (plan Step 4.3): the
    /// websocket path's switch, `--enforce-client-grants`, which is off by
    /// default. A client session then reads a share other than `home` only
    /// if the principal its Hello names holds `read.subscribe` there; one
    /// that names none holds nothing. Its writes and exchanges are not
    /// checked. Call before serving.
    pub fn enforce_client_grants(&self) {
        self.shared.client_grants.store(true, Ordering::SeqCst);
    }

    /// Seed the live replica with a boot registry snapshot: the home-share
    /// records land in the SAME store the subscribe path serves, so
    /// `dir.workspaces` is an ORDINARY share read the ordinary way (GDL-038) —
    /// no privileged read path, no registry RPC. Idempotent (re-seeding the
    /// same ops is a no-op); returns how many ops were newly appended.
    pub async fn seed_registry(&self, snap: &SystemSnapshot) -> usize {
        let mut store = self.shared.store.lock().await;
        let mut appended = 0usize;
        for op in envelope::snapshot_ops(&snap.records) {
            if matches!(store.append(op), Ok(Append::Appended)) {
                appended += 1;
            }
        }
        appended
    }

    /// What the served store's `open` set aside, as the line both roots
    /// print: its `home` journals that did not verify (plan Step 4.1b).
    pub async fn set_aside(&self) -> Option<String> {
        let store = self.shared.store.lock().await;
        store.set_aside().map(ToString::to_string)
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

/// Replace the grant fold, or leave none (plan Step 4.3), and check every
/// admitted subscription again before returning. Returns the new generation.
pub(crate) async fn refresh_policy(shared: &Arc<Shared>, fold: Option<Policy>) -> u64 {
    let generation = shared.policy.replace(fold);
    recheck(shared).await;
    generation
}

/// The re-check pass (plan Step 4.3, the authorization model's §6): each
/// admitted subscription is checked against the fold as it now is, under the
/// cut, so no op fanned out after a change reaches a session it refuses. A
/// refused peer stream is told why, and leaves the router, the session table
/// and the admission table; its writer then finishes the stream, so the
/// forwarding node's forward lapses. When client sessions are checked, a
/// client's refused zone is told why and leaves the router, and its other
/// zones go on. What was sent before stays sent: revocation is forward-only.
async fn recheck(shared: &Arc<Shared>) {
    let _cut = shared.cut.lock().await;
    let peers = shared.admitted.lock().await.clone();
    let clients = shared.client_grants.load(Ordering::SeqCst);
    let entries = shared.router.lock().await.entries();
    let mut refused = Vec::new();
    for (sid, (share, glade_id, key)) in entries {
        if share == HOME {
            continue;
        }
        let verdict = match peers.get(&sid) {
            Some(node) => {
                let holder = Holder::Node(*node);
                let checked = shared.policy.check(&holder, READ_SUBSCRIBE, &share);
                checked.map_err(|denial| refusal(&holder, READ_SUBSCRIBE, &share, denial))
            }
            None if clients => client_check(shared, sid, &share).await,
            None => Ok(()),
        };
        if let Err(why) = verdict {
            refused.push((sid, (share, glade_id, key), why));
        }
    }
    for (sid, zone, why) in refused {
        refuse_subscription(shared, sid, &zone, ErrorCode::Unauthorized, why).await;
    }
}

/// Refuse session `sid` its subscription to `zone`, with `code` and `why`
/// (R6's form for a subscription refused after its ack): it leaves the zone
/// in the router and is told with a lone `Error` naming the zone and no op.
/// A peer's subscription stream carries its one zone, so a refused peer also
/// leaves the admission table and the session table, where its writer then
/// finishes the stream. The caller holds the cut. The re-check pass refuses
/// so, and so does a forwarding node relaying its claim holder's refusal
/// (F5) or telling that its forward ended (plan Step 4.6), in `mesh/route.rs`.
pub(crate) async fn refuse_subscription(
    shared: &Arc<Shared>,
    sid: SessionId,
    (share, glade_id, key): &Zone,
    code: ErrorCode,
    why: String,
) {
    let mut router = shared.router.lock().await;
    router.unsubscribe(sid, share, glade_id, key);
    drop(router);
    send(shared, sid, &refusal_frame(code, why, share, glade_id)).await;
    let peer = shared.admitted.lock().await.remove(&sid).is_some();
    if peer {
        shared.out.lock().await.remove(&sid);
    }
}

/// The grant check for a client session (plan Step 4.3): the principal its
/// Hello bound, as the client claimed it, asked for `read.subscribe` on
/// `share`. A session that bound none holds nothing. `Err` is the refusal's
/// reason.
async fn client_check(shared: &Arc<Shared>, sid: SessionId, share: &str) -> Result<(), String> {
    let Some(principal) = shared.principals.lock().await.get(&sid).cloned() else {
        return Err(no_principal(READ_SUBSCRIBE, share));
    };
    let holder = Holder::Principal(principal);
    let checked = shared.policy.check(&holder, READ_SUBSCRIBE, share);
    checked.map_err(|denial| refusal(&holder, READ_SUBSCRIBE, share, denial))
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
    let mut client_heads = SessionHeads::new();

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
                crate::exchange::handle_response(&shared, sid, res).await;
            }
            Frame::Hello(h) => {
                // R4: announced heads are taken on the client's word and, as
                // a held op's seq does (R3), they only raise the session's.
                for sh in &h.heads {
                    let zone = (sh.share.clone(), sh.glade_id.clone(), sh.key.clone());
                    let m = client_heads.entry(zone).or_default();
                    for hd in &sh.heads {
                        raise(m, &hd.origin, hd.seq);
                    }
                }
                // Principals minimal (P0.S7): a Hello naming a principal BINDS
                // the session to it, and an unknown principal auto-appends a
                // minimal dir.principals record — identity as data, taken on
                // the client's word. No principal = origin-as-identity,
                // unchanged. A name written as a node's id, 64 lower-case hex
                // digits, binds nothing: no session may claim a node (plan
                // Step 4.3).
                let named = h
                    .principal
                    .as_deref()
                    .filter(|p| !p.is_empty() && !names_a_node(p));
                if let Some(p) = named {
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
                        // R6: a `Heads` naming no zone comes first, so a client
                        // waiting on its ack resolves (plan Step 2.2, F3).
                        let code = ErrorCode::UnknownShare;
                        for frame in refused_subscribe(code, reason, &s.share, &s.glade_id) {
                            send(&shared, sid, &frame).await;
                        }
                    }
                    route => {
                        // Local AND Forward both register + ack + ship the gap
                        // from the LOCAL replica (the replica serves the reads);
                        // Forward additionally routes the interest to the
                        // claim holder, whose ops arrive and fan out here.
                        // R4: all under the cut, which every fan-out holds from
                        // its append until its ops are queued, so each op of
                        // the zone reaches this session once, after the ack.
                        let zone = (s.share.clone(), s.glade_id.clone(), key.clone());
                        let their = client_heads.get(&zone).cloned().unwrap_or_default();
                        let cut = shared.cut.lock().await;
                        // The grant check, when client sessions are checked
                        // (plan Step 4.3): refused, the refused subscribe's
                        // two frames (R6), and nothing is registered, served
                        // or forwarded. `home` is exempt.
                        if s.share != HOME && shared.client_grants.load(Ordering::SeqCst) {
                            if let Err(why) = client_check(&shared, sid, &s.share).await {
                                drop(cut);
                                let code = ErrorCode::Unauthorized;
                                for frame in refused_subscribe(code, why, &s.share, &s.glade_id) {
                                    send(&shared, sid, &frame).await;
                                }
                                continue;
                            }
                        }
                        let mut router = shared.router.lock().await;
                        router.subscribe(sid, &s.share, &s.glade_id, &key);
                        drop(router);
                        let st = shared.store.lock().await;
                        let heads = ack(&st, &s.share, &s.glade_id, &key);
                        let gap = missing_for(&st, &s.share, &s.glade_id, &key, &their);
                        drop(st);
                        send(&shared, sid, &heads).await;
                        if !gap.is_empty() {
                            let ops = Frame::Ops(Ops {
                                ops: gap,
                                pri: None,
                            });
                            send(&shared, sid, &ops).await;
                        }
                        drop(cut);
                        if let crate::mesh::Route::Forward(peer) = route {
                            crate::mesh::forward_interest(&shared, peer, s.share.clone(), s.glade_id.clone(), key.clone()).await;
                        }
                    }
                }
            }
            Frame::Ops(ops) => {
                // One status per op, in order, on this session (R1-R3), from
                // the one acceptance path (cross-node writes plan X2.1).
                accept_ops(&shared, sid, &mut client_heads, ops.ops).await;
            }
            _ => {}
        }
    }

    shared.out.lock().await.remove(&sid);
    shared.router.lock().await.unsubscribe_all(sid);
    // a departing authority provider releases its exchange surfaces.
    shared.providers.lock().await.retain(|_, v| *v != sid);
    // then the calls: those it filed are forgotten, and those pending on it
    // are answered `ok: false` (F16b).
    crate::exchange::session_ended(&shared, sid).await;
    // the principal binding is session-scoped; the RECORD it minted stays.
    shared.principals.lock().await.remove(&sid);
    wtask.abort();
    Ok(())
}

/// A session's head for `origin` in one zone rises to `seq` and never falls,
/// whether a held op (R3, `accept.rs`) or a `Hello` (R4) names it.
pub(crate) fn raise(heads: &mut BTreeMap<String, i64>, origin: &str, seq: i64) {
    let head = heads.entry(origin.into()).or_insert(seq);
    *head = (*head).max(seq);
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
    use glade_wire::cbor;
    use glade_wire::generated::{
        Error, ExchangeReq, Head, Heads, Hello, Op, Ops, Shape, StreamHeads, Subscribe,
    };

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
        errors_and_ack(r).await.0
    }

    /// The `Error` frames up to the next ack, and the ack.
    async fn errors_and_ack(r: &mut ws::WsReader) -> (Vec<Error>, Heads) {
        let mut errors = Vec::new();
        loop {
            match recv(r).await {
                Frame::Error(e) => errors.push(e),
                Frame::Heads(ack) => return (errors, ack),
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
    /// it. Proves the websocket client path only: a peer's push and pull take
    /// a home op only if it verifies (plan Step 4.1b), and no grant is
    /// checked anywhere.
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

    /// F3 (question 13; the owner's ruling of 2026-09-27): a `stream` op has
    /// no op path, a stream being a live channel that is never stored
    /// (`GladeShapeDispatch.md`). A client's is refused `Protocol`, as its
    /// status (R1), before any of it is kept: it is not stored, fanned out or
    /// held by its sender, so the same origin's value op at the same seq
    /// lands after it, and a subscriber of the zone gets that op alone. On
    /// `home`, a stream op is refused `Unauthorized`, as every client op on
    /// `home` is. One the store already holds, as a node before F3 kept it,
    /// is refused too. Proves the websocket client path only.
    #[tokio::test]
    async fn a_client_stream_op_is_refused_and_never_stored() {
        let (shared, port) = serving("glade-server-stream-refused").await;
        let (mut r_sub, w_sub) = ws::connect("127.0.0.1", port).await.unwrap();
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();
        errors_before_ack(&mut r_sub, &w_sub).await; // subscribed to sh/g

        let live = Op {
            shape: Shape::Stream,
            ..op("w", 0, b"live")
        };
        let value = op("w", 0, b"value");
        let on_home = Op {
            share: HOME.into(),
            glade_id: G_GRANTS.into(),
            ..live.clone()
        };
        let kept = Op {
            glade_id: "kept".into(),
            ..live.clone()
        };
        shared.store.lock().await.append(kept.clone()).unwrap();
        let ops = vec![live.clone(), value.clone(), on_home.clone(), kept.clone()];
        w.send_binary(&Frame::Ops(Ops { ops, pri: None }).to_bytes())
            .await
            .unwrap();
        let statuses = errors_before_ack(&mut r, &w).await;

        let answers: Vec<Said> = statuses.iter().map(said).collect();
        let want = [
            status_for(&live, ErrorCode::Protocol),
            status_for(&value, ErrorCode::Ok),
            status_for(&on_home, ErrorCode::Unauthorized),
            status_for(&kept, ErrorCode::Protocol),
        ];
        assert_eq!(answers, want, "each stream op refused, in order");
        let why = "refused: stream has no op path; a stream is a live channel, never stored";
        assert_eq!(statuses[0].message, why);
        let st = shared.store.lock().await;
        let stored = st.scan("sh", "g", &[], "w", i64::MIN);
        drop(st);
        let only = std::slice::from_ref(&value);
        assert_eq!(stored, only, "only the value op is stored");
        let fanned = ops_until_bound(&mut r_sub, &w_sub).await;
        assert_eq!(fanned, [value], "the subscriber gets the value op alone");
    }

    /// Subscribe to `sh/g`: the statuses that arrived before its ack, and the
    /// gap after it.
    async fn statuses_and_gap(r: &mut ws::WsReader, w: &ws::WsWriter) -> (Vec<Error>, Vec<Op>) {
        let statuses = errors_before_ack(r, w).await;
        (statuses, ops_until_bound(r, w).await)
    }

    /// The ops that reach the session before the ack of a subscribe to a zone
    /// with no ops, which bounds them, since the session's frames leave in the
    /// order they are queued.
    async fn ops_until_bound(r: &mut ws::WsReader, w: &ws::WsWriter) -> Vec<Op> {
        let bound = Subscribe {
            share: "sh".into(),
            glade_id: "bound".into(),
            key: None,
            from: None,
        };
        w.send_binary(&Frame::Subscribe(bound).to_bytes())
            .await
            .unwrap();
        let mut ops = Vec::new();
        loop {
            match next(r, "ops, then the bound's ack").await {
                Frame::Ops(o) => ops.extend(o.ops),
                Frame::Heads(_) => return ops,
                other => panic!("expected ops, then an ack, got {other:?}"),
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

    /// R4 (client-writes plan Step 2.2): the ack is a cut. No op of a zone
    /// reaches a session before the ack of its subscribe, and each op the node
    /// holds reaches it once after the ack, in the gap or live. The test stops
    /// the subscriber where the subscribe arm used to race a writer. It holds
    /// the router's lock, which the arm takes to register the session, and the
    /// store's lock while that is free, so the writer's op waits ahead of the
    /// subscriber's gap: tokio's `Mutex` grants a lock in the order it was
    /// asked for. The test never waits for a lock while it holds one, so no
    /// order of the node's locks can deadlock it, and on the node as built any
    /// order of the two sessions gives the same answer. The pauses only give
    /// each frame time to reach its lock.
    #[tokio::test]
    async fn no_op_of_a_zone_reaches_a_subscriber_before_its_ack() {
        let (shared, port) = serving("glade-server-cut").await;
        let (mut r_sub, w_sub) = ws::connect("127.0.0.1", port).await.unwrap();
        let (_r_writer, w_writer) = ws::connect("127.0.0.1", port).await.unwrap();
        let pause = || tokio::time::sleep(std::time::Duration::from_millis(50));

        let router = shared.router.lock().await;
        w_sub.send_binary(&subscribe()).await.unwrap();
        pause().await; // the subscribe waits for the router's lock
        let store = shared.store.try_lock().ok();
        let live = op("w", 0, b"live");
        w_writer
            .send_binary(&ops_frame(live.clone()))
            .await
            .unwrap();
        pause().await; // the op waits for a lock
        drop(router);
        pause().await; // the subscriber registers, then waits for a lock
        drop(store);

        let first = next(&mut r_sub, "the subscriber's first frame").await;
        assert!(
            matches!(first, Frame::Heads(_)),
            "an op reached the subscriber before its ack: {first:?}"
        );
        match next(&mut r_sub, "the op, after the ack").await {
            Frame::Ops(o) => assert_eq!(o.ops, std::slice::from_ref(&live)),
            other => panic!("expected the op after the ack, got {other:?}"),
        }
        let again = ops_until_bound(&mut r_sub, &w_sub).await;
        assert!(
            again.is_empty(),
            "the op reached the subscriber twice: {again:?}"
        );
    }

    /// R5 (client-writes plan Step 2.2): the ack names each origin's head in
    /// the zone by seq and hash. `Head.hash` is the 32 bytes of the op at that
    /// seq, the hash that R1's `corr` spells in hex. A keyed zone's ack names
    /// its key, and an empty zone's ack names the zone and no origin.
    #[tokio::test]
    async fn the_ack_names_each_origin_head_with_its_hash() {
        let (_shared, port) = serving("glade-server-ack-hashes").await;
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();

        let a0 = op("a", 0, b"a zero");
        let a1 = Op {
            prev: Some(crate::chain::op_hash(&a0).to_vec()),
            ..op("a", 1, b"a one")
        };
        let b0 = op("b", 0, b"b zero");
        let keyed = keyed_op("a", 0, b"k", b"keyed");
        let ops = vec![a0, a1.clone(), b0.clone(), keyed.clone()];
        w.send_binary(&Frame::Ops(Ops { ops, pri: None }).to_bytes())
            .await
            .unwrap();
        let head = |o: &Op| Head {
            origin: o.origin.clone(),
            seq: o.seq,
            hash: Some(crate::chain::op_hash(o).to_vec()),
        };
        let zone = |glade_id: &str, key: &[u8], heads: Vec<Head>| Heads {
            streams: vec![StreamHeads {
                share: "sh".into(),
                glade_id: glade_id.into(),
                key: key.to_vec(),
                heads,
            }],
        };

        w.send_binary(&subscribe()).await.unwrap();
        let (statuses, ack) = errors_and_ack(&mut r).await;
        assert_eq!(ack, zone("g", b"", vec![head(&a1), head(&b0)]));
        assert_eq!(
            statuses[1].corr,
            Some(corr(&a1)),
            "the head's hash is a1's corr"
        );
        w.send_binary(&subscribe_key(Some(b"k".to_vec())))
            .await
            .unwrap();
        assert_eq!(
            errors_and_ack(&mut r).await.1,
            zone("g", b"k", vec![head(&keyed)])
        );
        let empty = Subscribe {
            share: "sh".into(),
            glade_id: "empty".into(),
            key: None,
            from: None,
        };
        w.send_binary(&Frame::Subscribe(empty).to_bytes())
            .await
            .unwrap();
        assert_eq!(errors_and_ack(&mut r).await.1, zone("empty", b"", vec![]));
    }

    /// R4 with R3 (owner, on Step 2.1's second question): the session's heads
    /// never fall. A `Hello` that announces a lower seq than the session holds
    /// leaves the higher one, so a later subscribe ships none of the session's
    /// own ops back to it.
    #[tokio::test]
    async fn a_later_hello_never_lowers_the_sessions_heads() {
        let (_shared, port) = serving("glade-server-hello-heads").await;
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();

        let zero = op("w", 0, b"zero");
        let one = Op {
            prev: Some(crate::chain::op_hash(&zero).to_vec()),
            ..op("w", 1, b"one")
        };
        let frame = Frame::Ops(Ops {
            ops: vec![zero, one],
            pri: None,
        });
        w.send_binary(&frame.to_bytes()).await.unwrap();
        let lower = StreamHeads {
            share: "sh".into(),
            glade_id: "g".into(),
            key: vec![],
            heads: vec![Head {
                origin: "w".into(),
                seq: 0,
                hash: None,
            }],
        };
        let hello = Hello {
            session: "s".into(),
            protocol: 1,
            principal: None,
            capability: None,
            heads: vec![lower],
        };
        w.send_binary(&Frame::Hello(hello).to_bytes())
            .await
            .unwrap();
        for _ in 0..2 {
            match next(&mut r, "an op's status").await {
                Frame::Error(e) => assert_eq!(e.code, ErrorCode::Ok),
                other => panic!("expected the ops' statuses, got {other:?}"),
            }
        }
        let welcome = next(&mut r, "the Welcome").await;
        assert!(matches!(welcome, Frame::Welcome(_)), "{welcome:?}");

        let (statuses, gap) = statuses_and_gap(&mut r, &w).await;
        assert!(statuses.is_empty(), "{statuses:?}");
        assert!(
            gap.is_empty(),
            "the session's own op came back after a Hello announced a lower seq: {gap:?}"
        );
    }

    // ---- the grant check on client sessions (plan Step 4.3) ---------------

    /// A server over a fresh store named `name`, whose grant fold is `policy`,
    /// with client sessions checked when `checked`: its state and its port.
    async fn guarded(name: &str, policy: Policy, checked: bool) -> (Arc<Shared>, u16) {
        let dir = std::env::temp_dir().join(format!("glade-server-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let server = Server::open(&dir).unwrap();
        refresh_policy(&server.shared, Some(policy)).await;
        if checked {
            server.enforce_client_grants();
        }
        let shared = server.shared.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));
        (shared, port)
    }

    /// A client session on `port` whose Hello claims `principal`, if any.
    async fn session(port: u16, principal: Option<&str>) -> (ws::WsReader, ws::WsWriter) {
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();
        if let Some(principal) = principal {
            let hello = Hello {
                session: "s".into(),
                protocol: 1,
                principal: Some(principal.into()),
                capability: None,
                heads: vec![],
            };
            w.send_binary(&Frame::Hello(hello).to_bytes())
                .await
                .unwrap();
            let welcome = next(&mut r, "the welcome").await;
            assert!(matches!(welcome, Frame::Welcome(_)), "{welcome:?}");
        }
        (r, w)
    }

    /// Subscribe `share/g` and read the answer: `Ok` with the number of zones
    /// an accepted ack names, or `Err` with the reason a refusal gives after
    /// its ack that names no zone (R6).
    async fn subscribe_on(
        r: &mut ws::WsReader,
        w: &ws::WsWriter,
        share: &str,
    ) -> Result<usize, Error> {
        let subscribe = Subscribe {
            share: share.into(),
            glade_id: "g".into(),
            key: None,
            from: None,
        };
        w.send_binary(&Frame::Subscribe(subscribe).to_bytes())
            .await
            .unwrap();
        match next(r, "the ack").await {
            Frame::Heads(h) if h.streams.is_empty() => match next(r, "the reason").await {
                Frame::Error(e) => Err(e),
                other => panic!("expected the reason, got {other:?}"),
            },
            Frame::Heads(h) => Ok(h.streams.len()),
            other => panic!("expected an ack, got {other:?}"),
        }
    }

    /// With client sessions checked (plan Step 4.3), a session that names no
    /// principal holds nothing: its subscribe to a share other than `home` is
    /// refused with the refused subscribe's two frames (R6), and nothing is
    /// registered, while `home` stays open. A Hello that claims 64 hex digits,
    /// a node's id, binds no principal, so it is refused the same way, though
    /// the fold grants that node. Unchecked, the default, the session is
    /// served.
    #[tokio::test]
    async fn a_session_claiming_no_principal_is_refused() {
        let node = "01".repeat(32);
        let mut policy = Policy::default();
        policy.grant(&node, "sh", [READ_SUBSCRIBE.to_string()]);

        let (_, port) = guarded("unchecked", policy.clone(), false).await;
        let (mut r, w) = session(port, None).await;
        let answer = subscribe_on(&mut r, &w, "sh").await;
        assert_eq!(
            answer.map_err(|e| e.message),
            Ok(1),
            "unchecked, the default"
        );

        let (shared, port) = guarded("no-principal", policy, true).await;
        let why = "unauthorized: a session that names no principal holds no grant of read.subscribe on sh";
        for claimed in [None, Some(node.as_str())] {
            let (mut r, w) = session(port, claimed).await;
            let e = subscribe_on(&mut r, &w, "sh").await.expect_err("refused");
            let named = (e.code, e.share.as_deref(), e.glade_id.as_deref());
            assert_eq!(named, (ErrorCode::Unauthorized, Some("sh"), Some("g")));
            assert_eq!((e.message.as_str(), e.corr), (why, None), "{claimed:?}");
            let home = subscribe_on(&mut r, &w, HOME).await;
            assert_eq!(home.map_err(|e| e.message), Ok(1), "home is open");
        }
        let bound = shared.principals.lock().await.len();
        assert_eq!(bound, 0, "a node's id binds no principal");
        let routed = shared.router.lock().await.route(0, "sh", "g", &[]);
        assert_eq!(routed, Vec::<SessionId>::new(), "nothing registered");
    }

    /// With client sessions checked, a session whose Hello claims a principal
    /// the fold grants `read.*` on `sh` is served, its ack and the ops that
    /// follow; one claiming another principal is refused. The principal is
    /// the client's claim: nothing proves it yet.
    #[tokio::test]
    async fn a_session_claiming_a_granted_principal_is_served() {
        let mut policy = Policy::default();
        policy.grant("alice", "sh", ["read.*".to_string()]);
        let (_, port) = guarded("granted-principal", policy, true).await;

        let (mut rb, wb) = session(port, Some("bob")).await;
        let e = subscribe_on(&mut rb, &wb, "sh")
            .await
            .expect_err("bob is refused");
        let why = "unauthorized: principal bob holds no grant of read.subscribe on sh";
        assert_eq!(e.message, why);

        let (mut ra, wa) = session(port, Some("alice")).await;
        let answer = subscribe_on(&mut ra, &wa, "sh").await;
        assert_eq!(answer.map_err(|e| e.message), Ok(1));
        let (mut rw, ww) = session(port, None).await;
        let written = op("w", 0, b"for alice");
        ww.send_binary(&ops_frame(written.clone())).await.unwrap();
        match next(&mut rw, "the writer's status").await {
            Frame::Error(e) => assert_eq!(said(&e), status_for(&written, ErrorCode::Ok)),
            other => panic!("the writer expected its op's status, got {other:?}"),
        }
        match next(&mut ra, "alice's op").await {
            Frame::Ops(ops) => assert_eq!(ops.ops[0].payload, b"for alice"),
            other => panic!("alice expected the op, got {other:?}"),
        }
    }

    /// The re-check pass reaches client sessions when they are checked: a
    /// revocation of alice on `sh` ends her live zone there, told by a lone
    /// `Error{Unauthorized}`, and her zone on `sh2` goes on.
    #[tokio::test]
    async fn a_revocation_ends_a_client_zone_of_a_claimed_principal() {
        let mut policy = Policy::default();
        policy.grant("alice", "sh", ["read.*".to_string()]);
        policy.grant("alice", "sh2", ["read.*".to_string()]);
        let (shared, port) = guarded("client-revoke", policy.clone(), true).await;
        let (mut ra, wa) = session(port, Some("alice")).await;
        for share in ["sh", "sh2"] {
            let answer = subscribe_on(&mut ra, &wa, share).await;
            assert_eq!(answer.map_err(|e| e.message), Ok(1), "{share}");
        }

        let mut revoked = policy;
        revoked.revoke("alice", "sh");
        refresh_policy(&shared, Some(revoked)).await;
        match next(&mut ra, "the lone refusal").await {
            Frame::Error(e) => {
                let named = (e.code, e.share.as_deref(), e.glade_id.as_deref());
                assert_eq!(named, (ErrorCode::Unauthorized, Some("sh"), Some("g")));
                let why = "unauthorized: principal alice's grants on sh are revoked";
                assert_eq!(e.message, why);
            }
            other => panic!("alice expected the refusal, got {other:?}"),
        }
        let routed = shared.router.lock().await.route(0, "sh", "g", &[]);
        assert_eq!(routed, Vec::<SessionId>::new(), "her sh zone ended");

        let (mut rw, ww) = session(port, None).await;
        let on_sh2 = Op {
            share: "sh2".into(),
            ..op("w", 0, b"sh2 goes on")
        };
        ww.send_binary(&ops_frame(on_sh2)).await.unwrap();
        let _status = next(&mut rw, "the writer's status").await;
        match next(&mut ra, "the sh2 op").await {
            Frame::Ops(ops) => assert_eq!(ops.ops[0].payload, b"sh2 goes on"),
            other => panic!("alice expected the sh2 op, got {other:?}"),
        }
    }

    // ---- a frame the node cannot take (F15) -------------------------------

    /// A Hello with no principal, as bytes.
    fn hello() -> Vec<u8> {
        let hello = Hello {
            session: "s".into(),
            protocol: 1,
            principal: None,
            capability: None,
            heads: vec![],
        };
        Frame::Hello(hello).to_bytes()
    }

    /// A new session on `port` sends `bytes`, then a Hello: the node refuses
    /// the frame and goes on, so the Hello is welcomed. A new client is
    /// served too.
    async fn refused_and_served(port: u16, bytes: &[u8], what: &str) {
        let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();
        w.send_binary(bytes).await.unwrap();
        w.send_binary(&hello()).await.unwrap();
        let welcome = next(&mut r, &format!("the welcome after {what}")).await;
        assert!(matches!(welcome, Frame::Welcome(_)), "{what}: {welcome:?}");
        session(port, Some("next")).await;
    }

    /// F15: a frame nested 100,000 deep, 100 KB, overflowed the stack of the
    /// thread decoding it and aborted the node. It is refused, and the node
    /// serves on: the session that sent it, and a new client.
    #[tokio::test]
    async fn a_frame_nested_100_000_deep_is_refused_and_the_node_serves_on() {
        let (_, port) = serving("glade-server-f15-nested").await;
        let nested = [&hello()[..1], &[0x81].repeat(100_000), &[0x80]].concat();
        refused_and_served(port, &nested, "a Hello nested 100,000 deep").await;
    }

    /// F15: a truncated frame, and one holding a CBOR tag, panicked the task
    /// of the session that sent it, leaving its socket open and unread. Each
    /// is refused, and the session goes on; a new client is served.
    #[tokio::test]
    async fn a_truncated_or_malformed_frame_is_refused_and_its_session_goes_on() {
        let (_, port) = serving("glade-server-f15-malformed").await;
        let hello = hello();
        let tagged = [&hello[..1], &[0xc0, 0x00]].concat();
        let truncated = &hello[..hello.len() - 1];
        refused_and_served(port, truncated, "a truncated Hello").await;
        refused_and_served(port, &tagged, "a Hello holding a CBOR tag").await;
    }

    /// TautCheckedDecode.md CD-G3: a frame whose message is well-formed CBOR
    /// of another shape, a field missing or of another type, or no map,
    /// panicked its session's task in the generated decode, which left its
    /// connection open and unread. Each is refused like any bad frame, as is
    /// an int the strict codec refuses, and the session goes on; a new client
    /// is served.
    #[tokio::test]
    async fn a_frame_of_another_shape_is_refused_and_its_session_goes_on() {
        let (_, port) = serving("glade-server-cdg3-shape").await;
        let hello = hello();
        let tag = &hello[..1];
        let session_an_int = cbor::Cbor::Map(vec![(1, cbor::Cbor::Int(1))]);
        let cases = [
            ([tag, &[0xa0]].concat(), "a Hello with no fields"),
            (
                [tag, &cbor::encode(&session_an_int)].concat(),
                "a Hello whose session is an int",
            ),
            ([tag, &[0x03]].concat(), "a Hello that is not a map"),
            (
                [tag, &[0x18, 0x01]].concat(),
                "a Hello that is a non-canonical int",
            ),
        ];
        for (bytes, what) in cases {
            refused_and_served(port, &bytes, what).await;
        }
    }

    /// A websocket to `port`, upgraded by hand, to write any bytes on.
    async fn upgraded(port: u16) -> TcpStream {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let key = ws::b64_encode(&[0u8; 16]);
        let upgrade = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        socket.write_all(upgrade.as_bytes()).await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
        }
        socket
    }

    /// F15: a websocket header that claims more than `MAX_FRAME_BYTES` was
    /// allocated and waited on, or, claiming all of `u64`, panicked the
    /// session and left its socket open. It ends that connection before its
    /// payload, and a new client is served.
    #[tokio::test]
    async fn a_header_over_the_frame_limit_ends_only_its_connection() {
        use crate::frame::MAX_FRAME_BYTES;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (_, port) = serving("glade-server-f15-oversized").await;
        for claimed in [MAX_FRAME_BYTES as u64 + 1, u64::MAX] {
            let mut socket = upgraded(port).await;
            let header = [&[0x82, 0xff][..], &claimed.to_be_bytes(), &[0; 4]].concat();
            socket.write_all(&header).await.unwrap();
            let five = std::time::Duration::from_secs(5);
            let read = tokio::time::timeout(five, socket.read(&mut [0; 1])).await;
            let ended = matches!(read, Ok(Ok(0) | Err(_)));
            assert!(ended, "a header claiming {claimed} bytes: {read:?}");
            session(port, Some("next")).await;
        }
    }
}
