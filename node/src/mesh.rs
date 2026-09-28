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
use std::io;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, PoisonError};

use glade_carrier_api::{CarrierLink, CarrierPort, TransportId};
use tokio::sync::Mutex;

use crate::assembly::{LinkNotes, PathSeen};
use crate::conversation::Linked;
use crate::frame::MAX_FRAME_BYTES;
use crate::iroh_carrier::IrohCarrier;
use crate::peer::NodeIdentity;
use crate::signing::NodeSigner;
use crate::transport::{Door, EndpointKey};

mod home;
mod link;
mod route;
mod serve;

pub use route::{directory_knows, who_serves};

pub(crate) use home::{ingest_and_fanout, push_home};
pub(crate) use link::release_links;
pub(crate) use route::{forward_interest, route_subscribe, Route};

use home::{pull_home, pull_on_gap, Gaps, Round};
use serve::serve_conversation;

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

/// The most bytes of ops a chunk holds, but for an op alone (plan Step
/// 4.5b, question 6): 1 MiB.
const CHUNK_BYTES: usize = 1 << 20;

/// What a chunk's frame holds beside its ops, with room to spare: the
/// conversation's header and the `Ops` frame's tag, map, keys, array head and
/// priority, 12 bytes for up to 255 ops.
const CHUNK_ROOM: usize = 32;

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

// The tests, by the area they exercise, in `mesh/tests/`. A braced module, so
// the condition encloses every part.
#[cfg(test)]
mod tests {
    mod checkpoints;
    mod forward;
    mod home;
    mod link;
    mod support;
    mod two_nodes;
}
