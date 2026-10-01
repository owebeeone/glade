//! One acceptance path for client ops (cross-node writes plan, step X2.1).
//!
//! A session's ops are decided here, one by one, whatever carried them: the
//! websocket `Ops` arm (`server.rs`), and at a claim holder the writes a
//! forwarding node sends on the forward it serves (X3.1, `mesh/serve.rs`).
//! So the placement checks are made here, where every client op passes: a
//! client's op by its share's route (W1, X2.3), a forwarded op by the
//! holder's own fold (W2, X3.1). GladeSubstrateV1 §6, "Session answers";
//! client-writes plan Step 2.1.

use std::collections::BTreeMap;
use std::sync::Arc;

use glade_wire::generated::{ErrorCode, Op, Ops, Shape};

use crate::frame::Frame;
use crate::mesh::{route_subscribe, who_serves, Route};
use crate::registry::HOME;
use crate::router::{SessionId, Zone};
use crate::server::{raise, send, Shared};
use crate::session::{error_frame, op_status, Heads};
use crate::store::Append;
use crate::sysdir::now_ms;

/// A session's heads, per zone-surface: each origin's highest seq that the
/// session announced (R4), or sent and the node holds (R3).
pub(crate) type SessionHeads = BTreeMap<Zone, Heads>;

/// What carried a batch of ops, which says how each is placed.
#[derive(Clone, Copy)]
pub(crate) enum Source<'a> {
    /// A client's session: each op is placed by the route its subscribe
    /// would get (W1, X2.3).
    Client,
    /// The forward of `zone` this node serves as the share's claim holder
    /// (W2, X3.1): it carries the forwarding node's writes on that zone
    /// alone, placed only while this node's fold names it the holder.
    Forward(&'a Zone),
}

/// Decide each op of `ops`, a batch from session `origin`, in order, and
/// answer each on `origin` once it is decided (R1): an `Error` frame whose
/// `corr` is the op's hash, `Ok` once the node holds the op, and for an
/// appended op only once its fan-out to the zone's other sessions is queued
/// (R2); otherwise the refusal. Only an op the node holds joins `heads` (R3).
/// One op's refusal never stops the batch. `source` says how an op is
/// placed ([`Source`]): an op not placed is answered `UnknownShare` with the
/// reason, and kept nowhere. A forward's op on another zone is refused.
pub(crate) async fn accept_ops(
    shared: &Arc<Shared>,
    origin: SessionId,
    heads: &mut SessionHeads,
    ops: Vec<Op>,
    source: Source<'_>,
) {
    let mut routes = BTreeMap::new();
    for op in ops {
        // H-R3: a client submits intent, and appends no record with a
        // privileged effect. Every home record kind has one, and the node
        // writes its own (`claims::publish`), so a client's op on home is
        // refused before any of it is kept (plan Step 4.3, part 1).
        if op.share == HOME {
            send(shared, origin, &home_refused(&op)).await;
            continue;
        }
        // F3 (question 13): a `stream` op has no op path, so a client's is
        // refused before any of it is kept, whatever the store holds.
        if op.shape == Shape::Stream {
            send(shared, origin, &stream_refused(&op)).await;
            continue;
        }
        // W2 (X3.1): a forward carries its own zone's ops alone.
        if let Source::Forward(zone) = source {
            if !in_zone(&op, zone) {
                send(shared, origin, &off_zone(&op, zone)).await;
                continue;
            }
        }
        // W1 (X2.3) and W2 (X3.1), after the refusals: an op every node
        // refuses is refused, never answered `UnknownShare`, which its
        // client would keep and send again (W5).
        if let Some(reason) = not_placed(shared, source, &mut routes, &op.share).await {
            let status = op_status(&op, ErrorCode::UnknownShare, reason);
            send(shared, origin, &status).await;
            continue;
        }
        // R4: the cut is held from the append until the fan-out is queued
        // (plan Step 2.2).
        let cut = shared.cut.lock().await;
        let res = shared.store.lock().await.append(op.clone());
        let status = match res {
            Ok(Append::Appended) => {
                hold(heads, &op);
                let status = op_status(&op, ErrorCode::Ok, "appended".into());
                let router = shared.router.lock().await;
                let targets = router.route(origin, &op.share, &op.glade_id, &op.key);
                drop(router);
                let frame = Frame::Ops(Ops {
                    ops: vec![op],
                    pri: None,
                });
                for t in targets {
                    send(shared, t, &frame).await;
                }
                status
            }
            Ok(Append::Duplicate) => {
                hold(heads, &op);
                op_status(&op, ErrorCode::Ok, "already held".into())
            }
            // Taken as seen, not held (R2): the session's heads stay, and
            // every op of its chain is above it.
            Ok(Append::BelowRetained) => {
                let message = format!(
                    "({},{}) is below the first seq its chain holds",
                    op.origin, op.seq
                );
                op_status(&op, ErrorCode::Retention, message)
            }
            Err(e) => error_frame(&e, &op),
        };
        drop(cut);
        send(shared, origin, &status).await;
    }
}

