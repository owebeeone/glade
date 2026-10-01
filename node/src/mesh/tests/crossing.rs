//! The forwarding node's writes (cross-node writes plan X3.2): a client's op
//! on a share another node holds goes up the zone's forward to the claim
//! holder (W1), which decides it (W2, X3.1). The forwarding node lands what
//! the holder accepted and relays every answer (W3), and answers
//! `UnknownShare` for a write it could not place (W5). Here B holds the
//! claim and A forwards, the plan's letters swapped.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use glade_wire::generated::{ErrorCode, Op, Shape};
use glade_wire::swmr::{encode_swmr, SwmrAction};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::Instant;

use super::support::{fresh, wait_store};
use super::two_nodes::{
    a_client, next_frame, ops_frame, payloads, relinked, sub, tree_zone, two_nodes,
    two_nodes_limited, TwoNodes,
};
use crate::accept::SharedHeads;
use crate::chain::op_hash;
use crate::claims::testing;
use crate::exchange::FORWARD_TIMEOUT;
use crate::frame::{Frame, MAX_FRAME_BYTES};
use crate::mesh::route::{Pending, Write};
use crate::mesh::testing::{endpoint_key, on_carrier};
use crate::mesh::{hex_id, release_links, who_serves};
use crate::registry::{Record, RegistryApi};
use crate::server::{Server, Shared};
use crate::session::op_status;
use crate::store::Store;
use crate::sysdata::CapabilityGrant;
use crate::sysdir::{boot_at, now_ms};
use crate::ws::{WsReader, WsWriter};

