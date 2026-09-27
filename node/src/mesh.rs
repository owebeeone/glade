//! The peer mesh (Lane R step 3): the node's links to its peers, over the peer
//! carrier port (plan Step 4.5b, part 3), so two nodes actually converge. A
//! link is one carrier link, HELLO its first frame each way; after HELLO each
//! exchange the two nodes have is a conversation on it (`conversation.rs`),
//! which the end that did not open it serves by its FIRST frame:
//!
//! - `Heads`: a home-share pull (the gap in chunks, then END — the s-sync
//!   shape, scoped to `home`). Each end opens one at HELLO, so convergence is
//!   a pull each way (GladePeerSyncNotes §4).
//! - `Subscribe`: a forwarded interest (claim routing, C2/C3).
//! - `ExchangeReq`: a forwarded exchange (`exchange.rs`).
//! - `Ops`: a peer's push of the `home` records it minted.
//!
//! Connect-time anti-entropy is scoped to the HOME share on purpose: the
//! directory replicates everywhere (WD §3 ladder 1 — every device a replica);
//! app-share content moves by INTEREST (a routed subscribe), never wholesale.
//! Ops ingested from a peer are appended through the same verify path as any
//! carrier and fanned out to local subscribers — the replica serves the reads.
//! A push the store refuses as a gap starts a pull of the pusher's home share
//! at once, one at a time per pusher (`pull_on_gap`).
//!
//! The notes the crossing reads (plan Step 4.5, part 2) are status lines, put
//! where the door says (stdout for the node): each link's path at HELLO and
//! whenever the carrier selects another, its close, each `home` round's
//! records and time, and, with `relay n0`, the home relay's state. The paths
//! and the relays' states are the port's notes (`LinkNotes`), when it has
//! them. A node with no link and no relay notes nothing.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::future::poll_fn;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant};

use glade_carrier_api::{CarrierLink, CarrierPort, TransportId};
use tokio::sync::Mutex;

use glade_grant_api::{GrantPort, Holder};
use glade_wire::cbor;
use glade_wire::generated::{
    Error, ErrorCode, Head, Heads, Op, Ops, Priority, StreamHeads, Subscribe,
};

use crate::assembly::{LinkNotes, PathSeen, RelayState};
use crate::conversation::{Conversation, Handler, LinkTask, Linked, Spawn, Work};
use crate::envelope;
use crate::frame::{Frame, MAX_FRAME_BYTES};
use crate::grants::{refusal, READ_SUBSCRIBE};
use crate::iroh_carrier::{carrier_addr, IrohCarrier};
use crate::netconf::PeerEntry;
use crate::peer::{carried, hello_accept_link, hello_dial_link, NodeIdentity, SyncOutcome};
use crate::peer::{HELLO_WITHIN, OPS_PER_CHUNK};
use crate::registry::HOME;
use crate::router::{SessionId, Zone};
use crate::server::{refuse_subscription, send, Server, Shared};
use crate::session::{heads_map, missing_for, refused_subscribe};
use crate::signing::NodeSigner;
use crate::store::{Append, Store, StoreError};
use crate::sysdir::now_ms;
use crate::tasks::Site;
use crate::transport::{key_of, Door, EndpointKey};

fn other<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// hex-render a node id (the directory rendering of the raw HELLO id).
pub(crate) fn hex_id(id: &[u8]) -> String {
    id.iter().map(|b| format!("{:02x}", b)).collect()
}

/// The node's peer fabric: the bound peer carrier port plus the live links,
/// keyed by the peer's directory node id (hex). A link is a carrier link that
/// survived the HELLO seam; the ServeClaim fold picks WHICH link a subscribe
/// rides (C2).
pub struct Mesh {
    /// The peer carrier port, bound (plan Step 4.5b): the accept loop and each
    /// dial use it, and the composition root that bound it closes it.
    port: Arc<dyn CarrierPort>,
    /// What the port notes beyond the carrier contract, if it can: each link's
    /// path and the home relays' states.
    notes: Option<Arc<dyn LinkNotes>>,
    /// This node's identity, which its HELLO signs with, and its endpoint
    /// key's id, which HELLO's channel names.
    identity: NodeIdentity,
    endpoint: [u8; 32],
    /// The most bytes a frame on a link holds, its header included: the limit
    /// the port was bound with.
    max: usize,
    /// Our directory node id (hex of the HELLO identity) — the id our own
    /// ServeClaims carry, so `who_serves == self` short-circuits to local.
    pub(crate) self_id: String,
    /// Live peer links: directory node id (hex) -> the newest link to it.
    pub(crate) links: Mutex<BTreeMap<String, Peer>>,
    /// The number the next link takes ([`Peer`]).
    numbered: AtomicU64,
    /// Zones whose interest is already forwarded to a claim holder — a second
    /// local subscriber joins the flow, it never opens a second conversation.
    pub(crate) forwarded: Mutex<BTreeSet<(String, String, Vec<u8>)>>,
    /// The door the port was lent (plan Steps 4.2b and 4.5b), if any: loaded
    /// from the served store before the first accept, then fed each transport
    /// record that lands there.
    pub(crate) door: Option<Arc<Door>>,
    /// D9's known set (plan Step 4.1b's part 2): this node, and each node a
    /// link's HELLO has proved since the mesh started. A peer's `home`
    /// record from any other node is deferred ([`Round`]).
    pub(crate) signer: NodeSigner,
    /// The pulls a gap has started ([`pull_on_gap`]): each pusher one runs
    /// from, with the gaps its pushes have been refused on since.
    gap_pulls: std::sync::Mutex<BTreeMap<[u8; 32], Gaps>>,
}

impl Mesh {
    /// Report `line` where the node reports its peers: through the door, so
    /// the assembled root puts it on its console; with no door, on stderr.
    fn report(&self, line: &str) {
        match &self.door {
            Some(door) => door.report(line),
            None => eprintln!("{line}"),
        }
    }

    /// Note `line`, a status line (plan Step 4.5): through the door, which
    /// both roots point at stdout, the assembled one through its console;
    /// with no door, nowhere.
    fn status(&self, line: &str) {
        if let Some(door) = &self.door {
            door.status(line);
        }
    }

    /// Note the path the carrier sends on to `peer`, if it has selected one.
    fn note_path(&self, peer: &str, path: Option<&PathSeen>) {
        if let Some(PathSeen { via, rtt_ms }) = path {
            self.status(&format!("link {peer} via {via}, rtt {rtt_ms} ms"));
        }
    }

    /// The path the port notes for its newest link to `remote`, if it notes
    /// paths and has selected one.
    fn path(&self, remote: Option<&TransportId>) -> Option<PathSeen> {
        self.notes.as_ref()?.path(remote?)
    }

    /// Report that an op of `share`'s `glade_id` was not sent to `node`: its
    /// frame is over the link's limit (plan Step 4.5b, question 6).
    fn over_limit(&self, node: &[u8; 32], share: &str, glade_id: &str) {
        let peer = hex_id(node);
        self.report(&format!(
            "not sent to peer {peer}: an op of {share} {glade_id} over the frame limit"
        ));
    }

    /// How many bytes of ops a chunk holds, but for an op alone: at most
    /// [`CHUNK_BYTES`], and room left for its frame under the link's limit.
    fn chunk_bytes(&self) -> usize {
        CHUNK_BYTES.min(self.max.saturating_sub(CHUNK_ROOM))
    }

    /// The conversations of the live link to `node` (hex), if it has one.
    pub(crate) async fn linked(&self, node: &str) -> Option<Arc<Linked>> {
        let links = self.links.lock().await;
        links.get(node).map(|peer| peer.linked.clone())
    }

    /// Unlink `node`'s link numbered `number`, if the table still holds it:
    /// a newer link to the node, which took its place, stays.
    async fn unlink(&self, node: &str, number: u64) {
        let mut links = self.links.lock().await;
        if links.get(node).is_some_and(|peer| peer.number == number) {
            links.remove(node);
        }
    }

    /// Note `gaps`, which a push of `pusher`'s left: with no pull from it
    /// running, one is to start for them, and they are handed back; else
    /// they wait for the running pull's end.
    fn note_gaps(&self, pusher: [u8; 32], gaps: Gaps) -> Option<Gaps> {
        if gaps.0.is_empty() {
            return None;
        }
        let mut running = self
            .gap_pulls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match running.entry(pusher) {
            Entry::Occupied(mut waiting) => {
                waiting.get_mut().merge(gaps);
                None
            }
            Entry::Vacant(slot) => {
                slot.insert(Gaps::default());
                Some(gaps)
            }
        }
    }

    /// The gaps noted from `pusher` since its pull started or last asked.
    /// With `end`, and none noted, its pull ends: no gap waits for it.
    fn take_gaps(&self, pusher: &[u8; 32], end: bool) -> Gaps {
        let mut running = self
            .gap_pulls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let noted = running
            .get_mut(pusher)
            .map(std::mem::take)
            .unwrap_or_default();
        if end && noted.0.is_empty() {
            running.remove(pusher);
        }
        noted
    }
}

/// One live link in the mesh's table (plan Step 4.5b): its number, which no
/// other link of the mesh takes, so that its end unlinks it alone and never a
/// newer link to the same node; the carrier link, which the release closes;
/// and its conversations.
pub(crate) struct Peer {
    number: u64,
    link: Arc<dyn CarrierLink>,
    linked: Arc<Linked>,
}

/// What a composition root hands the mesh (plan Step 4.5b): the peer carrier
/// port, bound with frames of at most `max_frame_bytes`, and its notes, if it
/// has them; the node's identity, and the id of the endpoint key the port was
/// lent; and the door the port was lent, if any, which the mesh loads from
/// the served store and feeds.
#[derive(Clone)]
pub struct PeerPort {
    pub port: Arc<dyn CarrierPort>,
    pub notes: Option<Arc<dyn LinkNotes>>,
    pub identity: NodeIdentity,
    pub endpoint: [u8; 32],
    pub door: Option<Arc<Door>>,
    pub max_frame_bytes: usize,
}