/// Why an op on `share` is not placed, if it is not, asked once per share
/// per frame, `routes` holding the frame's answers so far. A client's op is
/// placed by the C2 decision a subscribe of `share` would get (W1, X2.3):
/// `Absent` is not placed; `Local` places the op here, and so, until X3.2
/// sends it to the claim holder, does `Forward`. A forwarded op is placed
/// only while this node holds the share ([`not_held`]).
async fn not_placed(
    shared: &Arc<Shared>,
    source: Source<'_>,
    routes: &mut BTreeMap<String, Option<String>>,
    share: &str,
) -> Option<String> {
    if let Some(asked) = routes.get(share) {
        return asked.clone();
    }
    let absent = match source {
        Source::Forward(_) => not_held(shared, share).await,
        Source::Client => match route_subscribe(shared, share).await {
            Route::Absent(reason) => Some(reason),
            Route::Local | Route::Forward(_) => None,
        },
    };
    routes.insert(share.into(), absent.clone());
    absent
}

/// Why this node takes no forwarded write on `share` (W2, X3.1), if it does
/// not: only while its fold, at its clock, names it the share's live claim
/// holder, never for a share another node or no live claim holds.
async fn not_held(shared: &Arc<Shared>, share: &str) -> Option<String> {
    let me = shared.mesh.get().map(|mesh| mesh.self_id.as_str());
    let holder = who_serves(&*shared.store.lock().await, share, now_ms());
    match holder {
        Some(id) if Some(id.as_str()) == me => None,
        Some(id) => Some(format!("{share} is served by node {id}, not by this node")),
        None => Some(format!("no live ServeClaim for {share}")),
    }
}

/// Whether `op` is on `zone`.
fn in_zone(op: &Op, (share, glade_id, key): &Zone) -> bool {
    op.share == *share && op.glade_id == *glade_id && op.key == *key
}

/// The answer to an op of another zone on the forward of `zone` (W2,
/// X3.1): its status (R1), under the wire's `Protocol` code.
fn off_zone(op: &Op, (share, glade_id, _): &Zone) -> Frame {
    let message = format!("refused: the forward of {share} {glade_id} carries no other zone's op");
    op_status(op, ErrorCode::Protocol, message)
}

/// The answer to a client's op on the home share (ruling H-R3, plan Step 4.3):
/// the node's answer to any refused op, the op's status (R1), here under the
/// wire's `Unauthorized` code.
fn home_refused(op: &Op) -> Frame {
    let message = format!("refused: only the node writes the {HOME} share (H-R3)");
    op_status(op, ErrorCode::Unauthorized, message)
}

/// The answer to a client's `stream` op (F3, question 13; the owner's ruling
/// of 2026-09-27): its status (R1), under the wire's `Protocol` code. A
/// stream is a live channel, never stored, so it has no op path
/// (`GladeShapeDispatch.md`), and neither client sends or folds one.
fn stream_refused(op: &Op) -> Frame {
    let message = "refused: stream has no op path; a stream is a live channel, never stored";
    op_status(op, ErrorCode::Protocol, message.into())
}

