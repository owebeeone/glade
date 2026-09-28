use super::support::{fresh, wait_store};
use crate::frame::Frame;
use crate::mesh::hex_id;
use crate::mesh::testing::{endpoint_key, on_carrier};
use crate::registry::{Record, RegistryApi};
use crate::server::{Server, Shared};
use crate::store::Store;
use crate::sysdata::{CapabilityGrant, ServeClaim, WorkspaceEntry};
use crate::sysdir::{boot_at, now_ms};
use glade_wire::generated::{ErrorCode, Op, Ops, Subscribe};
use std::io;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn sub(share: &str, glade_id: &str) -> Vec<u8> {
    Frame::Subscribe(Subscribe { share: share.into(), glade_id: glade_id.into(), key: None, from: None })
        .to_bytes()
}

pub(super) fn tree_op(seq: i64, prev: Option<Vec<u8>>, payload: &[u8]) -> Op {
    Op {
        share: "ws-razel".into(),
        glade_id: "ws.tree".into(),
        key: vec![],
        origin: "prov-b".into(),
        seq,
        prev,
        lamport: seq,
        refs: vec![],
        shape: glade_wire::generated::Shape::Value,
        payload: payload.to_vec(),
    }
}

/// Read the next frame from a ws client, bounded — a hang is a failure
/// (the trace's rule: failure surfaces as data, never as silence).
pub(super) async fn next_frame(r: &mut crate::ws::WsReader, what: &str) -> Frame {
    let msg = tokio::time::timeout(Duration::from_secs(5), r.read())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap();
    match msg {
        crate::ws::Msg::Binary(b) => Frame::from_bytes(&b).unwrap(),
        _ => panic!("unexpected close waiting for {what}"),
    }
}

/// Two booted nodes for the grant check's journeys (plan Step 4.3), over
/// real iroh and websockets. B is adopted, so it checks its own fold: it
/// registers `ws-razel` with a live claim and `ws-attic` with a lapsed one,
/// and grants A's node id `grant` on `ws-razel`, when given one. A is
/// seeded and dials B. B's provider session has written two tree ops.
pub(super) struct TwoNodes {
    pub(super) a: Arc<Shared>,
    pub(super) b: Arc<Shared>,
    pub(super) a_id: String,
    pub(super) b_id: String,
    pub(super) port_a: u16,
    /// B's provider session, which writes the workspace content, and the
    /// two ops it wrote.
    pub(super) provider: (crate::ws::WsReader, crate::ws::WsWriter),
    pub(super) tree: [Op; 2],
}

pub(super) async fn two_nodes(name: &str, grant: Option<&[&str]>) -> TwoNodes {
    two_nodes_limited(name, grant, crate::frame::MAX_FRAME_BYTES).await
}

/// [`two_nodes`], each linked with frames of at most `max` bytes.
pub(super) async fn two_nodes_limited(name: &str, grant: Option<&[&str]>, max: usize) -> TwoNodes {
    let boot_a = boot_at(fresh(&format!("{name}-a-sys")), "gianni").unwrap();
    let mut boot_b = boot_at(fresh(&format!("{name}-b-sys")), "gianni").unwrap();
    let (a_id, b_id) = (boot_a.node_id.clone(), boot_b.node_id.clone());
    let workspace = |share: &str, name: &str, host: &str| {
        let eligible_hosts = vec![host.to_string()];
        Record::Workspace(WorkspaceEntry {
            workspace: share.into(),
            name: name.into(),
            eligible_hosts,
        })
    };
    let claim = |node: &str, share: &str, lease_expiry_ms: i64| {
        Record::Serve(ServeClaim {
            node: node.into(),
            share: share.into(),
            lease_expiry_ms,
            epoch: 1,
        })
    };
    let mut records = vec![
        workspace("ws-razel", "razel", &b_id),
        claim(&b_id, "ws-razel", now_ms() + 30_000),
        workspace("ws-attic", "attic", "attic-mini"),
        claim("attic-mini", "ws-attic", now_ms() - 1_000),
    ];
    if let Some(verbs) = grant {
        let verbs = verbs.iter().map(|verb| verb.to_string()).collect();
        let share = "ws-razel".into();
        records.push(Record::Grant(CapabilityGrant {
            principal: a_id.clone(),
            share,
            verbs,
        }));
    }
    for record in records {
        boot_b.registry.append(record, &b_id).unwrap();
    }
    let (id_a, id_b) = (boot_a.identity().unwrap(), boot_b.identity().unwrap());
    let a = Server::open(fresh(&format!("{name}-a-store"))).unwrap();
    let b = Server::open(fresh(&format!("{name}-b-store"))).unwrap();
    a.seed_registry(&boot_a.registry.snapshot()).await;
    b.adopt_boot(boot_b).await.unwrap();

    on_carrier(&a, id_a, endpoint_key(), None, max).await;
    let at_b = on_carrier(&b, id_b, endpoint_key(), None, max).await;
    a.connect_peer(at_b).await.unwrap();

    let (a_shared, b_shared) = (a.shared.clone(), b.shared.clone());
    let lis_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let lis_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (port_a, port_b) = (lis_a.local_addr().unwrap().port(), lis_b.local_addr().unwrap().port());
    tokio::spawn(a.run(lis_a));
    tokio::spawn(b.run(lis_b));

    // B's authority provider session writes the workspace content (the C4
    // source) — an ordinary session appending ordinary chained ops.
    let provider = crate::ws::connect("127.0.0.1", port_b).await.unwrap();
    let o0 = tree_op(0, None, b"tree-v0");
    let o1 = tree_op(1, Some(crate::chain::op_hash(&o0).to_vec()), b"tree-v1");
    let written = ops_frame(vec![o0.clone(), o1.clone()]);
    provider.1.send_binary(&written).await.unwrap();
    wait_store(&b_shared, |st| tree_len(st) == 2, "B to hold the tree").await;
    TwoNodes {
        a: a_shared,
        b: b_shared,
        a_id,
        b_id,
        port_a,
        provider,
        tree: [o0, o1],
    }
}

