use super::support::{
    a_linked_to_b, b_claims, b_pushes, behind_door, endpoint_of, fresh, minted, node_of, principal,
    pulling, pushed, signed, wait_for, Lines, A_KEY, A_SEED, B_KEY, B_SEED, C_KEY, C_SEED,
};
use crate::mesh::testing::{meshed, on_carrier};
use crate::mesh::{hex_id, who_serves};
use crate::registry::{Record, RegistryApi, G_CLAIMS, G_PRINCIPALS, HOME};
use crate::server::Server;
use crate::store::Store;
use crate::sysdata::{ServeClaim, WorkspaceEntry};
use crate::sysdir::{boot_at, now_ms};
use glade_wire::generated::Op;
use std::time::Duration;

/// Two booted nodes, wired over real iroh: after `connect_peer` the home
/// share has converged BOTH ways — each node's replica holds the other's
/// presence records (dir.nodes op from the other's origin), and the claim
/// fold routes from either replica.
#[tokio::test(flavor = "multi_thread")]
async fn two_booted_nodes_converge_home_share() {
    let boot_a = boot_at(fresh("conv-a-sys"), "gianni").unwrap();
    let boot_b = boot_at(fresh("conv-b-sys"), "gianni").unwrap();

    // B additionally registers a workspace + its serve claim (directory data).
    let mut boot_b = boot_b;
    boot_b
        .registry
        .append(
            Record::Workspace(WorkspaceEntry {
                workspace: "ws-razel".into(),
                name: "razel".into(),
                eligible_hosts: vec![boot_b.node_id.clone()],
            }),
            &boot_b.node_id,
        )
        .unwrap();
    boot_b
        .registry
        .append(
            Record::Serve(ServeClaim {
                node: boot_b.node_id.clone(),
                share: "ws-razel".into(),
                lease_expiry_ms: now_ms() + 30_000,
                epoch: 1,
            }),
            &boot_b.node_id,
        )
        .unwrap();

    let a = Server::open(fresh("conv-a-store")).unwrap();
    let b = Server::open(fresh("conv-b-store")).unwrap();
    a.seed_registry(&boot_a.registry.snapshot()).await;
    b.seed_registry(&boot_b.registry.snapshot()).await;

    meshed(&a, boot_a.identity().unwrap()).await;
    let at_b = meshed(&b, boot_b.identity().unwrap()).await;

    // The HELLO identity is the directory identity (one id, two renderings).
    let peer = a.connect_peer(at_b).await.unwrap();
    assert_eq!(peer, boot_b.node_id);

    // A pulled B: B's presence + workspace + claim are in A's replica...
    let (b_id, a_id) = (boot_b.node_id.clone(), boot_a.node_id.clone());
    {
        let bid = b_id.clone();
        wait_for(&a, move |st| !st.scan(HOME, crate::registry::G_NODES, &[], &bid, i64::MIN).is_empty(), "A to hold B's presence").await;
    }
    {
        let st = a.shared.store.lock().await;
        assert!(!st.scan(HOME, crate::registry::G_WORKSPACES, &[], &b_id, i64::MIN).is_empty(), "A holds B's WorkspaceEntry");
        // ...and A's LOCAL fold routes ws-razel to B, judged at A's clock.
        assert_eq!(who_serves(&st, "ws-razel", now_ms()), Some(b_id.clone()));
        assert_eq!(who_serves(&st, "ws-razel", now_ms() + 60_000), None, "lapsed at a later reader clock");
    }

    // ...and B pulled A (the reverse direction of the same connection).
    {
        let aid = a_id.clone();
        wait_for(&b, move |st| !st.scan(HOME, crate::registry::G_NODES, &[], &aid, i64::MIN).is_empty(), "B to hold A's presence").await;
    }
}

