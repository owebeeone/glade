use super::support::{
    a_linked_to_b, b_claims, b_pushes, fresh, minted, node_of, pulling, wait_for, B_SEED,
};
use crate::claims::testing;
use crate::mesh::testing::meshed;
use crate::mesh::{hex_id, who_serves};
use crate::registry::{G_CLAIMS, G_PRINCIPALS, HOME};
use crate::server::Server;
use crate::store::Store;
use crate::sysdir::{boot_at, now_ms};
use glade_wire::generated::Op;

// ---- signed checkpoints (plan Step 4.5c's part 2) ----------------------

/// A serve sends each `home` zone of `dir.checkpoints` before every
/// other, so that a puller places a checkpoint before the chain it folds,
/// which then starts at its floor: given `dir.binding-retractions`, which
/// sorts before it, and the rest in the store's order.
#[test]
fn the_serve_sends_checkpoints_before_every_other_home_zone() {
    use crate::registry::{G_BINDING_RETRACTIONS as RETRACTIONS, G_CHECKPOINTS};
    let held = [RETRACTIONS, G_CHECKPOINTS, G_CLAIMS, G_PRINCIPALS];
    let zones = held.map(|stream| (HOME.to_string(), stream.to_string(), Vec::new()));
    let order = crate::session::serve_order(zones.to_vec());
    let streams: Vec<&str> = order.iter().map(|(_, stream, _)| stream.as_str()).collect();
    let first = [G_CHECKPOINTS, RETRACTIONS, G_CLAIMS, G_PRINCIPALS];
    assert_eq!(streams, first);
}

/// B's checkpoint of its claims chain at `base`, the first of its
/// `dir.checkpoints` chain, sealed as its registry seals it.
fn b_checkpoint(chain: &[Op], base: usize) -> Op {
    let record = crate::sysdata::ChainCheckpoint {
        node: hex_id(&node_of(B_SEED)),
        stream: G_CLAIMS.into(),
        seq: base as i64,
        hash: crate::chain::op_hash(&chain[base]).to_vec(),
    };
    crate::envelope::testing::checkpoint(B_SEED, record, 0, None)
}

/// Each op's seq, in order.
fn seqs(ops: Vec<Op>) -> Vec<i64> {
    ops.iter().map(|op| op.seq).collect()
}

/// Plan Step 4.5c, over real iroh. A holds B's claims 0 to 3, and B has
/// folded its chain at 7: it holds the checkpoint, 8 and 9. A's pull at
/// the link takes the checkpoint first, and drops its prefix unchecked,
/// since it does not hold the op at the base; then the chain from the
/// floor, with no refusal. A routes B's share to B.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_behind_a_checkpoint_takes_it_first_and_the_chain_from_its_floor() {
    use crate::registry::G_CHECKPOINTS;
    let chain = b_claims(9);
    let checkpoint = b_checkpoint(&chain, 7);
    let b_holds = [chain.clone(), vec![checkpoint.clone()]].concat();
    let (a, a_lines, _b) = a_linked_to_b("behind", &chain[..4], &b_holds).await;
    let b_id = hex_id(&node_of(B_SEED));
    assert_eq!(*a_lines.lock().unwrap(), Vec::<String>::new());
    let st = a.shared.store.lock().await;
    assert_eq!(seqs(st.scan(HOME, G_CLAIMS, &[], &b_id, -1)), [8, 9]);
    assert_eq!(st.scan(HOME, G_CHECKPOINTS, &[], &b_id, -1), [checkpoint]);
    assert_eq!(who_serves(&st, "ws-razel", now_ms()), Some(b_id));
}

/// Plan Step 4.5c, over real iroh. A holds B's claims 0 to 7; B pushes 8,
/// 9 and its checkpoint at 7. A takes the three and drops its claims at
/// or below 7, having compared the one at 7, with no gap and no pull. A's
/// journal for B then holds the checkpoint, 8 and 9.
#[tokio::test(flavor = "multi_thread")]
async fn a_pushed_checkpoint_prunes_a_current_peer_with_no_gap() {
    use crate::registry::G_CHECKPOINTS;
    use crate::store::testing::{journal_of, slots};
    let chain = b_claims(9);
    let (a, a_lines, b) = a_linked_to_b("pruned", &[], &chain[..8]).await;
    let b_id = hex_id(&node_of(B_SEED));
    let pushed = [chain[8..].to_vec(), vec![b_checkpoint(&chain, 7)]].concat();
    minted(&b, &pushed).await;
    b_pushes(&b, &pushed).await;
    let placed = |st: &Store| !st.scan(HOME, G_CHECKPOINTS, &[], &b_id, -1).is_empty();
    wait_for(&a, placed, "B's checkpoint at A").await;
    assert!(!pulling(&a), "a pull started");
    let claims = |st: &Store| seqs(st.scan(HOME, G_CLAIMS, &[], &b_id, -1));
    assert_eq!(claims(&*a.shared.store.lock().await), [8, 9]);
    let root = std::env::temp_dir().join("glade-mesh-pruned-a");
    let retained = [(G_CHECKPOINTS, 0), (G_CLAIMS, 8), (G_CLAIMS, 9)];
    assert_eq!(slots(&journal_of(&root, HOME, &b_id)), retained);
    assert_eq!(*a_lines.lock().unwrap(), Vec::<String>::new());
}