/// The tree zone the journeys read, commons.
pub(super) fn tree_zone() -> (String, String, Vec<u8>) {
    ("ws-razel".into(), "ws.tree".into(), vec![])
}

/// How many of B's provider's tree ops `st` holds.
pub(super) fn tree_len(st: &Store) -> usize {
    st.scan("ws-razel", "ws.tree", &[], "prov-b", i64::MIN)
        .len()
}

/// The tree ops a node's replica holds from B's provider, as payloads.
pub(super) async fn tree_payloads(shared: &Arc<Shared>) -> Vec<Vec<u8>> {
    let store = shared.store.lock().await;
    let held = store.scan("ws-razel", "ws.tree", &[], "prov-b", i64::MIN);
    held.into_iter().map(|op| op.payload).collect()
}

/// Whether a node's fan-out of the tree zone reaches any session.
pub(super) async fn tree_routed(shared: &Arc<Shared>) -> bool {
    let router = shared.router.lock().await;
    !router.route(0, "ws-razel", "ws.tree", &[]).is_empty()
}

/// The nodes, in hex, whose subscription streams a node has admitted.
pub(super) async fn admitted(shared: &Arc<Shared>) -> Vec<String> {
    let admitted = shared.admitted.lock().await;
    admitted.values().map(|node| hex_id(node)).collect()
}

/// One `Ops` frame, as bytes.
pub(super) fn ops_frame(ops: Vec<Op>) -> Vec<u8> {
    Frame::Ops(Ops { ops, pri: None }).to_bytes()
}

/// Payloads from `r`'s ops, in order, until `n` have arrived.
pub(super) async fn payloads(r: &mut crate::ws::WsReader, n: usize, what: &str) -> Vec<Vec<u8>> {
    let mut payloads = Vec::new();
    while payloads.len() < n {
        if let Frame::Ops(ops) = next_frame(r, what).await {
            payloads.extend(ops.ops.into_iter().map(|o| o.payload));
        }
    }
    payloads
}

/// A client on A subscribes the tree zone and reads its ack.
pub(super) async fn a_client(t: &TwoNodes) -> (crate::ws::WsReader, crate::ws::WsWriter) {
    let (mut rc, wc) = crate::ws::connect("127.0.0.1", t.port_a).await.unwrap();
    wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
    let ack = next_frame(&mut rc, "ws.tree ack").await;
    assert!(matches!(ack, Frame::Heads(_)), "{ack:?}");
    (rc, wc)
}

/// Wait, bounded, until A's forward of the tree zone has lapsed.
pub(super) async fn forward_lapses(a: &Arc<Shared>) {
    let mesh = a.mesh.get().unwrap();
    for _ in 0..500 {
        if !mesh.forwarded.lock().await.contains(&tree_zone()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("A's forward of the tree zone did not lapse");
}

/// Subscribe the tree zone on a fresh conversation of A's link to B, as
/// A's forward does, and return B's answer: a refusal's two frames (R6),
/// and the reason, once B has ended the conversation.
pub(super) async fn refused_on_the_link(t: &TwoNodes) -> glade_wire::generated::Error {
    let linked = t.a.mesh.get().unwrap().linked(&t.b_id).await.unwrap();
    let mut conversation = linked.open();
    let (share, glade_id, _) = tree_zone();
    let subscribe = Subscribe {
        share,
        glade_id,
        key: None,
        from: None,
    };
    conversation.send(&Frame::Subscribe(subscribe)).unwrap();
    let mut frames = Vec::new();
    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), conversation.recv());
        match read.await.expect("B answered and ended the conversation") {
            Ok(frame) => frames.push(frame),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => panic!("reading B's answer: {e}"),
        }
    }
    match frames.as_slice() {
        [Frame::Heads(h), Frame::Error(e)] if h.streams.is_empty() => {
            assert_eq!(e.code, ErrorCode::Unauthorized);
            let named = (e.share.as_deref(), e.glade_id.as_deref());
            assert_eq!(named, (Some("ws-razel"), Some("ws.tree")));
            assert_eq!(e.corr, None);
            e.clone()
        }
        other => panic!("expected an ack that names no zone, then the reason: {other:?}"),
    }
}