/// Plan Step 4.2: each booted node binds its endpoint with its own
/// `endpoint.key`, and its binding, minted at boot, reaches the peer's
/// served store by the pull the HELLO opens. There it folds live for the
/// very key the peer's connection came from, which is what 4.2b's door
/// will read. It checks no refusal: 4.2a builds no door.
#[tokio::test(flavor = "multi_thread")]
async fn a_peers_binding_arrives_by_the_pull_and_folds_live() {
    use crate::transport::{Bound, TransportFold};
    let boot_a = boot_at(fresh("tb-a-sys"), "gianni").unwrap();
    let boot_b = boot_at(fresh("tb-b-sys"), "gianni").unwrap();
    let (id_a, key_a) = (boot_a.identity().unwrap(), boot_a.endpoint_key());
    let (id_b, key_b) = (boot_b.identity().unwrap(), boot_b.endpoint_key());
    let a = Server::open(fresh("tb-a-store")).unwrap();
    let b = Server::open(fresh("tb-b-store")).unwrap();
    a.adopt_boot(boot_a).await.unwrap();
    b.adopt_boot(boot_b).await.unwrap();
    let max = crate::frame::MAX_FRAME_BYTES;
    on_carrier(&a, id_a, key_a, None, max).await;
    let at_b = on_carrier(&b, id_b, key_b, None, max).await;
    assert_eq!(at_b.key, key_b.endpoint_id);
    a.connect_peer(at_b).await.unwrap();

    let live = |node: [u8; 32], key: [u8; 32]| {
        move |st: &Store| TransportFold::of_store(st).binds(&node, &key, now_ms()) == Bound::Live
    };
    let (b_at_a, a_at_b) = (
        live(id_b.node_id, key_b.endpoint_id),
        live(id_a.node_id, key_a.endpoint_id),
    );
    wait_for(&a, b_at_a, "B's binding at A").await;
    wait_for(&b, a_at_b, "A's binding at B").await;
}

/// Plan Step 4.1b, F2 closed for a peer's push: A pushes B a claim on
/// `ws-razel` at a higher epoch than B's own, which would route B's
/// share to A, in forms A did not sign: bare, under A's id, and sealed by
/// another key. B takes neither: when A's genuine marker, pushed after
/// them in the same frame, has landed, B still routes `ws-razel` to
/// itself and holds A's claims chain as it was. The same claim, sealed by
/// A, then lands in the slot they would have taken.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_a_peer_did_not_sign_is_refused_where_it_is_pushed() {
    let boot_a = boot_at(fresh("forged-a-sys"), "gianni").unwrap();
    let mut boot_b = boot_at(fresh("forged-b-sys"), "gianni").unwrap();
    let (a_id, b_id) = (boot_a.node_id.clone(), boot_b.node_id.clone());
    let claim = |node: &str, epoch| {
        let (node, share) = (node.to_string(), "ws-razel".to_string());
        let lease_expiry_ms = now_ms() + 30_000;
        Record::Serve(ServeClaim {
            node,
            share,
            lease_expiry_ms,
            epoch,
        })
    };
    boot_b.registry.append(claim(&b_id, 1), &b_id).unwrap();
    let a = Server::open(fresh("forged-a-store")).unwrap();
    let b = Server::open(fresh("forged-b-store")).unwrap();
    a.seed_registry(&boot_a.registry.snapshot()).await;
    b.seed_registry(&boot_b.registry.snapshot()).await;
    meshed(&a, boot_a.identity().unwrap()).await;
    let at_b = meshed(&b, boot_b.identity().unwrap()).await;
    a.connect_peer(at_b).await.unwrap();
    let held = |node: String| move |st: &Store| st.scan(HOME, G_CLAIMS, &[], &node, -1).len();
    wait_for(
        &b,
        |st| held(a_id.clone())(st) == 1,
        "B to hold A's home claim",
    )
    .await;

    let mut a_records = boot_a.registry.clone();
    let genuine = a_records.append_returning(claim(&a_id, 99), &a_id).unwrap();
    let bare = Op {
        payload: crate::envelope::record_bytes(&genuine.payload),
        ..genuine.clone()
    };
    let other = crate::peer::NodeIdentity::from_key([26; 32]);
    let forged = Op {
        payload: crate::envelope::seal(&other, &bare),
        ..genuine.clone()
    };
    let marker = Record::Principal(crate::sysdata::PrincipalRecord {
        principal: "marker".into(),
    });
    let marker = a_records.append_returning(marker, &a_id).unwrap();
    let linked = a.shared.mesh.get().unwrap().linked(&b_id).await.unwrap();
    pushed(&linked, vec![bare, forged, marker]);
    let principals = |st: &Store| {
        st.scan(HOME, crate::registry::G_PRINCIPALS, &[], &a_id, -1)
            .len()
    };
    wait_for(&b, |st| principals(st) == 1, "A's marker at B").await;
    {
        let st = b.shared.store.lock().await;
        assert_eq!(who_serves(&st, "ws-razel", now_ms()), Some(b_id.clone()));
        assert_eq!(held(a_id.clone())(&st), 1, "A's claims chain as it was");
    }
    pushed(&linked, vec![genuine.clone()]);
    wait_for(
        &b,
        |st| held(a_id.clone())(st) == 2,
        "A's signed claim at B",
    )
    .await;
    let st = b.shared.store.lock().await;
    assert_eq!(st.scan(HOME, G_CLAIMS, &[], &a_id, 0), [genuine]);
    assert_eq!(who_serves(&st, "ws-razel", now_ms()), Some(a_id.clone()));
}

