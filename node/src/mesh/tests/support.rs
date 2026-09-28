use crate::conversation::Linked;
use crate::frame::Frame;
use crate::mesh::hex_id;
use crate::mesh::testing::on_carrier;
use crate::netconf::PeerEntry;
use crate::registry::Record;
use crate::server::{Server, Shared};
use crate::store::Store;
use crate::sysdata::ServeClaim;
use crate::sysdir::now_ms;
use crate::transport::Door;
use glade_wire::generated::{Op, Ops};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn fresh(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("glade-mesh-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Poll until `pred` (over the node's store) holds, or panic after ~5s —
/// convergence is eventually-consistent, tests wait for it, never sleep blind.
pub(super) async fn wait_store<F: Fn(&Store) -> bool>(shared: &Arc<Shared>, pred: F, what: &str) {
    for _ in 0..500 {
        if pred(&*shared.store.lock().await) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}
pub(super) async fn wait_for<F: Fn(&Store) -> bool>(server: &Server, pred: F, what: &str) {
    wait_store(&server.shared, pred, what).await
}

pub(super) const A_SEED: [u8; 32] = [21; 32];
pub(super) const A_KEY: [u8; 32] = [22; 32];
pub(super) const B_SEED: [u8; 32] = [23; 32];
pub(super) const B_KEY: [u8; 32] = [24; 32];

/// The endpoint id the seed `key` gives, and the node id of `seed`.
pub(super) fn endpoint_of(key: [u8; 32]) -> [u8; 32] {
    crate::transport::EndpointKey::from_seed(key).endpoint_id
}
pub(super) fn node_of(seed: [u8; 32]) -> [u8; 32] {
    crate::signing::public_key(&seed)
}

/// The refusal lines a door reported.
pub(super) type Lines = Arc<std::sync::Mutex<Vec<String>>>;

/// A node behind a door that configures `configured`: node key `seed`,
/// endpoint key `key`; `records` in its served store before the mesh
/// starts; the door's refusal lines; its dialable address.
pub(super) async fn behind_door(
    name: &str,
    keys: ([u8; 32], [u8; 32]),
    configured: &[[u8; 32]],
    records: &[Op],
) -> (Server, Lines, PeerEntry) {
    let (server, lines, _, addr) = noting_door(name, keys, configured, records).await;
    (server, lines, addr)
}

/// [`behind_door`], its door also taking the mesh's status lines (plan
/// Step 4.5), which come back after its refusal lines.
pub(super) async fn noting_door(
    name: &str,
    (seed, key): ([u8; 32], [u8; 32]),
    configured: &[[u8; 32]],
    records: &[Op],
) -> (Server, Lines, Lines, PeerEntry) {
    let server = Server::open(fresh(name)).unwrap();
    for op in records {
        server.shared.store.lock().await.append(op.clone()).unwrap();
    }
    let (lines, notes) = (Lines::default(), Lines::default());
    let (sink, noting) = (lines.clone(), notes.clone());
    let door = Door::new(configured.iter().copied(), move |line: &str| {
        sink.lock().unwrap().push(line.into())
    });
    let door = door.with_status(move |line: &str| noting.lock().unwrap().push(line.into()));
    let (identity, key) = (
        crate::peer::NodeIdentity::from_key(seed),
        crate::transport::EndpointKey::from_seed(key),
    );
    let max = crate::frame::MAX_FRAME_BYTES;
    let at = on_carrier(&server, identity, key, Some(Arc::new(door)), max).await;
    (server, lines, notes, at)
}

/// The node `seed`'s record, first on its chain, sealed by it (plan Step
/// 4.1b).
pub(super) fn signed(seed: [u8; 32], record: Record) -> Op {
    crate::envelope::testing::sealed(seed, record)
}

/// Push `ops` on a conversation of `linked`'s own, as `push_home` does:
/// one `Ops` frame, then END.
pub(super) fn pushed(linked: &Arc<Linked>, ops: Vec<Op>) {
    let (conversation, frame) = (linked.open(), Frame::Ops(Ops { ops, pri: None }));
    conversation.send(&frame).unwrap();
    conversation.end();
}

pub(super) const C_SEED: [u8; 32] = [27; 32];
pub(super) const C_KEY: [u8; 32] = [28; 32];

/// A principal's record.
pub(super) fn principal(name: &str) -> Record {
    let principal = name.into();
    Record::Principal(crate::sysdata::PrincipalRecord { principal })
}

/// B's `dir.claims` chain, sealed by B: a claim on `ws-razel`, then `n`
/// renewals, each a lease further on.
pub(super) fn b_claims(n: i64) -> Vec<Op> {
    let identity = crate::peer::NodeIdentity::from_key(B_SEED);
    let b_id = hex_id(&identity.node_id);
    let mut records = crate::registry::Registry::sealed(identity);
    let lease = now_ms() + 30_000;
    let claim = |renewal: i64| {
        let (node, share) = (b_id.clone(), "ws-razel".to_string());
        let lease_expiry_ms = lease + renewal;
        let epoch = 1;
        Record::Serve(ServeClaim {
            node,
            share,
            lease_expiry_ms,
            epoch,
        })
    };
    (0..=n)
        .map(|renewal| records.append_returning(claim(renewal), &b_id).unwrap())
        .collect()
}

/// A behind a door, holding `a_held`, linked to B, which holds `held`,
/// its first records: A has pulled them. A's report lines, and B.
pub(super) async fn a_linked_to_b(
    name: &str,
    a_held: &[Op],
    held: &[Op],
) -> (Server, Lines, Server) {
    let (a_keys, b_keys) = ([endpoint_of(A_KEY)], [endpoint_of(B_KEY)]);
    let at = |node: &str| format!("{name}-{node}");
    let (b, _, at_b) = behind_door(&at("b"), (B_SEED, B_KEY), &a_keys, held).await;
    let (a, a_lines, _) = behind_door(&at("a"), (A_SEED, A_KEY), &b_keys, a_held).await;
    a.connect_peer(at_b.clone()).await.unwrap();
    (a, a_lines, b)
}

/// Land `ops` in B's served store, as a mint does, without a push.
pub(super) async fn minted(b: &Server, ops: &[Op]) {
    let mut store = b.shared.store.lock().await;
    for op in ops {
        store.append(op.clone()).unwrap();
    }
}

/// Push `ops` from B to A on a conversation of their own, as
/// `push_home` does.
pub(super) async fn b_pushes(b: &Server, ops: &[Op]) {
    let a_id = hex_id(&node_of(A_SEED));
    let linked = b.shared.mesh.get().unwrap().linked(&a_id).await.unwrap();
    pushed(&linked, ops.to_vec());
}

/// Whether a pull on a gap runs at `server`.
pub(super) fn pulling(server: &Server) -> bool {
    let mesh = server.shared.mesh.get().unwrap();
    !mesh.gap_pulls.lock().unwrap().is_empty()
}
