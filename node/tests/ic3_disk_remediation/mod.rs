//! External public consumers for the merged physical-host review counterexamples.
use super::*;
fn inventory(f: &Fixture, guard: &ReceiveGuard, entries: Vec<RecordRef>) -> Bytes {
    let inventory = Inventory {
        instance: f.descriptor.instance.clone(),
        source: guard.source,
        number: 1,
        entries,
        total_bytes: 0,
    };
    let header = signed(
        &NODE,
        &InventoryHeaderBody {
            instance: inventory.instance.clone(),
            identity: f.identity.clone(),
            source: inventory.source,
            target: glade_node::signing::public_key(&NODE),
            channel: guard.channel,
            nonce: [78; 32],
            number: 1,
            pages: usize::from(!inventory.entries.is_empty()),
            entries: inventory.entries.len(),
            total_bytes: 0,
            manifest: encode_profile(&inventory, &limits()).unwrap().digest(),
        },
    );
    encode(&(header, inventory), &limits()).unwrap()
}
fn reference() -> RecordRef {
    RecordRef {
        kind: RecordKind::Candidate,
        origin: "remote".into(),
        seq: 0,
        operation: [81; 32],
        bundle: [82; 32],
        receipt: None,
    }
}
async fn substitute(label: &str, received: Result<Bytes, Fault>, nonempty: bool) {
    let f = Fixture::new();
    let root = root(label);
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated")
    };
    let incoming = guard(&loaded);
    let received = if nonempty {
        Ok(inventory(&f, &incoming, vec![reference()]))
    } else {
        received
    };
    let empty = inventory(&f, &incoming, vec![]);
    let permit = session.begin_ingress(incoming.clone()).unwrap();
    let (permit, _) = session
        .receive_ingress(permit, || async { received })
        .unwrap()
        .await;
    let observation = Observation {
        id: 1,
        instance: incoming.id.instance.clone(),
        source: incoming.source,
        channel: incoming.channel,
        guard: incoming.id.clone(),
        bytes: empty,
        expected: vec![],
    };
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.observations.push(observation.clone());
    checkpoint.image.watermark = 1;
    assert!(
        matches!(
            session.settle_ingress(permit, observation, checkpoint),
            PersistResult::Refused(_)
        ),
        "normal settlement cannot substitute independently signed empty data for the received result"
    );
    let cut = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
        panic!("cut")
    };
    assert!(!cut.complete_local);
    assert_eq!(cut.guards, vec![incoming]);
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn signed_nonempty_cannot_be_replaced_by_different_signed_empty() {
    substitute("r1-nonempty", Ok(Bytes::new(vec![])), true).await;
}
#[tokio::test]
async fn malformed_prefix_cannot_be_replaced_by_signed_empty() {
    substitute(
        "r1-malformed",
        Ok(Bytes::new(b"partial invalid header".to_vec())),
        false,
    )
    .await;
}
#[tokio::test]
async fn receive_error_cannot_be_replaced_by_signed_empty() {
    substitute("r1-error", Err(Fault::Unavailable), false).await;
}
#[tokio::test]
async fn exact_nonempty_inventory_settles_and_retains_exact_missing_obligation() {
    let f = Fixture::new();
    let root = root("r1-positive");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated")
    };
    let incoming = guard(&loaded);
    let record = reference();
    let bytes = inventory(&f, &incoming, vec![record.clone()]);
    let permit = session.begin_ingress(incoming.clone()).unwrap();
    let (permit, _) = session
        .receive_ingress(permit, || async { Ok(bytes.clone()) })
        .unwrap()
        .await;
    let observation = Observation {
        id: 1,
        instance: incoming.id.instance.clone(),
        source: incoming.source,
        channel: incoming.channel,
        guard: incoming.id.clone(),
        bytes,
        expected: vec![record.clone()],
    };
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.observations.push(observation.clone());
    checkpoint.image.obligations.push(Obligation {
        observation: 1,
        expected: record,
        classification: Classification::Missing,
    });
    checkpoint.image.watermark = 1;
    assert!(matches!(
        session.settle_ingress(permit, observation.clone(), checkpoint),
        PersistResult::Committed { .. }
    ));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("exact retained snapshot")
    };
    assert_eq!(loaded.image.observations, vec![observation]);
    assert_eq!(loaded.image.obligations.len(), 1);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_retired_prepare_replay_recovers_original_attempt_before_and_after_reopen() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("r3-public");
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
    let LoadResult::Validated(before) = session.load() else {
        panic!("validated")
    };
    let original = before
        .image
        .state
        .retired_storage_invocations
        .values()
        .find_map(|q| match q {
            StorageInvocation::Prepare(q) => Some(q.clone()),
            _ => None,
        })
        .unwrap();
    let attempt = before.image.recovery.attempts[0].attempt.clone();
    for reply in [
        session.prepare(original.clone()),
        session.recover_plan(original.clone()),
    ] {
        assert_eq!(
            reply.result,
            PrepareResult::Prepared(Box::new(attempt.clone())),
            "retired invocation retains complete original prepare recovery authority"
        );
    }
    let LoadResult::Validated(after) = session.load() else {
        panic!("unchanged")
    };
    assert_eq!(after.image, before.image);
    assert_eq!(after.authoritative_floor, before.authoritative_floor);
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    assert_eq!(
        session.recover_plan(original.clone()).result,
        PrepareResult::Prepared(Box::new(attempt))
    );
    let mut changed = original.clone();
    changed.binding.expected_revision += 1;
    assert!(matches!(
        session.prepare(changed).result,
        PrepareResult::Refused(_)
    ));
    let mut unissued = original;
    unissued.invocation.number += 100;
    assert!(matches!(
        session.recover_plan(unissued).result,
        PrepareResult::Refused(_)
    ));
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
mod loss;

mod observations;
