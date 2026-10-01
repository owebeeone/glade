use std::collections::btree_map::Entry;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use glade_wire::generated::{Error, ErrorCode, Head, Op, Ops, Subscribe};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::conversation::{Conversation, Linked};
use crate::envelope;
use crate::exchange::FORWARD_TIMEOUT;
use crate::frame::Frame;
use crate::registry::HOME;
use crate::router::{SessionId, Zone};
use crate::server::{refuse_subscription, send, Shared};
use crate::session::op_status;
use crate::store::Store;
use crate::sysdir::now_ms;
use crate::tasks::Site;

use super::{ingest_and_fanout, Mesh};

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

/// A client's write on a share another node holds (W1, cross-node writes
/// plan X3.2): the op, and the session that wrote it, which the answer goes
/// to.
pub(crate) struct Write {
    pub(crate) op: Op,
    pub(crate) writer: SessionId,
}

/// A forward's handle: where writes are queued for it to send up, in the
/// order queued (W6).
pub(crate) type Upward = mpsc::UnboundedSender<Write>;

/// The A-side of the C2 decision's Forward arm: open a conversation on the
/// claim holder's link, send the interest (with our replica's heads as the
/// resume point), and ingest what comes back into the LOCAL replica — local
/// subscribers are then fed by the ordinary fan-out (replica serves reads,
/// trace C5→C6). Deduped per zone: one conversation carries any number of
/// local subscribers, and the zone's writes (X3.2), queued on the handle
/// returned; `None` with no live link to `peer`. The forward lapses with the
/// conversation; a later subscribe, or write, retries. Its end, and a
/// refusal the claim holder sends on it, reach the local subscribers
/// ([`lapse`]).
pub(crate) async fn forward_interest(
    shared: &Arc<Shared>,
    peer: String,
    share: String,
    glade_id: String,
    key: Vec<u8>,
) -> Option<Upward> {
    let mesh = shared.mesh.get().cloned()?;
    let zone = (share, glade_id, key);
    if let Some(up) = mesh.forwarded.lock().await.get(&zone) {
        return Some(up.clone()); // the forward already runs
    }
    let linked = mesh.linked(&peer).await?;
    let (up, writes) = mpsc::unbounded_channel();
    match mesh.forwarded.lock().await.entry(zone.clone()) {
        Entry::Occupied(running) => return Some(running.get().clone()),
        Entry::Vacant(slot) => slot.insert(up.clone()),
    };
    let forward = shared.clone();
    shared.tasks.spawn(Site::ForwardInterest, async move {
        let refused = run_forward(&forward, &linked, &peer, &zone, writes).await;
        lapse(&forward, &mesh, &peer, zone, refused.ok().flatten()).await;
    });
    Some(up)
}

/// Queue `write` on its zone's forward to the claim holder `holder`, opening
/// one if none runs (W1, cross-node writes plan X3.2). With no live link to
/// the holder, or once the forward has ended, it is not placed (W5).
pub(crate) async fn write_up(shared: &Arc<Shared>, holder: String, write: Write) {
    let op = &write.op;
    let (share, glade_id, key) = (op.share.clone(), op.glade_id.clone(), op.key.clone());
    let Some(up) = forward_interest(shared, holder.clone(), share, glade_id, key).await else {
        let why = format!("claim holder {holder} unreachable (no live peer link)");
        return unplaced(shared, write, why).await;
    };
    if let Err(ended) = up.send(write) {
        unplaced(shared, ended.0, format!("forward from node {holder} ended")).await;
    }
}

/// Answer `write`'s writer that its op was not placed, for `why` (W5):
/// `UnknownShare`, on which its client keeps the op, to send it again.
async fn unplaced(shared: &Arc<Shared>, write: Write, why: String) {
    let status = op_status(&write.op, ErrorCode::UnknownShare, why);
    send(shared, write.writer, &status).await;
}

/// A forward's end (F5, question 25; the owner's ruling of 2026-09-27; plan
/// Step 4.6, part 3, ruled 2026-09-30). The zone leaves the forwarded set
/// under the cut, so a subscribe registered after this forwards again, and
/// one registered before is among those told. Each local subscriber of the
/// zone is told with a lone `Error` and leaves the zone
/// ([`crate::server::refuse_subscription`]). When the claim holder `peer`
/// refused the read, at the subscribe (its ack named no zone) or later (its
/// re-check pass), the `Error` holds its code and its reason prefixed with
/// who refused; when the forward ended with no refusal (the claim holder
/// ended it, or the link closed), `UnknownShare`, an absent route's code,
/// and that the forward from `peer` ended. Nothing re-checks here: a
/// subscribe made later routes afresh.
async fn lapse(shared: &Arc<Shared>, mesh: &Mesh, peer: &str, zone: Zone, refused: Option<Error>) {
    let _cut = shared.cut.lock().await;
    mesh.forwarded.lock().await.remove(&zone);
    let (code, why) = match refused {
        Some(refused) => {
            let (share, reason) = (&zone.0, &refused.message);
            let why = format!("refused by node {peer}, which serves {share}: {reason}");
            (refused.code, why)
        }
        None => {
            let why = format!("forward from node {peer} ended");
            (ErrorCode::UnknownShare, why)
        }
    };
    let entries = shared.router.lock().await.entries();
    let subscribers = entries.into_iter().filter(|(_, at)| *at == zone);
    for (sid, _) in subscribers {
        refuse_subscription(shared, sid, &zone, code, why.clone()).await;
    }
}