/// c, a client of A, writes `payload` on the tree zone at `seq`, after
/// `prev`.
pub(super) fn c_writes(seq: i64, prev: Option<&Op>, payload: &[u8]) -> Op {
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
pub(super) async fn status_of(r: &mut WsReader, op: &Op) -> (ErrorCode, String) {
    let corr = Some(hex_id(&op_hash(op)));
    loop {
        if let Frame::Error(e) = next_frame(r, "the op's status").await {
            if e.corr == corr {
                return (e.code, e.message);
            }
        }
    }
}

/// c's ops on `glade_id` of `ws-razel` that `node` holds.
pub(super) async fn held_from_c(node: &Arc<Shared>, glade_id: &str) -> Vec<Op> {
    let store = node.store.lock().await;
    store.scan("ws-razel", glade_id, &[], "c-on-a", i64::MIN)
}

/// W5's node half (X3.2): a write pending at the claim holder when the
/// forward ends is answered `UnknownShare`. c, a client of A subscribed to
/// the tree zone, writes while B's store is held, so B cannot answer, and A
/// has taken the write; then B releases its links. c's op is answered
/// `UnknownShare`, naming it by its hash, with the forward's end, and A
/// keeps nothing of it. Once A is linked to B again, c subscribes the zone
/// again and sends the op again, as W5's client does: `Ok`, and each node
/// holds it once. Red before X3.2: A appended the op itself and answered
/// `Ok`; before X3.2b, the resend went unanswered.
#[tokio::test(flavor = "multi_thread")]
async fn writes_pending_when_the_forward_ends_are_answered_unknown_share() {
    let t = two_nodes("x32-pending", Some(&["read.*", "write.*"])).await;
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
    let kept = held_from_c(&t.a, "ws.tree").await;
    assert!(kept.is_empty(), "A keeps nothing: {kept:?}");

    relinked(&t).await;
    wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
    while !matches!(next_frame(&mut rc, "the ack").await, Frame::Heads(_)) {}
    wc.send_binary(&ops_frame(vec![op.clone()])).await.unwrap();
    let (code, why) = status_of(&mut rc, &op).await;
    assert_eq!(code, ErrorCode::Ok, "{why}");
    for node in [&t.a, &t.b] {
        let held = held_from_c(node, "ws.tree").await;
        assert_eq!(held, std::slice::from_ref(&op), "once");
    }
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

/// A third node, C, linked to B as A is: B grants it what it grants A,
/// `read.*` on `ws-razel`, C routes that share to B, and it serves clients
/// on the port returned.
async fn third_node(t: &TwoNodes, name: &str) -> (Arc<Shared>, u16) {
    let boot = boot_at(fresh(&format!("{name}-c-sys")), "gianni").unwrap();
    let grant = Record::Grant(CapabilityGrant {
        principal: boot.node_id.clone(),
        share: "ws-razel".into(),
        verbs: vec!["read.*".into()],
    });
    testing::accept(&t.b, vec![grant]).await.unwrap();
    let c = Server::open(fresh(&format!("{name}-c-store"))).unwrap();
    c.seed_registry(&boot.registry.snapshot()).await;
    let identity = boot.identity().unwrap();
    on_carrier(&c, identity, endpoint_key(), None, MAX_FRAME_BYTES).await;
    c.connect_peer(t.at_b.clone()).await.unwrap();
    let b_id = t.b_id.clone();
    let routed = move |st: &Store| who_serves(st, "ws-razel", now_ms()) == Some(b_id.clone());
    wait_store(&c.shared, routed, "C to route ws-razel to B").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shared = c.shared.clone();
    tokio::spawn(c.run(listener));
    (shared, port)
}

/// A client of the node at `port`, subscribed to each of `zones` of
/// `ws-razel`, each ack read.
async fn subscribed_to(port: u16, zones: &[&str]) -> (WsReader, WsWriter) {
    let (mut r, w) = crate::ws::connect("127.0.0.1", port).await.unwrap();
    for glade_id in zones {
        w.send_binary(&sub("ws-razel", glade_id)).await.unwrap();
        while !matches!(next_frame(&mut r, "the ack").await, Frame::Heads(_)) {}
    }
    (r, w)
}

/// A session of `node` subscribed by hand to each of `zones` of
/// `ws-razel`, which forwards nothing: its queue.
pub(super) async fn listening(node: &Arc<Shared>, zones: &[&str]) -> UnboundedReceiver<Vec<u8>> {
    let (tx, rx) = mpsc::unbounded_channel();
    let sid = node.next.fetch_add(1, Ordering::SeqCst);
    node.out.lock().await.insert(sid, tx);
    let mut router = node.router.lock().await;
    for glade_id in zones {
        router.subscribe(sid, "ws-razel", glade_id, &[]);
    }
    rx
}

/// c's ops among the frames `rx` holds now, in order.
pub(super) fn from_c(rx: &mut UnboundedReceiver<Vec<u8>>) -> Vec<Op> {
    let mut ops = Vec::new();
    while let Ok(bytes) = rx.try_recv() {
        if let Ok(Frame::Ops(frame)) = Frame::from_bytes(&bytes) {
            ops.extend(frame.ops.into_iter().filter(|op| op.origin == "c-on-a"));
        }
    }
    ops
}

/// c's next statuses on `r`, one for each of `ops`, by the hash each names
/// (R1): its code and its message. Nothing c wrote may come back to it.
async fn statuses(r: &mut WsReader, ops: &[Op]) -> BTreeMap<String, (ErrorCode, String)> {
    let mut answered = BTreeMap::new();
    while answered.len() < ops.len() {
        match next_frame(r, "c's statuses").await {
            Frame::Error(e) => {
                let corr = e.corr.expect("an op's status names it");
                answered.insert(corr, (e.code, e.message));
            }
            Frame::Ops(frame) => {
                let echo = frame.ops.iter().any(|op| op.origin == "c-on-a");
                assert!(!echo, "c got its own op back: {frame:?}");
            }
            other => panic!("expected c's statuses, got {other:?}"),
        }
    }
    answered
}

/// The op's hash, as a status names it.
fn named(op: &Op) -> String {
    hex_id(&op_hash(op))
}

/// W1 and W3 end to end (X3.2; the plan's
/// `a_write_on_b_reaches_a_and_every_subscriber`, its letters swapped). A
/// third node, C, forwards `ws-razel` to B as A does. c, a client of A, and
/// d, a client of C, subscribe the tree zone, `value`, and two zones of
/// `log` and `crdt`, as do a session of A's and one of B's. c writes an op on
/// each zone in one frame. c gets `Ok` for each, naming it, and none of its
/// ops back; every other subscriber, on A, B and C, gets each op; and each
/// node holds each op once. Red before X3.2b: the holder's answers went
/// unrelayed, so c's statuses did not come.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_on_a_reaches_b_and_every_subscriber() {
    const ZONES: [&str; 3] = ["ws.tree", "ws.log", "ws.crdt"];
    let t = two_nodes("x32-reach", Some(&["read.*", "write.*"])).await;
    let (c_node, port_c) = third_node(&t, "x32-reach").await;
    let (mut rc, wc) = subscribed_to(t.port_a, &ZONES).await;
    let (mut rd, _wd) = subscribed_to(port_c, &ZONES).await;
    let (mut on_a, mut on_b) = (listening(&t.a, &ZONES).await, listening(&t.b, &ZONES).await);

    let shaped = |glade_id: &str, shape, payload: &[u8]| Op {
        glade_id: glade_id.into(),
        shape,
        ..c_writes(0, None, payload)
    };
    let ops = [
        shaped("ws.tree", Shape::Value, b"a value"),
        shaped("ws.log", Shape::Log, b"a log entry"),
        shaped("ws.crdt", Shape::Crdt, b"a crdt delta"),
    ];
    wc.send_binary(&ops_frame(ops.to_vec())).await.unwrap();
    let answered = statuses(&mut rc, &ops).await;
    for op in &ops {
        let (code, why) = &answered[&named(op)];
        assert_eq!(*code, ErrorCode::Ok, "{}: {why}", op.glade_id);
    }
    let by_zone = |mut got: Vec<Op>| {
        got.sort_by(|x, y| x.glade_id.cmp(&y.glade_id));
        got
    };
    let mut want = ops.to_vec();
    want.sort_by(|x, y| x.glade_id.cmp(&y.glade_id));
    assert_eq!(by_zone(from_c(&mut on_a)), want, "A's other subscriber");
    assert_eq!(by_zone(from_c(&mut on_b)), want, "B's subscriber");
    let mut on_c = Vec::new();
    while on_c.len() < ops.len() {
        if let Frame::Ops(frame) = next_frame(&mut rd, "c's ops at C").await {
            on_c.extend(frame.ops.into_iter().filter(|op| op.origin == "c-on-a"));
        }
    }
    assert_eq!(by_zone(on_c), want, "C's subscriber");
    for node in [&t.a, &t.b, &c_node] {
        for op in &ops {
            let held = held_from_c(node, &op.glade_id).await;
            assert_eq!(held, std::slice::from_ref(op), "{} held once", op.glade_id);
        }
    }
    // R3: each op landed as c's, so c's next subscribe of the tree zone
    // ships the zone's gap without it.
    wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
    while !matches!(next_frame(&mut rc, "the ack").await, Frame::Heads(_)) {}
    match next_frame(&mut rc, "the tree's gap").await {
        Frame::Ops(gap) => {
            let echo = gap.ops.iter().any(|op| op.origin == "c-on-a");
            assert!(!echo, "c's op came back in the gap: {gap:?}");
        }
        other => panic!("expected the tree's gap, got {other:?}"),
    }
}

/// W3's landing goes through the verify path (X3.2): an op the holder
/// accepted that this node cannot hold gets this node's status, not the
/// holder's `Ok`. A's store already holds an op of c's at seq 0, put there
/// by hand; c writes another at that seq, which B, holding none of c's,
/// accepts. c is answered A's `Equivocation`, A keeps the op it held, and B
/// the one it accepted.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_the_holder_accepted_that_a_cannot_hold_gets_as_status() {
    let t = two_nodes("x32-fork", Some(&["read.*", "write.*"])).await;
    let held_at_a = c_writes(0, None, b"held at A");
    t.a.store.lock().await.append(held_at_a.clone()).unwrap();
    let (mut rc, wc) = crate::ws::connect("127.0.0.1", t.port_a).await.unwrap();

    let forked = c_writes(0, None, b"accepted at B");
    wc.send_binary(&ops_frame(vec![forked.clone()]))
        .await
        .unwrap();
    let (code, why) = status_of(&mut rc, &forked).await;
    assert_eq!(code, ErrorCode::Equivocation, "{why}");
    assert_eq!(held_from_c(&t.a, "ws.tree").await, [held_at_a]);
    assert_eq!(held_from_c(&t.b, "ws.tree").await, [forked]);
}

