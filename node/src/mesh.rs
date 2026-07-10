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

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::sync::Mutex;

use glade_wire::generated::{Heads, Op, Ops, Priority, StreamHeads};

use crate::frame::Frame;
use crate::iroh_carrier::{PeerAddr, PeerEndpoint, PeerLink};
use crate::peer::{read_frame, write_frame, OPS_PER_CHUNK};
use crate::registry::HOME;
use crate::router::SessionId;
use crate::server::{send, Server, Shared};
use crate::session::missing_for;
use crate::store::{Append, Store};

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
    /// Live peer links: directory node id (hex) -> connection.
    pub(crate) links: Mutex<BTreeMap<String, Connection>>,
}

impl Server {
    /// Wire the peer fabric onto this node: remember the endpoint, spawn the
    /// accept loop (hello + serve on accept). Returns the dialable address.
    /// Call once, before `run`.
    pub async fn enable_mesh(&self, endpoint: PeerEndpoint) -> io::Result<PeerAddr> {
        let addr = endpoint.addr()?;
        let mesh = Arc::new(Mesh { endpoint, links: Mutex::new(BTreeMap::new()) });
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
/// sync pull (serve the gap, close). `Subscribe` handling (forwarded interest)
/// lands with claim routing.
async fn handle_peer_stream(shared: Arc<Shared>, mut send: SendStream, mut recv: RecvStream) -> io::Result<()> {
    match read_frame(&mut recv).await? {
        Frame::Heads(h) => serve_home(&shared, &mut send, h).await,
        _ => Ok(()), // unknown opener: drop the stream, never the connection
    }
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

    /// Poll until `pred` (over the server's store) holds, or panic after ~5s —
    /// convergence is eventually-consistent, tests wait for it, never sleep blind.
    async fn wait_for<F: Fn(&Store) -> bool>(server: &Server, pred: F, what: &str) {
        for _ in 0..500 {
            if pred(&*server.shared.store.lock().await) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
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
}