impl PeerPort {
    /// The iroh adapter `carrier`, lent `key` and `door`, as the port and as
    /// its notes, bound as both roots bind it, with frames of at most
    /// [`MAX_FRAME_BYTES`] ([`IrohCarrier::bind_network`]).
    pub fn iroh(
        carrier: &IrohCarrier,
        identity: NodeIdentity,
        key: &EndpointKey,
        door: Option<Arc<Door>>,
    ) -> PeerPort {
        PeerPort {
            port: Arc::new(carrier.clone()),
            notes: Some(Arc::new(carrier.clone())),
            identity,
            endpoint: key.endpoint_id,
            door,
            max_frame_bytes: MAX_FRAME_BYTES,
        }
    }
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
fn relay_notes(before: &[RelayState], now: &[RelayState]) -> Vec<String> {
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

/// Serve one conversation the peer opened, by its first frame: `Heads` = a
/// home-scoped sync pull (serve the gap, END); `Subscribe` = a forwarded
/// interest (this node is the claim holder — serve gap + live ops until the
/// interest closes); `ExchangeReq` = a forwarded exchange (this node is the
/// claim holder — the attached authority answers, one conversation one
/// exchange, `exchange.rs`); `Ops` = a peer's home-share PUSH (freshly-minted
/// directory records, the B9 step) — scoped ingest, home ops only, one frame
/// per conversation, one [`Round`]; a chain it leaves short as a gap starts a
/// pull from the peer ([`pull_on_gap`]). `node` is the peer, as its HELLO
/// proved it.
async fn serve_conversation(
    shared: Arc<Shared>,
    node: [u8; 32],
    mut conversation: Conversation,
) -> io::Result<()> {
    let Some(mesh) = shared.mesh.get().cloned() else {
        return Ok(());
    };
    match conversation.recv().await? {
        Frame::Heads(h) => serve_home(&shared, &mesh, node, conversation, h).await,
        Frame::Subscribe(s) => serve_peer_subscribe(shared, &mesh, node, conversation, s).await,
        Frame::ExchangeReq(x) => {
            crate::exchange::serve_peer_exchange(shared, node, conversation, x).await
        }
        Frame::Ops(o) => {
            let mut round = Round::new(&shared, &mesh, node);
            for op in o.ops.into_iter().filter(|op| op.share == HOME) {
                round.take(op).await;
            }
            // Noted before the round's lines, so a refusal once reported is a
            // gap some pull answers for; a new pull starts after the lines.
            let pull = mesh.note_gaps(node, std::mem::take(&mut round.gaps));
            round.end();
            conversation.end();
            if let Some(gaps) = pull {
                let pulling = shared.clone();
                shared.tasks.spawn(Site::GapPull, async move {
                    pull_on_gap(&pulling, &mesh, node, gaps).await;
                });
            }
            Ok(())
        }
        _ => Ok(()), // unknown opener: reset the conversation, never the link
    }
}

/// Push freshly-minted home-share ops to every live peer link — the traces'
/// B9 "directory ops replicate" step for records written AFTER connect-time
/// anti-entropy (claim mints, renewals, creates). Scoped to SELF-minted
/// records by construction (only `claims::publish` calls it); the receiver
/// ingests and never re-pushes — transitive gossip is deferred. Best-effort:
/// a push that arrives out of order, or after a lost one, is refused as a gap
/// and heals by the pull that starts ([`pull_on_gap`]); a lost push with none
/// after it on its chain waits for the next connect-time pull. A push is one
/// conversation per link, its one `Ops` frame then END, only queued (plan
/// Step 4.5b); one over the frame limit is not sent, with a line.
pub(crate) async fn push_home(shared: &Arc<Shared>, ops: Vec<Op>) {
    let Some(mesh) = shared.mesh.get() else { return };
    let Some(first) = ops.first() else { return };
    let zone = (first.share.clone(), first.glade_id.clone());
    let links: Vec<Arc<Linked>> = {
        let links = mesh.links.lock().await;
        links.values().map(|peer| peer.linked.clone()).collect()
    };
    let frame = Frame::Ops(Ops { ops, pri: None });
    for linked in links {
        let conversation = linked.open();
        match conversation.send(&frame) {
            Ok(()) => conversation.end(),
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
                mesh.over_limit(&linked.node(), &zone.0, &zone.1);
            }
            Err(_) => {}
        }
    }
}

/// The claim holder's side of a forwarded interest (trace C3→C5): register the
/// peer as an ordinary subscriber session of the zone, ship the resume gap
/// against the `from` heads it announced, then let the normal fan-out feed the
/// conversation until the peer ends it (interest withdrawn / link gone).
///
/// The grant check (plan Step 4.3), enforced for every peer: a share other
/// than `home` is served only to a node the fold grants `read.subscribe` on
/// it. Refused, the conversation gets the refused subscribe's two frames
/// (R6), an ack that names no zone and the reason, then END, so the
/// forwarding node's forward lapses; nothing is registered. Admitted, the
/// conversation joins the admission table, and the re-check pass ends it if
/// a later fold refuses it (`server::refresh_policy`). Check and registration
/// hold the cut, so no fold change falls between them unseen.
async fn serve_peer_subscribe(
    shared: Arc<Shared>,
    mesh: &Mesh,
    node: [u8; 32],
    mut conversation: Conversation,
    s: Subscribe,
) -> io::Result<()> {
    let key = s.key.clone().unwrap_or_default();
    let holder = Holder::Node(node);
    let cut = shared.cut.lock().await;
    if s.share != HOME {
        if let Err(denial) = shared.policy.check(&holder, READ_SUBSCRIBE, &s.share) {
            drop(cut);
            let why = refusal(&holder, READ_SUBSCRIBE, &s.share, denial);
            for frame in refused_subscribe(ErrorCode::Unauthorized, why, &s.share, &s.glade_id) {
                conversation.send(&frame)?;
            }
            conversation.end();
            return Ok(());
        }
    }
    let sid = shared.next.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    shared.out.lock().await.insert(sid, tx.clone());
    shared.router.lock().await.subscribe(sid, &s.share, &s.glade_id, &key);
    shared.admitted.lock().await.insert(sid, node);

    // Ack + gap ride the SAME outbound channel as live fan-out, so a live op
    // can never overtake the resume gap on the conversation. The gap goes in
    // chunks under the link's frame limit (plan Step 4.5b).
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
    for ops in chunked(gap, mesh.chunk_bytes()) {
        let pri = Some(Priority::Bulk);
        let _ = tx.send(Frame::Ops(Ops { ops, pri }).to_bytes());
    }
    // From here the session table holds the only sender.
    drop(tx);
    drop(cut);

    // One loop is the subscription's writer and its reader (plan Step 4.5b):
    // it queues the session's frames on the conversation until the peer ends
    // it or the link ends, or until the session leaves the session table (the
    // re-check pass refused it), when it sends END. A frame over the link's
    // limit is an op over it alone: the loop ends there, with a line, and the
    // forward lapses. It holds no lock across a receive (`conversation.rs`).
    let finished = loop {
        tokio::select! {
            queued = rx.recv() => {
                let Some(frame) = queued else {
                    break true;
                };
                if let Err(e) = conversation.send_encoded(&frame) {
                    if e.kind() == io::ErrorKind::InvalidInput {
                        mesh.over_limit(&node, &s.share, &s.glade_id);
                    }
                    break false;
                }
            }
            read = conversation.recv() => {
                if read.is_err() {
                    break false;
                }
            }
        }
    };
    shared.out.lock().await.remove(&sid);
    shared.router.lock().await.unsubscribe_all(sid);
    shared.admitted.lock().await.remove(&sid);
    if finished {
        conversation.end();
    }
    Ok(())
}

/// The A-side of the C2 decision's Forward arm: open a conversation on the
/// claim holder's link, send the interest (with our replica's heads as the
/// resume point), and ingest what comes back into the LOCAL replica — local
/// subscribers are then fed by the ordinary fan-out (replica serves reads,
/// trace C5→C6). Deduped per zone: one conversation carries any number of
/// local subscribers. The forward lapses with the conversation; a later
/// subscribe retries. A refusal the claim holder sends on it reaches the
/// local subscribers ([`lapse`]).
pub(crate) async fn forward_interest(shared: &Arc<Shared>, peer: String, share: String, glade_id: String, key: Vec<u8>) {
    let Some(mesh) = shared.mesh.get().cloned() else { return };
    let zone = (share.clone(), glade_id.clone(), key.clone());
    if !mesh.forwarded.lock().await.insert(zone.clone()) {
        return; // interest already flowing
    }
    let Some(linked) = mesh.linked(&peer).await else {
        mesh.forwarded.lock().await.remove(&zone);
        return;
    };
    let forward = shared.clone();
    shared.tasks.spawn(Site::ForwardInterest, async move {
        let refused = run_forward(&forward, &linked, &share, &glade_id, &key).await;
        lapse(&forward, &mesh, &peer, zone, refused.ok().flatten()).await;
    });
}

/// A forward's end (F5, question 25; the owner's ruling of 2026-09-27). The
/// zone leaves the forwarded set under the cut, so a subscribe registered
/// after this forwards again, and one registered before is among those told.
/// When the claim holder `peer` refused the read, at the subscribe (its ack
/// named no zone) or later (its re-check pass), each local subscriber of the
/// zone is told with a lone `Error`, the claim holder's code and its reason
/// prefixed with who refused, and leaves the zone
/// ([`crate::server::refuse_subscription`]). Nothing re-checks the refusal
/// here: a subscribe made later forwards the interest again.
async fn lapse(shared: &Arc<Shared>, mesh: &Mesh, peer: &str, zone: Zone, refused: Option<Error>) {
    let _cut = shared.cut.lock().await;
    mesh.forwarded.lock().await.remove(&zone);
    let Some(refused) = refused else {
        return;
    };
    let entries = shared.router.lock().await.entries();
    let subscribers = entries.into_iter().filter(|(_, at)| *at == zone);
    let (share, reason) = (&zone.0, &refused.message);
    let why = format!("refused by node {peer}, which serves {share}: {reason}");
    for (sid, _) in subscribers {
        refuse_subscription(shared, sid, &zone, refused.code, why.clone()).await;
    }
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

/// Run one forward until its conversation ends: `Some` refusal when the claim
/// holder refused the read, which ends it (F5).
async fn run_forward(
    shared: &Arc<Shared>,
    linked: &Arc<Linked>,
    share: &str,
    glade_id: &str,
    key: &[u8],
) -> io::Result<Option<Error>> {
    let mut conversation = linked.open();
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
    conversation.send(&Frame::Subscribe(sub))?;
    let from_sid = shared.next.fetch_add(1, Ordering::SeqCst);
    loop {
        let frame = match conversation.recv().await {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // interest closed
            Err(e) => return Err(e),
        };
        match frame {
            Frame::Ops(ops) => {
                for op in ops.ops {
                    // Scoped ingest: this conversation carries ONE zone's interest —
                    // the holder can't use it to push any other zone into our
                    // replica.
                    if op.share == share && op.glade_id == glade_id && op.key == key {
                        let _ = ingest_and_fanout(shared, from_sid, op).await;
                    }
                }
            }
            // The claim holder's refusal (plan Step 4.3), after an ack that
            // names no zone or, from its re-check pass, alone; it then
            // ends the conversation.
            Frame::Error(refused) => return Ok(Some(refused)),
            _ => {}
        }
    }
    conversation.end();
    Ok(None)
}

/// Respond to a peer's home-share pull: ship exactly the home-zone ops the
/// peer lacks (bulk, in chunks under the link's frame limit, plan Step
/// 4.5b), then END — END = gap complete. Scoped to HOME: connect-time
/// anti-entropy replicates the directory only; app shares move by interest
/// (see the module note). `node` is the peer, as its HELLO proved it.
async fn serve_home(
    shared: &Arc<Shared>,
    mesh: &Mesh,
    node: [u8; 32],
    conversation: Conversation,
    their: Heads,
) -> io::Result<()> {
    let mut by_zone: BTreeMap<(String, String, Vec<u8>), BTreeMap<String, i64>> = BTreeMap::new();
    for sh in their.streams {
        let m = by_zone.entry((sh.share.clone(), sh.glade_id.clone(), sh.key.clone())).or_default();
        for hd in sh.heads {
            m.insert(hd.origin, hd.seq);
        }
    }
    // Collect the gap under the store lock, then send it without.
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
    for ops in chunked(gap, mesh.chunk_bytes()) {
        let first = ops.first();
        let zone = first.map(|op| (op.share.clone(), op.glade_id.clone()));
        let pri = Some(Priority::Bulk);
        let sent = conversation.send(&Frame::Ops(Ops { ops, pri }));
        if let (Err(e), Some((share, glade_id))) = (&sent, zone) {
            if e.kind() == io::ErrorKind::InvalidInput {
                mesh.over_limit(&node, &share, &glade_id);
            }
        }
        sent?;
    }
    conversation.end(); // END = gap complete
    Ok(())
}

/// The most bytes of ops a chunk holds, but for an op alone (plan Step
/// 4.5b, question 6): 1 MiB.
const CHUNK_BYTES: usize = 1 << 20;

/// What a chunk's frame holds beside its ops, with room to spare: the
/// conversation's header and the `Ops` frame's tag, map, keys, array head and
/// priority, 12 bytes for up to 255 ops.
const CHUNK_ROOM: usize = 32;

/// `ops`, in order, in chunks (plan Step 4.5b, question 6): each of at most
/// [`OPS_PER_CHUNK`] ops and, but for an op alone, of at most `bytes` bytes of
/// them as each encodes.
fn chunked(ops: Vec<Op>, bytes: usize) -> Vec<Vec<Op>> {
    let mut chunks = Vec::new();
    let (mut chunk, mut held) = (Vec::new(), 0);
    for op in ops {
        let size = cbor::encode(&op.to_cbor()).len();
        let full = chunk.len() == OPS_PER_CHUNK || held + size > bytes;
        if full && !chunk.is_empty() {
            chunks.push(std::mem::take(&mut chunk));
            held = 0;
        }
        held += size;
        chunk.push(op);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

/// Pull the peer's home-share gap on `conversation`, one this end opened:
/// announce our home heads, ingest until the peer's END. Every op lands
/// through the same verify path as any carrier (`Store::append` chain
/// checks) and fans out to local subscribers — a directory update reaches a
/// live `dir.workspaces` subscription with no re-request (the B9 step).
/// Non-home ops on this conversation are dropped: the pull asked for the
/// directory, a peer can't use it to push app content. The pull is one
/// [`Round`], whose outcome it returns.
async fn pull_home(
    shared: &Arc<Shared>,
    mesh: &Mesh,
    peer: [u8; 32],
    mut conversation: Conversation,
) -> io::Result<SyncOutcome> {
    let ours: Vec<StreamHeads> = {
        let st = shared.store.lock().await;
        st.all_heads().into_iter().filter(|sh| sh.share == HOME).collect()
    };
    conversation.send(&Frame::Heads(Heads { streams: ours }))?;
    let mut round = Round::new(shared, mesh, peer);
    let ended = loop {
        let frame = match conversation.recv().await {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break Ok(()), // peer's END = done
            Err(e) => break Err(e),
        };
        if let Frame::Ops(ops) = frame {
            for op in ops.ops.into_iter().filter(|op| op.share == HOME) {
                round.take(op).await;
            }
        }
    };
    conversation.end();
    let outcome = round.end();
    ended.map(|()| outcome)
}

/// One round of a peer's `home` records, a pull or one push (plan Step 4.1b's
/// part 2; `GladeNodeSigning.md` D9). A record is taken only from a node this
/// node knows, itself or one whose HELLO it has verified this run. Another
/// node's chain is deferred for the round: never stored or folded, and asked
/// for again at the next pull, whose heads lack it. A chain the store refuses
/// is cut short too, since each later op chains on the one before. At the
/// round's end each chain cut short is reported, a line each.
struct Round<'a> {
    shared: &'a Arc<Shared>,
    mesh: &'a Mesh,
    peer: [u8; 32],
    /// A fresh session id no local session holds: fan-out excludes only the
    /// ingesting link, never a real subscriber.
    from: SessionId,
    outcome: SyncOutcome,
    /// Each chain cut short, by (stream, origin): why, and how many of its
    /// ops were not taken.
    cut: BTreeMap<(String, String), (Cut, usize)>,
    /// The chains cut short as a gap, which a push's round hands to
    /// [`pull_on_gap`]. A pull's round leaves them: a pull starts no pull.
    gaps: Gaps,
}

/// Why a round cut a chain short.
enum Cut {
    /// Its origin is not a node this node knows (D9).
    Deferred,
    /// The store refused one of its ops, for this reason.
    Refused(String),
}

impl<'a> Round<'a> {
    fn new(shared: &'a Arc<Shared>, mesh: &'a Mesh, peer: [u8; 32]) -> Round<'a> {
        let from = shared.next.fetch_add(1, Ordering::SeqCst);
        let (outcome, cut) = (SyncOutcome::default(), BTreeMap::new());
        Round {
            shared,
            mesh,
            peer,
            from,
            outcome,
            cut,
            gaps: Gaps::default(),
        }
    }

    /// Take `op`, a `home` op the peer sent, unless its chain is cut short.
    async fn take(&mut self, op: Op) {
        let chain = (op.glade_id.clone(), op.origin.clone());
        if let Some((_, missed)) = self.cut.get_mut(&chain) {
            *missed += 1;
            self.gaps.reached(&chain, op.seq);
            return;
        }
        let known = key_of(&op.origin).is_some_and(|node| self.mesh.signer.knows(&node));
        if !known {
            self.cut.insert(chain, (Cut::Deferred, 1));
            return;
        }
        let seq = op.seq;
        match ingest_and_fanout(self.shared, self.from, op).await {
            Ok(_) => self.outcome.applied += 1,
            Err(e) => {
                if matches!(e, StoreError::Gap { .. }) {
                    self.gaps.refused(chain.clone(), seq);
                }
                self.cut.insert(chain, (Cut::Refused(e.to_string()), 1));
            }
        }
    }

    /// End the round: report each chain cut short, and hand back what the
    /// round took, refused and deferred.
    fn end(self) -> SyncOutcome {
        let Round {
            mesh,
            peer,
            mut outcome,
            cut,
            ..
        } = self;
        let peer = hex_id(&peer);
        for ((stream, origin), (why, n)) in cut {
            let head = format!("{n} home record(s) of node {origin} on {stream} from peer {peer}");
            let chain = (HOME.to_string(), stream, Vec::new(), origin);
            let line = match why {
                Cut::Deferred => {
                    outcome.deferred.push(chain);
                    format!("deferred {head}: not a node this node knows")
                }
                Cut::Refused(why) => {
                    outcome.rejected.push(chain);
                    format!("refused {head}: {why}")
                }
            };
            mesh.report(&line);
        }
        outcome
    }
}

/// Pull from `pusher` at once, the store having refused a push of its as a
/// gap (the hardening's question 2, ruled (b)): its home share, from this
/// node's heads, on a new conversation of its live link, as at connect. So a
/// chain that a push reached out of order heals now, not at the next link.
/// Receiver-side only, with no wire change. One pull runs per pusher. A gap
/// noted while it runs is judged at its end, and pulled for again only if
/// still short, since its push may have come after the pusher answered. A
/// gap noted before it began is covered by it, and another pull would not
/// heal what it did not. Each pull reports a line: what it took, and each
/// chain it was for, healed or not. A deferred chain (D9) is not a gap: it
/// waits for the next pull at connect.
async fn pull_on_gap(shared: &Arc<Shared>, mesh: &Mesh, pusher: [u8; 32], mut gaps: Gaps) {
    let peer = hex_id(&pusher);
    loop {
        let pulled = pull_from(shared, mesh, &peer, pusher).await;
        let mut during = mesh.take_gaps(&pusher, false);
        let (line, again) = {
            let st = shared.store.lock().await;
            let again = if pulled.is_ok() {
                during.split_short(&st)
            } else {
                Gaps::default()
            };
            gaps.merge(during);
            (gaps.line(&st, &peer, &pulled), again)
        };
        mesh.report(&line);
        if !again.0.is_empty() {
            gaps = again;
            continue;
        }
        gaps = mesh.take_gaps(&pusher, true);
        if gaps.0.is_empty() {
            return;
        }
    }
}

/// One pull of `pusher`'s home share, on a new conversation of its live link.
async fn pull_from(
    shared: &Arc<Shared>,
    mesh: &Mesh,
    peer: &str,
    pusher: [u8; 32],
) -> io::Result<SyncOutcome> {
    let linked = mesh.linked(peer).await;
    let gone = || io::Error::new(io::ErrorKind::NotConnected, "no live link");
    pull_home(shared, mesh, pusher, linked.ok_or_else(gone)?.open()).await
}

/// The chains a peer's pushes left short as a gap, by (stream, origin): the
/// highest seq of each that they carried, and how many of them were refused
/// on it.
#[derive(Default)]
struct Gaps(BTreeMap<(String, String), (i64, usize)>);

impl Gaps {
    /// Note a push refused on `chain` as a gap, at `seq`.
    fn refused(&mut self, chain: (String, String), seq: i64) {
        let (last, pushes) = self.0.entry(chain).or_insert((seq, 0));
        *last = seq.max(*last);
        *pushes += 1;
    }

    /// Note that the push carried `chain` to `seq`, if it is short.
    fn reached(&mut self, chain: &(String, String), seq: i64) {
        if let Some((last, _)) = self.0.get_mut(chain) {
            *last = seq.max(*last);
        }
    }

    /// Add `other`'s chains and refusals to these.
    fn merge(&mut self, other: Gaps) {
        for (chain, (seq, pushes)) in other.0 {
            let (last, refused) = self.0.entry(chain).or_insert((seq, 0));
            *last = seq.max(*last);
            *refused += pushes;
        }
    }

    /// Split off the chains that `st` holds short of their seq.
    fn split_short(&mut self, st: &Store) -> Gaps {
        let is_short = |((stream, origin), (seq, _)): &((String, String), (i64, usize))| {
            !holds(st, stream, origin, *seq)
        };
        let (short, held) = std::mem::take(&mut self.0).into_iter().partition(is_short);
        self.0 = held;
        Gaps(short)
    }

    /// The line a pull for these gaps reports, from `peer`: what it took, and
    /// each chain, healed or not as `st` holds it.
    fn line(&self, st: &Store, peer: &str, pulled: &io::Result<SyncOutcome>) -> String {
        let mut chains = Vec::new();
        for ((stream, origin), (seq, _)) in &self.0 {
            let healed = if holds(st, stream, origin, *seq) {
                "healed"
            } else {
                "not healed"
            };
            chains.push(format!("{stream} of node {origin} {healed}"));
        }
        let gaps: usize = self.0.values().map(|(_, pushes)| pushes).sum();
        let chains = chains.join("; ");
        match pulled {
            Ok(outcome) => {
                let n = outcome.applied;
                format!("pulled {n} home record(s) from peer {peer} after {gaps} gap(s): {chains}")
            }
            Err(e) => format!("a pull from peer {peer} after {gaps} gap(s) failed: {e}: {chains}"),
        }
    }
}

/// Whether `st` holds `origin`'s chain of `home`'s `stream` up to `seq`.
fn holds(st: &Store, stream: &str, origin: &str, seq: i64) -> bool {
    let heads = st.heads(HOME, stream, &[]);
    heads
        .into_iter()
        .any(|(held, head)| held == origin && head >= seq)
}

/// Land one peer-ingested op in the local replica (same chain checks as any
/// append) and fan it out to the local subscribers of its zone. Rejected or
/// duplicate ops fan out to no one — the fold only ever sees the valid set.
/// The cut is held from the append until the fan-out is queued, so a local
/// subscriber gets the op once, after its ack (R4, client-writes plan Step
/// 2.2). Returns the store's answer.
pub(crate) async fn ingest_and_fanout(
    shared: &Arc<Shared>,
    from: SessionId,
    op: Op,
) -> Result<Append, StoreError> {
    let (share, glade_id, key) = (op.share.clone(), op.glade_id.clone(), op.key.clone());
    let _cut = shared.cut.lock().await;
    let res = shared.store.lock().await.append(op.clone());
    if matches!(res, Ok(Append::Appended)) {
        let mesh = shared.mesh.get();
        let revoked = mesh.and_then(|mesh| Some((mesh, mesh.door.as_ref()?.note(&op)?)));
        if let Some((mesh, pair)) = revoked {
            close_revoked(mesh, pair).await;
        }
        let targets = shared.router.lock().await.route(from, &share, &glade_id, &key);
        if !targets.is_empty() {
            let frame = Frame::Ops(Ops { ops: vec![op], pri: None });
            for t in targets {
                send(shared, t, &frame).await;
            }
        }
    }
    res
}

/// End the revoking node's live link, if it rides the key it revoked (plan
/// Step 4.2b): its HELLO was taken before the door knew. `Linked::end`
/// returns at once, as it must under the cut, and the link's writer closes
/// it (plan Step 4.5b); the link leaves the table when its reader sees it end.
async fn close_revoked(mesh: &Mesh, (endpoint, node): ([u8; 32], [u8; 32])) {
    if let Some(peer) = mesh.links.lock().await.get(&hex_id(&node)) {
        if peer.link.remote_id() == Some(TransportId(endpoint.to_vec())) {
            peer.linked.end();
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
            let Some(c) = envelope::folded(&op, crate::sysdata::ServeClaim::from_cbor) else {
                continue;
            };
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
            let entry = envelope::folded(&op, crate::sysdata::WorkspaceEntry::from_cbor);
            if entry.is_some_and(|entry| entry.workspace == share) {
                return true;
            }
        }
    }
    for (origin, _) in store.heads(HOME, crate::registry::G_CLAIMS, &[]) {
        for op in store.scan(HOME, crate::registry::G_CLAIMS, &[], &origin, i64::MIN) {
            let claim = envelope::folded(&op, crate::sysdata::ServeClaim::from_cbor);
            if claim.is_some_and(|claim| claim.share == share) {
                return true;
            }
        }
    }
    false
}

// The one helper every two-node test binds a node's mesh with (plan Step
// 4.5b), here, in `exchange.rs` and in `claims.rs`. A braced module, so the
// condition encloses the whole section.
#[cfg(test)]
pub(crate) mod testing {
    use std::num::NonZeroUsize;
    use std::sync::Arc;

    use glade_carrier_api::{CarrierAddr, CarrierConfig, CarrierPort};

    use super::PeerPort;
    use crate::frame::MAX_FRAME_BYTES;
    use crate::iroh_carrier::{entry_of, IrohCarrier, Lent, FIRST_WORD};
    use crate::netconf::{PeerEntry, Relays};
    use crate::peer::NodeIdentity;
    use crate::server::Server;
    use crate::transport::{Door, EndpointKey};

    /// A fresh endpoint key, which dies with the test.
    pub(crate) fn endpoint_key() -> EndpointKey {
        EndpointKey::from_seed(crate::signing::random_seed().unwrap())
    }

    /// Enable `server`'s mesh as the node `identity`, on an adapter of its
    /// own: a fresh endpoint key, no door and the node's frame limit. Its
    /// address, as a peer's entry names it.
    pub(crate) async fn meshed(server: &Server, identity: NodeIdentity) -> PeerEntry {
        on_carrier(server, identity, endpoint_key(), None, MAX_FRAME_BYTES).await
    }

    /// Enable `server`'s mesh as the node `identity`, on an adapter of its
    /// own lent `key` and `door`, bound on `127.0.0.1` alone with frames of
    /// at most `max` bytes, the mesh's limit too. Its address, as a peer's
    /// entry names it.
    pub(crate) async fn on_carrier(
        server: &Server,
        identity: NodeIdentity,
        key: EndpointKey,
        door: Option<Arc<Door>>,
        max: usize,
    ) -> PeerEntry {
        let (relays, first_word) = (Relays::Off, FIRST_WORD);
        let lent = Lent {
            key,
            door: door.clone(),
            relays,
            first_word,
        };
        let carrier = IrohCarrier::new(Some(lent));
        let local = CarrierAddr("127.0.0.1:0".into());
        let max_frame_bytes = NonZeroUsize::new(max).unwrap();
        let config = CarrierConfig {
            local,
            max_frame_bytes,
        };
        let bound = carrier.bind(config).await.unwrap();
        let port = PeerPort {
            max_frame_bytes: max,
            ..PeerPort::iroh(&carrier, identity, &key, door)
        };
        server.enable_mesh(port).await.unwrap();
        entry_of(&bound).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{endpoint_key, meshed, on_carrier};
    use super::*;
    use crate::claims::testing;
    use crate::registry::{Record, RegistryApi, G_CLAIMS, G_PRINCIPALS};
    use crate::sysdata::{CapabilityGrant, CapabilityRevocation, ServeClaim, WorkspaceEntry};
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

        meshed(&a, boot_a.identity().unwrap()).await;
        let at_b = meshed(&b, boot_b.identity().unwrap()).await;

        // The HELLO identity is the directory identity (one id, two renderings).
        let peer = a.connect_peer(at_b).await.unwrap();
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

    /// Plan Step 4.2: each booted node binds its endpoint with its own
    /// `endpoint.key`, and its binding, minted at boot, reaches the peer's
    /// served store by the pull the HELLO opens. There it folds live for the
    /// very key the peer's connection came from, which is what 4.2b's door
    /// will read. It checks no refusal: 4.2a builds no door.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peers_binding_arrives_by_the_pull_and_folds_live() {
        use crate::transport::{Bound, TransportFold};
        let boot_a = boot_at(fresh("tb-a-sys"), "gianni").unwrap();
        let boot_b = boot_at(fresh("tb-b-sys"), "gianni").unwrap();
        let (id_a, key_a) = (boot_a.identity().unwrap(), boot_a.endpoint_key());
        let (id_b, key_b) = (boot_b.identity().unwrap(), boot_b.endpoint_key());
        let a = Server::open(fresh("tb-a-store")).unwrap();
        let b = Server::open(fresh("tb-b-store")).unwrap();
        a.adopt_boot(boot_a).await.unwrap();
        b.adopt_boot(boot_b).await.unwrap();
        let max = crate::frame::MAX_FRAME_BYTES;
        on_carrier(&a, id_a, key_a, None, max).await;
        let at_b = on_carrier(&b, id_b, key_b, None, max).await;
        assert_eq!(at_b.key, key_b.endpoint_id);
        a.connect_peer(at_b).await.unwrap();

        let live = |node: [u8; 32], key: [u8; 32]| {
            move |st: &Store| {
                TransportFold::of_store(st).binds(&node, &key, now_ms()) == Bound::Live
            }
        };
        let (b_at_a, a_at_b) = (
            live(id_b.node_id, key_b.endpoint_id),
            live(id_a.node_id, key_a.endpoint_id),
        );
        wait_for(&a, b_at_a, "B's binding at A").await;
        wait_for(&b, a_at_b, "A's binding at B").await;
    }

    /// Plan Step 4.1b, F2 closed for a peer's push: A pushes B a claim on
    /// `ws-razel` at a higher epoch than B's own, which would route B's
    /// share to A, in forms A did not sign: bare, under A's id, and sealed by
    /// another key. B takes neither: when A's genuine marker, pushed after
    /// them in the same frame, has landed, B still routes `ws-razel` to
    /// itself and holds A's claims chain as it was. The same claim, sealed by
    /// A, then lands in the slot they would have taken.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_claim_a_peer_did_not_sign_is_refused_where_it_is_pushed() {
        let boot_a = boot_at(fresh("forged-a-sys"), "gianni").unwrap();
        let mut boot_b = boot_at(fresh("forged-b-sys"), "gianni").unwrap();
        let (a_id, b_id) = (boot_a.node_id.clone(), boot_b.node_id.clone());
        let claim = |node: &str, epoch| {
            let (node, share) = (node.to_string(), "ws-razel".to_string());
            let lease_expiry_ms = now_ms() + 30_000;
            Record::Serve(ServeClaim {
                node,
                share,
                lease_expiry_ms,
                epoch,
            })
        };
        boot_b.registry.append(claim(&b_id, 1), &b_id).unwrap();
        let a = Server::open(fresh("forged-a-store")).unwrap();
        let b = Server::open(fresh("forged-b-store")).unwrap();
        a.seed_registry(&boot_a.registry.snapshot()).await;
        b.seed_registry(&boot_b.registry.snapshot()).await;
        meshed(&a, boot_a.identity().unwrap()).await;
        let at_b = meshed(&b, boot_b.identity().unwrap()).await;
        a.connect_peer(at_b).await.unwrap();
        let held = |node: String| move |st: &Store| st.scan(HOME, G_CLAIMS, &[], &node, -1).len();
        wait_for(
            &b,
            |st| held(a_id.clone())(st) == 1,
            "B to hold A's home claim",
        )
        .await;

        let mut a_records = boot_a.registry.clone();
        let genuine = a_records.append_returning(claim(&a_id, 99), &a_id).unwrap();
        let bare = Op {
            payload: crate::envelope::record_bytes(&genuine.payload),
            ..genuine.clone()
        };
        let other = crate::peer::NodeIdentity::from_key([26; 32]);
        let forged = Op {
            payload: crate::envelope::seal(&other, &bare),
            ..genuine.clone()
        };
        let marker = Record::Principal(crate::sysdata::PrincipalRecord {
            principal: "marker".into(),
        });
        let marker = a_records.append_returning(marker, &a_id).unwrap();
        let linked = a.shared.mesh.get().unwrap().linked(&b_id).await.unwrap();
        pushed(&linked, vec![bare, forged, marker]);
        let principals = |st: &Store| {
            st.scan(HOME, crate::registry::G_PRINCIPALS, &[], &a_id, -1)
                .len()
        };
        wait_for(&b, |st| principals(st) == 1, "A's marker at B").await;
        {
            let st = b.shared.store.lock().await;
            assert_eq!(who_serves(&st, "ws-razel", now_ms()), Some(b_id.clone()));
            assert_eq!(held(a_id.clone())(&st), 1, "A's claims chain as it was");
        }
        pushed(&linked, vec![genuine.clone()]);
        wait_for(
            &b,
            |st| held(a_id.clone())(st) == 2,
            "A's signed claim at B",
        )
        .await;
        let st = b.shared.store.lock().await;
        assert_eq!(st.scan(HOME, G_CLAIMS, &[], &a_id, 0), [genuine]);
        assert_eq!(who_serves(&st, "ws-razel", now_ms()), Some(a_id.clone()));
    }

    // ---- the door (plan Step 4.2b), over real iroh ---------------------------

    const A_SEED: [u8; 32] = [21; 32];
    const A_KEY: [u8; 32] = [22; 32];
    const B_SEED: [u8; 32] = [23; 32];
    const B_KEY: [u8; 32] = [24; 32];

    /// The endpoint id the seed `key` gives, and the node id of `seed`.
    fn endpoint_of(key: [u8; 32]) -> [u8; 32] {
        crate::transport::EndpointKey::from_seed(key).endpoint_id
    }
    fn node_of(seed: [u8; 32]) -> [u8; 32] {
        crate::signing::public_key(&seed)
    }

    /// The refusal lines a door reported.
    type Lines = Arc<std::sync::Mutex<Vec<String>>>;

    /// A node behind a door that configures `configured`: node key `seed`,
    /// endpoint key `key`; `records` in its served store before the mesh
    /// starts; the door's refusal lines; its dialable address.
    async fn behind_door(
        name: &str,
        keys: ([u8; 32], [u8; 32]),
        configured: &[[u8; 32]],
        records: &[Op],
    ) -> (Server, Lines, PeerEntry) {
        let (server, lines, _, addr) = noting_door(name, keys, configured, records).await;
        (server, lines, addr)
    }

    /// [`behind_door`], its door also taking the mesh's status lines (plan
    /// Step 4.5), which come back after its refusal lines.
    async fn noting_door(
        name: &str,
        (seed, key): ([u8; 32], [u8; 32]),
        configured: &[[u8; 32]],
        records: &[Op],
    ) -> (Server, Lines, Lines, PeerEntry) {
        let server = Server::open(fresh(name)).unwrap();
        for op in records {
            server.shared.store.lock().await.append(op.clone()).unwrap();
        }
        let (lines, notes) = (Lines::default(), Lines::default());
        let (sink, noting) = (lines.clone(), notes.clone());
        let door = Door::new(configured.iter().copied(), move |line: &str| {
            sink.lock().unwrap().push(line.into())
        });
        let door = door.with_status(move |line: &str| noting.lock().unwrap().push(line.into()));
        let (identity, key) = (
            crate::peer::NodeIdentity::from_key(seed),
            crate::transport::EndpointKey::from_seed(key),
        );
        let max = crate::frame::MAX_FRAME_BYTES;
        let at = on_carrier(&server, identity, key, Some(Arc::new(door)), max).await;
        (server, lines, notes, at)
    }

    /// A note that begins with `head` and ends with a time, ` ms`.
    fn timed(head: String) -> impl Fn(&str) -> bool {
        move |line: &str| line.starts_with(&head) && line.ends_with(" ms")
    }

    /// Wait, bounded at 5 s, for a line in `lines` that `wanted` takes.
    async fn noted(lines: &Lines, wanted: impl Fn(&str) -> bool) {
        for _ in 0..500 {
            if lines.lock().unwrap().iter().any(|line| wanted(line)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("not noted: {:?}", lines.lock().unwrap());
    }

    /// Plan Step 4.5, over real iroh on loopback: each end of a link notes the
    /// path it sends on at HELLO, `link <node> via direct <ip:port>, rtt <n>
    /// ms`, the address the other end is bound at, and notes `link <node>
    /// closed` once the other end has closed.
    #[tokio::test(flavor = "multi_thread")]
    async fn each_end_notes_its_link_at_hello_and_its_close() {
        let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));
        let admits_a = [endpoint_of(A_KEY)];
        let b = noting_door("notes-link-b", (B_SEED, B_KEY), &admits_a, &[]).await;
        let (_b, _, b_notes, at_b) = b;
        let admits_b = [endpoint_of(B_KEY)];
        let a = noting_door("notes-link-a", (A_SEED, A_KEY), &admits_b, &[]).await;
        let (a, _, a_notes, at_a) = a;
        a.connect_peer(at_b.clone()).await.expect("a link");
        let link = |id: &str, at: &PeerEntry| format!("link {id} via direct {}, rtt ", at.via[0]);
        noted(&a_notes, timed(link(&b_id, &at_b))).await;
        noted(&b_notes, timed(link(&a_id, &at_a))).await;

        a.shared.mesh.get().unwrap().port.close().await;
        let closed = format!("link {a_id} closed");
        noted(&b_notes, |line| line == closed).await;
    }

    /// Plan Step 4.5: each end notes its `home` round when its pull from the
    /// other ends, `home round with node <id>: <n> record(s) in <ms> ms`,
    /// `n` being the other's `home` records it took: B holds two of its own,
    /// and A one.
    #[tokio::test(flavor = "multi_thread")]
    async fn each_end_notes_its_home_round() {
        use crate::sysdata::{NodeRecord, PrincipalRecord};
        let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));
        let presence = |id: &str| {
            let (node_id, operator) = (id.to_string(), "gianni".to_string());
            Record::Node(NodeRecord { node_id, operator })
        };
        let principal = Record::Principal(PrincipalRecord {
            principal: "alice".into(),
        });
        let b_own = [signed(B_SEED, presence(&b_id)), signed(B_SEED, principal)];
        let a_own = [signed(A_SEED, presence(&a_id))];
        let admits_a = [endpoint_of(A_KEY)];
        let b = noting_door("notes-round-b", (B_SEED, B_KEY), &admits_a, &b_own).await;
        let (_b, _, b_notes, at_b) = b;
        let admits_b = [endpoint_of(B_KEY)];
        let a = noting_door("notes-round-a", (A_SEED, A_KEY), &admits_b, &a_own).await;
        let (a, _, a_notes, _) = a;
        a.connect_peer(at_b).await.expect("a link");
        let round = |id: &str, n: usize| format!("home round with node {id}: {n} record(s) in ");
        noted(&a_notes, timed(round(&b_id, 2))).await;
        noted(&b_notes, timed(round(&a_id, 1))).await;
    }

    /// Plan Step 4.5: the `relay` lines, from home relay states as the
    /// adapter reads them off iroh, with no relay reached: `relay <url>` once
    /// one is connected, again after a drop and at a change of home relay,
    /// and `relay <url> not connected: <error>` once for each error, where
    /// iroh reports a failure again at every retry.
    #[test]
    fn the_relay_lines_follow_the_home_relays_states() {
        let ap = "https://aps1-1.relay.n0.iroh.link./";
        let eu = "https://euc1-1.relay.n0.iroh.link./";
        let state = |url: &str, connected: bool, error: Option<&str>| {
            let (url, error) = (url.to_string(), error.map(str::to_string));
            vec![RelayState {
                url,
                connected,
                error,
            }]
        };
        let failed = |error: &str| vec![format!("relay {ap} not connected: {error}")];
        let (reset, late) = ("connection reset", "timed out");
        let steps = [
            (vec![], vec![]),
            (state(ap, false, None), vec![]),
            (state(ap, true, None), vec![format!("relay {ap}")]),
            (state(ap, true, None), vec![]),
            (state(ap, false, Some(reset)), failed(reset)),
            (state(ap, false, Some(reset)), vec![]),
            (state(ap, false, Some(late)), failed(late)),
            (state(ap, true, None), vec![format!("relay {ap}")]),
            (state(eu, false, None), vec![]),
            (state(eu, true, None), vec![format!("relay {eu}")]),
        ];
        let mut before = Vec::new();
        for (now, lines) in steps {
            assert_eq!(relay_notes(&before, &now), lines, "{now:?}");
            before = now;
        }
    }

    /// The node `seed`'s record, first on its chain, sealed by it (plan Step
    /// 4.1b).
    fn signed(seed: [u8; 32], record: Record) -> Op {
        crate::envelope::testing::sealed(seed, record)
    }

    async fn links(server: &Server) -> usize {
        server.shared.mesh.get().unwrap().links.lock().await.len()
    }

    /// Push `ops` on a conversation of `linked`'s own, as `push_home` does:
    /// one `Ops` frame, then END.
    fn pushed(linked: &Arc<Linked>, ops: Vec<Op>) {
        let (conversation, frame) = (linked.open(), Frame::Ops(Ops { ops, pri: None }));
        conversation.send(&frame).unwrap();
        conversation.end();
    }

    /// Done-when (plan Step 4.2b): an endpoint key the door does not know is
    /// refused at accept, before HELLO. The refusing node reports one line
    /// naming the key and the reason; the dialer learns no reason.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_endpoint_key_is_refused_at_accept_and_reported() {
        let (b, b_lines, at_b) = behind_door("door-unknown-b", (B_SEED, B_KEY), &[], &[]).await;
        let (a, _, _) = behind_door(
            "door-unknown-a",
            (A_SEED, A_KEY),
            &[endpoint_of(B_KEY)],
            &[],
        )
        .await;
        let refused = a
            .connect_peer(at_b.clone())
            .await
            .expect_err("an unknown key linked");
        assert_ne!(refused.kind(), io::ErrorKind::PermissionDenied, "{refused}");
        assert!(
            !refused.to_string().contains("unknown"),
            "a reason crossed: {refused}"
        );
        let line = format!(
            "peer refused: endpoint {}: unknown endpoint key",
            crate::transport::tag(&endpoint_of(A_KEY))
        );
        assert_eq!(*b_lines.lock().unwrap(), [line]);
        assert_eq!(links(&b).await, 0);
    }

    /// Done-when: a key bound by a record the door holds links; once the
    /// door's fold has the node's revocation of it, the live link is closed
    /// and the next connection is refused at accept.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_bound_key_links_and_is_refused_once_its_revocation_lands() {
        use crate::transport::{sign_binding, sign_revocation};
        let bound = signed(
            A_SEED,
            Record::Transport(sign_binding(&A_SEED, &endpoint_of(A_KEY), 1)),
        );
        let (b, b_lines, at_b) = behind_door("door-bound-b", (B_SEED, B_KEY), &[], &[bound]).await;
        let (a, _, _) =
            behind_door("door-bound-a", (A_SEED, A_KEY), &[endpoint_of(B_KEY)], &[]).await;
        let linked = a.connect_peer(at_b.clone()).await;
        linked.expect("a bound key links");
        assert_eq!(links(&b).await, 1);

        let revoked = signed(
            A_SEED,
            Record::TransportRevoke(sign_revocation(&A_SEED, &endpoint_of(A_KEY))),
        );
        let from = b.shared.next.fetch_add(1, Ordering::SeqCst);
        let landed = ingest_and_fanout(&b.shared, from, revoked).await;
        assert!(matches!(landed, Ok(Append::Appended)), "{landed:?}");
        for _ in 0..500 {
            if links(&b).await == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(links(&b).await, 0, "the live link was closed");
        a.connect_peer(at_b.clone())
            .await
            .expect_err("a revoked key linked");
        let key = crate::transport::tag(&endpoint_of(A_KEY));
        let node = hex_id(&node_of(A_SEED));
        let line = format!("peer refused: endpoint {key}: revoked by node {node}");
        assert_eq!(*b_lines.lock().unwrap(), [line]);
    }

    /// Done-when's HELLO half: a key the door knows, bound to another node,
    /// admits the connection at accept, and the HELLO of a node not bound to
    /// it is refused and reported, unanswered.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_key_bound_to_another_node_cannot_complete_hello() {
        let other = [25; 32];
        let binding = crate::transport::sign_binding(&other, &endpoint_of(A_KEY), 1);
        let elsewhere = signed(other, Record::Transport(binding));
        let (b, b_lines, at_b) =
            behind_door("door-elsewhere-b", (B_SEED, B_KEY), &[], &[elsewhere]).await;
        let (a, _, _) = behind_door(
            "door-elsewhere-a",
            (A_SEED, A_KEY),
            &[endpoint_of(B_KEY)],
            &[],
        )
        .await;
        a.connect_peer(at_b.clone())
            .await
            .expect_err("a node linked through another's key");
        let (m, n) = (hex_id(&node_of(other)), hex_id(&node_of(A_SEED)));
        let why = format!("HELLO refused: bound to node {m}, not {n}");
        let line = format!(
            "peer refused: endpoint {}: {why}",
            crate::transport::tag(&endpoint_of(A_KEY))
        );
        assert_eq!(*b_lines.lock().unwrap(), [line]);
        assert_eq!(links(&b).await, 0);
    }

    // ---- D9's known set (plan Step 4.1b's part 2), over real iroh ----------

    const C_SEED: [u8; 32] = [27; 32];
    const C_KEY: [u8; 32] = [28; 32];

    /// A principal's record.
    fn principal(name: &str) -> Record {
        let principal = name.into();
        Record::Principal(crate::sysdata::PrincipalRecord { principal })
    }

    /// Plan Step 4.1b's part 2 (D9). B holds its own record and two of C's,
    /// a node A has not met. A's pull from B takes B's record and defers C's
    /// chain, kept nowhere, with one line. Once A has met C, by a HELLO, B's
    /// push of C's records lands them; and a record under B's id that B did
    /// not sign, in the same push, is refused, with one line.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_third_nodes_records_are_deferred_until_its_hello_and_reported() {
        let c_identity = crate::peer::NodeIdentity::from_key(C_SEED);
        let c_id = hex_id(&c_identity.node_id);
        let mut c_records = crate::registry::Registry::sealed(c_identity);
        let c_ops: Vec<Op> = ["c0", "c1"]
            .into_iter()
            .map(|name| c_records.append_returning(principal(name), &c_id).unwrap())
            .collect();
        let b_op = signed(B_SEED, principal("b"));
        let records = [c_ops.clone(), vec![b_op.clone()]].concat();
        let a_keys = [endpoint_of(A_KEY)];
        let (b, _, at_b) = behind_door("d9-b", (B_SEED, B_KEY), &a_keys, &records).await;
        let (_c, _, at_c) = behind_door("d9-c", (C_SEED, C_KEY), &a_keys, &[]).await;
        let known = [endpoint_of(B_KEY), endpoint_of(C_KEY)];
        let (a, a_lines, _) = behind_door("d9-a", (A_SEED, A_KEY), &known, &[]).await;
        let held = |st: &Store, origin: &str| st.scan(HOME, G_PRINCIPALS, &[], origin, -1);
        let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));

        a.connect_peer(at_b.clone()).await.unwrap();
        {
            let st = a.shared.store.lock().await;
            let b_held = std::slice::from_ref(&b_op);
            assert_eq!(held(&st, &b_id), b_held, "B's own record lands");
            assert_eq!(held(&st, &c_id), [], "C's is kept nowhere");
        }
        let deferred = format!(
            "deferred 2 home record(s) of node {c_id} on dir.principals from peer {b_id}: not a node this node knows"
        );
        assert_eq!(*a_lines.lock().unwrap(), std::slice::from_ref(&deferred));

        a.connect_peer(at_c.clone()).await.unwrap();
        let bare = Op {
            seq: 1,
            prev: Some(crate::chain::op_hash(&b_op).to_vec()),
            payload: principal("b1").encode(),
            ..b_op.clone()
        };
        let forged = Op {
            payload: crate::envelope::seal(&c_identity, &bare),
            ..bare
        };
        let linked = b.shared.mesh.get().unwrap().linked(&a_id).await.unwrap();
        pushed(&linked, [vec![forged], c_ops].concat());
        wait_for(&a, |st| held(st, &c_id).len() == 2, "C's records at A").await;
        let refused = format!(
            "refused 1 home record(s) of node {b_id} on dir.principals from peer {b_id}: ({b_id},1) does not verify: its signature does not verify"
        );
        for _ in 0..500 {
            if a_lines.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*a_lines.lock().unwrap(), [deferred, refused]);
        assert_eq!(held(&*a.shared.store.lock().await, &b_id), [b_op]);
    }

    // ---- a pull on a gap (the hardening's question 2), over real iroh ------

    /// B's `dir.claims` chain, sealed by B: a claim on `ws-razel`, then `n`
    /// renewals, each a lease further on.
    fn b_claims(n: i64) -> Vec<Op> {
        let identity = crate::peer::NodeIdentity::from_key(B_SEED);
        let b_id = hex_id(&identity.node_id);
        let mut records = crate::registry::Registry::sealed(identity);
        let lease = now_ms() + 30_000;
        let claim = |renewal: i64| {
            let (node, share) = (b_id.clone(), "ws-razel".to_string());
            let lease_expiry_ms = lease + renewal;
            let epoch = 1;
            Record::Serve(ServeClaim {
                node,
                share,
                lease_expiry_ms,
                epoch,
            })
        };
        (0..=n)
            .map(|renewal| records.append_returning(claim(renewal), &b_id).unwrap())
            .collect()
    }

    /// A behind a door, linked to B, which holds `held`, its first records:
    /// A has pulled them. A's report lines, and B.
    async fn a_linked_to_b(name: &str, held: &[Op]) -> (Server, Lines, Server) {
        let (a_keys, b_keys) = ([endpoint_of(A_KEY)], [endpoint_of(B_KEY)]);
        let at = |node: &str| format!("{name}-{node}");
        let (b, _, at_b) = behind_door(&at("b"), (B_SEED, B_KEY), &a_keys, held).await;
        let (a, a_lines, _) = behind_door(&at("a"), (A_SEED, A_KEY), &b_keys, &[]).await;
        a.connect_peer(at_b.clone()).await.unwrap();
        (a, a_lines, b)
    }

    /// Land `ops` in B's served store, as a mint does, without a push.
    async fn minted(b: &Server, ops: &[Op]) {
        let mut store = b.shared.store.lock().await;
        for op in ops {
            store.append(op.clone()).unwrap();
        }
    }

    /// Push `ops` from B to A on a conversation of their own, as
    /// `push_home` does.
    async fn b_pushes(b: &Server, ops: &[Op]) {
        let a_id = hex_id(&node_of(A_SEED));
        let linked = b.shared.mesh.get().unwrap().linked(&a_id).await.unwrap();
        pushed(&linked, ops.to_vec());
    }

    /// The lines once there are `n`, waiting at most about 5 s for them.
    async fn reported(lines: &Lines, n: usize) -> Vec<String> {
        for _ in 0..500 {
            if lines.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        lines.lock().unwrap().clone()
    }

    /// Whether a pull on a gap runs at `server`.
    fn pulling(server: &Server) -> bool {
        let mesh = server.shared.mesh.get().unwrap();
        !mesh.gap_pulls.lock().unwrap().is_empty()
    }

    /// A's line for a push of B's claims chain at `seq`, refused as a gap
    /// while A holds B's claim alone.
    fn refused_as_a_gap(b_id: &str, seq: i64) -> String {
        let head = format!("1 home record(s) of node {b_id} on dir.claims from peer {b_id}");
        format!("refused {head}: a gap: expected seq 1, got {seq}")
    }

    /// The hardening's question 2, ruled (b). B's renewal pushed ahead of the
    /// one before it is refused as a gap at A, and A pulls from B at once:
    /// the chain heals without the link coming up again, with a line saying
    /// so. The late push of the earlier renewal changes nothing, and the next
    /// renewal lands in order. Before, the chain stayed short until the next
    /// link, and so did every later renewal on it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_renewal_pushed_ahead_of_the_one_before_it_heals_by_a_pull() {
        let chain = b_claims(3);
        let (a, a_lines, b) = a_linked_to_b("gap-order", &chain[..1]).await;
        let b_id = hex_id(&node_of(B_SEED));
        let claims = |st: &Store| st.scan(HOME, G_CLAIMS, &[], &b_id, -1);
        minted(&b, &chain[1..3]).await;
        b_pushes(&b, &chain[2..3]).await;
        wait_for(&a, |st| claims(st).len() == 3, "the chain to heal at A").await;
        b_pushes(&b, &chain[1..2]).await;
        minted(&b, &chain[3..]).await;
        b_pushes(&b, &chain[3..]).await;
        wait_for(&a, |st| claims(st).len() == 4, "the next renewal at A").await;
        assert_eq!(claims(&*a.shared.store.lock().await), chain);
        let pulled = format!(
            "pulled 2 home record(s) from peer {b_id} after 1 gap(s): dir.claims of node {b_id} healed"
        );
        let lines = [refused_as_a_gap(&b_id, 2), pulled];
        assert_eq!(reported(&a_lines, 2).await, lines);
    }

    /// A burst of gaps from one pusher is answered by one pull. B's store is
    /// held, so A's pull waits for B's answer, while B pushes three renewals
    /// out of order, each refused as a gap. The later two wait for the
    /// running pull, which heals all three, so no second pull runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_burst_of_gaps_from_one_pusher_is_answered_by_one_pull() {
        let chain = b_claims(4);
        let (a, a_lines, b) = a_linked_to_b("gap-burst", &chain[..1]).await;
        let b_id = hex_id(&node_of(B_SEED));
        minted(&b, &chain[1..]).await;
        let mut lines = Vec::new();
        {
            let _answer_waits = b.shared.store.lock().await;
            for seq in [4, 3, 2] {
                let at = seq as usize;
                b_pushes(&b, &chain[at..at + 1]).await;
                lines.push(refused_as_a_gap(&b_id, seq));
                assert_eq!(reported(&a_lines, lines.len()).await, lines);
                assert!(pulling(&a), "the pull the first gap started runs");
            }
        }
        let claims = |st: &Store| st.scan(HOME, G_CLAIMS, &[], &b_id, -1);
        wait_for(&a, |st| claims(st) == chain, "the chain to heal at A").await;
        lines.push(format!(
            "pulled 4 home record(s) from peer {b_id} after 3 gap(s): dir.claims of node {b_id} healed"
        ));
        assert_eq!(reported(&a_lines, lines.len()).await, lines);
        for _ in 0..500 {
            if !pulling(&a) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!pulling(&a), "the pull ended");
        assert_eq!(*a_lines.lock().unwrap(), lines, "and no other ran");
    }

    /// A deferred chain is not a gap (D9 beside the hardening's question 2):
    /// B pushes two records of C, a node A has not met, and A defers them
    /// and starts no pull. B's renewal pushed ahead of the one before it
    /// then starts one, which names B's chain alone.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_deferred_chain_starts_no_pull() {
        let chain = b_claims(2);
        let (a, a_lines, b) = a_linked_to_b("gap-deferred", &chain[..1]).await;
        let b_id = hex_id(&node_of(B_SEED));
        let c_identity = crate::peer::NodeIdentity::from_key(C_SEED);
        let c_id = hex_id(&c_identity.node_id);
        let mut c_records = crate::registry::Registry::sealed(c_identity);
        let c_ops: Vec<Op> = ["c0", "c1"]
            .into_iter()
            .map(|name| c_records.append_returning(principal(name), &c_id).unwrap())
            .collect();
        b_pushes(&b, &c_ops).await;
        let deferred = format!(
            "deferred 2 home record(s) of node {c_id} on dir.principals from peer {b_id}: not a node this node knows"
        );
        let only = std::slice::from_ref(&deferred);
        assert_eq!(reported(&a_lines, 1).await, only);
        assert!(!pulling(&a), "a deferred chain started a pull");

        minted(&b, &chain[1..]).await;
        b_pushes(&b, &chain[2..]).await;
        let pulled = format!(
            "pulled 2 home record(s) from peer {b_id} after 1 gap(s): dir.claims of node {b_id} healed"
        );
        let lines = [deferred, refused_as_a_gap(&b_id, 2), pulled];
        assert_eq!(reported(&a_lines, 3).await, lines);
        let st = a.shared.store.lock().await;
        assert_eq!(st.scan(HOME, G_CLAIMS, &[], &b_id, -1), chain);
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

    /// Two booted nodes for the grant check's journeys (plan Step 4.3), over
    /// real iroh and websockets. B is adopted, so it checks its own fold: it
    /// registers `ws-razel` with a live claim and `ws-attic` with a lapsed one,
    /// and grants A's node id `grant` on `ws-razel`, when given one. A is
    /// seeded and dials B. B's provider session has written two tree ops.
    struct TwoNodes {
        a: Arc<Shared>,
        b: Arc<Shared>,
        a_id: String,
        b_id: String,
        port_a: u16,
        /// B's provider session, which writes the workspace content, and the
        /// two ops it wrote.
        provider: (crate::ws::WsReader, crate::ws::WsWriter),
        tree: [Op; 2],
    }

    async fn two_nodes(name: &str, grant: Option<&[&str]>) -> TwoNodes {
        two_nodes_limited(name, grant, crate::frame::MAX_FRAME_BYTES).await
    }

    /// [`two_nodes`], each linked with frames of at most `max` bytes.
    async fn two_nodes_limited(name: &str, grant: Option<&[&str]>, max: usize) -> TwoNodes {
        let boot_a = boot_at(fresh(&format!("{name}-a-sys")), "gianni").unwrap();
        let mut boot_b = boot_at(fresh(&format!("{name}-b-sys")), "gianni").unwrap();
        let (a_id, b_id) = (boot_a.node_id.clone(), boot_b.node_id.clone());
        let workspace = |share: &str, name: &str, host: &str| {
            let eligible_hosts = vec![host.to_string()];
            Record::Workspace(WorkspaceEntry {
                workspace: share.into(),
                name: name.into(),
                eligible_hosts,
            })
        };
        let claim = |node: &str, share: &str, lease_expiry_ms: i64| {
            Record::Serve(ServeClaim {
                node: node.into(),
                share: share.into(),
                lease_expiry_ms,
                epoch: 1,
            })
        };
        let mut records = vec![
            workspace("ws-razel", "razel", &b_id),
            claim(&b_id, "ws-razel", now_ms() + 30_000),
            workspace("ws-attic", "attic", "attic-mini"),
            claim("attic-mini", "ws-attic", now_ms() - 1_000),
        ];
        if let Some(verbs) = grant {
            let verbs = verbs.iter().map(|verb| verb.to_string()).collect();
            let share = "ws-razel".into();
            records.push(Record::Grant(CapabilityGrant {
                principal: a_id.clone(),
                share,
                verbs,
            }));
        }
        for record in records {
            boot_b.registry.append(record, &b_id).unwrap();
        }
        let (id_a, id_b) = (boot_a.identity().unwrap(), boot_b.identity().unwrap());
        let a = Server::open(fresh(&format!("{name}-a-store"))).unwrap();
        let b = Server::open(fresh(&format!("{name}-b-store"))).unwrap();
        a.seed_registry(&boot_a.registry.snapshot()).await;
        b.adopt_boot(boot_b).await.unwrap();

        on_carrier(&a, id_a, endpoint_key(), None, max).await;
        let at_b = on_carrier(&b, id_b, endpoint_key(), None, max).await;
        a.connect_peer(at_b).await.unwrap();

        let (a_shared, b_shared) = (a.shared.clone(), b.shared.clone());
        let lis_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let lis_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (port_a, port_b) = (lis_a.local_addr().unwrap().port(), lis_b.local_addr().unwrap().port());
        tokio::spawn(a.run(lis_a));
        tokio::spawn(b.run(lis_b));

        // B's authority provider session writes the workspace content (the C4
        // source) — an ordinary session appending ordinary chained ops.
        let provider = crate::ws::connect("127.0.0.1", port_b).await.unwrap();
        let o0 = tree_op(0, None, b"tree-v0");
        let o1 = tree_op(1, Some(crate::chain::op_hash(&o0).to_vec()), b"tree-v1");
        let written = ops_frame(vec![o0.clone(), o1.clone()]);
        provider.1.send_binary(&written).await.unwrap();
        wait_store(&b_shared, |st| tree_len(st) == 2, "B to hold the tree").await;
        TwoNodes {
            a: a_shared,
            b: b_shared,
            a_id,
            b_id,
            port_a,
            provider,
            tree: [o0, o1],
        }
    }

    /// The tree zone the journeys read, commons.
    fn tree_zone() -> (String, String, Vec<u8>) {
        ("ws-razel".into(), "ws.tree".into(), vec![])
    }

    /// How many of B's provider's tree ops `st` holds.
    fn tree_len(st: &Store) -> usize {
        st.scan("ws-razel", "ws.tree", &[], "prov-b", i64::MIN)
            .len()
    }

    /// The tree ops a node's replica holds from B's provider, as payloads.
    async fn tree_payloads(shared: &Arc<Shared>) -> Vec<Vec<u8>> {
        let store = shared.store.lock().await;
        let held = store.scan("ws-razel", "ws.tree", &[], "prov-b", i64::MIN);
        held.into_iter().map(|op| op.payload).collect()
    }

    /// Whether a node's fan-out of the tree zone reaches any session.
    async fn tree_routed(shared: &Arc<Shared>) -> bool {
        let router = shared.router.lock().await;
        !router.route(0, "ws-razel", "ws.tree", &[]).is_empty()
    }

    /// The nodes, in hex, whose subscription streams a node has admitted.
    async fn admitted(shared: &Arc<Shared>) -> Vec<String> {
        let admitted = shared.admitted.lock().await;
        admitted.values().map(|node| hex_id(node)).collect()
    }

    /// One `Ops` frame, as bytes.
    fn ops_frame(ops: Vec<Op>) -> Vec<u8> {
        Frame::Ops(Ops { ops, pri: None }).to_bytes()
    }

    /// Payloads from `r`'s ops, in order, until `n` have arrived.
    async fn payloads(r: &mut crate::ws::WsReader, n: usize, what: &str) -> Vec<Vec<u8>> {
        let mut payloads = Vec::new();
        while payloads.len() < n {
            if let Frame::Ops(ops) = next_frame(r, what).await {
                payloads.extend(ops.ops.into_iter().map(|o| o.payload));
            }
        }
        payloads
    }

    /// A client on A subscribes the tree zone and reads its ack.
    async fn a_client(t: &TwoNodes) -> (crate::ws::WsReader, crate::ws::WsWriter) {
        let (mut rc, wc) = crate::ws::connect("127.0.0.1", t.port_a).await.unwrap();
        wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
        let ack = next_frame(&mut rc, "ws.tree ack").await;
        assert!(matches!(ack, Frame::Heads(_)), "{ack:?}");
        (rc, wc)
    }

    /// Wait, bounded, until A's forward of the tree zone has lapsed.
    async fn forward_lapses(a: &Arc<Shared>) {
        let mesh = a.mesh.get().unwrap();
        for _ in 0..500 {
            if !mesh.forwarded.lock().await.contains(&tree_zone()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("A's forward of the tree zone did not lapse");
    }

    /// Subscribe the tree zone on a fresh conversation of A's link to B, as
    /// A's forward does, and return B's answer: a refusal's two frames (R6),
    /// and the reason, once B has ended the conversation.
    async fn refused_on_the_link(t: &TwoNodes) -> glade_wire::generated::Error {
        let linked = t.a.mesh.get().unwrap().linked(&t.b_id).await.unwrap();
        let mut conversation = linked.open();
        let (share, glade_id, _) = tree_zone();
        let subscribe = Subscribe {
            share,
            glade_id,
            key: None,
            from: None,
        };
        conversation.send(&Frame::Subscribe(subscribe)).unwrap();
        let mut frames = Vec::new();
        loop {
            let read = tokio::time::timeout(Duration::from_secs(5), conversation.recv());
            match read.await.expect("B answered and ended the conversation") {
                Ok(frame) => frames.push(frame),
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => panic!("reading B's answer: {e}"),
            }
        }
        match frames.as_slice() {
            [Frame::Heads(h), Frame::Error(e)] if h.streams.is_empty() => {
                assert_eq!(e.code, ErrorCode::Unauthorized);
                let named = (e.share.as_deref(), e.glade_id.as_deref());
                assert_eq!(named, (Some("ws-razel"), Some("ws.tree")));
                assert_eq!(e.corr, None);
                e.clone()
            }
            other => panic!("expected an ack that names no zone, then the reason: {other:?}"),
        }
    }

    /// The 30-step s-discovery trace's slice for this step, E2E over real iroh
    /// + real websockets: (a) phase A — a client on node A lists
    /// `home/dir.workspaces` from A's LOCAL replica and sees the workspace B
    /// registered; (b) phase C — subscribing that workspace's share routes the
    /// interest via the folded ServeClaim to B, the ops arrive, converge into
    /// A's replica, and keep flowing live; (c) phase E — a share whose only
    /// claim is lapsed at the reader's clock answers with an ack that names no
    /// zone, then STATUS data, bounded, and the session stays usable. B serves
    /// A in phase (b) because its fold grants A's node id `read.*` on the
    /// share (plan Step 4.3); without it, B refuses
    /// (`a_peer_without_a_grant_is_refused_by_its_claimed_node_id`).
    #[tokio::test(flavor = "multi_thread")]
    async fn s_discovery_golden_path_end_to_end() {
        let t = two_nodes("e2e", Some(&["read.*"])).await;
        let (a_shared, b_id, port_a) = (t.a.clone(), t.b_id.clone(), t.port_a);
        let wp = &t.provider.1;
        let o1 = t.tree[1].clone();

        // ---- (a) phase A: list the directory from A's LOCAL replica ---------
        let (mut rc, wc) = crate::ws::connect("127.0.0.1", port_a).await.unwrap();
        wc.send_binary(&sub(HOME, crate::registry::G_WORKSPACES)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "dir.workspaces ack").await, Frame::Heads(_)));
        let mut names = Vec::new();
        while names.len() < 2 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "workspace entries").await {
                for op in ops.ops {
                    assert_eq!(op.origin, b_id, "entries carry their writing origin");
                    let entry = envelope::record(&op, crate::sysdata::WorkspaceEntry::from_cbor);
                    names.push(entry.unwrap().workspace);
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
        // R6 (client-writes plan Step 2.2): a refused subscribe gets an ack
        // that names no zone, then the reason, so a client waiting on its ack
        // resolves instead of hanging (the plan's F3).
        wc.send_binary(&sub("ws-attic", "ws.tree")).await.unwrap();
        match next_frame(&mut rc, "ws-attic's refusal ack").await {
            Frame::Heads(h) => assert!(
                h.streams.is_empty(),
                "the refusal's ack names no zone: {h:?}"
            ),
            other => panic!("expected an ack that names no zone, got {other:?}"),
        }
        match next_frame(&mut rc, "ws-attic status").await {
            Frame::Error(e) => {
                assert_eq!(e.code, glade_wire::generated::ErrorCode::UnknownShare);
                assert_eq!(e.share.as_deref(), Some("ws-attic"));
                assert_eq!(e.glade_id.as_deref(), Some("ws.tree"));
                assert_eq!(e.corr, None, "a refused subscribe names no op");
                assert!(e.message.contains("no live ServeClaim"), "the reason rides the status: {}", e.message);
            }
            other => panic!("expected STATUS (Error frame), got {other:?}"),
        }
        // absence is data, not a dead session: the next ask still answers,
        // with an ack that names its zone.
        wc.send_binary(&sub(HOME, crate::registry::G_CLAIMS)).await.unwrap();
        match next_frame(&mut rc, "post-absence ack").await {
            Frame::Heads(h) => {
                assert_eq!(h.streams.len(), 1, "an accepted ack names its zone: {h:?}")
            }
            other => panic!("expected the post-absence ack, got {other:?}"),
        }
    }

    // ---- the grant check at the serve hop (plan Step 4.3) ------------------

    /// The golden path's phase (b), turned round: B's fold grants A's node id
    /// nothing on `ws-razel`, so B refuses A's forwarded subscribe by the node
    /// id A's HELLO claimed, and proved: the refused subscribe's two frames,
    /// then the stream is finished. A's client is acked from A's replica, A's
    /// forward lapses, and nothing of the zone reaches A.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peer_without_a_grant_is_refused_by_its_claimed_node_id() {
        let t = two_nodes("refused", None).await;
        let a = &t.a_id;
        let why = format!("unauthorized: node {a} holds no grant of read.subscribe on ws-razel");
        assert_eq!(refused_on_the_link(&t).await.message, why);

        let _client = a_client(&t).await;
        let (share, glade_id, key) = tree_zone();
        forward_interest(&t.a, t.b_id.clone(), share, glade_id, key).await;
        forward_lapses(&t.a).await;
        assert_eq!(tree_payloads(&t.a).await, Vec::<Vec<u8>>::new());
        assert!(!tree_routed(&t.b).await, "B routes the zone to no one");
        assert_eq!(admitted(&t.b).await, Vec::<String>::new());
    }

    /// Its twin: B's fold grants A's node id exactly `read.subscribe` on
    /// `ws-razel`, so A's forwarded subscribe, by that claimed node id, is
    /// admitted and served, and B's admission table holds the stream.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peer_granted_by_its_claimed_node_id_is_served() {
        let t = two_nodes("granted", Some(&["read.subscribe"])).await;
        let (mut rc, _wc) = a_client(&t).await;
        let got = payloads(&mut rc, 2, "routed tree ops").await;
        assert_eq!(got, [b"tree-v0".to_vec(), b"tree-v1".to_vec()]);
        assert_eq!(admitted(&t.b).await, [t.a_id.as_str()]);
    }

    /// A revocation accepted while A's forwarded stream is live, through B's
    /// directory authority as a runtime route would take it, ends the stream
    /// before the accepting call returns: the fold's generation advances, the
    /// stream leaves B's router and admission table, and A's forward lapses.
    /// An op written after the revocation stays at B, and a new subscribe by
    /// A's claimed node id is refused as revoked. What A got before stays.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_revocation_ends_a_forwarded_stream_of_a_claimed_node_id() {
        let t = two_nodes("revoke", Some(&["read.*"])).await;
        let (mut rc, _wc) = a_client(&t).await;
        assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);
        assert_eq!(admitted(&t.b).await, [t.a_id.as_str()]);

        let before = t.b.policy.generation();
        let revocation = CapabilityRevocation {
            principal: t.a_id.clone(),
            share: "ws-razel".into(),
        };
        let revoke = vec![Record::Revoke(revocation)];
        let generation = testing::accept(&t.b, revoke).await.unwrap();
        assert_eq!(generation, before + 1);
        assert_eq!(
            admitted(&t.b).await,
            Vec::<String>::new(),
            "the pass ended the stream"
        );
        assert!(!tree_routed(&t.b).await, "B routes the zone to no one");
        forward_lapses(&t.a).await;

        let prev = crate::chain::op_hash(&t.tree[1]).to_vec();
        let written = ops_frame(vec![tree_op(2, Some(prev), b"tree-v2")]);
        t.provider.1.send_binary(&written).await.unwrap();
        wait_store(&t.b, |st| tree_len(st) == 3, "B to hold v2").await;
        let at_a = tree_payloads(&t.a).await;
        assert_eq!(
            at_a,
            [b"tree-v0".to_vec(), b"tree-v1".to_vec()],
            "v2 stayed at B"
        );

        let why = format!(
            "unauthorized: node {}'s grants on ws-razel are revoked",
            t.a_id
        );
        assert_eq!(refused_on_the_link(&t).await.message, why);
    }

    /// The fail direction: once B's fold cannot be read, the live forwarded
    /// stream of A's claimed node id ends and a new subscribe is refused, as
    /// for no grant. The fold is made unreadable as a quarantined grant or
    /// revocation leaves it at boot (`Registry::policy`).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stale_fold_fails_closed() {
        let t = two_nodes("stale", Some(&["read.*"])).await;
        let (mut rc, _wc) = a_client(&t).await;
        assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);

        crate::server::refresh_policy(&t.b, None).await;
        assert_eq!(
            admitted(&t.b).await,
            Vec::<String>::new(),
            "the pass ended the stream"
        );
        assert!(!tree_routed(&t.b).await, "B routes the zone to no one");
        forward_lapses(&t.a).await;
        let a = &t.a_id;
        let why = format!(
            "unauthorized: the grant fold is unavailable, so node {a} may not read.subscribe on ws-razel"
        );
        assert_eq!(refused_on_the_link(&t).await.message, why);
    }

    // ---- the claim holder's refusal, relayed (F5) --------------------------

    /// The next frame a client of A reads, which must be a lone refusal that
    /// names the tree zone and no op: its code and its reason.
    async fn told(r: &mut crate::ws::WsReader) -> (ErrorCode, String) {
        match next_frame(r, "the relayed refusal").await {
            Frame::Error(e) => {
                let named = (e.share.as_deref(), e.glade_id.as_deref(), e.corr.as_deref());
                assert_eq!(named, (Some("ws-razel"), Some("ws.tree"), None));
                (e.code, e.message)
            }
            other => panic!("expected the relayed refusal, got {other:?}"),
        }
    }

    /// The next frame on `r` after a subscribe to a zone no directory knows,
    /// which A serves locally: its ack, unless something came before it.
    async fn bound(r: &mut crate::ws::WsReader, w: &crate::ws::WsWriter) -> Frame {
        w.send_binary(&sub("plain", "bound")).await.unwrap();
        next_frame(r, "the bound's ack").await
    }

    /// F5 (question 25; the owner's ruling of 2026-09-27): B grants A's node
    /// id nothing on `ws-razel`, so it refuses A's forwarded subscribe, and
    /// the refusal now reaches A's own subscriber of the zone. It is acked
    /// from A's replica, as before, then told with a lone `Error`, B's code
    /// and B's reason prefixed with who refused, and leaves A's router for
    /// the zone. A client that subscribes later forwards the interest again,
    /// is refused again and is told the same, and the first is not told
    /// twice. Nothing re-checks: once B grants A, a new subscribe is served,
    /// and the refused client, which has not subscribed again, gets nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_claim_holders_refusal_reaches_the_forwarding_nodes_subscribers() {
        let t = two_nodes("relayed", None).await;
        let (a, b) = (&t.a_id, &t.b_id);
        let why = format!(
            "refused by node {b}, which serves ws-razel: unauthorized: node {a} holds no grant of \
             read.subscribe on ws-razel"
        );
        let refusal = (ErrorCode::Unauthorized, why);
        let (mut first, first_w) = a_client(&t).await;
        assert_eq!(told(&mut first).await, refusal);
        forward_lapses(&t.a).await;
        assert!(!tree_routed(&t.a).await, "A routes the zone to no one");

        let (mut later, _later_w) = a_client(&t).await;
        assert_eq!(told(&mut later).await, refusal);
        let next = bound(&mut first, &first_w).await;
        assert!(matches!(next, Frame::Heads(_)), "told twice: {next:?}");
        forward_lapses(&t.a).await;

        let grant = CapabilityGrant {
            principal: t.a_id.clone(),
            share: "ws-razel".into(),
            verbs: vec!["read.subscribe".into()],
        };
        let granted = testing::accept(&t.b, vec![Record::Grant(grant)]).await;
        granted.unwrap();
        let (mut again, _again_w) = a_client(&t).await;
        let got = payloads(&mut again, 2, "routed tree ops").await;
        assert_eq!(got, [b"tree-v0".to_vec(), b"tree-v1".to_vec()]);
        let next = bound(&mut first, &first_w).await;
        assert!(matches!(next, Frame::Heads(_)), "served unasked: {next:?}");
    }

    /// F5, mid-stream: B's re-check pass ends A's admitted forward when a
    /// revocation lands, and its lone refusal reaches A's subscriber, who
    /// has had the zone's ops; the subscriber leaves A's router for the zone.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_revocation_on_the_claim_holder_reaches_the_forwarding_nodes_subscribers() {
        let t = two_nodes("relayed-revocation", Some(&["read.*"])).await;
        let (mut rc, _wc) = a_client(&t).await;
        assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);

        let revocation = CapabilityRevocation {
            principal: t.a_id.clone(),
            share: "ws-razel".into(),
        };
        testing::accept(&t.b, vec![Record::Revoke(revocation)])
            .await
            .unwrap();
        let (a, b) = (&t.a_id, &t.b_id);
        let why = format!(
            "refused by node {b}, which serves ws-razel: unauthorized: node {a}'s grants on \
             ws-razel are revoked"
        );
        assert_eq!(told(&mut rc).await, (ErrorCode::Unauthorized, why));
        forward_lapses(&t.a).await;
        assert!(!tree_routed(&t.a).await, "A routes the zone to no one");
    }

    // ---- the mesh on the carrier port (plan Step 4.5b, part 3) -------------

    /// Plan Step 4.5b (question 4): HELLO runs in the accepted link's own
    /// task, not the accept loop. C, a key A's door knows, links to A at the
    /// carrier, its first word sent, and says no HELLO; B then dials A and
    /// links within a second, while C's HELLO still waits, unreported.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dialer_that_never_says_hello_does_not_hold_the_accept_loop() {
        use crate::iroh_carrier::{Lent, FIRST_WORD};
        use glade_carrier_api::{CarrierAddr, CarrierConfig};
        let knows = [endpoint_of(B_KEY), endpoint_of(C_KEY)];
        let (a, a_lines, at_a) = behind_door("held-a", (A_SEED, A_KEY), &knows, &[]).await;
        let (b, _, _) = behind_door("held-b", (B_SEED, B_KEY), &[endpoint_of(A_KEY)], &[]).await;
        let key = EndpointKey::from_seed(C_KEY);
        let (door, relays, first_word) = (None, crate::netconf::Relays::Off, FIRST_WORD);
        let c = IrohCarrier::new(Some(Lent {
            key,
            door,
            relays,
            first_word,
        }));
        let local = CarrierAddr("127.0.0.1:0".into());
        let max_frame_bytes = std::num::NonZeroUsize::new(MAX_FRAME_BYTES).unwrap();
        c.bind(CarrierConfig {
            local,
            max_frame_bytes,
        })
        .await
        .unwrap();
        let to_a = carrier_addr(&at_a).unwrap();
        let silent = c.dial(&to_a).await.expect("C links at the carrier");

        let began = Instant::now();
        let linked = tokio::time::timeout(Duration::from_secs(1), b.connect_peer(at_a)).await;
        let Ok(linked) = linked else {
            panic!("B still dialing after {:?}", began.elapsed());
        };
        assert_eq!(linked.expect("B links"), hex_id(&node_of(A_SEED)));
        assert_eq!(links(&a).await, 1, "B alone is linked");
        let reported = a_lines.lock().unwrap().clone();
        assert_eq!(reported, Vec::<String>::new(), "C's HELLO still waits");
        drop(silent);
        c.close().await;
    }

    /// Plan Step 4.5b (section 5), in place of 4.1b's protocol-2 test: the
    /// node's endpoint offers `glade/carrier/1` alone, so an endpoint that
    /// offers only `glade/node/3`, as every node before 4.5b does, fails at
    /// the handshake either way, before any HELLO: its dial to the node, and
    /// the node's dial to it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_glade_node_3_endpoint_fails_at_connect_either_way() {
        use crate::netconf::Via;
        use iroh::endpoint::{presets, PortmapperConfig};
        use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
        let bound = Duration::from_secs(10);
        let v3: &[u8] = b"glade/node/3";
        let old = Endpoint::builder(presets::Minimal)
            .alpns(vec![v3.to_vec()])
            .portmapper_config(PortmapperConfig::Disabled)
            .clear_ip_transports()
            .bind_addr((std::net::Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .unwrap();
        let old_accepts = old.clone();
        tokio::spawn(async move {
            while let Some(incoming) = old_accepts.accept().await {
                if let Ok(connecting) = incoming.accept() {
                    let _ = connecting.await;
                }
            }
        });
        let a = Server::open(fresh("node-3-a")).unwrap();
        let at_a = meshed(&a, NodeIdentity::from_key(A_SEED)).await;
        let Some(Via::Ip(socket)) = at_a.via.first().cloned() else {
            panic!("A bound no socket: {at_a:?}");
        };
        let id = EndpointId::from_bytes(&at_a.key).unwrap();
        let to_a = EndpointAddr::from_parts(id, [TransportAddr::Ip(socket)]);
        let dialed = tokio::time::timeout(bound, old.connect(to_a, v3)).await;
        let refused = dialed.expect("bounded").is_err();
        assert!(refused, "a glade/node/3 dialer connected");

        let sockets = old.bound_sockets();
        let socket = sockets.into_iter().find(|socket| socket.is_ipv4()).unwrap();
        let key = *old.id().as_bytes();
        let at_old = PeerEntry {
            key,
            via: vec![Via::Ip(socket)],
        };
        let dialed = tokio::time::timeout(bound, a.connect_peer(at_old)).await;
        let refused = dialed.expect("bounded").is_err();
        assert!(refused, "a glade/node/3 endpoint took the node's dial");
        old.close().await;
    }

    /// Plan Step 4.5b (section 8, the table's race): A links to B twice, and
    /// the newer link takes the older's place in each end's table. The older
    /// then ends: each end notes its close, and each still holds the newer,
    /// which serves a pull either way.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_newer_link_outlives_the_close_of_an_older_one_to_the_same_node() {
        let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));
        let b = noting_door("twice-b", (B_SEED, B_KEY), &[endpoint_of(A_KEY)], &[]).await;
        let (b, _, b_notes, at_b) = b;
        let a = noting_door("twice-a", (A_SEED, A_KEY), &[endpoint_of(B_KEY)], &[]).await;
        let (a, _, a_notes, _) = a;
        a.connect_peer(at_b.clone()).await.expect("the older link");
        let (a_mesh, b_mesh) = (a.shared.mesh.get().unwrap(), b.shared.mesh.get().unwrap());
        let older = a_mesh.linked(&b_id).await.unwrap();
        a.connect_peer(at_b).await.expect("the newer link");
        older.end();
        let (a_closed, b_closed) = (format!("link {b_id} closed"), format!("link {a_id} closed"));
        noted(&a_notes, |line| line == a_closed).await;
        noted(&b_notes, |line| line == b_closed).await;
        let held = (links(&a).await, links(&b).await);
        assert_eq!(held, (1, 1), "each end holds the newer link");
        let pulled = pull_from(&a.shared, a_mesh, &b_id, node_of(B_SEED)).await;
        pulled.expect("the newer link serves A's pull");
        let pulled = pull_from(&b.shared, b_mesh, &a_id, node_of(A_SEED)).await;
        pulled.expect("and B's");
    }

    /// Plan Step 4.5b (question 6): with links whose frames hold at most
    /// 4 KiB, B serves A's forward of a zone whose gap is over five times
    /// that. It crosses in chunks, each under the limit, and reaches A whole
    /// and in order; in one frame it would be refused, and the forward would
    /// lapse.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_forwarded_gap_crosses_in_chunks_under_the_frame_limit() {
        const LIMIT: usize = 4 << 10;
        let t = two_nodes_limited("chunked", Some(&["read.*"]), LIMIT).await;
        let (mut prev, mut written) = (crate::chain::op_hash(&t.tree[1]).to_vec(), Vec::new());
        for seq in 2..22 {
            let op = tree_op(seq, Some(prev), &[seq as u8; 1 << 10]);
            prev = crate::chain::op_hash(&op).to_vec();
            written.push(op);
        }
        {
            let mut store = t.b.store.lock().await;
            for op in &written {
                store.append(op.clone()).unwrap();
            }
        }
        let ops = || t.tree.iter().chain(&written);
        let gap: usize = ops().map(|op| cbor::encode(&op.to_cbor()).len()).sum();
        assert!(gap > 5 * LIMIT, "a gap of {gap} bytes");
        let _client = a_client(&t).await;
        wait_store(&t.a, |st| tree_len(st) == 22, "the whole gap at A").await;
        let sent: Vec<Vec<u8>> = ops().map(|op| op.payload.clone()).collect();
        assert_eq!(tree_payloads(&t.a).await, sent);
    }
}