/// R3: the session's heads take the seq of an op the node holds, and never
/// fall, so a repeat of a lower seq leaves them where they are.
fn hold(heads: &mut SessionHeads, op: &Op) {
    let zone = (op.share.clone(), op.glade_id.clone(), op.key.clone());
    raise(heads.entry(zone).or_default(), &op.origin, op.seq);
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use tokio::sync::mpsc::{self, UnboundedReceiver};

    use super::*;
    use crate::registry::G_GRANTS;
    use crate::server::Server;

    /// Poll `future` to completion on this thread, with no runtime, socket or
    /// clock (LBT-008). The path waits only on locks nothing else holds, so a
    /// future still pending after many polls is a defect.
    fn run<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..64 {
            if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
                return out;
            }
        }
        panic!("still pending: the path waited on something outside the test");
    }

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

    /// The op's hash in lower-case hex, the `corr` R1 promises, written out
    /// here rather than taken from the code under test.
    fn corr(op: &Op) -> Option<String> {
        let hash = crate::chain::op_hash(op);
        Some(hash.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Every frame queued on a session, in order.
    fn queued(rx: &mut UnboundedReceiver<Vec<u8>>) -> Vec<Frame> {
        let mut frames = Vec::new();
        while let Ok(bytes) = rx.try_recv() {
            frames.push(Frame::from_bytes(&bytes).unwrap());
        }
        frames
    }

    /// X2.1: one batch from one session, decided with no socket. New, repeat,
    /// fork, past a gap and `home` are answered on that session `Ok`, `Ok`,
    /// `Equivocation`, `Protocol` and `Unauthorized`, in order, each naming
    /// its op by hash (R1). The session is subscribed to the zone, yet only
    /// the zone's other subscriber gets the appended op, once: the fan-out
    /// skips the origin. Only the held op's seq joins the origin's heads (R3).
    #[test]
    fn the_acceptance_path_answers_a_batch_without_a_socket() {
        let dir = std::env::temp_dir().join("glade-accept-batch");
        let _ = std::fs::remove_dir_all(&dir);
        let shared = Server::open(&dir).unwrap().shared;
        let (origin, other) = (1, 2);
        let mut queues = Vec::new();
        for sid in [origin, other] {
            let (tx, rx) = mpsc::unbounded_channel();
            shared.out.try_lock().unwrap().insert(sid, tx);
            let mut router = shared.router.try_lock().unwrap();
            router.subscribe(sid, "sh", "g", &[]);
            queues.push(rx);
        }

        let new = op("w", 0, b"zero");
        let fork = op("w", 0, b"another zero");
        let past_gap = op("w", 5, b"five");
        let on_home = Op {
            share: HOME.into(),
            glade_id: G_GRANTS.into(),
            ..op("w", 0, b"home")
        };
        let batch = vec![
            new.clone(),
            new.clone(),
            fork.clone(),
            past_gap.clone(),
            on_home.clone(),
        ];
        let mut heads = SessionHeads::new();
        run(accept_ops(
            &shared,
            origin,
            &mut heads,
            batch,
            Source::Client,
        ));

        let answers: Vec<(ErrorCode, Option<String>)> = queued(&mut queues[0])
            .into_iter()
            .map(|frame| match frame {
                Frame::Error(e) => (e.code, e.corr),
                other => panic!("the origin got a frame that is no answer: {other:?}"),
            })
            .collect();
        let want = [
            (ErrorCode::Ok, corr(&new)),
            (ErrorCode::Ok, corr(&new)),
            (ErrorCode::Equivocation, corr(&fork)),
            (ErrorCode::Protocol, corr(&past_gap)),
            (ErrorCode::Unauthorized, corr(&on_home)),
        ];
        assert_eq!(answers, want, "an answer per op, in order, naming it");
        let fanned = queued(&mut queues[1]);
        let once = [Frame::Ops(Ops {
            ops: vec![new],
            pri: None,
        })];
        assert_eq!(fanned, once, "the other subscriber gets the new op, once");
        let zone = ("sh".to_string(), "g".to_string(), vec![]);
        let held = SessionHeads::from([(zone, Heads::from([("w".to_string(), 0)]))]);
        assert_eq!(heads, held, "only the held op's seq joins the heads");
    }
}
