//! The peer mesh (Lane R step 3): accept-loop wiring for the iroh carrier.
//!
//! R2 left `PeerEndpoint`/`PeerLink` as a library capability; this module wires
//! it into the running node so two nodes actually converge. Per connection
//! (after the `NodeHello` seam on stream 0):
//!
//! - **stream 0** carries the dialer's home-share pull (dialer sends `Heads`,
//!   acceptor serves the gap and closes — the s-sync shape, scoped to `home`).
//! - the **acceptor opens its own stream** and pulls the same way, so
//!   convergence is a pull each way (GladePeerSyncNotes §4).
//! - any further stream is dispatched by its FIRST frame: `Heads` = a sync
//!   pull to serve, `Subscribe` = a forwarded interest (claim routing, C2/C3).
//!
//! Connect-time anti-entropy is scoped to the HOME share on purpose: the
//! directory replicates everywhere (WD §3 ladder 1 — every device a replica);
//! app-share content moves by INTEREST (a routed subscribe), never wholesale.
//! Ops ingested from a peer are appended through the same verify path as any
//! carrier and fanned out to local subscribers — the replica serves the reads.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::sync::Mutex;

use glade_wire::generated::{Head, Heads, Op, Ops, Priority, StreamHeads, Subscribe};

use crate::frame::Frame;
use crate::iroh_carrier::{PeerAddr, PeerEndpoint, PeerLink};
use crate::peer::{read_frame, write_frame, OPS_PER_CHUNK};
use crate::registry::HOME;
use crate::router::SessionId;
use crate::server::{send, Server, Shared};
use crate::session::{heads_map, missing_for};
use crate::store::{Append, Store};
use crate::sysdir::now_ms;

fn other<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// hex-render a node id (the directory rendering of the raw HELLO id).
pub(crate) fn hex_id(id: &[u8]) -> String {
    id.iter().map(|b| format!("{:02x}", b)).collect()
}

/// The node's peer fabric: the bound endpoint plus the live links, keyed by the
/// peer's directory node id (hex). A link is a QUIC connection that survived the
/// HELLO seam; the ServeClaim fold picks WHICH link a subscribe rides (C2).
pub struct Mesh {
    pub(crate) endpoint: PeerEndpoint,
    /// Our directory node id (hex of the HELLO identity) — the id our own
    /// ServeClaims carry, so `who_serves == self` short-circuits to local.
    pub(crate) self_id: String,
    /// Live peer links: directory node id (hex) -> connection.
    pub(crate) links: Mutex<BTreeMap<String, Connection>>,
    /// Zones whose interest is already forwarded to a claim holder — a second
    /// local subscriber joins the flow, it never opens a second stream.
    pub(crate) forwarded: Mutex<BTreeSet<(String, String, Vec<u8>)>>,
}

/// Where a subscribe is served (the C2 decision). Decided per subscribe, at
/// the reader's clock — a lapsed lease at read time IS the absence case.
pub(crate) enum Route {
    /// Serve from the local replica (also every non-directory share, and the
    /// whole legacy no-mesh node).
    Local,
    /// Forward the interest to the claim-holding node (directory node id).
    Forward(String),
    /// No live claim / no route: answer with STATUS data (the reason), never
    /// a hang (trace E2/E5).
    Absent(String),
}

/// The C2 routing step: consult the folded ServeClaims in the LOCAL replica.
/// Rules, in order: no mesh → local (legacy contract, byte-for-byte); the home
/// share → always local (every node replicates it); a live claim held by self
/// → local; a live claim held by a linked peer → forward; a live claim with no
/// link → absent (unreachable); no live claim but the directory KNOWS the
/// share → absent (lease lapsed at the reader's clock — trace E2); a share the
/// directory has never heard of → local (plain app-share serving).
pub(crate) async fn route_subscribe(shared: &Arc<Shared>, share: &str) -> Route {
    let Some(mesh) = shared.mesh.get() else { return Route::Local };
    if share == HOME {
        return Route::Local;
    }
    let (holder, known) = {
        let st = shared.store.lock().await;
        (who_serves(&st, share, now_ms()), directory_knows(&st, share))
    };
    match holder {
        Some(id) if id == mesh.self_id => Route::Local,
        Some(id) => {
            if mesh.links.lock().await.contains_key(&id) {
                Route::Forward(id)
            } else {
                Route::Absent(format!("claim holder {id} unreachable (no live peer link)"))
            }
        }
        None if known => Route::Absent(format!("no live ServeClaim for {share}")),
        None => Route::Local,
    }
}

