use super::*;
#[test]
fn actual_genuine_local_commit_persists_owned_callback_history_and_exact_original_receipt() {
    let f = ic3_disk_fixture::disk_profile_named("fresh-ic3");
    let root = root("commit");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    let outcomes = session
        .submit_local(candidate.clone(), [90; 32], [91; 32])
        .expect("actual genuine bounded durable admission path");
    let receipt = outcomes
        .into_iter()
        .find_map(|outcome| match outcome {
            Outcome::AcceptedLocal(receipt) => Some(receipt),
            _ => None,
        })
        .expect("only actual selected terminal can acknowledge admission");
    assert_eq!(receipt.storage, StorageClass::LocalProcessRestart);
    let LoadResult::Validated(before) = session.load() else {
        panic!("full cached State and Recovery validation")
    };
    assert_eq!(
        before.image.state.instances[&candidate.instance]
            .accepted
            .len(),
        1
    );
    assert_eq!(
        before.image.state.instances[&candidate.instance].revision,
        1
    );
    assert_eq!(before.image.recovery.attempts.len(), 1);
    assert!(matches!(
        before.image.recovery.attempts[0].phase,
        AttemptPhase::Terminal(TerminalOutcome::Committed { .. })
    ));
    assert!(!before.image.state.retired_storage_invocations.is_empty());
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 93);
    let outcomes = session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    assert!(outcomes.contains(&Outcome::ExactRetry(receipt.clone())));
    let LoadResult::Validated(after) = session.load() else {
        panic!("original terminal and receipt")
    };
    assert_eq!(after.image.recovery.attempts.len(), 1);
    assert_eq!(
        after.image.state.instances[&f.descriptor.instance].accepted[0].receipt,
        receipt
    );
    assert_eq!(
        after.image.state.instances[&f.descriptor.instance].used,
        before.image.state.instances[&f.descriptor.instance].used
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn actual_native_attempt_provider_recovers_and_returns_immutable_terminal_for_exact_request() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("native-provider");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    let RecoveryResult::Validated(recovery) = session.recover() else {
        panic!("actual physical provider must recover full native attempts");
    };
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("complete native image");
    };
    let original = loaded
        .image
        .state
        .retired_storage_invocations
        .values()
        .find_map(|invocation| match invocation {
            StorageInvocation::Resolve(request) if request.operation == OperationKind::Begin => {
                Some(request.clone())
            }
            _ => None,
        })
        .unwrap();
    let reply = session.begin(BeginRequest {
        request: original.clone(),
        current: LocalPrecondition {
            policy: [255; 32],
            time: TimeInterval {
                earliest_ms: 0,
                latest_ms: u64::MAX,
            },
        },
    });
    let ResolveResult::Terminal(terminal) = reply.result else {
        panic!("exact terminal remains retrievable after callback consumption");
    };
    assert_eq!(
        recovery.attempts[0].phase,
        AttemptPhase::Terminal(terminal.outcome)
    );
    let mut foreign = original;
    foreign.invocation.number += 100;
    assert_eq!(
        session.inspect(foreign).result,
        ResolveResult::Refused(Fault::CallbackMismatch)
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn actual_storage_attempt_host_narrow_open_preserves_replica_created_namespace_and_owner() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("narrow-host");
    let original = request(&f, [0; 32]);
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(original.clone()).unwrap();
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let narrow = OpenRequest {
        owner: original.owner.clone(),
        invocation_namespace: original.invocation_namespace,
        limits: original.limits.storage.clone(),
    };
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = StorageAttemptHost::open(&mut host, narrow.clone())
        .expect("actual narrow host must use the retained trusted namespace");
    let RecoveryResult::Validated(recovered) = session.recover() else {
        panic!("real native recovery consumer");
    };
    assert_eq!(recovered.owner, narrow.owner);
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("same combined scope");
    };
    assert_eq!(loaded.image.state.plan_namespace, original.plan_namespace);
    assert_eq!(
        StorageAttemptSession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let mut wrong = narrow;
    wrong.owner.generation += 1;
    let before = std::fs::read(root.join("trusted/replica.floor")).unwrap();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    assert!(StorageAttemptHost::open(&mut host, wrong).is_err());
    assert_eq!(
        std::fs::read(root.join("trusted/replica.floor")).unwrap(),
        before,
        "foreign owner cannot cause recovery selection or lease writes"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn trusted_floor_refuses_data_only_rollback_and_missing_floor_after_real_local_commit() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("rollback");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let prior: Vec<_> = (0..2)
        .map(|slot| std::fs::read(root.join(format!("data/image.{slot}"))).unwrap())
        .collect();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    assert!(session
        .submit_local(candidate, [90; 32], [91; 32])
        .unwrap()
        .iter()
        .any(|x| matches!(x, Outcome::AcceptedLocal(_))));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let floor = std::fs::read(root.join("trusted/replica.floor")).unwrap();
    let digest = Bytes::new(floor.clone()).digest();
    for (slot, bytes) in prior.into_iter().enumerate() {
        std::fs::write(root.join(format!("data/image.{slot}")), bytes).unwrap();
    }
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    assert!(matches!(
        host.open_replica(request(&f, digest)),
        Err(Fault::Integrity)
    ));
    assert_eq!(
        std::fs::read(root.join("trusted/replica.floor")).unwrap(),
        floor
    );
    std::fs::remove_file(root.join("trusted/replica.floor")).unwrap();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    assert!(host.open_replica(request(&f, digest)).is_err());
    assert!(
        !root.join("trusted/replica.floor").exists(),
        "no floor is minted on reopen"
    );
    std::fs::remove_dir_all(root).unwrap();
}
