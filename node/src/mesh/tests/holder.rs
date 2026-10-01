//! W2 at the claim holder (cross-node writes plan X3.1): the holder takes the
//! forwarding node's writes on the forward it serves, through its acceptance
//! path with the stream's session as origin, and answers each on the
//! stream. The forwarding node is played by hand: a conversation of A's link
//! to B subscribes the tree zone, as A's forward does, then writes on it.
//! Here B holds the claim and A forwards, the plan's letters swapped.

use std::sync::atomic::Ordering;
use std::time::Duration;

use glade_wire::generated::{ErrorCode, Op, Ops, Shape};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use super::support::{node_of, signed, C_SEED};
use super::two_nodes::{subscribed_on_the_link, two_nodes, TwoNodes};
use crate::chain::op_hash;
use crate::conversation::Conversation;
use crate::frame::Frame;
use crate::mesh::hex_id;
use crate::registry::{Record, G_PRINCIPALS, HOME};
use crate::sysdata::ServeClaim;
use crate::sysdir::now_ms;

/// The next frame B sends on `forward`, bounded: a hang is a failure.
async fn next_on(forward: &mut Conversation, what: &str) -> Frame {
    let read = tokio::time::timeout(Duration::from_secs(5), forward.recv());
    let read = read
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    read.unwrap_or_else(|e| panic!("the forward ended waiting for {what}: {e}"))
}

/// A's forward of the tree zone, played by hand: subscribed on a
/// conversation of A's link to B, B's ack and the tree's two ops read.
async fn forward_by_hand(t: &TwoNodes) -> Conversation {
    let mut forward = subscribed_on_the_link(t).await;
    let ack = next_on(&mut forward, "B's ack").await;
    assert!(matches!(ack, Frame::Heads(_)), "{ack:?}");
    let mut gap = Vec::new();
    while gap.len() < t.tree.len() {
        match next_on(&mut forward, "the tree's gap").await {
            Frame::Ops(ops) => gap.extend(ops.ops),
            other => panic!("expected the tree's gap, got {other:?}"),
        }
    }
    assert_eq!(gap, t.tree, "the gap, in order");
    forward
}

/// A session of B subscribed to the tree zone, registered by hand: its
/// queue.
async fn b_subscriber(t: &TwoNodes) -> UnboundedReceiver<Vec<u8>> {
    let (tx, rx) = mpsc::unbounded_channel();
    let sid = t.b.next.fetch_add(1, Ordering::SeqCst);
    t.b.out.lock().await.insert(sid, tx);
    t.b.router
        .lock()
        .await
        .subscribe(sid, "ws-razel", "ws.tree", &[]);
    rx
}

/// The first op c, a client of A, writes on the tree zone.
fn c_writes(payload: &[u8]) -> Op {
    Op {
        share: "ws-razel".into(),
        glade_id: "ws.tree".into(),
        key: vec![],
        origin: "c-on-a".into(),
        seq: 0,
        prev: None,
        lamport: 0,
        refs: vec![],
        shape: Shape::Value,
        payload: payload.to_vec(),
    }
}

/// A writes `ops` on its forward, one `Ops` frame.
fn written(forward: &Conversation, ops: &[Op]) {
    let ops = ops.to_vec();
    forward.send(&Frame::Ops(Ops { ops, pri: None })).unwrap();
}

/// B's next frame on the forward, which must be an op's status (R1): its
/// code, whether it names `op` by its hash, and its message.
async fn status(forward: &mut Conversation, op: &Op) -> (ErrorCode, bool, String) {
    match next_on(forward, "the op's status").await {
        Frame::Error(e) => {
            let hash: String = op_hash(op).iter().map(|b| format!("{b:02x}")).collect();
            (e.code, e.corr == Some(hash), e.message)
        }
        other => panic!("expected the op's status, got {other:?}"),
    }
}

/// How many ops of `glade_id` on `share` B holds from c.
async fn held_at_b(t: &TwoNodes, share: &str, glade_id: &str) -> usize {
    let store = t.b.store.lock().await;
    store.scan(share, glade_id, &[], "c-on-a", i64::MIN).len()
}

