use super::support::wait_store;
use super::two_nodes::{
    a_client, admitted, forward_lapses, next_frame, ops_frame, payloads, refused_on_the_link,
    relinked, sub, subscribed_on_the_link, tree_len, tree_op, tree_payloads, tree_routed,
    tree_unrouted, tree_zone, two_nodes, two_nodes_limited, TwoNodes,
};
use crate::chain::op_hash;
use crate::claims::testing;
use crate::conversation::Conversation;
use crate::envelope;
use crate::frame::Frame;
use crate::mesh::{forward_interest, release_links};
use crate::registry::{Record, HOME};
use crate::store::Store;
use crate::sysdata::{CapabilityGrant, CapabilityRevocation};
use glade_wire::cbor;
use glade_wire::generated::{ErrorCode, Head, Heads, Op, Ops, StreamHeads};
use std::time::Duration;

// ---- the s-discovery golden path, end to end ---------------------------

/// The 30-step s-discovery trace's slice for this step, E2E over real iroh
/// + real websockets: (a) phase A — a client on node A lists
/// `home/dir.workspaces` from A's LOCAL replica and sees the workspace B
/// registered; (b) phase C — subscribing that workspace's share routes the
/// interest via the folded ServeClaim to B, the ops arrive, converge into
/// A's replica, and keep flowing live; (c) phase E — a share whose only
/// claim is lapsed at the reader's clock answers with an ack that names no
/// zone, then STATUS data, bounded, and the session stays usable. B serves
/// A in phase (b) because its fold grants A's node id `read.*` on the
/// share (plan Step 4.3); without it, B refuses
/// (`a_peer_without_a_grant_is_refused_by_its_claimed_node_id`).
#[tokio::test(flavor = "multi_thread")]
async fn s_discovery_golden_path_end_to_end() {
    let t = two_nodes("e2e", Some(&["read.*"])).await;
    let (a_shared, b_id, port_a) = (t.a.clone(), t.b_id.clone(), t.port_a);
    let wp = &t.provider.1;
    let o1 = t.tree[1].clone();

    // ---- (a) phase A: list the directory from A's LOCAL replica ---------
    let (mut rc, wc) = crate::ws::connect("127.0.0.1", port_a).await.unwrap();
    wc.send_binary(&sub(HOME, crate::registry::G_WORKSPACES)).await.unwrap();
    assert!(matches!(next_frame(&mut rc, "dir.workspaces ack").await, Frame::Heads(_)));
    let mut names = Vec::new();
    while names.len() < 2 {
        if let Frame::Ops(ops) = next_frame(&mut rc, "workspace entries").await {
            for op in ops.ops {
                assert_eq!(op.origin, b_id, "entries carry their writing origin");
                let entry = envelope::record(&op, crate::sysdata::WorkspaceEntry::from_cbor);
                names.push(entry.unwrap().workspace);
            }
        }
    }
    names.sort();
    assert_eq!(names, vec!["ws-attic".to_string(), "ws-razel".to_string()], "the list from the local replica");

    // ---- (b) phase C: the claim routes the workspace share to B ---------
    wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
    assert!(matches!(next_frame(&mut rc, "ws.tree ack").await, Frame::Heads(_)));
    let mut payloads = Vec::new();
    while payloads.len() < 2 {
        if let Frame::Ops(ops) = next_frame(&mut rc, "routed tree ops").await {
            payloads.extend(ops.ops.into_iter().map(|o| o.payload));
        }
    }
    assert_eq!(payloads, vec![b"tree-v0".to_vec(), b"tree-v1".to_vec()], "the routed gap converges in order");
    // ...and INTO A's replica — the replica served the read (C5).
    wait_store(&a_shared, |st| st.scan("ws-razel", "ws.tree", &[], "prov-b", i64::MIN).len() == 2, "A's replica to hold the routed zone").await;

    // live: the provider writes v2 on B; it reaches the A-side client with
    // no re-request (the C5→C6 stream keeps flowing).
    let o2 = tree_op(2, Some(crate::chain::op_hash(&o1).to_vec()), b"tree-v2");
    wp.send_binary(&Frame::Ops(Ops { ops: vec![o2], pri: None }).to_bytes()).await.unwrap();
    loop {
        if let Frame::Ops(ops) = next_frame(&mut rc, "live tree op").await {
            if ops.ops.iter().any(|o| o.payload == b"tree-v2") {
                break;
            }
        }
    }

    // ---- (c) phase E: no live claim -> STATUS data, bounded -------------
    // R6 (client-writes plan Step 2.2): a refused subscribe gets an ack
    // that names no zone, then the reason, so a client waiting on its ack
    // resolves instead of hanging (the plan's F3).
    wc.send_binary(&sub("ws-attic", "ws.tree")).await.unwrap();
    match next_frame(&mut rc, "ws-attic's refusal ack").await {
        Frame::Heads(h) => assert!(
            h.streams.is_empty(),
            "the refusal's ack names no zone: {h:?}"
        ),
        other => panic!("expected an ack that names no zone, got {other:?}"),
    }
    match next_frame(&mut rc, "ws-attic status").await {
        Frame::Error(e) => {
            assert_eq!(e.code, glade_wire::generated::ErrorCode::UnknownShare);
            assert_eq!(e.share.as_deref(), Some("ws-attic"));
            assert_eq!(e.glade_id.as_deref(), Some("ws.tree"));
            assert_eq!(e.corr, None, "a refused subscribe names no op");
            assert!(e.message.contains("no live ServeClaim"), "the reason rides the status: {}", e.message);
        }
        other => panic!("expected STATUS (Error frame), got {other:?}"),
    }
    // absence is data, not a dead session: the next ask still answers,
    // with an ack that names its zone.
    wc.send_binary(&sub(HOME, crate::registry::G_CLAIMS)).await.unwrap();
    match next_frame(&mut rc, "post-absence ack").await {
        Frame::Heads(h) => {
            assert_eq!(h.streams.len(), 1, "an accepted ack names its zone: {h:?}")
        }
        other => panic!("expected the post-absence ack, got {other:?}"),
    }
}

