use super::*;
use std::collections::BTreeMap;
fn initial() -> (ReplicaImage, ReplicaFloor, DecodeLimits) {
    let decode = DecodeLimits {
        bytes: 65536,
        items: 4096,
        depth: 64,
        name_bytes: 256,
    };
    let owner = Owner {
        store: StoreIncarnation([1; 32]),
        generation: 1,
    };
    let instance = Instance {
        descriptor: [2; 32],
        incarnation: "structural-pair".into(),
    };
    let storage = Limits {
        instances: 1,
        attempts: 8,
        invocation_history: 32,
        max_batch_bytes: 16384,
        max_receipts: 8,
        max_windows: 8,
        max_name_bytes: 256,
        critical_bytes: 16384,
        critical_per_attempt: 1024,
    };
    let limits = ReplicaLimits {
        storage: storage.clone(),
        decode: decode.clone(),
        image_bytes: 65536,
        guard_history: 16,
        inbox_items: 16,
        inbox_bytes: 8192,
        inventory_items: 32,
        inventory_bytes: 8192,
    };
    let issuance = IssuanceFloor {
        time_floor_ms: 0,
        policy_generation: 0,
        policy_digest: [3; 32],
        plan_namespace: [4; 32],
        invocation_namespace: [5; 32],
        next_effect_id: 0,
        origins: BTreeMap::new(),
    };
    let floor = ReplicaFloor {
        issuance,
        version: 1,
        owner: owner.clone(),
        root_registration: [6; 32],
        path_binding: [7; 32],
        lease: 1,
        sequence: 0,
        selected: Selection {
            slot: 0,
            generation: 0,
            digest: [0; 32],
        },
        next_attempt: 0,
        invocation_high_water: 0,
        guard_high_water: 0,
        guards: BTreeMap::new(),
        intent: None,
    };
    let identity = IdentityBinding {
        root: [8; 32],
        intent: Bytes::new(vec![]),
        zone: "private".into(),
        parameters: Bytes::new(vec![]),
        declaration: Bytes::new(vec![]),
        declaration_version: "1".into(),
        schema: Bytes::new(vec![]),
        schema_identity: "pair".into(),
        schema_version: "1".into(),
        corpus_identity: "pair".into(),
        corpus_version: "1".into(),
        corpus_digest: [9; 32],
        proof_profile: "ic3-direct-root-ed25519/v1".into(),
        recovery_profile: "ic3-records-floor/v1".into(),
        retention_policy: "full-history-no-gc/v1".into(),
        rejoin_policy: "exact-evidence-full-cut/v1".into(),
        initial_chain_base: 0,
        migration_generation: 0,
    };
    let state = State {
        storage_owner: Some(owner.clone()),
        storage_limits: Some(storage.clone()),
        plan_namespace: [4; 32],
        invocation_namespace: [5; 32],
        ..State::default()
    };
    let recovery = Recovery {
        owner,
        invocation_namespace: [5; 32],
        limits: storage,
        preparing: vec![],
        revisions: BTreeMap::new(),
        attempts: vec![],
        next_attempt: 0,
        invocation_high_water: 0,
        invocation_count: 0,
    };
    let image = ReplicaImage {
        version: 1,
        instance,
        identity,
        generation: 0,
        predecessor: [0; 32],
        state,
        recovery,
        prior_floor: floor.clone(),
        origins: vec![],
        observations: vec![],
        obligations: vec![],
        sessions: vec![],
        watermark: 0,
        permanent_loss: vec![],
        limits,
    };
    (image, floor, decode)
}
fn root(name: &str) -> PathBuf {
    let root = fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("glade-ic3-b2-pair-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    root
}
#[test]
fn nonrecursive_pair_selection_and_exact_pending_fallback_retain_all_floors() {
    let root = root("fallback");
    let data = root.join("data");
    let trusted = root.join("trusted");
    let (image, floor, bounds) = initial();
    let layout = DiskLayout::provision(&data, &trusted, [6; 32]).unwrap();
    let mut pair = PairStore::create(layout, image, floor, bounds.clone()).unwrap();
    let selected = pair.envelope.floor.selected.clone();
    assert_ne!(
        selected.digest, [0; 32],
        "selection hashes an existing nonrecursive image"
    );
    let mut next = pair.image.clone();
    next.generation = 1;
    next.predecessor = selected.digest;
    next.prior_floor = pair.envelope.floor.clone();
    next.state.next_effect_id = 17;
    let mut projected = pair.envelope.floor.clone();
    projected.issuance.next_effect_id = 17;
    projected.next_attempt = 23;
    projected.invocation_high_water = 31;
    projected.guard_high_water = 47;
    projected.issuance.time_floor_ms = 101;
    projected.issuance.policy_generation = 5;
    projected.issuance.policy_digest = [10; 32];
    projected.issuance.origins.insert(
        "origin".into(),
        OriginFloor {
            certificate: [11; 32],
            next_seq: 4,
            lamport: 19,
            prev: Some([12; 32]),
            reserved_intents: vec![Bytes::new(vec![13])],
            frontier: vec![],
        },
    );
    let expected = projected.issuance.clone();
    assert_eq!(
        pair.transact(next, projected, DiskOperation::PublishAccepted, |cut| {
            if cut == PairCut::Image(SlotCut::BeforeWrite) {
                return Err(Fault::Unavailable);
            }
            Ok(())
        }),
        Err(Fault::Unavailable)
    );
    drop(pair);
    crate::test_support::children::settled();
    let layout = DiskLayout::open(&data, &trusted, [6; 32]).unwrap();
    let pair = PairStore::open(layout, bounds.clone())
        .expect("old image plus exact advanced floor, never false terminal");
    assert_eq!(pair.image.generation, 0);
    assert_eq!(pair.envelope.floor.selected, selected);
    assert_eq!(pair.envelope.floor.issuance, expected);
    assert_eq!(
        (
            pair.envelope.floor.next_attempt,
            pair.envelope.floor.invocation_high_water,
            pair.envelope.floor.guard_high_water
        ),
        (23, 31, 47)
    );
    assert_eq!(pair.envelope.unmaterialized.len(), 1);
    assert_eq!(
        pair.envelope.unmaterialized[0].kind,
        DiskOperation::PublishAccepted
    );
    drop(pair);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn installed_next_image_finishes_lost_selection_reply_without_hash_recursion() {
    let root = root("lost");
    let data = root.join("data");
    let trusted = root.join("trusted");
    let (image, floor, bounds) = initial();
    let layout = DiskLayout::provision(&data, &trusted, [6; 32]).unwrap();
    let mut pair = PairStore::create(layout, image, floor, bounds.clone()).unwrap();
    let mut next = pair.image.clone();
    next.generation = 1;
    next.predecessor = pair.envelope.floor.selected.digest;
    next.prior_floor = pair.envelope.floor.clone();
    let projected = pair.envelope.floor.clone();
    assert!(pair
        .transact(next.clone(), projected, DiskOperation::Checkpoint, |cut| {
            if cut == PairCut::Selection(FloorCut::Renamed) {
                return Err(Fault::Unavailable);
            }
            Ok(())
        })
        .is_err());
    drop(pair);
    crate::test_support::children::settled();
    let layout = DiskLayout::open(&data, &trusted, [6; 32]).unwrap();
    let pair = PairStore::open(layout, bounds).unwrap();
    assert_eq!(pair.image, next);
    assert_eq!(pair.envelope.floor.selected.generation, 1);
    assert!(pair.envelope.pending.is_none());
    assert!(pair.envelope.floor.intent.is_none());
    assert!(pair.envelope.unmaterialized.is_empty());
    assert_ne!(
        pair.image.prior_floor.selected.digest,
        pair.envelope.floor.selected.digest
    );
    drop(pair);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn completed_next_image_installs_exact_guard_settlement_after_missing_selection() {
    let root = root("settlement");
    let data = root.join("data");
    let trusted = root.join("trusted");
    let (mut image, mut floor, bounds) = initial();
    let guard = ReceiveGuard {
        id: GuardId {
            store: floor.owner.store.clone(),
            instance: image.instance.clone(),
            number: 0,
        },
        owner: floor.owner.clone(),
        lease: 1,
        source: [15; 32],
        channel: [16; 32],
        role: "inventory".into(),
        base_generation: 0,
        base_digest: [0; 32],
        max_bytes: 8192,
        max_items: 16,
        deadline_ms: 1000,
    };
    floor.guard_high_water = 1;
    floor
        .guards
        .insert(guard.id.clone(), GuardStatus::Active(guard.clone()));
    image.prior_floor = floor.clone();
    let layout = DiskLayout::provision(&data, &trusted, [6; 32]).unwrap();
    let mut pair = PairStore::create(layout, image, floor, bounds.clone()).unwrap();
    let mut next = pair.image.clone();
    next.generation = 1;
    next.predecessor = pair.envelope.floor.selected.digest;
    next.prior_floor = pair.envelope.floor.clone();
    let mut projected = pair.envelope.floor.clone();
    let settlement = GuardStatus::Settled {
        guard: guard.clone(),
        observation: [17; 32],
        image_generation: 1,
    };
    projected
        .guards
        .insert(guard.id.clone(), settlement.clone());
    assert!(pair
        .transact(next, projected, DiskOperation::SettleIngress, |cut| {
            if cut == PairCut::Selection(FloorCut::BeforeTemp) {
                return Err(Fault::Unavailable);
            }
            Ok(())
        })
        .is_err());
    drop(pair);
    crate::test_support::children::settled();
    let layout = DiskLayout::open(&data, &trusted, [6; 32]).unwrap();
    let pair = PairStore::open(layout, bounds).unwrap();
    assert_eq!(
        pair.envelope.floor.guards.get(&guard.id),
        Some(&settlement),
        "complete next image must recover exact intended settlement; absent next must stay active"
    );
    drop(pair);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn live_root_substitution_refuses_before_touching_trusted_selection() {
    for side in ["data", "trusted"] {
        let root = root(&format!("substitute-{side}"));
        let data = root.join("data");
        let trusted = root.join("trusted");
        let (image, floor, bounds) = initial();
        let layout = DiskLayout::provision(&data, &trusted, [6; 32]).unwrap();
        let mut pair = PairStore::create(layout, image, floor, bounds).unwrap();
        let before = floors::read(&trusted.join("replica.floor"), 65536).unwrap();
        let target = if side == "data" { &data } else { &trusted };
        let displaced = root.join("displaced");
        fs::rename(target, &displaced).unwrap();
        fs::create_dir(target).unwrap();
        for entry in fs::read_dir(&displaced).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
        let mut next = pair.image.clone();
        next.generation = 1;
        next.predecessor = pair.envelope.floor.selected.digest;
        next.prior_floor = pair.envelope.floor.clone();
        assert_eq!(
            pair.transact(
                next,
                pair.envelope.floor.clone(),
                DiskOperation::Checkpoint,
                |_| Ok(())
            ),
            Err(Fault::Integrity)
        );
        assert_eq!(
            floors::read(&trusted.join("replica.floor"), 65536).unwrap(),
            before,
            "held root replacement must be rejected before any floor advance"
        );
        drop(pair);
        fs::remove_dir_all(root).unwrap();
    }
}
#[path = "tests/process.rs"]
mod process;
#[test]
fn failed_recovery_resync_keeps_original_pending_floor_and_does_not_select() {
    let root = root("sync-order");
    let data = root.join("data");
    let trusted = root.join("trusted");
    let (image, floor, bounds) = initial();
    let layout = DiskLayout::provision(&data, &trusted, [6; 32]).unwrap();
    let mut pair = PairStore::create(layout, image, floor, bounds.clone()).unwrap();
    let mut next = pair.image.clone();
    next.generation = 1;
    next.predecessor = pair.envelope.floor.selected.digest;
    next.prior_floor = pair.envelope.floor.clone();
    assert!(pair
        .transact(
            next,
            pair.envelope.floor.clone(),
            DiskOperation::Checkpoint,
            |cut| {
                if cut == PairCut::Selection(FloorCut::BeforeTemp) {
                    return Err(Fault::Unavailable);
                }
                Ok(())
            }
        )
        .is_err());
    drop(pair);
    let before = floors::read(&trusted.join("replica.floor"), bounds.bytes)
        .unwrap()
        .digest();
    crate::test_support::children::settled();
    let layout = DiskLayout::open(&data, &trusted, [6; 32]).unwrap();
    let mut reached = false;
    let result = PairStore::open_with(layout, bounds.clone(), |cut| {
        if cut == PairCut::RecoveryBeforeSync {
            reached = true;
            return Err(Fault::Unavailable);
        }
        Ok(())
    });
    assert!(
        matches!(result, Err(Fault::Unavailable)),
        "sync uncertainty cannot finalize recovered selection"
    );
    assert!(reached);
    assert_eq!(
        floors::read(&trusted.join("replica.floor"), bounds.bytes)
            .unwrap()
            .digest(),
        before
    );
    crate::test_support::children::settled();
    let layout = DiskLayout::open(&data, &trusted, [6; 32]).unwrap();
    let mut order = vec![];
    let pair = PairStore::open_with(layout, bounds, |cut| {
        order.push(cut);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        order,
        vec![PairCut::RecoveryBeforeSync, PairCut::RecoveryFileSynced]
    );
    assert_eq!(pair.image.generation, 1);
    drop(pair);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn malformed_next_image_is_refused_before_any_intent_or_slot_write() {
    let root = root("invalid-next");
    let data = root.join("data");
    let trusted = root.join("trusted");
    let (image, floor, bounds) = initial();
    let layout = DiskLayout::provision(&data, &trusted, [6; 32]).unwrap();
    let mut pair = PairStore::create(layout, image, floor, bounds).unwrap();
    let before = floors::read(&trusted.join("replica.floor"), 65536)
        .unwrap()
        .digest();
    let mut next = pair.image.clone();
    next.version = 2;
    next.generation = 1;
    next.predecessor = pair.envelope.floor.selected.digest;
    next.prior_floor = pair.envelope.floor.clone();
    assert_eq!(
        pair.transact(
            next,
            pair.envelope.floor.clone(),
            DiskOperation::Checkpoint,
            |_| Ok(())
        ),
        Err(Fault::Integrity)
    );
    assert_eq!(
        floors::read(&trusted.join("replica.floor"), 65536)
            .unwrap()
            .digest(),
        before,
        "invalid next envelope cannot leave a durable pending intent"
    );
    assert!(!pair.uncertain());
    drop(pair);
    fs::remove_dir_all(root).unwrap();
}

#[allow(dead_code)]
#[path = "tests/legacy_v1.rs"]
mod legacy_v1;
#[test]
fn closed_observation_marker_format_refuses_previous_independent_reader_and_preserves_unrelated_publications(
) {
    let root = root("marker-closed-format");
    let (image, floor, bounds) = initial();
    let layout = DiskLayout::provision(&root.join("data"), &root.join("trusted"), [6; 32]).unwrap();
    let mut pair = PairStore::create(layout, image, floor, bounds.clone()).unwrap();
    let prior = legacy_v1::FloorEnvelope {
        floor: pair.envelope.floor.clone(),
        pending: None,
        completion: None,
        unmaterialized: vec![],
    };
    let old = encode(&prior, &bounds).unwrap();
    assert!(
        decode::<FloorEnvelope>(old.value(), &bounds).is_err(),
        "v1 is refused rather than silently migrated"
    );
    pair.observation_marker(true, |_| Ok(())).unwrap();
    let current = encode(&pair.envelope, &bounds).unwrap();
    assert!(
        decode::<legacy_v1::FloorEnvelope>(current.value(), &bounds).is_err(),
        "byte-exact 9dbc v1 decoder refuses the unsupported protected v2 floor"
    );
    let projected = pair.envelope.floor.clone();
    pair.floor_only(projected, |_| Ok(())).unwrap();
    assert!(pair.envelope.observing);
    let mut next = pair.image.clone();
    next.generation += 1;
    next.predecessor = pair.envelope.floor.selected.digest;
    next.prior_floor = pair.envelope.floor.clone();
    let projected = pair.envelope.floor.clone();
    pair.transact(next, projected, DiskOperation::Checkpoint, |_| Ok(()))
        .unwrap();
    assert!(
        pair.envelope.observing,
        "unrelated checkpoint cannot clear unknown observation authority"
    );
    drop(pair);
    crate::test_support::children::settled();
    let layout = DiskLayout::open(&root.join("data"), &root.join("trusted"), [6; 32]).unwrap();
    assert!(matches!(
        PairStore::open(layout, bounds),
        Err(Fault::Unavailable)
    ));
    std::fs::remove_dir_all(root).unwrap();
}
