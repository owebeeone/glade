use super::ingress::incoming;
use super::*;
#[tokio::test]
async fn owned_cancel_is_joined_before_loss_close_and_x_cannot_block_independent_y() {
    let x = ic3_disk_fixture::disk_profile_named("X");
    let y = ic3_disk_fixture::disk_profile_named("Y");
    let root_x = root("hung-X");
    let root_y = root("free-Y");
    let mut host_x = DiskHost::new(evidence(&x), config(&root_x, true));
    let mut session_x = host_x.open_replica(request(&x, [0; 32])).unwrap();
    let guard = incoming(&mut session_x);
    let permit = session_x.begin_ingress(guard.clone()).unwrap();
    let (cancel, wait) = tokio::sync::oneshot::channel();
    let receive = session_x
        .receive_ingress(permit, || async move {
            wait.await.unwrap_or(Err(Fault::Unavailable))
        })
        .unwrap();
    let task = tokio::spawn(receive);
    assert_eq!(
        ReplicaRecoverySession::close(&mut session_x),
        CloseResult::Pending
    );
    assert!(!task.is_finished());
    let floor_x = std::fs::read(root_x.join("trusted/replica.floor")).unwrap();
    let mut blocked = DiskHost::new(evidence(&x), config(&root_x, false));
    assert!(
        blocked
            .open_replica(request(&x, Bytes::new(floor_x.clone()).digest()))
            .is_err(),
        "Pending close retains exact X root ownership"
    );
    let mut request_y = request(&y, [0; 32]);
    request_y.owner.store = StoreIncarnation([174; 32]);
    request_y.plan_namespace = [175; 32];
    request_y.invocation_namespace = [176; 32];
    let mut roots_y = config(&root_y, true);
    roots_y.registration = [173; 32];
    let mut host_y = DiskHost::new(evidence(&y), roots_y);
    let mut session_y = host_y.open_replica(request_y.clone()).unwrap();
    ic3_disk_fixture::local_authorization(&y, &mut session_y, 92);
    let mut candidate = Candidate {
        instance: y.query.instance.clone(),
        operation: y.query.operation.clone(),
        evidence: y.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    assert!(session_y
        .submit_local(candidate, [90; 32], [91; 32])
        .unwrap()
        .iter()
        .any(|outcome| matches!(outcome, Outcome::AcceptedLocal(_))));
    let LoadResult::Validated(loaded_y) = session_y.load() else {
        panic!("independent Y full selected state");
    };
    assert_eq!(loaded_y.authoritative_floor.owner, request_y.owner);
    assert_eq!(
        loaded_y.authoritative_floor.issuance.plan_namespace,
        request_y.plan_namespace
    );
    assert_eq!(
        loaded_y.authoritative_floor.issuance.invocation_namespace,
        request_y.invocation_namespace
    );
    assert_eq!(
        std::fs::read(root_x.join("trusted/replica.floor")).unwrap(),
        floor_x,
        "Y never needs X's hung worker/floor CAS"
    );
    cancel.send(Err(Fault::Unavailable)).unwrap();
    let (permit, result) = task
        .await
        .expect("node owner joins actual read task before classification");
    assert_eq!(result, Err(Fault::Unavailable));
    assert_eq!(session_x.authority.can_settle(&permit), Ok(()));
    assert!(matches!(
        session_x.retain_ingress_loss(permit, Bytes::new(vec![])),
        PersistResult::Committed { .. }
    ));
    let ReconstructResult::Cut(cut) =
        session_x.reconstruct_cut(session_x.reconstruction_request().unwrap())
    else {
        panic!("joined unknown read remains conservative");
    };
    assert!(!cut.complete_local);
    assert!(cut.permanent_loss);
    assert_eq!(
        ReplicaRecoverySession::close(&mut session_x),
        CloseResult::Closed
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session_y),
        CloseResult::Closed
    );
    drop(session_x);
    drop(session_y);
    std::fs::remove_dir_all(root_x).unwrap();
    std::fs::remove_dir_all(root_y).unwrap();
}