// ---- signed checkpoints (plan Step 4.5c's part 3) ----------------------

/// Plan Step 4.5c, the done-when's second clause, over real iroh. A,
/// serving `home` and `ws-a` with a threshold of 4, renews by hand while
/// linked to B, and folds its claims chain at its third and sixth ticks.
/// B follows each push across both checkpoints: it holds A's chain as A
/// does, its journal for A keeps one checkpoint and at most 4 + 2 of A's
/// claims, and it routes `ws-a` to A, as A does. C, met afterwards, takes
/// A's chain from the second checkpoint by its pull, and routes alike.
#[tokio::test(flavor = "multi_thread")]
async fn a_linked_peer_and_a_new_one_take_the_checkpointed_chain() {
    use crate::claims::Leases;
    use crate::registry::G_CHECKPOINTS;
    use crate::store::testing::journal_of;
    let booted = |node: &str| boot_at(fresh(&format!("folded-{node}-sys")), "gianni").unwrap();
    let (boot_a, boot_b, boot_c) = (booted("a"), booted("b"), booted("c"));
    let a_id = boot_a.node_id.clone();
    let ids = [&boot_a, &boot_b, &boot_c].map(|boot| boot.identity().unwrap());
    let b_root = fresh("folded-b-store");
    let a = Server::open(fresh("folded-a-store")).unwrap();
    let b = Server::open(&b_root).unwrap();
    let c = Server::open(fresh("folded-c-store")).unwrap();
    let leases = Leases {
        renew_ms: 3_600_000,
        checkpoint_after: 4,
        ..Leases::default()
    };
    let adopted = a.adopt_boot_tuned(boot_a, leases, |_: &str| {});
    adopted.await.unwrap();
    b.adopt_boot(boot_b).await.unwrap();
    c.adopt_boot(boot_c).await.unwrap();
    let [id_a, id_b, id_c] = ids;
    let at_a = meshed(&a, id_a).await;
    let at_b = meshed(&b, id_b).await;
    meshed(&c, id_c).await;
    a.connect_peer(at_b).await.unwrap();
    a.serve_workspace("ws-a", "a").await.unwrap();

    let chain = |st: &Store| seqs(st.scan(HOME, G_CLAIMS, &[], &a_id, -1));
    let checkpoints = |st: &Store| st.scan(HOME, G_CHECKPOINTS, &[], &a_id, -1);
    for tick in 1..=6 {
        testing::tick(&a.shared).await;
        let held = chain(&*a.shared.store.lock().await);
        let follows = move |st: &Store| chain(st) == held;
        wait_for(&b, follows, "B to hold A's claims chain as A does").await;
        if tick >= 3 {
            let journal = journal_of(&b_root, HOME, &a_id);
            let on = |stream: &str| {
                let held = journal.iter().filter(|op| op.glade_id == stream);
                held.count()
            };
            let (claims, folded) = (on(G_CLAIMS), on(G_CHECKPOINTS));
            let said = format!("tick {tick}: B's copy holds {claims} of A's claims");
            assert!(claims <= 4 + 2, "{said}");
            assert_eq!(folded, 1, "tick {tick}: B's journal for A");
        }
        let now = now_ms();
        let at_b = who_serves(&*b.shared.store.lock().await, "ws-a", now);
        assert_eq!(at_b, Some(a_id.clone()), "tick {tick}: B routes ws-a");
        let at_a = who_serves(&*a.shared.store.lock().await, "ws-a", now);
        assert_eq!(at_a, at_b, "tick {tick}: as A does");
    }
    let (held, folded) = {
        let st = a.shared.store.lock().await;
        (chain(&st), checkpoints(&st))
    };
    assert_eq!(folded.iter().map(|op| op.seq).collect::<Vec<_>>(), [1]);
    assert_eq!(checkpoints(&*b.shared.store.lock().await), folded, "B's");

    c.connect_peer(at_a).await.unwrap();
    let taken = move |st: &Store| chain(st) == held && checkpoints(st) == folded;
    wait_for(&c, taken, "C to take A's chain from its checkpoint").await;
    let serves = who_serves(&*c.shared.store.lock().await, "ws-a", now_ms());
    assert_eq!(serves, Some(a_id), "C routes ws-a to A");
}
