use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use glade_grant_api::{GrantPort, Holder};
use glade_wire::cbor;
use glade_wire::generated::{ErrorCode, Heads, Op, Ops, Priority, Subscribe};

use crate::accept::{accept_ops, SharedHeads, Source};
use crate::conversation::Conversation;
use crate::frame::Frame;
use crate::grants::{refusal, READ_SUBSCRIBE};
use crate::peer::OPS_PER_CHUNK;
use crate::registry::HOME;
use crate::server::Shared;
use crate::session::{ack, missing_for, refused_subscribe, serve_order};
use crate::tasks::Site;

use super::{forwards_return, hex_id, pull_on_gap, Mesh, Round};

/// Serve one conversation the peer opened, by its first frame: `Heads` = a
/// home-scoped sync pull (serve the gap, END); `Subscribe` = a forwarded
/// interest (this node is the claim holder — serve gap + live ops until the
/// interest closes); `ExchangeReq` = a forwarded exchange (this node is the
/// claim holder — the attached authority answers, one conversation one
/// exchange, `exchange.rs`); `Ops` = a peer's home-share PUSH (freshly-minted
/// directory records, the B9 step) — scoped ingest, home ops only, one frame
/// per conversation, one [`Round`]; a chain it leaves short as a gap starts a
/// pull from the peer ([`pull_on_gap`]), and a round that lands records may
/// route a local subscriber's zone to the peer, so it brings back the
/// forwards they call for ([`forwards_return`], cross-node writes plan
/// X4.2b). `node` is the peer, as its HELLO proved it.
pub(super) async fn serve_conversation(
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
            let landed = round.end().applied;
            conversation.end();
            if landed > 0 {
                forwards_return(&shared, &hex_id(&node)).await;
            }
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
///
/// The ack is a cut (R4, R5; cross-node writes plan X2.2), as a client's is:
/// the ack and the gap are read under one hold of the store lock and queued
/// under the cut that registered the stream, which every fan-out holds from
/// its append until its ops are queued, so each op of the zone reaches the
/// peer once, after the ack. The ack names each origin's head by seq and
/// hash.
///
/// The holder decides the forwarding node's writes (W2, cross-node writes
/// plan X3.1): an `Ops` frame on the stream goes through the acceptance path
/// with the stream's session as origin ([`Source::Forward`]), so the
/// fan-out skips the stream, and each op's status is queued on it behind
/// the fan-out before it. Each op needs `write.append` on `node` there
/// (X4.1), and is refused `Unauthorized` without it.
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
    let (acked, gap) = {
        let st = shared.store.lock().await;
        let gap = missing_for(&st, &s.share, &s.glade_id, &key, &their);
        (ack(&st, &s.share, &s.glade_id, &key), gap)
    };
    let _ = tx.send(acked.to_bytes());
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
    // What it reads is the forwarding node's writes, decided as they come.
    let zone = (s.share.clone(), s.glade_id.clone(), key);
    let heads = SharedHeads::default();
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
                match read {
                    Ok(Frame::Ops(ops)) => {
                        let source = Source::Forward(&zone, node);
                        accept_ops(&shared, sid, &heads, ops.ops, source).await;
                    }
                    Ok(_) => {}
                    Err(_) => break false,
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

/// Respond to a peer's home-share pull: ship exactly the home-zone ops the
/// peer lacks (bulk, in chunks under the link's frame limit, plan Step
/// 4.5b), `dir.checkpoints` first (plan Step 4.5c), so the chunks, which keep
/// the order, carry each checkpoint ahead of the chain it folds, then END —
/// END = gap complete. Scoped to HOME: connect-time
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
        for (share, glade_id, key) in serve_order(st.zones()) {
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
