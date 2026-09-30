use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use glade_wire::generated::{Error, ErrorCode, Head, Subscribe};

use crate::conversation::Linked;
use crate::envelope;
use crate::frame::Frame;
use crate::registry::HOME;
use crate::router::Zone;
use crate::server::{refuse_subscription, Shared};
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

/// The A-side of the C2 decision's Forward arm: open a conversation on the
/// claim holder's link, send the interest (with our replica's heads as the
/// resume point), and ingest what comes back into the LOCAL replica — local
/// subscribers are then fed by the ordinary fan-out (replica serves reads,
/// trace C5→C6). Deduped per zone: one conversation carries any number of
/// local subscribers. The forward lapses with the conversation; a later
/// subscribe retries. Its end, and a refusal the claim holder sends on it,
/// reach the local subscribers ([`lapse`]).
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
