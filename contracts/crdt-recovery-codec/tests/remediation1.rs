//! Exact review counterexamples: identity class and interpreted-operation limits.
use glade_crdt_recovery_codec::*;
use glade_wire::{
    cbor::{self, Cbor},
    generated::{Head, Op, Shape},
};
use std::collections::BTreeMap;
fn limits() -> DecodeLimits {
    DecodeLimits {
        bytes: 65536,
        items: 8192,
        depth: 64,
        name_bytes: 512,
    }
}
fn instance() -> Instance {
    Instance {
        descriptor: [2; 32],
        incarnation: "i".into(),
    }
}
fn plan() -> PlanKey {
    PlanKey {
        namespace: [1; 32],
        instance: instance(),
        number: 1,
    }
}
#[test]
fn concrete_container_roots_cannot_substitute_plan_for_guard() {
    let l = limits();
    let boxed = Box::new(plan());
    let b = encode(&boxed, &l).unwrap();
    assert_eq!(decode::<Box<PlanKey>>(b.value(), &l), Ok(boxed));
    assert_eq!(decode::<Box<GuardId>>(b.value(), &l), Err(Error::Invalid));
    let vector = vec![plan()];
    let b = encode(&vector, &l).unwrap();
    assert_eq!(decode::<Vec<PlanKey>>(b.value(), &l), Ok(vector));
    assert_eq!(decode::<Vec<GuardId>>(b.value(), &l), Err(Error::Invalid));
    let optional = Some(plan());
    let b = encode(&optional, &l).unwrap();
    assert_eq!(decode::<Option<PlanKey>>(b.value(), &l), Ok(optional));
    assert_eq!(
        decode::<Option<GuardId>>(b.value(), &l),
        Err(Error::Invalid)
    );
    let tuple = (plan(), plan());
    let b = encode(&tuple, &l).unwrap();
    assert_eq!(decode::<(PlanKey, PlanKey)>(b.value(), &l), Ok(tuple));
    assert_eq!(
        decode::<(GuardId, PlanKey)>(b.value(), &l),
        Err(Error::Invalid)
    );
    assert_eq!(
        decode::<(PlanKey, GuardId)>(b.value(), &l),
        Err(Error::Invalid)
    );
    let map = BTreeMap::from([(plan(), plan())]);
    let b = encode(&map, &l).unwrap();
    assert_eq!(decode::<BTreeMap<PlanKey, PlanKey>>(b.value(), &l), Ok(map));
    assert_eq!(
        decode::<BTreeMap<GuardId, PlanKey>>(b.value(), &l),
        Err(Error::Invalid)
    );
    assert_eq!(
        decode::<BTreeMap<PlanKey, GuardId>>(b.value(), &l),
        Err(Error::Invalid)
    );
    let deep = Some(vec![Box::new(plan())]);
    let b = encode(&deep, &l).unwrap();
    assert_eq!(decode::<Option<Vec<Box<PlanKey>>>>(b.value(), &l), Ok(deep));
    assert_eq!(
        decode::<Option<Vec<Box<GuardId>>>>(b.value(), &l),
        Err(Error::Invalid)
    );
}
fn operation(name: usize, refs: usize) -> Operation {
    Operation::new(Op {
        share: "s".into(),
        glade_id: "g".into(),
        key: vec![1],
        origin: "o".repeat(name),
        seq: 1,
        prev: None,
        lamport: 1,
        refs: (0..refs)
            .map(|n| Head {
                origin: "r".into(),
                seq: n as i64,
                hash: None,
            })
            .collect(),
        shape: Shape::Crdt,
        payload: vec![1],
    })
}
fn candidate(op: Operation) -> Candidate {
    Candidate {
        instance: instance(),
        operation: op,
        evidence: EvidenceSet {
            certificate: Bytes::new(vec![]),
            permit: Bytes::new(vec![]),
            policy: Bytes::new(vec![]),
            admission: None,
        },
    }
}
fn bundle(op: Operation) -> TransferBundle {
    TransferBundle {
        instance: instance(),
        candidate: candidate(op),
        kind: RecordKind::Candidate,
        receipt: None,
        fork: None,
    }
}
fn count(value: &Cbor) -> usize {
    1 + match value {
        Cbor::Array(v) => v.iter().map(count).sum(),
        Cbor::Map(v) => v.iter().map(|(_, v)| 1 + count(v)).sum(),
        _ => 0,
    }
}
#[test]
fn root_operation_name_limit_applies_inside_opaque_canonical_bytes() {
    let op = operation(33, 0);
    let bytes = encode(&op, &limits()).unwrap();
    let mut l = limits();
    l.name_bytes = 32;
    assert_eq!(decode::<Operation>(bytes.value(), &l), Err(Error::Capacity));
    l.name_bytes = 33;
    assert_eq!(decode::<Operation>(bytes.value(), &l), Ok(op));
}
#[test]
fn root_operation_item_budget_includes_interpreted_refs_cumulatively() {
    let op = operation(1, 9);
    let bytes = encode(&op, &limits()).unwrap();
    let mut l = limits();
    let inner = cbor::try_decode_with(op.canonical().value(), 64, None).unwrap();
    l.items = 3 + count(&inner) - 1;
    assert_eq!(decode::<Operation>(bytes.value(), &l), Err(Error::Capacity));
    l.items += 1;
    assert_eq!(decode::<Operation>(bytes.value(), &l), Ok(op));
}
#[test]
fn root_operation_depth_includes_the_embedded_interpretation_boundary() {
    let op = operation(1, 0);
    let bytes = encode(&op, &limits()).unwrap();
    let mut l = limits();
    l.depth = 3;
    assert_eq!(decode::<Operation>(bytes.value(), &l), Err(Error::Capacity));
    l.depth = 4;
    assert_eq!(decode::<Operation>(bytes.value(), &l), Ok(op));
}
#[test]
fn nested_candidate_and_remote_transfer_cannot_hide_operation_name() {
    let op = operation(129, 0);
    let value = candidate(op.clone());
    let bytes = encode(&value, &limits()).unwrap();
    let mut l = limits();
    l.name_bytes = 128;
    assert_eq!(decode::<Candidate>(bytes.value(), &l), Err(Error::Capacity));
    let value = bundle(op);
    let bytes = encode_profile(&value, &limits()).unwrap();
    assert_eq!(
        decode_profile::<TransferBundle>(bytes.value(), &l),
        Err(Error::Capacity)
    );
    l.name_bytes = 129;
    assert_eq!(
        decode_profile::<TransferBundle>(bytes.value(), &l),
        Ok(value)
    );
}
#[test]
fn repeated_nested_operations_share_one_budget_not_one_budget_each() {
    let ops = vec![candidate(operation(1, 8)), candidate(operation(1, 8))];
    let bytes = encode(&ops, &limits()).unwrap();
    let mut l = limits();
    // The released wrapper currently has a two-item identity tree after R1.
    let outer = cbor::try_decode_with(
        &bytes.value()["glade.crdt.recovery\0\x01".len()..],
        64,
        None,
    )
    .unwrap();
    let inner = cbor::try_decode_with(ops[0].operation.canonical().value(), 64, None).unwrap();
    l.items = count(&outer) + 2 * count(&inner) - 1;
    assert_eq!(
        decode::<Vec<Candidate>>(bytes.value(), &l),
        Err(Error::Capacity)
    );
    l.items += 1;
    assert_eq!(decode::<Vec<Candidate>>(bytes.value(), &l), Ok(ops));
}