impl Server {
    /// Wire the peer fabric onto this node: remember the endpoint, spawn the
    /// accept loop (hello + serve on accept). Returns the dialable address.
    /// Call once, before `run`.
    pub async fn enable_mesh(&self, endpoint: PeerEndpoint) -> io::Result<PeerAddr> {
        let addr = endpoint.addr()?;
        let mesh = Arc::new(Mesh {
            self_id: hex_id(&endpoint.identity().node_id),
            endpoint,
            links: Mutex::new(BTreeMap::new()),
            forwarded: Mutex::new(BTreeSet::new()),
        });
        self.shared
            .mesh
            .set(mesh.clone())
            .map_err(|_| other("mesh already enabled"))?;
        let shared = self.shared.clone();
        tokio::spawn(async move {
            // The accept loop borrows the mesh's endpoint clone, so the
            // endpoint outlives every link it produces (the R2 footgun).
            loop {
                match mesh.endpoint.accept().await {
                    Ok(Some(link)) => {
                        let (shared, mesh) = (shared.clone(), mesh.clone());
                        tokio::spawn(async move {
                            let _ = run_link(shared, mesh, link, false).await;
                        });
                    }
                    Ok(None) => break, // endpoint closed
                    Err(_) => continue, // one bad handshake never stops the loop
                }
            }
        });
        Ok(addr)
    }

    /// Dial a peer, run the HELLO seam, register the link, and converge the
    /// home share (a pull each way rides the connection). Returns the peer's
    /// directory node id (hex).
    pub async fn connect_peer(&self, addr: &PeerAddr) -> io::Result<String> {
        let mesh = self.shared.mesh.get().cloned().ok_or_else(|| other("mesh not enabled"))?;
        let link = mesh.endpoint.dial(addr).await?;
        let peer = hex_id(&link.peer.peer_id);
        run_link(self.shared.clone(), mesh, link, true).await?;
        Ok(peer)
    }
}

/// Drive one established (post-HELLO) link, dialer or acceptor side:
/// register it, dispatch inbound streams, and run OUR home-share pull.
/// Returns once our own pull has completed (the link itself lives on).
async fn run_link(shared: Arc<Shared>, mesh: Arc<Mesh>, link: PeerLink, dialed: bool) -> io::Result<()> {
    let PeerLink { peer, conn, send: s0_send, recv: s0_recv } = link;
    let peer_hex = hex_id(&peer.peer_id);
    mesh.links.lock().await.insert(peer_hex.clone(), conn.clone());

    // Unlink on close, whoever closes first.
    {
        let (mesh, conn, peer_hex) = (mesh.clone(), conn.clone(), peer_hex.clone());
        tokio::spawn(async move {
            conn.closed().await;
            mesh.links.lock().await.remove(&peer_hex);
        });
    }

    // Dispatch every inbound stream by its first frame.
    {
        let (shared, conn) = (shared.clone(), conn.clone());
        tokio::spawn(async move {
            while let Ok((send, recv)) = conn.accept_bi().await {
                let shared = shared.clone();
                tokio::spawn(async move {
                    let _ = handle_peer_stream(shared, send, recv).await;
                });
            }
        });
    }

    // Our home-share pull: the dialer rides stream 0 (the acceptor's stream-0
    // handler above serves it); the acceptor opens its own stream.
    if dialed {
        pull_home(&shared, s0_send, s0_recv).await
    } else {
        // Stream 0 on the acceptor side is the DIALER's pull channel: serve it.
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                let _ = handle_peer_stream(shared, s0_send, s0_recv).await;
            });
        }
        let (send, recv) = conn.open_bi().await.map_err(other)?;
        pull_home(&shared, send, recv).await
    }
}

