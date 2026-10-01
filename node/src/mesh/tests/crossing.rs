//! The forwarding node's writes (cross-node writes plan X3.2): a client's op
//! on a share another node holds goes up the zone's forward to the claim
//! holder (W1), which decides it (W2, X3.1), and the forwarding node answers
//! `UnknownShare` for a write it could not place (W5). Here B holds the claim
//! and A forwards, the plan's letters swapped.

use std::time::Duration;

use glade_wire::generated::{ErrorCode, Op, Shape};
use tokio::time::Instant;

use super::two_nodes::{
    a_client, next_frame, ops_frame, payloads, sub, tree_zone, two_nodes, two_nodes_limited,
};
use crate::chain::op_hash;
use crate::exchange::FORWARD_TIMEOUT;
use crate::frame::Frame;
use crate::mesh::route::{Pending, Write};
use crate::mesh::{hex_id, release_links};
use crate::ws::{WsReader, WsWriter};

/// c, a client of A, writes `payload` on the tree zone at `seq`, after
/// `prev`.
fn c_writes(seq: i64, prev: Option<&Op>, payload: &[u8]) -> Op {
    Op {
        share: "ws-razel".into(),
        glade_id: "ws.tree".into(),
        key: vec![],
        origin: "c-on-a".into(),
        seq,
        prev: prev.map(|op| op_hash(op).to_vec()),
        lamport: seq,
        refs: vec![],
        shape: Shape::Value,
        payload: payload.to_vec(),
    }
}

/// Wait until A's session has taken every frame `w` sent before: a zone no
/// directory knows is subscribed, and its ack read. Nothing else may come.
async fn taken(r: &mut WsReader, w: &WsWriter) {
    w.send_binary(&sub("plain", "barrier")).await.unwrap();
    let ack = next_frame(r, "the barrier's ack").await;
    assert!(matches!(ack, Frame::Heads(_)), "{ack:?}");
}

/// The status `r` next reads for `op` (R1), passing over anything else: its
/// code and its message.
async fn status_of(r: &mut WsReader, op: &Op) -> (ErrorCode, String) {
    let corr = Some(hex_id(&op_hash(op)));
    loop {
        if let Frame::Error(e) = next_frame(r, "the op's status").await {
            if e.corr == corr {
                return (e.code, e.message);
            }
        }
    }
}

/// W5's node half (X3.2): a write pending at the claim holder when the
/// forward ends is answered `UnknownShare`. c, a client of A subscribed to
/// the tree zone, writes while B's store is held, so B cannot answer, and A
/// has taken the write; then B releases its links. c's op is answered
/// `UnknownShare`, naming it by its hash, with the forward's end, and A
/// keeps nothing of it. Red before X3.2: A appended the op itself and
/// answered `Ok`.
#[tokio::test(flavor = "multi_thread")]
async fn writes_pending_when_the_forward_ends_are_answered_unknown_share() {
    let t = two_nodes("x32-pending", Some(&["read.*"])).await;
    let (mut rc, wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);

    let op = c_writes(0, None, b"pending");
    let held = t.b.store.lock().await;
    wc.send_binary(&ops_frame(vec![op.clone()])).await.unwrap();
    taken(&mut rc, &wc).await;
    release_links(&t.b).await;
    let ended = format!("forward from node {} ended", t.b_id);
    assert_eq!(
        status_of(&mut rc, &op).await,
        (ErrorCode::UnknownShare, ended)
    );
    drop(held);
    let store = t.a.store.lock().await;
    let kept = store.scan("ws-razel", "ws.tree", &[], "c-on-a", i64::MIN);
    assert!(kept.is_empty(), "A keeps nothing: {kept:?}");
}

/// An op over the peer link's frame limit cannot cross (X3.2). On links of
/// 4 KiB frames, c's 8 KiB op on the tree zone is refused at A, `Protocol`,
/// naming it by its hash; neither node keeps it, and A's forward of the
/// zone goes on. Red before X3.2: A appended it itself and answered `Ok`.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_over_the_links_frame_limit_is_refused_where_it_was_written() {
    const LIMIT: usize = 4 << 10;
    let t = two_nodes_limited("x32-over", Some(&["read.*"]), LIMIT).await;
    let (mut rc, wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);

    let big = c_writes(0, None, &[7; 2 * LIMIT]);
    wc.send_binary(&ops_frame(vec![big.clone()])).await.unwrap();
    let (code, why) = status_of(&mut rc, &big).await;
    assert_eq!(code, ErrorCode::Protocol, "{why}");
    assert!(why.contains("frame limit"), "{why}");
    for node in [&t.a, &t.b] {
        let store = node.store.lock().await;
        let kept = store.scan("ws-razel", "ws.tree", &[], "c-on-a", i64::MIN);
        assert!(kept.is_empty(), "kept: {kept:?}");
    }
    let forwards = t.a.mesh.get().unwrap().forwarded.lock().await;
    assert!(forwards.contains_key(&tree_zone()), "the forward goes on");
}

/// W5's wait (X3.2), on the forward's table of pending writes, with no node
/// or clock: each write is due its answer 12 s after it was sent, the first
/// sent first. At its instant the first is due its `UnknownShare`, and the
/// next is not; a second later, the next is.
#[test]
fn a_write_unanswered_for_12_s_is_due_its_unknown_share() {
    let write = |seq| Write {
        op: c_writes(seq, None, b"unanswered"),
        writer: 7,
    };
    let seqs = |writes: Vec<Write>| writes.iter().map(|w| w.op.seq).collect::<Vec<_>>();
    let sent = Instant::now();
    let mut pending = Pending::default();
    pending.hold(write(0), sent);
    pending.hold(write(1), sent + Duration::from_secs(1));
    let due = sent + FORWARD_TIMEOUT;
    assert_eq!(
        FORWARD_TIMEOUT,
        Duration::from_secs(12),
        "the exchanges' wait"
    );
    assert_eq!(pending.due(), Some(due));
    let just_before = due - Duration::from_millis(1);
    assert!(pending.expired(just_before).is_empty(), "nothing before");
    assert_eq!(seqs(pending.expired(due)), [0], "the first, alone");
    assert_eq!(pending.due(), Some(due + Duration::from_secs(1)));
    assert_eq!(seqs(pending.expired(due + Duration::from_secs(1))), [1]);
    assert_eq!(pending.due(), None, "nothing left");
}