/// W2 (X3.1): A's write on its forward lands at the claim holder, through
/// its acceptance path with the stream as origin. B stores it, B's
/// subscriber of the zone gets it once, and the forward's next frame is the
/// op's status, `Ok` naming it by its hash: the fan-out, which skips its
/// origin, sent the stream no copy ahead of it. Red before X3.1: B's read
/// loop discarded every frame A sent on the forward, so no status came.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwarded_op_lands_at_the_claim_holder_and_is_answered() {
    let t = two_nodes("x31-lands", Some(&["read.subscribe"])).await;
    let mut subscriber = b_subscriber(&t).await;
    let mut forward = forward_by_hand(&t).await;

    let op = c_writes(b"from c");
    written(&forward, std::slice::from_ref(&op));
    let (code, named, why) = status(&mut forward, &op).await;
    assert_eq!((code, named), (ErrorCode::Ok, true), "{why}");
    assert_eq!(held_at_b(&t, "ws-razel", "ws.tree").await, 1, "stored at B");
    let fanned = Frame::from_bytes(&subscriber.try_recv().unwrap()).unwrap();
    let once = Frame::Ops(Ops {
        ops: vec![op],
        pri: None,
    });
    assert_eq!(fanned, once, "B's subscriber gets the op");
    assert!(subscriber.try_recv().is_err(), "once");
}

/// W2's refusals (X3.1): the holder takes only the forward's own zone. On
/// the tree zone's forward, one frame writes an op of another zone of the
/// share, then an op on `home`, then the zone's own op. They are answered
/// on the forward in order, each naming its op by its hash: `Protocol`,
/// `Unauthorized` (H-R3) and `Ok`. B keeps neither refused op, and the
/// zone's own op lands after them. Red before X3.1: no status.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwarded_op_off_its_zone_or_on_home_is_refused_and_not_stored() {
    let t = two_nodes("x31-off", Some(&["read.subscribe"])).await;
    let mut forward = forward_by_hand(&t).await;

    let off = Op {
        glade_id: "ws.notes".into(),
        ..c_writes(b"another zone")
    };
    let on_home = Op {
        share: HOME.into(),
        glade_id: G_PRINCIPALS.into(),
        ..c_writes(b"home")
    };
    let own = c_writes(b"its own zone");
    written(&forward, &[off.clone(), on_home.clone(), own.clone()]);
    let mut answers = Vec::new();
    for op in [&off, &on_home, &own] {
        let (code, named, _) = status(&mut forward, op).await;
        answers.push((code, named));
    }
    let want = [
        (ErrorCode::Protocol, true),
        (ErrorCode::Unauthorized, true),
        (ErrorCode::Ok, true),
    ];
    assert_eq!(answers, want, "an answer per op, in order, naming it");
    assert_eq!(
        held_at_b(&t, "ws-razel", "ws.notes").await,
        0,
        "off its zone"
    );
    assert_eq!(held_at_b(&t, HOME, G_PRINCIPALS).await, 0, "on home");
    assert_eq!(held_at_b(&t, "ws-razel", "ws.tree").await, 1, "its own");
}

/// W2's holder check (X3.1): the holder takes a forwarded write only while
/// its fold names it the share's holder. Once B's fold holds a live claim
/// on `ws-razel` by another node at a higher epoch, as a takeover leaves it,
/// the forward's op is answered `UnknownShare`, naming that node, and kept
/// nowhere: B stores none, and its subscriber of the zone gets none. Red
/// before X3.1: no status.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_that_no_longer_holds_the_claim_takes_no_forwarded_write() {
    let t = two_nodes("x31-moved", Some(&["read.subscribe"])).await;
    let mut subscriber = b_subscriber(&t).await;
    let mut forward = forward_by_hand(&t).await;
    let taker = hex_id(&node_of(C_SEED));
    let taken = ServeClaim {
        node: taker.clone(),
        share: "ws-razel".into(),
        lease_expiry_ms: now_ms() + 30_000,
        epoch: 2,
    };
    let taken = signed(C_SEED, Record::Serve(taken));
    t.b.store.lock().await.append(taken).unwrap();

    let op = c_writes(b"too late");
    written(&forward, std::slice::from_ref(&op));
    let (code, named, why) = status(&mut forward, &op).await;
    assert_eq!((code, named), (ErrorCode::UnknownShare, true), "{why}");
    assert!(why.contains(&taker), "the status names the holder: {why}");
    assert_eq!(
        held_at_b(&t, "ws-razel", "ws.tree").await,
        0,
        "nothing stored"
    );
    assert!(subscriber.try_recv().is_err(), "nothing fanned out");
}
