//! Genuine native coupling controls for each original Pure commit kind.
use super::*;
pub(super) fn offered(f: &Fixture, payload: &[u8], seq: i64) -> Candidate {
    let mut op = f.query.operation.inner().clone();
    op.payload = payload.to_vec();
    op.seq = seq;
    op.lamport = seq;
    op.prev = if seq == 0 { None } else { Some(vec![248; 32]) };
    let operation = Operation::new(op);
    let evidence = Fixture::proofs(
        &f.query.instance,
        &f.identity,
        &f.descriptor,
        &f.certificate,
        &f.policy,
        &operation,
        Some((crate::signing::public_key(&NODE), &NODE)),
    );
    Candidate {
        instance: f.query.instance.clone(),
        operation,
        evidence,
    }
}
#[test]
fn all_four_genuine_native_kinds_couple_original_custody_and_reopen_projection() {
    for kind in [
        CommitKind::AcceptedBatch,
        CommitKind::Candidate,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
    ] {
        let f = ic3_disk_fixture::disk_profile();
        let root = root(&format!("kind-{kind:?}"));
        let mut host = DiskHost::new(evidence(&f), config(&root, true));
        let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
        if kind == CommitKind::QualifiedFork {
            ic3_disk_fixture::local_authorization(&f, &mut session, 92);
            let mut candidate = Candidate {
                instance: f.query.instance.clone(),
                operation: f.query.operation.clone(),
                evidence: f.query.evidence.clone(),
            };
            candidate.evidence.admission = None;
            session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
        }
        let candidate = match kind {
            CommitKind::AcceptedBatch => offered(
                &f,
                br#"{"after":null,"atom_id":"a","kind":"insert","text":"A"}"#,
                0,
            ),
            CommitKind::Candidate => offered(
                &f,
                br#"{"after":null,"atom_id":"b","kind":"insert","text":"B"}"#,
                1,
            ),
            CommitKind::SecurityEvidence => offered(&f, b"invalid payload", 0),
            CommitKind::QualifiedFork => offered(
                &f,
                br#"{"after":null,"atom_id":"b","kind":"insert","text":"B"}"#,
                0,
            ),
        };
        let effects = super::super::runtime::event(
            &mut session,
            Event::OfferReplica(Box::new(candidate.clone())),
        )
        .unwrap();
        let outcomes = super::super::runtime::drain(&mut session, effects).unwrap();
        let LoadResult::Validated(loaded) = session.load() else {
            panic!("full genuine kind {kind:?}: {outcomes:?}");
        };
        let retained = loaded.image.recovery.attempts.last().unwrap();
        assert_eq!(retained.attempt.binding.kind, kind);
        assert!(matches!(
            retained.phase,
            AttemptPhase::Terminal(TerminalOutcome::Committed { .. })
        ));
        let value = &loaded.image.state.instances[&f.query.instance];
        match kind {
            CommitKind::AcceptedBatch => {
                assert_eq!(value.accepted.len(), 1);
            }
            CommitKind::Candidate => {
                assert_eq!(value.candidates, vec![candidate]);
                assert!(!value.pending.is_empty());
            }
            CommitKind::SecurityEvidence => {
                assert_eq!(value.security_evidence, vec![candidate]);
                assert!(value.accepted.is_empty());
            }
            CommitKind::QualifiedFork => {
                assert_eq!(value.accepted.len(), 2);
                assert_eq!(value.qualified_forks.len(), 1);
                assert!(!value.quarantine.is_empty());
            }
        }
        let before = loaded.image.state.clone();
        assert_eq!(
            ReplicaRecoverySession::close(&mut session),
            CloseResult::Closed
        );
        drop(session);
        let digest =
            Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
        let mut host = DiskHost::new(evidence(&f), config(&root, false));
        let mut session = host.open_replica(request(&f, digest)).unwrap();
        let LoadResult::Validated(reopened) = session.load() else {
            panic!("reopened exact genuine kind {kind:?}");
        };
        assert_eq!(
            reopened.image.state, before,
            "once-only full kind restoration"
        );
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn genuine_already_accepted_fork_proof_keeps_original_zero_new_receipt_batch_shape() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("fork-proof-shape");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let first = offered(
        &f,
        br#"{"after":null,"atom_id":"a","kind":"insert","text":"A"}"#,
        0,
    );
    let second = offered(
        &f,
        br#"{"after":null,"atom_id":"b","kind":"insert","text":"B"}"#,
        0,
    );
    for candidate in [first.clone(), second.clone()] {
        let effects =
            super::super::runtime::event(&mut session, Event::OfferReplica(Box::new(candidate)))
                .unwrap();
        super::super::runtime::drain(&mut session, effects).unwrap();
    }
    let effects = super::super::runtime::event(
        &mut session,
        Event::OfferFork(Box::new(ForkEvidence { first, second })),
    )
    .unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("original full fork proof batch");
    };
    assert_eq!(
        loaded.image.recovery.attempts.len(),
        3,
        "already accepted two-sided proof still has a valid original core storage plan"
    );
    let attempt = loaded.image.recovery.attempts.last().unwrap();
    assert_eq!(attempt.attempt.binding.kind, CommitKind::QualifiedFork);
    assert!(attempt.attempt.binding.receipts.is_empty());
    assert!(matches!(
        attempt.phase,
        AttemptPhase::Terminal(TerminalOutcome::Committed { .. })
    ));
    assert_eq!(
        loaded.image.state.instances[&f.query.instance]
            .accepted
            .len(),
        2
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
