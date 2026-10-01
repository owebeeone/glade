use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use glade_carrier_api::TransportId;

use glade_wire::generated::{Heads, Op, Ops, StreamHeads};

use crate::conversation::{Conversation, Linked};
use crate::frame::Frame;
use crate::peer::SyncOutcome;
use crate::registry::HOME;
use crate::router::SessionId;
use crate::server::{send, Shared};
use crate::store::{Append, Store, StoreError};
use crate::transport::key_of;

use super::{forwards_return, hex_id, Mesh};

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

/// Pull the peer's home-share gap on `conversation`, one this end opened:
/// announce our home heads, ingest until the peer's END. Every op lands
/// through the same verify path as any carrier (`Store::append` chain
/// checks) and fans out to local subscribers — a directory update reaches a
/// live `dir.workspaces` subscription with no re-request (the B9 step).
/// Non-home ops on this conversation are dropped: the pull asked for the
/// directory, a peer can't use it to push app content. The pull is one
/// [`Round`], whose outcome it returns.
pub(super) async fn pull_home(
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
pub(super) struct Round<'a> {
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
    pub(super) gaps: Gaps,
}

/// Why a round cut a chain short.
enum Cut {
    /// Its origin is not a node this node knows (D9).
    Deferred,
    /// The store refused one of its ops, for this reason.
    Refused(String),
}

impl<'a> Round<'a> {
    pub(super) fn new(shared: &'a Arc<Shared>, mesh: &'a Mesh, peer: [u8; 32]) -> Round<'a> {
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
    pub(super) async fn take(&mut self, op: Op) {
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
    pub(super) fn end(self) -> SyncOutcome {
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
pub(super) async fn pull_on_gap(
    shared: &Arc<Shared>,
    mesh: &Mesh,
    pusher: [u8; 32],
    mut gaps: Gaps,
) {
    let peer = hex_id(&pusher);
    loop {
        let pulled = pull_from(shared, mesh, &peer, pusher).await;
        // X4.2b: what the pull landed may route a local subscriber's zone
        // to the pusher.
        if pulled.as_ref().is_ok_and(|outcome| outcome.applied > 0) {
            forwards_return(shared, &peer).await;
        }
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
pub(super) async fn pull_from(
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
pub(super) struct Gaps(pub(super) BTreeMap<(String, String), (i64, usize)>);

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
    pub(super) fn merge(&mut self, other: Gaps) {
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