// ---- the grant check at the serve hop (plan Step 4.3) ------------------

/// The golden path's phase (b), turned round: B's fold grants A's node id
/// nothing on `ws-razel`, so B refuses A's forwarded subscribe by the node
/// id A's HELLO claimed, and proved: the refused subscribe's two frames,
/// then the stream is finished. A's client is acked from A's replica, A's
/// forward lapses, and nothing of the zone reaches A.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_without_a_grant_is_refused_by_its_claimed_node_id() {
    let t = two_nodes("refused", None).await;
    let a = &t.a_id;
    let why = format!("unauthorized: node {a} holds no grant of read.subscribe on ws-razel");
    assert_eq!(refused_on_the_link(&t).await.message, why);

    let _client = a_client(&t).await;
    let (share, glade_id, key) = tree_zone();
    forward_interest(&t.a, t.b_id.clone(), share, glade_id, key).await;
    forward_lapses(&t.a).await;
    assert_eq!(tree_payloads(&t.a).await, Vec::<Vec<u8>>::new());
    assert!(!tree_routed(&t.b).await, "B routes the zone to no one");
    assert_eq!(admitted(&t.b).await, Vec::<String>::new());
}

/// Its twin: B's fold grants A's node id exactly `read.subscribe` on
/// `ws-razel`, so A's forwarded subscribe, by that claimed node id, is
/// admitted and served, and B's admission table holds the stream.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_granted_by_its_claimed_node_id_is_served() {
    let t = two_nodes("granted", Some(&["read.subscribe"])).await;
    let (mut rc, _wc) = a_client(&t).await;
    let got = payloads(&mut rc, 2, "routed tree ops").await;
    assert_eq!(got, [b"tree-v0".to_vec(), b"tree-v1".to_vec()]);
    assert_eq!(admitted(&t.b).await, [t.a_id.as_str()]);
}

