//! W2's grant (cross-node writes plan X4.1, question 6): a forwarded write
//! needs `write.append` at the claim holder, on the forwarding node's id,
//! through 4.3's `GrantPort` adapter. Refused, it is answered
//! `Unauthorized`, naming the node id its HELLO claimed, and kept nowhere;
//! a forward the holder refuses refuses the writes it carries. Here B holds
//! the claim and A forwards, the plan's letters swapped. The session's half,
//! behind 4.3's switch, is in `server.rs`.

use glade_wire::generated::{ErrorCode, Op};

use super::crossing::{c_writes, from_c, held_from_c, listening, status_of};
use super::two_nodes::{a_client, admitted, forward_lapses, ops_frame, payloads, two_nodes};
use crate::claims::testing;
use crate::registry::Record;
use crate::sysdata::CapabilityRevocation;
use crate::ws::{WsReader, WsWriter};

/// c, a new client of A, writes `op` alone on its session.
async fn c_on_a(port_a: u16, op: &Op) -> (WsReader, WsWriter) {
    let (r, w) = crate::ws::connect("127.0.0.1", port_a).await.unwrap();
    w.send_binary(&ops_frame(vec![op.clone()])).await.unwrap();
    (r, w)
}

/// X4.1: B's fold grants A's node id `read.*` on `ws-razel` and no write
/// verb, so B refuses c's write, which A forwards, per op: `Unauthorized`,
/// naming the node id A's HELLO claimed, relayed to c naming its op.
/// Neither node keeps it, and B's subscriber of the zone gets nothing. Red
/// before X4.1: B took the write, `Ok`.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwarded_write_without_a_grant_is_refused_by_its_claimed_node_id() {
    let t = two_nodes("x41-ungranted", Some(&["read.*"])).await;
    let mut on_b = listening(&t.b, &["ws.tree"]).await;

    let op = c_writes(0, None, b"ungranted");
    let (mut rc, _wc) = c_on_a(t.port_a, &op).await;
    let a = &t.a_id;
    let why = format!("unauthorized: node {a} holds no grant of write.append on ws-razel");
    assert_eq!(
        status_of(&mut rc, &op).await,
        (ErrorCode::Unauthorized, why)
    );
    for node in [&t.a, &t.b] {
        let held = held_from_c(node, "ws.tree").await;
        assert!(held.is_empty(), "kept: {held:?}");
    }
    assert!(from_c(&mut on_b).is_empty(), "fanned out at B");
}

/// Its twin: B's fold grants A's node id exactly `read.subscribe` and
/// `write.append` on `ws-razel`, so c's write, which A forwards under that
/// claimed node id, is taken: `Ok`, and each node holds it once.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwarded_write_granted_by_its_claimed_node_id_lands() {
    let verbs = ["read.subscribe", "write.append"];
    let t = two_nodes("x41-granted", Some(&verbs)).await;

    let op = c_writes(0, None, b"granted");
    let (mut rc, _wc) = c_on_a(t.port_a, &op).await;
    let (code, why) = status_of(&mut rc, &op).await;
    assert_eq!(code, ErrorCode::Ok, "{why}");
    for node in [&t.a, &t.b] {
        let held = held_from_c(node, "ws.tree").await;
        assert_eq!(held, std::slice::from_ref(&op), "once");
    }
}

/// A revocation of A's node id on `ws-razel`, accepted at B through its
/// directory authority (4.3's `revoke` line; its re-check pass), ends A's
/// forward, and A's writes with it. c's write before it lands. Once A's
/// forward has lapsed, c's next write opens a forward that B refuses, so it
/// is answered `Unauthorized`, B's reason prefixed with who refused, and
/// neither node keeps it. Red before X4.1: the refused forward's write was
/// answered `UnknownShare`, not placed, which c would keep and send again.
#[tokio::test(flavor = "multi_thread")]
async fn a_revocation_ends_a_forwarded_write_stream() {
    let t = two_nodes("x41-revoke", Some(&["read.*", "write.*"])).await;
    let (mut rc, wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);
    let before = c_writes(0, None, b"before");
    wc.send_binary(&ops_frame(vec![before.clone()]))
        .await
        .unwrap();
    let (code, why) = status_of(&mut rc, &before).await;
    assert_eq!(code, ErrorCode::Ok, "{why}");

    let revocation = CapabilityRevocation {
        principal: t.a_id.clone(),
        share: "ws-razel".into(),
    };
    let revoke = vec![Record::Revoke(revocation)];
    testing::accept(&t.b, revoke).await.unwrap();
    assert_eq!(
        admitted(&t.b).await,
        Vec::<String>::new(),
        "the stream ended"
    );
    forward_lapses(&t.a).await;

    let after = c_writes(1, Some(&before), b"after");
    wc.send_binary(&ops_frame(vec![after.clone()]))
        .await
        .unwrap();
    let (a, b) = (&t.a_id, &t.b_id);
    let why = format!(
        "refused by node {b}, which serves ws-razel: unauthorized: node {a}'s grants on \
         ws-razel are revoked"
    );
    assert_eq!(
        status_of(&mut rc, &after).await,
        (ErrorCode::Unauthorized, why)
    );
    for node in [&t.a, &t.b] {
        let held = held_from_c(node, "ws.tree").await;
        assert_eq!(held, std::slice::from_ref(&before), "the write before only");
    }
}
