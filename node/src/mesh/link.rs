use std::collections::{BTreeMap, BTreeSet};
use std::future::poll_fn;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::Poll;
use std::time::{Duration, Instant};

use glade_carrier_api::{CarrierLink, TransportId};
use tokio::sync::Mutex;

use crate::assembly::{PathSeen, RelayState};
use crate::conversation::{Handler, LinkTask, Linked, Spawn, Work};
use crate::iroh_carrier::carrier_addr;
use crate::netconf::PeerEntry;
use crate::peer::HELLO_WITHIN;
use crate::peer::{carried, hello_accept_link, hello_dial_link};
use crate::server::{Server, Shared};
use crate::signing::NodeSigner;
use crate::tasks::Site;

use super::{hex_id, other, pull_home, serve_conversation, Mesh, Peer, PeerPort};

impl Server {
    /// Wire the peer fabric onto this node over `peer`, a bound port (plan
    /// Step 4.5b): load the door from the served store, watch the home
    /// relays' states if the port notes them, and spawn the accept loop. Call
    /// once, before `run`.
    pub async fn enable_mesh(&self, peer: PeerPort) -> io::Result<()> {
        let PeerPort {
            port,
            notes,
            identity,
            endpoint,
            door,
            max_frame_bytes: max,
        } = peer;
        if let Some(door) = &door {
            door.load(&*self.shared.store.lock().await);
        }
        let mesh = Arc::new(Mesh {
            port,
            notes,
            identity,
            endpoint,
            max,
            self_id: hex_id(&identity.node_id),
            links: Mutex::new(BTreeMap::new()),
            numbered: AtomicU64::new(0),
            forwarded: Mutex::new(BTreeSet::new()),
            door,
            signer: NodeSigner::new(Some(identity)),
            gap_pulls: std::sync::Mutex::new(BTreeMap::new()),
        });
        self.shared
            .mesh
            .set(mesh.clone())
            .map_err(|_| other("mesh already enabled"))?;
        // With n0's relays, the home relay's state is noted as it changes
        // (plan Step 4.5), until the port closes; with none, the watch ends
        // at once.
        if let Some(notes) = &mesh.notes {
            let (noting, mut before) = (mesh.clone(), Vec::new());
            let watch = notes.relay_watch(Box::new(move |now| {
                for line in relay_notes(&before, &now) {
                    noting.status(&line);
                }
                before = now;
            }));
            self.shared.tasks.spawn(Site::RelayWatch, watch);
        }
        let shared = self.shared.clone();
        self.shared.tasks.spawn(Site::AcceptLoop, async move {
            // Accept only: each link's HELLO runs in its own task (plan Step
            // 4.5b), so a dialer slow to say it never holds the next.
            loop {
                match mesh.port.accept().await {
                    Ok(Some(link)) => {
                        let (link_shared, mesh) = (shared.clone(), mesh.clone());
                        shared.tasks.spawn(Site::AcceptedLink, async move {
                            let _ = accepted(link_shared, mesh, Arc::from(link)).await;
                        });
                    }
                    // The port closed.
                    Ok(None) => break,
                    // One refused attempt never stops the loop.
                    Err(_) => continue,
                }
            }
        });
        Ok(())
    }

    /// Dial a peer at every address `target` names (plan Steps 4.5 and
    /// 4.5b), run HELLO on the link, register it, and converge the home share
    /// (a pull each way rides the link). Returns the peer's directory node id
    /// (hex).
    pub async fn connect_peer(&self, target: impl Into<PeerEntry>) -> io::Result<String> {
        let mesh = self.shared.mesh.get().cloned().ok_or_else(|| other("mesh not enabled"))?;
        let entry = target.into();
        let at = carrier_addr(&entry).ok_or_else(|| other("the peer names no address to dial"))?;
        let link: Arc<dyn CarrierLink> = Arc::from(mesh.port.dial(&at).await.map_err(carried)?);
        let (own, door) = (&mesh.endpoint, mesh.door.as_deref());
        let hello = hello_dial_link(&*link, own, &mesh.identity, door, HELLO_WITHIN).await?;
        let node = hello.peer_id;
        run_link(self.shared.clone(), mesh, link, node, true).await?;
        Ok(hex_id(&node))
    }
}

