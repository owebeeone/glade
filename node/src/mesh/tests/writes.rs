//! W1 at one node (cross-node writes plan X2.3): a client's op is placed by
//! the route its subscribe would get. The node's mesh is enabled and linked
//! to no one, and its directory knows `ws-attic` only by a lapsed claim, so
//! that share routes `Absent`, while a share no record names routes `Local`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use glade_wire::generated::{ErrorCode, Op, Ops, Shape};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use super::support::{fresh, signed, A_SEED, C_SEED};
use super::two_nodes::{next_frame, ops_frame};
use crate::chain::op_hash;
use crate::frame::Frame;
use crate::mesh::testing::meshed;
use crate::peer::NodeIdentity;
use crate::registry::Record;
use crate::server::{Server, Shared};
use crate::sysdata::ServeClaim;
use crate::sysdir::now_ms;

/// One node, its mesh enabled and linked to no one, whose directory knows
/// `ws-attic` only by a claim that lapsed a second ago, as the two-node
/// tests seed it; it serves clients on the port it returns.
async fn attic_node(name: &str) -> (Arc<Shared>, u16) {
    let server = Server::open(fresh(name)).unwrap();
    let lapsed = ServeClaim {
        node: "attic-mini".into(),
        share: "ws-attic".into(),
        lease_expiry_ms: now_ms() - 1_000,
        epoch: 1,
    };
    let claim = signed(C_SEED, Record::Serve(lapsed));
    server.shared.store.lock().await.append(claim).unwrap();
    meshed(&server, NodeIdentity::from_key(A_SEED)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let node = server.shared.clone();
    tokio::spawn(server.run(listener));
    (node, port)
}

/// A session of the node subscribed to `share`'s `notes`, registered by
/// hand, as one that subscribed while the share was served: its queue.
async fn subscribed(node: &Arc<Shared>, share: &str) -> UnboundedReceiver<Vec<u8>> {
    let (tx, rx) = mpsc::unbounded_channel();
    let sid = node.next.fetch_add(1, Ordering::SeqCst);
    node.out.lock().await.insert(sid, tx);
    node.router.lock().await.subscribe(sid, share, "notes", &[]);
    rx
}

/// A client's first op on `share`'s `notes`.
fn note(share: &str) -> Op {
    Op {
        share: share.into(),
        glade_id: "notes".into(),
        key: vec![],
        origin: "w".into(),
        seq: 0,
        prev: None,
        lamport: 0,
        refs: vec![],
        shape: Shape::Value,
        payload: b"a note".to_vec(),
    }
}

/// A client of the node at `port` writes `op`, and reads its status (R1),
/// which names the op's zone and the op by its hash: the code and the
/// message.
async fn written(port: u16, op: &Op) -> (ErrorCode, String) {
    let (mut r, w) = crate::ws::connect("127.0.0.1", port).await.unwrap();
    w.send_binary(&ops_frame(vec![op.clone()])).await.unwrap();
    match next_frame(&mut r, "the op's status").await {
        Frame::Error(e) => {
            let hash: String = op_hash(op).iter().map(|b| format!("{b:02x}")).collect();
            let zone = (Some(op.share.as_str()), Some(op.glade_id.as_str()));
            assert_eq!((e.share.as_deref(), e.glade_id.as_deref()), zone);
            assert_eq!(e.corr, Some(hash), "the status names the op by its hash");
            (e.code, e.message)
        }
        other => panic!("expected the op's status, got {other:?}"),
    }
}

/// W1, `Absent` (X2.3): the directory knows `ws-attic`, by a lapsed claim,
/// and no live claim names a holder, so a client's write there is not
/// placed. Its status is `UnknownShare` with the route's reason, naming the
/// op by its hash, and the node keeps nothing: no op of the zone is stored,
/// and its subscriber gets none. Red before X2.3: the op was appended,
/// fanned out and answered `Ok`.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_to_a_share_with_no_live_claim_is_not_placed() {
    let (node, port) = attic_node("writes-absent").await;
    let mut subscriber = subscribed(&node, "ws-attic").await;

    let why = "no live ServeClaim for ws-attic".to_string();
    let status = written(port, &note("ws-attic")).await;
    assert_eq!(status, (ErrorCode::UnknownShare, why));
    let held = node.store.lock().await.heads("ws-attic", "notes", &[]);
    assert!(held.is_empty(), "nothing stored: {held:?}");
    assert!(subscriber.try_recv().is_err(), "nothing fanned out");
}

/// The guard (X2.3): a share no directory record names is no directory
/// concern, so its route is `Local`, and a client's write there lands as it
/// did: `Ok`, stored, and fanned out to the zone's subscriber.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_to_a_share_the_directory_never_heard_of_still_lands() {
    let (node, port) = attic_node("writes-unheard").await;
    let mut subscriber = subscribed(&node, "plain").await;

    let op = note("plain");
    assert_eq!(written(port, &op).await.0, ErrorCode::Ok);
    let store = node.store.lock().await;
    let held = store.scan("plain", "notes", &[], "w", i64::MIN);
    drop(store);
    assert_eq!(held, std::slice::from_ref(&op), "stored");
    let fanned = Frame::from_bytes(&subscriber.try_recv().unwrap()).unwrap();
    let once = Frame::Ops(Ops {
        ops: vec![op],
        pri: None,
    });
    assert_eq!(fanned, once, "fanned out to the zone's subscriber");
}