/// W3's refusals (X3.2; the plan's
/// `a_write_the_holder_refuses_is_refused_at_b_and_kept_nowhere`, its
/// letters swapped). B's provider writes a SWMR zone's first op. c, a
/// client of A that subscribes nothing, then writes as a second SWMR writer
/// there, and a `crdt` op on the tree zone, a `value` zone. B refuses both,
/// `Protocol`, and A relays each refusal to c, naming its op; neither node
/// keeps either, and A's subscriber of both zones gets neither. Red before
/// X3.2: A appended both itself and answered `Ok`; before X3.2b, the
/// refusals went unrelayed.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_the_holder_refuses_is_refused_at_a_and_kept_nowhere() {
    let t = two_nodes("x32-refused", Some(&["read.*", "write.*"])).await;
    let swmr = |origin: &str, body: &[u8]| Op {
        glade_id: "ws.swmr".into(),
        origin: origin.into(),
        shape: Shape::Swmr,
        payload: encode_swmr(SwmrAction::Snapshot, body),
        ..c_writes(0, None, b"")
    };
    let first = swmr("prov-b", b"the provider's");
    t.provider
        .1
        .send_binary(&ops_frame(vec![first]))
        .await
        .unwrap();
    let swmr_held = |st: &Store| {
        !st.scan("ws-razel", "ws.swmr", &[], "prov-b", i64::MIN)
            .is_empty()
    };
    wait_store(&t.b, swmr_held, "B to hold the SWMR zone").await;
    let mut on_a = listening(&t.a, &["ws.swmr", "ws.tree"]).await;

    let (mut rc, wc) = crate::ws::connect("127.0.0.1", t.port_a).await.unwrap();
    let second_writer = swmr("c-on-a", b"c's");
    let crdt_on_value = Op {
        shape: Shape::Crdt,
        ..c_writes(0, None, b"a crdt delta")
    };
    let ops = [second_writer, crdt_on_value];
    wc.send_binary(&ops_frame(ops.to_vec())).await.unwrap();
    let answered = statuses(&mut rc, &ops).await;
    let whys = ["SWMR writer conflict", "shape conflict"];
    for (op, why) in ops.iter().zip(whys) {
        let (code, message) = &answered[&named(op)];
        assert_eq!(*code, ErrorCode::Protocol, "{message}");
        assert!(message.contains(why), "{message}");
    }
    for node in [&t.a, &t.b] {
        for op in &ops {
            let held = held_from_c(node, &op.glade_id).await;
            assert!(held.is_empty(), "{} kept: {held:?}", op.glade_id);
        }
    }
    assert!(
        from_c(&mut on_a).is_empty(),
        "nothing of c's fanned out at A"
    );
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
        heads: SharedHeads::default(),
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