/// Serve one inbound peer stream by its first frame: `Heads` = a home-scoped
/// sync pull (serve the gap, close); `Subscribe` = a forwarded interest (this
/// node is the claim holder — serve gap + live ops until the interest closes);
/// `ExchangeReq` = a forwarded exchange (this node is the claim holder — the
/// attached authority answers, one stream one exchange, `exchange.rs`).
async fn handle_peer_stream(shared: Arc<Shared>, mut send: SendStream, mut recv: RecvStream) -> io::Result<()> {
    match read_frame(&mut recv).await? {
        Frame::Heads(h) => serve_home(&shared, &mut send, h).await,
        Frame::Subscribe(s) => serve_peer_subscribe(shared, send, recv, s).await,
        Frame::ExchangeReq(x) => crate::exchange::serve_peer_exchange(shared, send, recv, x).await,
        _ => Ok(()), // unknown opener: drop the stream, never the connection
    }
}

/// The claim holder's side of a forwarded interest (trace C3→C5): register the
/// peer as an ordinary subscriber session of the zone, ship the resume gap
/// against the `from` heads it announced, then let the normal fan-out feed the
/// stream until the peer closes it (interest withdrawn / link gone).
async fn serve_peer_subscribe(
    shared: Arc<Shared>,
    mut qsend: SendStream,
    mut recv: RecvStream,
    s: Subscribe,
) -> io::Result<()> {
    let key = s.key.clone().unwrap_or_default();
    let sid = shared.next.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    shared.out.lock().await.insert(sid, tx.clone());
    shared.router.lock().await.subscribe(sid, &s.share, &s.glade_id, &key);

    // Writer: drain the session outbound onto the QUIC stream, u32-framed —
    // the peer framing every glade stream speaks.
    let wtask = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Some(bytes) = rx.recv().await {
            let ok = qsend.write_all(&(bytes.len() as u32).to_le_bytes()).await.is_ok()
                && qsend.write_all(&bytes).await.is_ok()
                && qsend.flush().await.is_ok();
            if !ok {
                break;
            }
        }
    });

    // Ack + gap ride the SAME outbound channel as live fan-out, so a live op
    // can never overtake the resume gap on the stream.
    let their: crate::session::Heads =
        s.from.clone().unwrap_or_default().into_iter().map(|h| (h.origin, h.seq)).collect();
    let (server_heads, gap) = {
        let st = shared.store.lock().await;
        (heads_map(&st, &s.share, &s.glade_id, &key), missing_for(&st, &s.share, &s.glade_id, &key, &their))
    };
    let ack = Frame::Heads(Heads {
        streams: vec![StreamHeads {
            share: s.share.clone(),
            glade_id: s.glade_id.clone(),
            key: key.clone(),
            heads: server_heads.iter().map(|(o, sq)| Head { origin: o.clone(), seq: *sq, hash: None }).collect(),
        }],
    });
    let _ = tx.send(ack.to_bytes());
    if !gap.is_empty() {
        let _ = tx.send(Frame::Ops(Ops { ops: gap, pri: Some(Priority::Bulk) }).to_bytes());
    }

    // Hold the subscription open until the peer closes its end.
    while read_frame(&mut recv).await.is_ok() {}
    shared.out.lock().await.remove(&sid);
    shared.router.lock().await.unsubscribe_all(sid);
    wtask.abort();
    Ok(())
}

