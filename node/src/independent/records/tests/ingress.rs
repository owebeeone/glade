use super::*;
pub(super) fn incoming(session: &mut DiskSession<Source>) -> ReceiveGuard {
    let LoadResult::Validated(value) = session.load() else {
        panic!("genuine current image");
    };
    let floor = value.authoritative_floor;
    ReceiveGuard {
        id: GuardId {
            store: floor.owner.store.clone(),
            instance: value.image.instance,
            number: floor.guard_high_water,
        },
        owner: floor.owner,
        lease: floor.lease,
        source: crate::signing::public_key(&NODE),
        channel: [77; 32],
        role: "local-qualification".into(),
        base_generation: floor.selected.generation,
        base_digest: floor.selected.digest,
        max_bytes: 8192,
        max_items: 16,
        deadline_ms: 1000,
    }
}
#[test]
fn unknown_abandon_retains_unretired_owned_permit_until_durable_selection() {
    let f = Fixture::new();
    let root = root("unknown-abandon");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    let permit = session.begin_ingress(guard.clone()).unwrap();
    session.observe_cut = Box::new(|_, _| Err(Fault::Unavailable));
    assert_eq!(session.abandon_ingress(permit), PersistResult::Unknown);
    assert!(
        !session.authority.can_close(),
        "unknown physical result must not retire utility capability"
    );
    let super::super::persist::PendingIngress::Abandon { permit } =
        &session.pending_ingress[&guard.id]
    else {
        panic!("exact owned unknown continuation");
    };
    assert_eq!(permit.guard(), &guard);
    assert_eq!(session.authority.can_abandon(permit), Ok(()));
    assert_eq!(
        session.retry_ingress(&guard.id),
        PersistResult::Refused(Fault::Unavailable)
    );
    assert!(session.pending_ingress.contains_key(&guard.id));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let ReconstructResult::Cut(cut) =
        session.reconstruct_cut(session.reconstruction_request().unwrap())
    else {
        panic!("exact retained cut");
    };
    assert!(!cut.complete_local);
    assert_eq!(cut.guards, vec![guard]);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_selected_permanent_ingress_loss_is_validated_but_never_complete() {
    let f = Fixture::new();
    let root = root("selected-ingress-loss");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    let _permit = session.begin_ingress(guard.clone()).unwrap();
    let mut image = session.checkpoint_template().unwrap().image;
    let marker = encode(
        &(
            "glade/ic3/ingress-loss/v1".to_owned(),
            (
                guard.clone(),
                (image.generation, Bytes::new([200; 32].to_vec())),
            ),
        ),
        &image.limits.decode,
    )
    .unwrap();
    image.permanent_loss.push(marker.clone());
    let mut floor = session.pair.as_ref().unwrap().envelope.floor.clone();
    floor.guards.insert(
        guard.id.clone(),
        GuardStatus::Loss {
            guard: guard.clone(),
            loss: marker.digest(),
            image_generation: image.generation,
        },
    );
    session
        .pair
        .as_mut()
        .unwrap()
        .transact(image, floor, DiskOperation::SettleIngress, |_| Ok(()))
        .unwrap();
    assert!(
        matches!(session.load(), LoadResult::Validated(_)),
        "exact durable conservative loss is usable for local append, not corruption"
    );
    let ReconstructResult::Cut(cut) =
        session.reconstruct_cut(session.reconstruction_request().unwrap())
    else {
        panic!("honest lost cut");
    };
    assert!(!cut.complete_local);
    assert!(cut.permanent_loss);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn drained_partial_receive_can_retain_loss_and_close_without_false_complete_on_restart() {
    let f = Fixture::new();
    let root = root("drained-partial-loss");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    let permit = session.begin_ingress(guard.clone()).unwrap();
    let receive = session
        .receive_ingress(permit, || async {
            Ok(Bytes::new(b"partial invalid header".to_vec()))
        })
        .unwrap();
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    let (permit, observed) = receive.await;
    assert_eq!(session.authority.can_settle(&permit), Ok(()));
    assert!(matches!(
        session.retain_ingress_loss(permit, observed.unwrap()),
        PersistResult::Committed { .. }
    ));
    assert!(session.authority.can_close());
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let ReconstructResult::Cut(cut) =
        session.reconstruct_cut(session.reconstruction_request().unwrap())
    else {
        panic!("reopened permanent loss");
    };
    assert!(!cut.complete_local);
    assert!(cut.permanent_loss);
    assert!(cut.guards.is_empty());
    let next = incoming(&mut session);
    let permit = session.begin_ingress(next).unwrap();
    assert!(matches!(
        session.abandon_ingress(permit),
        PersistResult::Committed { .. }
    ));
    let ReconstructResult::Cut(cut) =
        session.reconstruct_cut(session.reconstruction_request().unwrap())
    else {
        panic!("later exact untouched round");
    };
    assert!(cut.permanent_loss);
    assert!(
        !cut.complete_local,
        "later rounds cannot erase old unknown frontier"
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_guard_establishment_never_returns_a_consumable_owned_permit() {
    let f = Fixture::new();
    let root = root("failed-guard-write");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    session.observe_cut = Box::new(|operation, _| {
        if operation == DiskOperation::BeginIngress {
            Err(Fault::Unavailable)
        } else {
            Ok(())
        }
    });
    assert!(
        matches!(session.begin_ingress(guard), Err(Fault::Unavailable)),
        "a failed real guard write supplies no consume capability"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn unknown_settlement_keeps_exact_owned_observation_and_failed_loss_write_keeps_floor_guard()
{
    let f = Fixture::new();
    let root = root("unknown-observation-and-loss");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    let observation = super::ingress_cuts::empty(&f, &guard);
    let permit = session.begin_ingress(guard.clone()).unwrap();
    let receive = session
        .receive_ingress(permit, || std::future::ready(Ok(observation.bytes.clone())))
        .unwrap();
    let (permit, _) = receive.await;
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.observations.push(observation.clone());
    checkpoint.image.watermark = 1;
    let expected = checkpoint.clone();
    session.observe_cut = Box::new(|_, _| Err(Fault::Unavailable));
    assert_eq!(
        session.settle_ingress(permit, observation.clone(), checkpoint),
        PersistResult::Unknown
    );
    assert!(!session.authority.can_close());
    let super::super::persist::PendingIngress::Settle {
        permit,
        observation: kept,
        checkpoint,
    } = session.pending_ingress.remove(&guard.id).unwrap()
    else {
        panic!("exact owned settlement continuation");
    };
    assert_eq!(*kept, observation);
    assert_eq!(*checkpoint, expected);
    assert_eq!(session.authority.can_settle(&permit), Ok(()));
    assert_eq!(
        session.retain_ingress_loss(permit, kept.bytes),
        PersistResult::Refused(Fault::Unavailable)
    );
    assert!(matches!(
        session.pending_ingress.get(&guard.id),
        Some(super::super::persist::PendingIngress::Loss { .. })
    ));
    assert!(!session.authority.can_close());
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let ReconstructResult::Cut(value) =
        session.reconstruct_cut(session.reconstruction_request().unwrap())
    else {
        panic!("both failed writes cannot erase earlier receive");
    };
    assert!(!value.complete_local);
    assert_eq!(value.guards, vec![guard]);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn record_observation_resumes_only_exact_owned_drained_refused_continuation() {
    let f = Fixture::new();
    let root = root("record-owned-observation");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = incoming(&mut session);
    let observation = super::ingress_cuts::empty(&f, &guard);
    let permit = session.begin_ingress(guard).unwrap();
    let receive = session
        .receive_ingress(permit, || std::future::ready(Ok(observation.bytes.clone())))
        .unwrap();
    let (permit, _) = receive.await;
    let wrong = session.checkpoint_template().unwrap();
    assert_eq!(
        session.settle_ingress(permit, observation.clone(), wrong),
        PersistResult::Refused(Fault::BindingMismatch)
    );
    let mut correct = session.checkpoint_template().unwrap();
    correct.image.observations.push(observation.clone());
    correct.image.watermark = 1;
    assert!(matches!(session.record_observation(observation.clone(), correct), PersistResult::Committed { .. }), "the exact retained owned continuation can settle after correcting a pre-I/O checkpoint refusal");
    assert!(session.authority.can_close());
    assert!(session.pending_ingress.is_empty());
    let before = session.pair.as_ref().unwrap().image.clone();
    let checkpoint = session.checkpoint_template().unwrap();
    assert!(matches!(
        session.record_observation(observation.clone(), checkpoint),
        PersistResult::Committed { .. }
    ));
    assert_eq!(
        session.pair.as_ref().unwrap().image.observations,
        before.observations,
        "exact repeated observation never duplicates history"
    );
    let mut foreign = observation;
    foreign.bytes = Bytes::new(vec![255]);
    let checkpoint = session.checkpoint_template().unwrap();
    assert_eq!(
        session.record_observation(foreign, checkpoint),
        PersistResult::Refused(Fault::Ownership)
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