/// Run one forward until its conversation ends: `Some` refusal when the claim
/// holder refused the read, which ends it (F5). The writes queued for it go
/// up in order, each held pending ([`Pending`]); one unanswered for
/// [`FORWARD_TIMEOUT`] is answered `UnknownShare`, and so is each one
/// pending or queued when the forward ends: it was not placed (W5).
async fn run_forward(
    shared: &Arc<Shared>,
    linked: &Arc<Linked>,
    peer: &str,
    zone: &Zone,
    mut writes: mpsc::UnboundedReceiver<Write>,
) -> io::Result<Option<Error>> {
    let mut pending = Pending::default();
    let ended = carry(shared, linked, peer, zone, &mut writes, &mut pending).await;
    writes.close();
    let why = format!("forward from node {peer} ended");
    let queued = std::iter::from_fn(|| writes.try_recv().ok());
    for write in pending.drain().chain(queued) {
        unplaced(shared, write, why.clone()).await;
    }
    ended
}

/// [`run_forward`]'s conversation, from its subscribe to its end.
async fn carry(
    shared: &Arc<Shared>,
    linked: &Arc<Linked>,
    peer: &str,
    (share, glade_id, key): &Zone,
    writes: &mut mpsc::UnboundedReceiver<Write>,
    pending: &mut Pending,
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
    let mut queued = true;
    let waited = FORWARD_TIMEOUT.as_secs();
    let unanswered = format!("no answer from node {peer} in {waited} s");
    loop {
        let due = pending.due();
        tokio::select! {
            write = writes.recv(), if queued => match write {
                Some(write) => up(shared, &conversation, pending, write).await?,
                None => queued = false,
            },
            read = conversation.recv() => match read {
                Ok(Frame::Ops(ops)) => {
                    for op in ops.ops {
                        // Scoped ingest: this conversation carries ONE zone's interest —
                        // the holder can't use it to push any other zone into our
                        // replica.
                        if op.share == *share && op.glade_id == *glade_id && op.key == *key {
                            let _ = ingest_and_fanout(shared, from_sid, op).await;
                        }
                    }
                }
                // An op's status (R1) names its op: it ends nothing.
                Ok(Frame::Error(answer)) if answer.corr.is_some() => {}
                // The claim holder's refusal (plan Step 4.3), after an ack that
                // names no zone or, from its re-check pass, alone; it then
                // ends the conversation.
                Ok(Frame::Error(refused)) => return Ok(Some(refused)),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // interest closed
                Err(e) => return Err(e),
            },
            () = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                for write in pending.expired(Instant::now()) {
                    unplaced(shared, write, unanswered.clone()).await;
                }
            }
        }
    }
    conversation.end();
    Ok(None)
}

/// Send `write` up the forward, held pending at the claim holder (W1, W6).
/// An op over the link's frame limit cannot cross, and is refused here; any
/// other failure ends the forward, `write` held for its end to answer.
async fn up(
    shared: &Arc<Shared>,
    conversation: &Conversation,
    pending: &mut Pending,
    write: Write,
) -> io::Result<()> {
    let sent = conversation.send(&Frame::Ops(Ops {
        ops: vec![write.op.clone()],
        pri: None,
    }));
    let over = |e: &io::Error| e.kind() == io::ErrorKind::InvalidInput;
    if sent.as_ref().is_err_and(over) {
        let why = "refused: the op is over the peer link's frame limit".to_string();
        let status = op_status(&write.op, ErrorCode::Protocol, why);
        send(shared, write.writer, &status).await;
        return Ok(());
    }
    pending.hold(write, Instant::now());
    sent
}

/// The writes a forward has sent up and holds pending at the claim holder,
/// in the order sent, each with the instant its answer is due by (W5, W6).
#[derive(Default)]
pub(super) struct Pending(VecDeque<(Write, Instant)>);

impl Pending {
    /// Hold `write`, sent at `sent`: due an answer [`FORWARD_TIMEOUT`] later.
    pub(super) fn hold(&mut self, write: Write, sent: Instant) {
        self.0.push_back((write, sent + FORWARD_TIMEOUT));
    }

    /// When the first write held is due its answer by, if one is held.
    pub(super) fn due(&self) -> Option<Instant> {
        self.0.front().map(|(_, due)| *due)
    }

    /// The writes due an answer by `now`, which have had none, first sent
    /// first.
    pub(super) fn expired(&mut self, now: Instant) -> Vec<Write> {
        let mut expired = Vec::new();
        while self.0.front().is_some_and(|(_, due)| *due <= now) {
            expired.extend(self.0.pop_front().map(|(write, _)| write));
        }
        expired
    }

    /// Every write held, first sent first.
    fn drain(&mut self) -> impl Iterator<Item = Write> + '_ {
        self.0.drain(..).map(|(write, _)| write)
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