/// The A-side of the C2 decision's Forward arm: open a stream on the claim
/// holder's link, send the interest (with our replica's heads as the resume
/// point), and ingest what comes back into the LOCAL replica — local
/// subscribers are then fed by the ordinary fan-out (replica serves reads,
/// trace C5→C6). Deduped per zone: one stream carries any number of local
/// subscribers. The forward lapses with the stream; a later subscribe retries.
pub(crate) async fn forward_interest(shared: &Arc<Shared>, peer: String, share: String, glade_id: String, key: Vec<u8>) {
    let Some(mesh) = shared.mesh.get().cloned() else { return };
    let zone = (share.clone(), glade_id.clone(), key.clone());
    if !mesh.forwarded.lock().await.insert(zone.clone()) {
        return; // interest already flowing
    }
    let conn = mesh.links.lock().await.get(&peer).cloned();
    let Some(conn) = conn else {
        mesh.forwarded.lock().await.remove(&zone);
        return;
    };
    let shared = shared.clone();
    tokio::spawn(async move {
        let _ = run_forward(&shared, conn, &share, &glade_id, &key).await;
        mesh.forwarded.lock().await.remove(&(share, glade_id, key));
    });
}

async fn run_forward(shared: &Arc<Shared>, conn: Connection, share: &str, glade_id: &str, key: &[u8]) -> io::Result<()> {
    let (mut qsend, mut recv) = conn.open_bi().await.map_err(other)?;
    let from: Vec<Head> = {
        let st = shared.store.lock().await;
        st.heads(share, glade_id, key).into_iter().map(|(origin, seq)| Head { origin, seq, hash: None }).collect()
    };
    let sub = Subscribe {
        share: share.into(),
        glade_id: glade_id.into(),
        key: if key.is_empty() { None } else { Some(key.to_vec()) },
        from: Some(from),
    };
    write_frame(&mut qsend, &Frame::Subscribe(sub)).await?;
    let from_sid = shared.next.fetch_add(1, Ordering::SeqCst);
    loop {
        let frame = match read_frame(&mut recv).await {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // interest closed
            Err(e) => return Err(e),
        };
        if let Frame::Ops(ops) = frame {
            for op in ops.ops {
                // Scoped ingest: this stream carries ONE zone's interest — the
                // holder can't use it to push any other zone into our replica.
                if op.share == share && op.glade_id == glade_id && op.key == key {
                    ingest_and_fanout(shared, from_sid, op).await;
                }
            }
        }
    }
    Ok(())
}

/// Respond to a peer's home-share pull: ship exactly the home-zone ops the
/// peer lacks (size-capped bulk chunks), then close — close = gap complete.
/// Scoped to HOME: connect-time anti-entropy replicates the directory only;
/// app shares move by interest (see the module note).
async fn serve_home(shared: &Arc<Shared>, send: &mut SendStream, their: Heads) -> io::Result<()> {
    let mut by_zone: BTreeMap<(String, String, Vec<u8>), BTreeMap<String, i64>> = BTreeMap::new();
    for sh in their.streams {
        let m = by_zone.entry((sh.share.clone(), sh.glade_id.clone(), sh.key.clone())).or_default();
        for hd in sh.heads {
            m.insert(hd.origin, hd.seq);
        }
    }
    // Collect the gap under the store lock, then stream without it.
    let gap: Vec<Op> = {
        let st = shared.store.lock().await;
        let mut gap = Vec::new();
        for (share, glade_id, key) in st.zones() {
            if share != HOME {
                continue;
            }
            let their_v = by_zone.get(&(share.clone(), glade_id.clone(), key.clone())).cloned().unwrap_or_default();
            gap.extend(missing_for(&st, &share, &glade_id, &key, &their_v));
        }
        gap
    };
    for chunk in gap.chunks(OPS_PER_CHUNK) {
        write_frame(send, &Frame::Ops(Ops { ops: chunk.to_vec(), pri: Some(Priority::Bulk) })).await?;
    }
    use tokio::io::AsyncWriteExt;
    send.shutdown().await // close = gap complete
}

