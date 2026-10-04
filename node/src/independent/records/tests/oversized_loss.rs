//! Inspect retained continuation without using private extraction as the public closure.
use super::*;
#[tokio::test]
async fn failed_oversized_loss_publication_preserves_exact_original_owned_continuation() {
    let f = Fixture::new();
    let root = root("rem2-exact-loss-custody");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let g = super::ingress::incoming(&mut session);
    let bytes = Bytes::new(vec![81; g.max_bytes + 1]);
    let permit = session.begin_ingress(g.clone()).unwrap();
    let (permit, _) = session
        .receive_ingress(permit, || async { Ok(bytes.clone()) })
        .unwrap()
        .await;
    let observation = Observation {
        id: 1,
        instance: g.id.instance.clone(),
        source: g.source,
        channel: g.channel,
        guard: g.id.clone(),
        bytes: bytes.clone(),
        expected: vec![],
    };
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.observations.push(observation.clone());
    checkpoint.image.watermark = 1;
    assert_eq!(
        session.settle_ingress(permit, observation.clone(), checkpoint.clone()),
        PersistResult::Refused(Fault::Capacity)
    );
    std::fs::create_dir(root.join("trusted/replica.next")).unwrap();
    assert_eq!(
        session.retain_pending_ingress_loss(&g.id),
        PersistResult::Unknown
    );
    let Some(super::super::persist::PendingIngress::Loss {
        permit,
        observed,
        settlement: Some((original, request)),
    }) = session.pending_ingress.get(&g.id)
    else {
        panic!("failed publication keeps exact owned continuation");
    };
    assert_eq!(**original, observation);
    assert_eq!(**request, checkpoint);
    assert_eq!(observed.value(), bytes.digest());
    assert!(permit.was_started());
    assert_eq!(permit.received_digest(), None);
    assert!(session.authority.can_settle(permit).is_ok());
    assert_eq!(
        session
            .pair
            .as_ref()
            .unwrap()
            .envelope
            .floor
            .guards
            .get(&g.id),
        Some(&GuardStatus::Active(g))
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