// ---- D9's known set (plan Step 4.1b's part 2), over real iroh ----------

/// Plan Step 4.1b's part 2 (D9). B holds its own record and two of C's,
/// a node A has not met. A's pull from B takes B's record and defers C's
/// chain, kept nowhere, with one line. Once A has met C, by a HELLO, B's
/// push of C's records lands them; and a record under B's id that B did
/// not sign, in the same push, is refused, with one line.
#[tokio::test(flavor = "multi_thread")]
async fn a_third_nodes_records_are_deferred_until_its_hello_and_reported() {
    let c_identity = crate::peer::NodeIdentity::from_key(C_SEED);
    let c_id = hex_id(&c_identity.node_id);
    let mut c_records = crate::registry::Registry::sealed(c_identity);
    let c_ops: Vec<Op> = ["c0", "c1"]
        .into_iter()
        .map(|name| c_records.append_returning(principal(name), &c_id).unwrap())
        .collect();
    let b_op = signed(B_SEED, principal("b"));
    let records = [c_ops.clone(), vec![b_op.clone()]].concat();
    let a_keys = [endpoint_of(A_KEY)];
    let (b, _, at_b) = behind_door("d9-b", (B_SEED, B_KEY), &a_keys, &records).await;
    let (_c, _, at_c) = behind_door("d9-c", (C_SEED, C_KEY), &a_keys, &[]).await;
    let known = [endpoint_of(B_KEY), endpoint_of(C_KEY)];
    let (a, a_lines, _) = behind_door("d9-a", (A_SEED, A_KEY), &known, &[]).await;
    let held = |st: &Store, origin: &str| st.scan(HOME, G_PRINCIPALS, &[], origin, -1);
    let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));

    a.connect_peer(at_b.clone()).await.unwrap();
    {
        let st = a.shared.store.lock().await;
        let b_held = std::slice::from_ref(&b_op);
        assert_eq!(held(&st, &b_id), b_held, "B's own record lands");
        assert_eq!(held(&st, &c_id), [], "C's is kept nowhere");
    }
    let deferred = format!(
        "deferred 2 home record(s) of node {c_id} on dir.principals from peer {b_id}: not a node this node knows"
    );
    assert_eq!(*a_lines.lock().unwrap(), std::slice::from_ref(&deferred));

    a.connect_peer(at_c.clone()).await.unwrap();
    let bare = Op {
        seq: 1,
        prev: Some(crate::chain::op_hash(&b_op).to_vec()),
        payload: principal("b1").encode(),
        ..b_op.clone()
    };
    let forged = Op {
        payload: crate::envelope::seal(&c_identity, &bare),
        ..bare
    };
    let linked = b.shared.mesh.get().unwrap().linked(&a_id).await.unwrap();
    pushed(&linked, [vec![forged], c_ops].concat());
    wait_for(&a, |st| held(st, &c_id).len() == 2, "C's records at A").await;
    let refused = format!(
        "refused 1 home record(s) of node {b_id} on dir.principals from peer {b_id}: ({b_id},1) does not verify: its signature does not verify"
    );
    for _ in 0..500 {
        if a_lines.lock().unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*a_lines.lock().unwrap(), [deferred, refused]);
    assert_eq!(held(&*a.shared.store.lock().await, &b_id), [b_op]);
}

// ---- a pull on a gap (the hardening's question 2), over real iroh ------

