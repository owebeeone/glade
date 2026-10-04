//! Complete structural image/floor tests, not physical recovery proof.
use glade_crdt_recovery_codec::*;
mod image_fixture;
use image_fixture::{image, limits};
#[test]
fn full_image_retains_all_kernel_callbacks_attempts_guards_issuance_and_loss() {
    let image = image();
    let bytes = encode(&image, &limits()).unwrap();
    assert_eq!(
        decode::<ReplicaImage>(bytes.value(), &limits()).unwrap(),
        image
    );
}
#[test]
fn image_hash_and_external_selection_do_not_require_a_hash_fixed_point() {
    let image = image();
    let bytes = encode(&image, &limits()).unwrap();
    let mut current = image.prior_floor.clone();
    current.selected = Selection {
        slot: 1,
        generation: image.generation,
        digest: bytes.digest(),
    };
    let recovery = ReplicaRecovery {
        image: image.clone(),
        authoritative_floor: current.clone(),
    };
    assert_eq!(encode(&recovery.image, &limits()).unwrap(), bytes);
    assert_ne!(image.prior_floor.selected.digest, current.selected.digest);
    assert_eq!(
        decode::<ReplicaRecovery>(encode(&recovery, &limits()).unwrap().value(), &limits())
            .unwrap(),
        recovery
    );
}
#[test]
fn interrupted_intent_preserves_issuance_and_guard_floors_without_next_image() {
    let image = image();
    let mut floor = image.prior_floor;
    let mut next_issuance = floor.issuance.clone();
    next_issuance
        .origins
        .get_mut("o")
        .unwrap()
        .reserved_intents
        .push(Bytes::new(vec![41]));
    floor.intent = Some(FloorIntent {
        issuance: next_issuance.clone(),
        prior: floor.selected.clone(),
        next: Selection {
            slot: 1,
            generation: 1,
            digest: [42; 32],
        },
        next_attempt: u64::MAX,
        invocation_high_water: u64::MAX,
        guard_high_water: u64::MAX,
        guards: floor.guards.clone(),
    });
    let bytes = encode(&floor, &limits()).unwrap();
    let decoded = decode::<ReplicaFloor>(bytes.value(), &limits()).unwrap();
    assert_eq!(decoded.intent.unwrap().issuance, next_issuance);
    assert!(matches!(
        decoded.guards.values().next(),
        Some(GuardStatus::Active(_))
    ));
}
#[test]
fn corrupt_truncated_trailing_unknown_versions_and_capacity_fail_closed() {
    let bytes = encode(&image(), &limits()).unwrap();
    for end in 0..bytes.value().len() {
        assert!(decode::<ReplicaImage>(&bytes.value()[..end], &limits()).is_err());
    }
    let mut trailing = bytes.value().to_vec();
    trailing.push(0);
    assert_eq!(
        decode::<ReplicaImage>(&trailing, &limits()),
        Err(Error::Invalid)
    );
    let mut wrong = bytes.value().to_vec();
    wrong[20] = 2;
    assert!(decode::<ReplicaImage>(&wrong, &limits()).is_err());
    let mut small = limits();
    small.bytes = bytes.value().len() - 1;
    assert_eq!(
        decode::<ReplicaImage>(bytes.value(), &small),
        Err(Error::Capacity)
    );
    small = limits();
    small.items = 4;
    assert_eq!(
        decode::<ReplicaImage>(bytes.value(), &small),
        Err(Error::Capacity)
    );
    small = limits();
    small.depth = 2;
    assert_eq!(
        decode::<ReplicaImage>(bytes.value(), &small),
        Err(Error::Capacity)
    );
}