/// A revocation accepted while A's forwarded stream is live, through B's
/// directory authority as a runtime route would take it, ends the stream
/// before the accepting call returns: the fold's generation advances, the
/// stream leaves B's router and admission table, and A's forward lapses.
/// An op written after the revocation stays at B, and a new subscribe by
/// A's claimed node id is refused as revoked. What A got before stays.
#[tokio::test(flavor = "multi_thread")]
async fn a_revocation_ends_a_forwarded_stream_of_a_claimed_node_id() {
    let t = two_nodes("revoke", Some(&["read.*"])).await;
    let (mut rc, _wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);
    assert_eq!(admitted(&t.b).await, [t.a_id.as_str()]);

    let before = t.b.policy.generation();
    let revocation = CapabilityRevocation {
        principal: t.a_id.clone(),
        share: "ws-razel".into(),
    };
    let revoke = vec![Record::Revoke(revocation)];
    let generation = testing::accept(&t.b, revoke).await.unwrap();
    assert_eq!(generation, before + 1);
    assert_eq!(
        admitted(&t.b).await,
        Vec::<String>::new(),
        "the pass ended the stream"
    );
    assert!(!tree_routed(&t.b).await, "B routes the zone to no one");
    forward_lapses(&t.a).await;

    let prev = crate::chain::op_hash(&t.tree[1]).to_vec();
    let written = ops_frame(vec![tree_op(2, Some(prev), b"tree-v2")]);
    t.provider.1.send_binary(&written).await.unwrap();
    wait_store(&t.b, |st| tree_len(st) == 3, "B to hold v2").await;
    let at_a = tree_payloads(&t.a).await;
    assert_eq!(
        at_a,
        [b"tree-v0".to_vec(), b"tree-v1".to_vec()],
        "v2 stayed at B"
    );

    let why = format!(
        "unauthorized: node {}'s grants on ws-razel are revoked",
        t.a_id
    );
    assert_eq!(refused_on_the_link(&t).await.message, why);
}

/// The fail direction: once B's fold cannot be read, the live forwarded
/// stream of A's claimed node id ends and a new subscribe is refused, as
/// for no grant. The fold is made unreadable as a quarantined grant or
/// revocation leaves it at boot (`Registry::policy`).
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_fold_fails_closed() {
    let t = two_nodes("stale", Some(&["read.*"])).await;
    let (mut rc, _wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);

    crate::server::refresh_policy(&t.b, None).await;
    assert_eq!(
        admitted(&t.b).await,
        Vec::<String>::new(),
        "the pass ended the stream"
    );
    assert!(!tree_routed(&t.b).await, "B routes the zone to no one");
    forward_lapses(&t.a).await;
    let a = &t.a_id;
    let why = format!(
        "unauthorized: the grant fold is unavailable, so node {a} may not read.subscribe on ws-razel"
    );
    assert_eq!(refused_on_the_link(&t).await.message, why);
}

// ---- the claim holder's refusal, relayed (F5) --------------------------

/// The next frame a client of A reads, which must be a lone refusal that
/// names the tree zone and no op: its code and its reason.
async fn told(r: &mut crate::ws::WsReader) -> (ErrorCode, String) {
    match next_frame(r, "the relayed refusal").await {
        Frame::Error(e) => {
            let named = (e.share.as_deref(), e.glade_id.as_deref(), e.corr.as_deref());
            assert_eq!(named, (Some("ws-razel"), Some("ws.tree"), None));
            (e.code, e.message)
        }
        other => panic!("expected the relayed refusal, got {other:?}"),
    }
}

/// The next frame on `r` after a subscribe to a zone no directory knows,
/// which A serves locally: its ack, unless something came before it.
async fn bound(r: &mut crate::ws::WsReader, w: &crate::ws::WsWriter) -> Frame {
    w.send_binary(&sub("plain", "bound")).await.unwrap();
    next_frame(r, "the bound's ack").await
}

