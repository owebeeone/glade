use super::ingress::incoming;
use super::*;
#[test]
fn guard_history_and_critical_discharge_reservation_refuse_before_new_consumption() {
    let f = ic3_disk_fixture::disk_profile_with(|limits| {
        limits.guard_history = 1;
    });
    let root = root("guard-capacity");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    let permit = session.begin_ingress(guard).unwrap();
    assert!(matches!(
        session.abandon_ingress(permit),
        PersistResult::Committed { .. }
    ));
    let guard = incoming(&mut session);
    let before = std::fs::read(root.join("trusted/replica.floor")).unwrap();
    assert!(matches!(session.begin_ingress(guard), Err(Fault::Capacity)));
    assert!(std::fs::read(root.join("trusted/replica.floor")).unwrap() == before);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
    let f = ic3_disk_fixture::disk_profile_with(|limits| {
        limits.storage.critical_bytes = 1;
        limits.storage.critical_per_attempt = 1;
        limits.storage.attempts = 1;
    });
    let root = super::root("guard-no-critical-reserve");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    let before = std::fs::read(root.join("trusted/replica.floor")).unwrap();
    assert!(
        matches!(session.begin_ingress(guard), Err(Fault::Capacity)),
        "guard unknown/loss record must reserve finite critical slot space before receive"
    );
    assert!(std::fs::read(root.join("trusted/replica.floor")).unwrap() == before);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn admitted_guard_critical_loss_can_settle_when_ordinary_image_region_is_full() {
    let f = ic3_disk_fixture::disk_profile_with(|limits| {
        limits.image_bytes = 100_000;
        limits.storage.critical_per_attempt = 20_000;
        limits.storage.critical_bytes = 80_000;
    });
    let root = root("loss-reserved-region");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.generation = 1;
    policy.previous = Some(envelope(&f.policy).digest());
    let ceiling =
        session.request.limits.image_bytes - session.request.limits.storage.critical_bytes;
    let mut selected = None;
    for count in (150u64..600).step_by(4) {
        policy.revoked = (0..count)
            .map(|number| {
                let mut digest = [249; 32];
                digest[..8].copy_from_slice(&number.to_be_bytes());
                digest
            })
            .collect();
        let signed = signed(&ROOT, &policy);
        let mut next = session.checkpoint_template().unwrap().image;
        next.state
            .instances
            .get_mut(&f.query.instance)
            .unwrap()
            .policy = envelope(&signed);
        let size = encode(&next, &next.limits.decode).unwrap().value().len();
        if size <= ceiling {
            selected = Some(signed);
        } else {
            break;
        }
    }
    session.evidence.source.0.policy =
        selected.expect("bounded genuine signed current policy fills ordinary region");
    super::super::authentication::persist(&mut session).unwrap();
    let guard = incoming(&mut session);
    let permit = session
        .begin_ingress(guard)
        .expect("critical loss space is reserved before receive");
    let receive = session
        .receive_ingress(permit, || {
            std::future::ready(Ok(Bytes::new(b"partial".to_vec())))
        })
        .unwrap();
    let (permit, bytes) = receive.await;
    assert!(matches!(session.retain_ingress_loss(permit, bytes.unwrap()), PersistResult::Committed { .. }), "discharging a reserved receive must use its critical region even when ordinary admission has reached its ceiling");
    let ReconstructResult::Cut(value) =
        session.reconstruct_cut(session.reconstruction_request().unwrap())
    else {
        panic!("selected critical loss image");
    };
    assert!(value.permanent_loss);
    assert!(!value.complete_local);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