/// An accepted link's task (plan Step 4.5b): its HELLO, within
/// [`HELLO_WITHIN`], then its driver. A refused HELLO is reported through the
/// door, and the link, dropped unanswered, ends.
async fn accepted(
    shared: Arc<Shared>,
    mesh: Arc<Mesh>,
    link: Arc<dyn CarrierLink>,
) -> io::Result<()> {
    let (own, door) = (&mesh.endpoint, mesh.door.as_deref());
    let hello = hello_accept_link(&*link, own, &mesh.identity, door, HELLO_WITHIN).await?;
    run_link(shared, mesh, link, hello.peer_id, false).await
}

/// Drive one link after its HELLO, which proved `node`, dialer or acceptor
/// side: register it, start its conversations, and run OUR home-share pull.
/// Returns once our own pull has completed (the link itself lives on).
async fn run_link(
    shared: Arc<Shared>,
    mesh: Arc<Mesh>,
    link: Arc<dyn CarrierLink>,
    node: [u8; 32],
    dialed: bool,
) -> io::Result<()> {
    let peer = hex_id(&node);
    // The node its HELLO proved, which every conversation of the link serves
    // (the grant check's holder, plan Step 4.3), and which may write this
    // node's directory from now on (D9's known set, plan Step 4.1b's part 2).
    mesh.signer.authenticated(node);
    // The path at HELLO (plan Step 4.5), as the port notes it.
    let remote = link.remote_id();
    let noted = mesh.path(remote.as_ref());
    mesh.note_path(&peer, noted.as_ref());
    let (number, holding) = (mesh.numbered.fetch_add(1, Ordering::SeqCst), Arc::default());
    let watched = Watched {
        peer: peer.clone(),
        number,
        remote,
        noted,
        holding: Arc::clone(&holding),
    };
    // Started and registered under the table's lock, so that its reader,
    // should the link end at once, finds it there to unlink.
    let linked = {
        let mut links = mesh.links.lock().await;
        let (spawn, handler) = (spawner(&shared, &mesh, watched), handler(&shared, node));
        let linked = Linked::start(link.clone(), node, dialed, mesh.max, spawn, handler);
        let _ = holding.set(linked.clone());
        let entry = Peer {
            number,
            link,
            linked: linked.clone(),
        };
        links.insert(peer.clone(), entry);
        linked
    };
    // Our home-share pull, this end's first conversation, which the peer
    // serves. The round is noted with what it took and how long it ran (plan
    // Step 4.5); the dialer's `peer-connected` follows it.
    let began = Instant::now();
    let pulled = pull_home(&shared, &mesh, node, linked.open()).await;
    if let Ok(round) = &pulled {
        let (n, ms) = (round.applied, began.elapsed().as_millis());
        let line = format!("home round with node {peer}: {n} record(s) in {ms} ms");
        mesh.status(&line);
    }
    pulled.map(drop)
}

/// What a link's reader watches beside its frames (plan Step 4.5b): the link,
/// by its peer and its number; its far end, whose path the port notes; the
/// path noted at HELLO; and the link's conversations, which the reader holds
/// until the link ends, so that a link lives until it ends, whether or not
/// the table still holds it, as a QUIC connection did.
#[derive(Clone)]
struct Watched {
    peer: String,
    number: u64,
    remote: Option<TransportId>,
    noted: Option<PathSeen>,
    holding: Arc<OnceLock<Arc<Linked>>>,
}