/// F5 (question 25; the owner's ruling of 2026-09-27): B grants A's node
/// id nothing on `ws-razel`, so it refuses A's forwarded subscribe, and
/// the refusal now reaches A's own subscriber of the zone. It is acked
/// from A's replica, as before, then told with a lone `Error`, B's code
/// and B's reason prefixed with who refused, and leaves A's router for
/// the zone. A client that subscribes later forwards the interest again,
/// is refused again and is told the same, and the first is not told
/// twice. Nothing re-checks: once B grants A, a new subscribe is served,
/// and the refused client, which has not subscribed again, gets nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_holders_refusal_reaches_the_forwarding_nodes_subscribers() {
    let t = two_nodes("relayed", None).await;
    let (a, b) = (&t.a_id, &t.b_id);
    let why = format!(
        "refused by node {b}, which serves ws-razel: unauthorized: node {a} holds no grant of \
         read.subscribe on ws-razel"
    );
    let refusal = (ErrorCode::Unauthorized, why);
    let (mut first, first_w) = a_client(&t).await;
    assert_eq!(told(&mut first).await, refusal);
    forward_lapses(&t.a).await;
    assert!(!tree_routed(&t.a).await, "A routes the zone to no one");

    let (mut later, _later_w) = a_client(&t).await;
    assert_eq!(told(&mut later).await, refusal);
    let next = bound(&mut first, &first_w).await;
    assert!(matches!(next, Frame::Heads(_)), "told twice: {next:?}");
    forward_lapses(&t.a).await;

    let grant = CapabilityGrant {
        principal: t.a_id.clone(),
        share: "ws-razel".into(),
        verbs: vec!["read.subscribe".into()],
    };
    let granted = testing::accept(&t.b, vec![Record::Grant(grant)]).await;
    granted.unwrap();
    let (mut again, _again_w) = a_client(&t).await;
    let got = payloads(&mut again, 2, "routed tree ops").await;
    assert_eq!(got, [b"tree-v0".to_vec(), b"tree-v1".to_vec()]);
    let next = bound(&mut first, &first_w).await;
    assert!(matches!(next, Frame::Heads(_)), "served unasked: {next:?}");
}

/// F5, mid-stream: B's re-check pass ends A's admitted forward when a
/// revocation lands, and its lone refusal reaches A's subscriber, who
/// has had the zone's ops; the subscriber leaves A's router for the zone.
#[tokio::test(flavor = "multi_thread")]
async fn a_revocation_on_the_claim_holder_reaches_the_forwarding_nodes_subscribers() {
    let t = two_nodes("relayed-revocation", Some(&["read.*"])).await;
    let (mut rc, _wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);

    let revocation = CapabilityRevocation {
        principal: t.a_id.clone(),
        share: "ws-razel".into(),
    };
    testing::accept(&t.b, vec![Record::Revoke(revocation)])
        .await
        .unwrap();
    let (a, b) = (&t.a_id, &t.b_id);
    let why = format!(
        "refused by node {b}, which serves ws-razel: unauthorized: node {a}'s grants on \
         ws-razel are revoked"
    );
    assert_eq!(told(&mut rc).await, (ErrorCode::Unauthorized, why));
    forward_lapses(&t.a).await;
    assert!(!tree_routed(&t.a).await, "A routes the zone to no one");
}

/// Plan Step 4.5b (question 6): with links whose frames hold at most
/// 4 KiB, B serves A's forward of a zone whose gap is over five times
/// that. It crosses in chunks, each under the limit, and reaches A whole
/// and in order; in one frame it would be refused, and the forward would
/// lapse.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwarded_gap_crosses_in_chunks_under_the_frame_limit() {
    const LIMIT: usize = 4 << 10;
    let t = two_nodes_limited("chunked", Some(&["read.*"]), LIMIT).await;
    let (mut prev, mut written) = (crate::chain::op_hash(&t.tree[1]).to_vec(), Vec::new());
    for seq in 2..22 {
        let op = tree_op(seq, Some(prev), &[seq as u8; 1 << 10]);
        prev = crate::chain::op_hash(&op).to_vec();
        written.push(op);
    }
    {
        let mut store = t.b.store.lock().await;
        for op in &written {
            store.append(op.clone()).unwrap();
        }
    }
    let ops = || t.tree.iter().chain(&written);
    let gap: usize = ops().map(|op| cbor::encode(&op.to_cbor()).len()).sum();
    assert!(gap > 5 * LIMIT, "a gap of {gap} bytes");
    let _client = a_client(&t).await;
    wait_store(&t.a, |st| tree_len(st) == 22, "the whole gap at A").await;
    let sent: Vec<Vec<u8>> = ops().map(|op| op.payload.clone()).collect();
    assert_eq!(tree_payloads(&t.a).await, sent);
}