/// Pull the peer's home-share gap: announce our home heads, ingest until the
/// peer closes. Every op lands through the same verify path as any carrier
/// (`Store::append` chain checks) and fans out to local subscribers — a
/// directory update reaches a live `dir.workspaces` subscription with no
/// re-request (the B9 step). Non-home ops on this stream are dropped: the
/// pull asked for the directory, a peer can't use it to push app content.
async fn pull_home(shared: &Arc<Shared>, mut send: SendStream, mut recv: RecvStream) -> io::Result<()> {
    let ours: Vec<StreamHeads> = {
        let st = shared.store.lock().await;
        st.all_heads().into_iter().filter(|sh| sh.share == HOME).collect()
    };
    write_frame(&mut send, &Frame::Heads(Heads { streams: ours })).await?;
    // A fresh session id no local session holds: fan-out excludes only the
    // ingesting link, never a real subscriber.
    let from_sid = shared.next.fetch_add(1, Ordering::SeqCst);
    loop {
        let frame = match read_frame(&mut recv).await {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // peer closed = done
            Err(e) => return Err(e),
        };
        if let Frame::Ops(ops) = frame {
            for op in ops.ops {
                if op.share == HOME {
                    ingest_and_fanout(shared, from_sid, op).await;
                }
            }
        }
    }
    Ok(())
}

/// Land one peer-ingested op in the local replica (same chain checks as any
/// append) and fan it out to the local subscribers of its zone. Rejected or
/// duplicate ops fan out to no one — the fold only ever sees the valid set.
pub(crate) async fn ingest_and_fanout(shared: &Arc<Shared>, from: SessionId, op: Op) {
    let (share, glade_id, key) = (op.share.clone(), op.glade_id.clone(), op.key.clone());
    let res = shared.store.lock().await.append(op.clone());
    if matches!(res, Ok(Append::Appended)) {
        let targets = shared.router.lock().await.route(from, &share, &glade_id, &key);
        if !targets.is_empty() {
            let frame = Frame::Ops(Ops { ops: vec![op], pri: None });
            for t in targets {
                send(shared, t, &frame).await;
            }
        }
    }
}

/// Fold the local replica's home share for the current claim holder of
/// `share`, judged at the READER's clock `now_ms` (lease expiry never enters
/// the fold — WD §2); highest live epoch wins. `None` = no live claim.
pub fn who_serves(store: &Store, share: &str, now_ms: i64) -> Option<String> {
    let mut best: Option<crate::sysdata::ServeClaim> = None;
    for (origin, _) in store.heads(HOME, crate::registry::G_CLAIMS, &[]) {
        for op in store.scan(HOME, crate::registry::G_CLAIMS, &[], &origin, i64::MIN) {
            let c = crate::sysdata::ServeClaim::from_cbor(&glade_wire::cbor::decode(&op.payload));
            if c.share == share && c.lease_expiry_ms > now_ms && best.as_ref().map_or(true, |b| c.epoch > b.epoch) {
                best = Some(c);
            }
        }
    }
    best.map(|c| c.node)
}