/// How a link's tasks start (plan Step 4.5b): each at its site, its reader
/// watching the link as [`reading`] says.
fn spawner(shared: &Arc<Shared>, mesh: &Arc<Mesh>, watched: Watched) -> Spawn {
    let (shared, mesh) = (shared.clone(), mesh.clone());
    Arc::new(move |task, work| {
        let _ = match task {
            LinkTask::Writer => shared.tasks.spawn(Site::LinkWriter, work),
            LinkTask::Reader => {
                let reader = reading(mesh.clone(), watched.clone(), work);
                shared.tasks.spawn(Site::LinkReader, reader)
            }
            LinkTask::Inbound => shared.tasks.spawn(Site::InboundConversation, work),
        };
    })
}

/// What a link does with each conversation its peer opens (plan Step 4.5b):
/// [`serve_conversation`], for `node`, the peer its HELLO proved.
fn handler(shared: &Arc<Shared>, node: [u8; 32]) -> Handler {
    let shared = shared.clone();
    Arc::new(move |conversation| {
        let shared = shared.clone();
        Box::pin(async move {
            let _ = serve_conversation(shared, node, conversation).await;
        })
    })
}

/// How often a link's reader reads the path the carrier sends on (plan Step
/// 4.5).
const PATH_POLL: Duration = Duration::from_millis(250);

/// A link's reader, `work` (plan Step 4.5b). Meanwhile it notes each path the
/// port selects after the one noted at HELLO, reading it every
/// [`PATH_POLL`], as a poll needs no change stream from the transport. At the
/// link's end it unlinks the link, unless a newer one to the node has taken
/// its place, and notes the close.
async fn reading(mesh: Arc<Mesh>, watched: Watched, mut work: Work) {
    let Watched {
        peer,
        number,
        remote,
        mut noted,
        holding,
    } = watched;
    let watch = mesh.notes.clone().zip(remote);
    loop {
        tokio::select! {
            () = &mut work => break,
            () = tokio::time::sleep(PATH_POLL), if watch.is_some() => {
                let now = watch.as_ref().and_then(|(notes, remote)| notes.path(remote));
                let via = |path: &Option<PathSeen>| path.as_ref().map(|path| path.via.clone());
                if via(&now) != via(&noted) {
                    mesh.note_path(&peer, now.as_ref());
                    noted = now;
                }
            }
        }
    }
    drop(holding);
    mesh.unlink(&peer, number).await;
    mesh.status(&format!("link {peer} closed"));
}

/// The `relay` lines a change of the home relays' states, from `before` to
/// `now`, calls for (plan Step 4.5): `relay <url>` once one is connected,
/// which covers a change of home relay, and `relay <url> not connected:
/// <error>` once its connection has failed or dropped, each error once.
pub(super) fn relay_notes(before: &[RelayState], now: &[RelayState]) -> Vec<String> {
    let mut lines = Vec::new();
    for state in now {
        let was = before.iter().find(|held| held.url == state.url);
        if state.connected {
            if !was.is_some_and(|was| was.connected) {
                lines.push(format!("relay {}", state.url));
            }
        } else if let Some(error) = &state.error {
            if was.is_none_or(|was| was.error.as_ref() != Some(error)) {
                lines.push(format!("relay {} not connected: {error}", state.url));
            }
        }
    }
    lines
}

/// Close every live peer link and forget it, with the interests forwarded
/// over them: the assembled root's `Sessions` stop (plan Step 3.3), once
/// every task that could register a link has ended. The link table is taken
/// by value, so the mesh keeps no link that could hold the port's socket
/// open, and the links close all together, each within the port's drain,
/// with no reason (plan Step 4.5b); the port's own close, at `PeerCarrier`'s
/// release, then ends anything left. Returns how many links were closed.
pub(crate) async fn release_links(shared: &Arc<Shared>) -> usize {
    let Some(mesh) = shared.mesh.get() else {
        return 0;
    };
    let links = std::mem::take(&mut *mesh.links.lock().await);
    let mut closing: Vec<_> = links.values().map(|peer| peer.link.close()).collect();
    poll_fn(|cx| {
        closing.retain_mut(|close| close.as_mut().poll(cx).is_pending());
        if closing.is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    mesh.forwarded.lock().await.clear();
    links.len()
}