// ---- a forward's end, told (plan Step 4.6, part 3) ---------------------

/// Plan Step 4.6, part 3 (question 5, ruled 2026-09-30): B, the claim
/// holder, releases its links, so A's forward of the zone ends with no
/// refusal. A's subscriber, which has had the zone's ops, is told once, with
/// a lone `Error`, `UnknownShare` (an absent route's code), that the forward
/// from B ended, and leaves A's router for the zone.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwards_end_without_a_refusal_reaches_the_forwarding_nodes_subscribers() {
    let t = two_nodes("ended", Some(&["read.*"])).await;
    let (mut rc, wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);

    assert_eq!(release_links(&t.b).await, 1);
    let why = format!("forward from node {} ended", t.b_id);
    assert_eq!(told(&mut rc).await, (ErrorCode::UnknownShare, why));
    assert!(!tree_routed(&t.a).await, "A routes the zone to no one");
    let next = bound(&mut rc, &wc).await;
    assert!(matches!(next, Frame::Heads(_)), "told twice: {next:?}");
}

/// A forward's end is told to the zone's subscribers at that end alone:
/// once the last of them has left, B's release ends the forward and no one
/// is told, nor is a client of A that holds another zone.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwards_end_tells_no_one_once_its_last_subscriber_has_left() {
    let t = two_nodes("ended-unheard", Some(&["read.*"])).await;
    let (mut other, other_w) = crate::ws::connect("127.0.0.1", t.port_a).await.unwrap();
    assert!(matches!(bound(&mut other, &other_w).await, Frame::Heads(_)));
    let (mut rc, wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);
    drop((rc, wc));
    tree_unrouted(&t.a).await;

    release_links(&t.b).await;
    forward_lapses(&t.a).await;
    let next = bound(&mut other, &other_w).await;
    assert!(matches!(next, Frame::Heads(_)), "told: {next:?}");
}

/// After the end, A's subscriber subscribes again, once A is linked to B
/// again, and the subscribe forwards afresh: the zone from A's replica,
/// then B's next op, which only a new forward carries.
#[tokio::test(flavor = "multi_thread")]
async fn a_subscribe_after_a_forwards_end_forwards_again_once_linked() {
    let t = two_nodes("ended-again", Some(&["read.*"])).await;
    let (mut rc, wc) = a_client(&t).await;
    assert_eq!(payloads(&mut rc, 2, "routed tree ops").await.len(), 2);
    release_links(&t.b).await;
    forward_lapses(&t.a).await;

    relinked(&t).await;
    wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
    let replica = payloads(&mut rc, 2, "the tree from A's replica").await;
    assert_eq!(replica, [b"tree-v0".to_vec(), b"tree-v1".to_vec()]);
    let prev = crate::chain::op_hash(&t.tree[1]).to_vec();
    let written = ops_frame(vec![tree_op(2, Some(prev), b"tree-v2")]);
    t.provider.1.send_binary(&written).await.unwrap();
    let fed = payloads(&mut rc, 1, "v2 through a new forward").await;
    assert_eq!(fed, [b"tree-v2".to_vec()]);
}

// ---- the peer ack is a cut (cross-node writes plan X2.2) ---------------

/// The next frame B sends on `conversation`, bounded: a hang is a failure.
async fn from_b(conversation: &mut Conversation, what: &str) -> Frame {
    let next = tokio::time::timeout(Duration::from_secs(5), conversation.recv());
    let Ok(read) = next.await else {
        panic!("timed out waiting for {what}");
    };
    read.unwrap_or_else(|e| panic!("reading {what}: {e}"))
}