/// W6's answers (X3.2), on the same table: an answer settles the write it
/// names by its op's hash (R1), the first held, so an op sent twice takes
/// its answers in turn; an answer naming no write held settles nothing.
#[test]
fn an_answer_settles_the_first_write_it_names() {
    let write = |seq, writer| Write {
        op: c_writes(seq, None, b"answered"),
        writer,
        heads: SharedHeads::default(),
    };
    let answer = |op: &Op| match op_status(op, ErrorCode::Ok, "appended".into()) {
        Frame::Error(e) => e,
        other => panic!("{other:?}"),
    };
    let (twice, other) = (write(0, 1).op, write(1, 1).op);
    let mut pending = Pending::default();
    let sent = Instant::now();
    for held in [write(0, 1), write(1, 1), write(0, 2)] {
        pending.hold(held, sent);
    }
    let settled = |w: Option<Write>| w.map(|w| (w.op.seq, w.writer));
    assert_eq!(settled(pending.answered(&answer(&twice))), Some((0, 1)));
    assert_eq!(settled(pending.answered(&answer(&twice))), Some((0, 2)));
    assert_eq!(settled(pending.answered(&answer(&twice))), None);
    assert_eq!(settled(pending.answered(&answer(&other))), Some((1, 1)));
    assert_eq!(pending.due(), None, "nothing left");
}