/// Does the directory know `share` at all — a `WorkspaceEntry` naming it, or
/// any claim (live or lapsed) for it? Distinguishes "directory-managed share
/// with no live host" (absent, trace E2: the directory knows the last eligible
/// host) from "not a directory concern" (plain local app share).
pub fn directory_knows(store: &Store, share: &str) -> bool {
    for (origin, _) in store.heads(HOME, crate::registry::G_WORKSPACES, &[]) {
        for op in store.scan(HOME, crate::registry::G_WORKSPACES, &[], &origin, i64::MIN) {
            if crate::sysdata::WorkspaceEntry::from_cbor(&glade_wire::cbor::decode(&op.payload)).workspace == share {
                return true;
            }
        }
    }
    for (origin, _) in store.heads(HOME, crate::registry::G_CLAIMS, &[]) {
        for op in store.scan(HOME, crate::registry::G_CLAIMS, &[], &origin, i64::MIN) {
            if crate::sysdata::ServeClaim::from_cbor(&glade_wire::cbor::decode(&op.payload)).share == share {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Record, RegistryApi};
    use crate::sysdata::{ServeClaim, WorkspaceEntry};
    use crate::sysdir::{boot_at, now_ms};
    use std::path::PathBuf;
    use std::time::Duration;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-mesh-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Poll until `pred` (over the node's store) holds, or panic after ~5s —
    /// convergence is eventually-consistent, tests wait for it, never sleep blind.
    async fn wait_store<F: Fn(&Store) -> bool>(shared: &Arc<Shared>, pred: F, what: &str) {
        for _ in 0..500 {
            if pred(&*shared.store.lock().await) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }
    async fn wait_for<F: Fn(&Store) -> bool>(server: &Server, pred: F, what: &str) {
        wait_store(&server.shared, pred, what).await
    }

    /// Two booted nodes, wired over real iroh: after `connect_peer` the home
    /// share has converged BOTH ways — each node's replica holds the other's
    /// presence records (dir.nodes op from the other's origin), and the claim
    /// fold routes from either replica.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_booted_nodes_converge_home_share() {
        let boot_a = boot_at(fresh("conv-a-sys"), "gianni").unwrap();
        let boot_b = boot_at(fresh("conv-b-sys"), "gianni").unwrap();

        // B additionally registers a workspace + its serve claim (directory data).
        let mut boot_b = boot_b;
        boot_b
            .registry
            .append(
                Record::Workspace(WorkspaceEntry {
                    workspace: "ws-razel".into(),
                    name: "razel".into(),
                    eligible_hosts: vec![boot_b.node_id.clone()],
                }),
                &boot_b.node_id,
            )
            .unwrap();
        boot_b
            .registry
            .append(
                Record::Serve(ServeClaim {
                    node: boot_b.node_id.clone(),
                    share: "ws-razel".into(),
                    lease_expiry_ms: now_ms() + 30_000,
                    epoch: 1,
                }),
                &boot_b.node_id,
            )
            .unwrap();

        let a = Server::open(fresh("conv-a-store")).unwrap();
        let b = Server::open(fresh("conv-b-store")).unwrap();
        a.seed_registry(&boot_a.registry.snapshot()).await;
        b.seed_registry(&boot_b.registry.snapshot()).await;

        let ep_a = PeerEndpoint::bind_with(boot_a.identity().unwrap()).await.unwrap();
        let ep_b = PeerEndpoint::bind_with(boot_b.identity().unwrap()).await.unwrap();
        a.enable_mesh(ep_a).await.unwrap();
        let addr_b = b.enable_mesh(ep_b).await.unwrap();

        // The HELLO identity is the directory identity (one id, two renderings).
        let peer = a.connect_peer(&addr_b).await.unwrap();
        assert_eq!(peer, boot_b.node_id);

        // A pulled B: B's presence + workspace + claim are in A's replica...
        let (b_id, a_id) = (boot_b.node_id.clone(), boot_a.node_id.clone());
        {
            let bid = b_id.clone();
            wait_for(&a, move |st| !st.scan(HOME, crate::registry::G_NODES, &[], &bid, i64::MIN).is_empty(), "A to hold B's presence").await;
        }
        {
            let st = a.shared.store.lock().await;
            assert!(!st.scan(HOME, crate::registry::G_WORKSPACES, &[], &b_id, i64::MIN).is_empty(), "A holds B's WorkspaceEntry");
            // ...and A's LOCAL fold routes ws-razel to B, judged at A's clock.
            assert_eq!(who_serves(&st, "ws-razel", now_ms()), Some(b_id.clone()));
            assert_eq!(who_serves(&st, "ws-razel", now_ms() + 60_000), None, "lapsed at a later reader clock");
        }

        // ...and B pulled A (the reverse direction of the same connection).
        {
            let aid = a_id.clone();
            wait_for(&b, move |st| !st.scan(HOME, crate::registry::G_NODES, &[], &aid, i64::MIN).is_empty(), "B to hold A's presence").await;
        }
    }

    // ---- the s-discovery golden path, end to end ---------------------------

    fn sub(share: &str, glade_id: &str) -> Vec<u8> {
        Frame::Subscribe(Subscribe { share: share.into(), glade_id: glade_id.into(), key: None, from: None })
            .to_bytes()
    }

    fn tree_op(seq: i64, prev: Option<Vec<u8>>, payload: &[u8]) -> Op {
        Op {
            share: "ws-razel".into(),
            glade_id: "ws.tree".into(),
            key: vec![],
            origin: "prov-b".into(),
            seq,
            prev,
            lamport: seq,
            refs: vec![],
            shape: glade_wire::generated::Shape::Value,
            payload: payload.to_vec(),
        }
    }

    /// Read the next frame from a ws client, bounded — a hang is a failure
    /// (the trace's rule: failure surfaces as data, never as silence).
    async fn next_frame(r: &mut crate::ws::WsReader, what: &str) -> Frame {
        let msg = tokio::time::timeout(Duration::from_secs(5), r.read())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .unwrap();
        match msg {
            crate::ws::Msg::Binary(b) => Frame::from_bytes(&b).unwrap(),
            _ => panic!("unexpected close waiting for {what}"),
        }
    }

    /// The 30-step s-discovery trace's slice for this step, E2E over real iroh
    /// + real websockets: (a) phase A — a client on node A lists
    /// `home/dir.workspaces` from A's LOCAL replica and sees the workspace B
    /// registered; (b) phase C — subscribing that workspace's share routes the
    /// interest via the folded ServeClaim to B, the ops arrive, converge into
    /// A's replica, and keep flowing live; (c) phase E — a share whose only
    /// claim is lapsed at the reader's clock answers with STATUS data, bounded,
    /// and the session stays usable.
    #[tokio::test(flavor = "multi_thread")]
    async fn s_discovery_golden_path_end_to_end() {
        // Node B (workspace host): registers ws-razel + its live claim; the
        // sleeping ws-attic has only a claim that is LAPSED at any later read.
        let mut boot_b = boot_at(fresh("e2e-b-sys"), "gianni").unwrap();
        let b_id = boot_b.node_id.clone();
        boot_b
            .registry
            .append(
                Record::Workspace(WorkspaceEntry { workspace: "ws-razel".into(), name: "razel".into(), eligible_hosts: vec![b_id.clone()] }),
                &b_id,
            )
            .unwrap();
        boot_b
            .registry
            .append(
                Record::Serve(ServeClaim { node: b_id.clone(), share: "ws-razel".into(), lease_expiry_ms: now_ms() + 30_000, epoch: 1 }),
                &b_id,
            )
            .unwrap();
        boot_b
            .registry
            .append(
                Record::Workspace(WorkspaceEntry { workspace: "ws-attic".into(), name: "attic".into(), eligible_hosts: vec!["attic-mini".into()] }),
                &b_id,
            )
            .unwrap();
        boot_b
            .registry
            .append(
                Record::Serve(ServeClaim { node: "attic-mini".into(), share: "ws-attic".into(), lease_expiry_ms: now_ms() - 1_000, epoch: 1 }),
                &b_id,
            )
            .unwrap();

        let boot_a = boot_at(fresh("e2e-a-sys"), "gianni").unwrap();

        let a = Server::open(fresh("e2e-a-store")).unwrap();
        let b = Server::open(fresh("e2e-b-store")).unwrap();
        a.seed_registry(&boot_a.registry.snapshot()).await;
        b.seed_registry(&boot_b.registry.snapshot()).await;

        let ep_a = PeerEndpoint::bind_with(boot_a.identity().unwrap()).await.unwrap();
        let ep_b = PeerEndpoint::bind_with(boot_b.identity().unwrap()).await.unwrap();
        a.enable_mesh(ep_a).await.unwrap();
        let addr_b = b.enable_mesh(ep_b).await.unwrap();
        a.connect_peer(&addr_b).await.unwrap();

        let (a_shared, b_shared) = (a.shared.clone(), b.shared.clone());
        let lis_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let lis_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (port_a, port_b) = (lis_a.local_addr().unwrap().port(), lis_b.local_addr().unwrap().port());
        tokio::spawn(a.run(lis_a));
        tokio::spawn(b.run(lis_b));

        // B's authority provider session writes the workspace content (the C4
        // source) — an ordinary session appending ordinary chained ops.
        let (_rp, wp) = crate::ws::connect("127.0.0.1", port_b).await.unwrap();
        let o0 = tree_op(0, None, b"tree-v0");
        let o1 = tree_op(1, Some(crate::chain::op_hash(&o0).to_vec()), b"tree-v1");
        wp.send_binary(&Frame::Ops(Ops { ops: vec![o0.clone(), o1.clone()], pri: None }).to_bytes()).await.unwrap();
        wait_store(&b_shared, |st| st.scan("ws-razel", "ws.tree", &[], "prov-b", i64::MIN).len() == 2, "B to hold the provider ops").await;

        // ---- (a) phase A: list the directory from A's LOCAL replica ---------
        let (mut rc, wc) = crate::ws::connect("127.0.0.1", port_a).await.unwrap();
        wc.send_binary(&sub(HOME, crate::registry::G_WORKSPACES)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "dir.workspaces ack").await, Frame::Heads(_)));
        let mut names = Vec::new();
        while names.len() < 2 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "workspace entries").await {
                for op in ops.ops {
                    assert_eq!(op.origin, b_id, "entries carry their writing origin");
                    names.push(crate::sysdata::WorkspaceEntry::from_cbor(&glade_wire::cbor::decode(&op.payload)).workspace);
                }
            }
        }
        names.sort();
        assert_eq!(names, vec!["ws-attic".to_string(), "ws-razel".to_string()], "the list from the local replica");

        // ---- (b) phase C: the claim routes the workspace share to B ---------
        wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "ws.tree ack").await, Frame::Heads(_)));
        let mut payloads = Vec::new();
        while payloads.len() < 2 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "routed tree ops").await {
                payloads.extend(ops.ops.into_iter().map(|o| o.payload));
            }
        }
        assert_eq!(payloads, vec![b"tree-v0".to_vec(), b"tree-v1".to_vec()], "the routed gap converges in order");
        // ...and INTO A's replica — the replica served the read (C5).
        wait_store(&a_shared, |st| st.scan("ws-razel", "ws.tree", &[], "prov-b", i64::MIN).len() == 2, "A's replica to hold the routed zone").await;

        // live: the provider writes v2 on B; it reaches the A-side client with
        // no re-request (the C5→C6 stream keeps flowing).
        let o2 = tree_op(2, Some(crate::chain::op_hash(&o1).to_vec()), b"tree-v2");
        wp.send_binary(&Frame::Ops(Ops { ops: vec![o2], pri: None }).to_bytes()).await.unwrap();
        loop {
            if let Frame::Ops(ops) = next_frame(&mut rc, "live tree op").await {
                if ops.ops.iter().any(|o| o.payload == b"tree-v2") {
                    break;
                }
            }
        }

        // ---- (c) phase E: no live claim -> STATUS data, bounded -------------
        wc.send_binary(&sub("ws-attic", "ws.tree")).await.unwrap();
        match next_frame(&mut rc, "ws-attic status").await {
            Frame::Error(e) => {
                assert_eq!(e.code, glade_wire::generated::ErrorCode::UnknownShare);
                assert_eq!(e.share.as_deref(), Some("ws-attic"));
                assert!(e.message.contains("no live ServeClaim"), "the reason rides the status: {}", e.message);
            }
            other => panic!("expected STATUS (Error frame), got {other:?}"),
        }
        // absence is data, not a dead session: the next ask still answers.
        wc.send_binary(&sub(HOME, crate::registry::G_CLAIMS)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "post-absence ack").await, Frame::Heads(_)));
    }
}