/// B's provider writes `op`, a client op on B.
async fn provider_writes(t: &TwoNodes, op: &Op) {
    let frame = ops_frame(vec![op.clone()]);
    t.provider.1.send_binary(&frame).await.unwrap();
}

/// R4 on the peer path (X2.2): no op of a zone reaches a forwarding node
/// before the ack of its subscribe, and each op the claim holder holds
/// reaches it once after the ack. B's store lock is held while a client op
/// on B waits for it, and A's subscribe reaches B behind that op; then the
/// lock is let go. The ack comes first, the op in the gap after it, and B's
/// next op follows the gap: the op is not sent again. Green from the start:
/// slice 4.3 part 2 put the stream's registration under the cut, which every
/// fan-out holds from its append until its ops are queued. With the stream
/// registered first, as before it, the op reached A ahead of the ack. As in
/// the client's test (`server.rs`), the pauses only give each frame time to
/// reach its lock, and any order of the two gives the same answer.
#[tokio::test(flavor = "multi_thread")]
async fn no_op_of_a_zone_reaches_a_forwarding_node_before_its_ack() {
    let t = two_nodes("peer-cut", Some(&["read.subscribe"])).await;
    let pause = || tokio::time::sleep(Duration::from_millis(50));
    let o2 = tree_op(2, Some(op_hash(&t.tree[1]).to_vec()), b"tree-v2");
    let o3 = tree_op(3, Some(op_hash(&o2).to_vec()), b"tree-v3");

    let store = t.b.store.lock().await;
    provider_writes(&t, &o2).await;
    pause().await; // the op takes the cut, then waits for the store
    let mut conversation = subscribed_on_the_link(&t).await;
    pause().await; // the subscribe reaches B and waits for a lock
    drop(store);

    let first = from_b(&mut conversation, "B's first frame").await;
    assert!(
        matches!(&first, Frame::Heads(h) if h.streams.len() == 1),
        "an op reached A before its ack: {first:?}"
    );
    provider_writes(&t, &o3).await;
    let mut ops = Vec::new();
    while ops.last() != Some(&o3) {
        match from_b(&mut conversation, "the zone's ops").await {
            Frame::Ops(o) => ops.extend(o.ops),
            other => panic!("expected the zone's ops, got {other:?}"),
        }
    }
    let once = [t.tree[0].clone(), t.tree[1].clone(), o2, o3];
    assert_eq!(ops, once, "each op once, in order, after the ack");
}

/// R5 on the peer path (X2.2): B's ack of A's forwarded subscribe names the
/// zone and each origin's head there by seq and hash, the 32 bytes of the op
/// at that seq, as a client's ack does (client-writes plan Step 2.2). Red
/// before X2.2: the peer ack named each head with no hash.
#[tokio::test(flavor = "multi_thread")]
async fn the_peer_ack_names_each_origin_head_with_its_hash() {
    let t = two_nodes("peer-ack-hashes", Some(&["read.subscribe"])).await;
    let other = Op {
        origin: "prov-c".into(),
        ..tree_op(0, None, b"c zero")
    };
    provider_writes(&t, &other).await;
    let both = |st: &Store| st.heads("ws-razel", "ws.tree", &[]).len() == 2;
    wait_store(&t.b, both, "B to hold the zone's two origins").await;

    let mut conversation = subscribed_on_the_link(&t).await;
    let head = |op: &Op| Head {
        origin: op.origin.clone(),
        seq: op.seq,
        hash: Some(op_hash(op).to_vec()),
    };
    let (share, glade_id, key) = tree_zone();
    let heads = vec![head(&t.tree[1]), head(&other)];
    let zone = StreamHeads {
        share,
        glade_id,
        key,
        heads,
    };
    let ack = Frame::Heads(Heads {
        streams: vec![zone],
    });
    assert_eq!(from_b(&mut conversation, "B's ack").await, ack);
}
