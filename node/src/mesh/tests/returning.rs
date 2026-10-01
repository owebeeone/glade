//! Forwards come back with the link (cross-node writes plan X4.2). A
//! forward that ends tells its zone's subscribers, who leave the zone (plan
//! Step 4.6, part 3). A subscriber can still be left with no forward: one
//! whose link was lost between its route and its forward's start (4.6 part
//! 3's open point). Once its link returns, its forward does. Here B holds
//! the claim and A forwards, the plan's letters swapped.

use std::time::Duration;

use glade_wire::generated::{ErrorCode, Op};
use tokio::sync::mpsc::UnboundedReceiver;

use super::crossing::{c_writes, held_from_c, listening, status_of};
use super::two_nodes::{
    a_client, forward_lapses, ops_frame, payloads, relinked, tree_op, two_nodes,
};
use crate::chain::op_hash;
use crate::frame::Frame;
use crate::mesh::release_links;
use crate::server::Shared;

/// Wait, bounded, until A holds no link to the node `peer` (hex).
async fn unlinked(a: &Shared, peer: &str) {
    let mesh = a.mesh.get().unwrap();
    for _ in 0..500 {
        if !mesh.links.lock().await.contains_key(peer) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("A still holds its link to {peer}");
}

/// The next op `rx` takes, bounded: a hang is a failure.
async fn next_op(rx: &mut UnboundedReceiver<Vec<u8>>, what: &str) -> Op {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(5), rx.recv());
        let bytes = next
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        let frame = Frame::from_bytes(&bytes.expect("the session's queue is open")).unwrap();
        if let Frame::Ops(ops) = frame {
            if let Some(op) = ops.ops.into_iter().next() {
                return op;
            }
        }
    }
}

/// X4.2: A's link to B drops (B releases its links), so A's forward of the
/// tree zone ends, and its subscriber c is told and leaves. A session of A
/// is then subscribed to the zone with no forward, registered by hand as a
/// subscribe whose link was lost before its forward started leaves one. c
/// writes while A is unlinked: `UnknownShare`, which c keeps. Once A is
/// linked to B again, the session gets B's next op without subscribing
/// again, and c's write, sent again, lands at both nodes, `Ok`. Red before
/// X4.2: nothing reopened the forward, so nothing reached the session.
#[tokio::test(flavor = "multi_thread")]
async fn a_forward_resumes_when_the_link_returns() {
    let t = two_nodes("x42-returns", Some(&["read.*", "write.*"])).await;
    let (mut rc, wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);
    release_links(&t.b).await;
    forward_lapses(&t.a).await;
    unlinked(&t.a, &t.b_id).await;
    let mut waiting = listening(&t.a, &["ws.tree"]).await;

    let kept = c_writes(0, None, b"written unlinked");
    wc.send_binary(&ops_frame(vec![kept.clone()]))
        .await
        .unwrap();
    let (code, why) = status_of(&mut rc, &kept).await;
    assert_eq!(code, ErrorCode::UnknownShare, "{why}");

    relinked(&t).await;
    let v2 = tree_op(2, Some(op_hash(&t.tree[1]).to_vec()), b"tree-v2");
    t.provider
        .1
        .send_binary(&ops_frame(vec![v2.clone()]))
        .await
        .unwrap();
    assert_eq!(next_op(&mut waiting, "B's next op").await, v2);

    wc.send_binary(&ops_frame(vec![kept.clone()]))
        .await
        .unwrap();
    let (code, why) = status_of(&mut rc, &kept).await;
    assert_eq!(code, ErrorCode::Ok, "{why}");
    for node in [&t.a, &t.b] {
        let held = held_from_c(node, "ws.tree").await;
        assert_eq!(held, std::slice::from_ref(&kept), "once");
    }
}