/// The lines once there are `n`, waiting at most about 5 s for them.
async fn reported(lines: &Lines, n: usize) -> Vec<String> {
    for _ in 0..500 {
        if lines.lock().unwrap().len() >= n {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    lines.lock().unwrap().clone()
}

/// A's line for a push of B's claims chain at `seq`, refused as a gap
/// while A holds B's claim alone.
fn refused_as_a_gap(b_id: &str, seq: i64) -> String {
    let head = format!("1 home record(s) of node {b_id} on dir.claims from peer {b_id}");
    format!("refused {head}: a gap: expected seq 1, got {seq}")
}

/// The hardening's question 2, ruled (b). B's renewal pushed ahead of the
/// one before it is refused as a gap at A, and A pulls from B at once:
/// the chain heals without the link coming up again, with a line saying
/// so. The late push of the earlier renewal changes nothing, and the next
/// renewal lands in order. Before, the chain stayed short until the next
/// link, and so did every later renewal on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_renewal_pushed_ahead_of_the_one_before_it_heals_by_a_pull() {
    let chain = b_claims(3);
    let (a, a_lines, b) = a_linked_to_b("gap-order", &[], &chain[..1]).await;
    let b_id = hex_id(&node_of(B_SEED));
    let claims = |st: &Store| st.scan(HOME, G_CLAIMS, &[], &b_id, -1);
    minted(&b, &chain[1..3]).await;
    b_pushes(&b, &chain[2..3]).await;
    wait_for(&a, |st| claims(st).len() == 3, "the chain to heal at A").await;
    b_pushes(&b, &chain[1..2]).await;
    minted(&b, &chain[3..]).await;
    b_pushes(&b, &chain[3..]).await;
    wait_for(&a, |st| claims(st).len() == 4, "the next renewal at A").await;
    assert_eq!(claims(&*a.shared.store.lock().await), chain);
    let pulled = format!(
        "pulled 2 home record(s) from peer {b_id} after 1 gap(s): dir.claims of node {b_id} healed"
    );
    let lines = [refused_as_a_gap(&b_id, 2), pulled];
    assert_eq!(reported(&a_lines, 2).await, lines);
}

/// A burst of gaps from one pusher is answered by one pull. B's store is
/// held, so A's pull waits for B's answer, while B pushes three renewals
/// out of order, each refused as a gap. The later two wait for the
/// running pull, which heals all three, so no second pull runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_gaps_from_one_pusher_is_answered_by_one_pull() {
    let chain = b_claims(4);
    let (a, a_lines, b) = a_linked_to_b("gap-burst", &[], &chain[..1]).await;
    let b_id = hex_id(&node_of(B_SEED));
    minted(&b, &chain[1..]).await;
    let mut lines = Vec::new();
    {
        let _answer_waits = b.shared.store.lock().await;
        for seq in [4, 3, 2] {
            let at = seq as usize;
            b_pushes(&b, &chain[at..at + 1]).await;
            lines.push(refused_as_a_gap(&b_id, seq));
            assert_eq!(reported(&a_lines, lines.len()).await, lines);
            assert!(pulling(&a), "the pull the first gap started runs");
        }
    }
    let claims = |st: &Store| st.scan(HOME, G_CLAIMS, &[], &b_id, -1);
    wait_for(&a, |st| claims(st) == chain, "the chain to heal at A").await;
    lines.push(format!(
        "pulled 4 home record(s) from peer {b_id} after 3 gap(s): dir.claims of node {b_id} healed"
    ));
    assert_eq!(reported(&a_lines, lines.len()).await, lines);
    for _ in 0..500 {
        if !pulling(&a) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!pulling(&a), "the pull ended");
    assert_eq!(*a_lines.lock().unwrap(), lines, "and no other ran");
}

/// A deferred chain is not a gap (D9 beside the hardening's question 2):
/// B pushes two records of C, a node A has not met, and A defers them
/// and starts no pull. B's renewal pushed ahead of the one before it
/// then starts one, which names B's chain alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferred_chain_starts_no_pull() {
    let chain = b_claims(2);
    let (a, a_lines, b) = a_linked_to_b("gap-deferred", &[], &chain[..1]).await;
    let b_id = hex_id(&node_of(B_SEED));
    let c_identity = crate::peer::NodeIdentity::from_key(C_SEED);
    let c_id = hex_id(&c_identity.node_id);
    let mut c_records = crate::registry::Registry::sealed(c_identity);
    let c_ops: Vec<Op> = ["c0", "c1"]
        .into_iter()
        .map(|name| c_records.append_returning(principal(name), &c_id).unwrap())
        .collect();
    b_pushes(&b, &c_ops).await;
    let deferred = format!(
        "deferred 2 home record(s) of node {c_id} on dir.principals from peer {b_id}: not a node this node knows"
    );
    let only = std::slice::from_ref(&deferred);
    assert_eq!(reported(&a_lines, 1).await, only);
    assert!(!pulling(&a), "a deferred chain started a pull");

    minted(&b, &chain[1..]).await;
    b_pushes(&b, &chain[2..]).await;
    let pulled = format!(
        "pulled 2 home record(s) from peer {b_id} after 1 gap(s): dir.claims of node {b_id} healed"
    );
    let lines = [deferred, refused_as_a_gap(&b_id, 2), pulled];
    assert_eq!(reported(&a_lines, 3).await, lines);
    let st = a.shared.store.lock().await;
    assert_eq!(st.scan(HOME, G_CLAIMS, &[], &b_id, -1), chain);
}
