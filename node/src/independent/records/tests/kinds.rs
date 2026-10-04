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
        retained_prepare_replays(&mut session, &loaded.image);
        let before = loaded.image.state.clone();
        assert_eq!(
            ReplicaRecoverySession::close(&mut session),
            CloseResult::Closed
        );
        drop(session);
        let digest =
            Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
        crate::test_support::children::settled();
        let mut host = DiskHost::new(evidence(&f), config(&root, false));
        let mut session = host.open_replica(request(&f, digest)).unwrap();
        let LoadResult::Validated(reopened) = session.load() else {
            panic!("reopened exact genuine kind {kind:?}");
        };
        assert_eq!(
            reopened.image.state, before,
            "once-only full kind restoration"
        );
        retained_prepare_replays(&mut session, &reopened.image);
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}
pub(super) fn retained_prepare_replays(session: &mut DiskSession<Source>, image: &ReplicaImage) {
    for original in image
        .state
        .storage_invocations
        .values()
        .chain(image.state.retired_storage_invocations.values())
    {
        let StorageInvocation::Prepare(original) = original else {
            continue;
        };
        let attempt = image
            .recovery
            .attempts
            .iter()
            .find(|attempt| attempt.attempt.binding.plan == original.binding.plan)
            .unwrap();
        for reply in [
            session.prepare(original.clone()),
            session.recover_plan(original.clone()),
        ] {
            assert!(
                reply.result == PrepareResult::Prepared(Box::new(attempt.attempt.clone())),
                "exact complete issued request retains original binding without new admission"
            );
        }
        let mut changed = original.clone();
        changed.binding.expected_revision = changed.binding.expected_revision.saturating_add(1);
        assert!(matches!(
            session.prepare(changed).result,
            PrepareResult::Refused(_)
        ));
    }
    let LoadResult::Validated(after) = session.load() else {
        panic!("same complete recovery")
    };
    assert_eq!(
        &after.image, image,
        "request lookup changes no counters, charges, receipts or phase"
    );
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

#[test]
fn each_genuine_kind_keeps_live_and_retired_prepare_authority_while_reserved() {
    for kind in [
        CommitKind::AcceptedBatch,
        CommitKind::Candidate,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
    ] {
        let f = ic3_disk_fixture::disk_profile();
        let root = root(&format!("prepare-live-{kind:?}"));
        let mut host = DiskHost::new(evidence(&f), config(&root, true));
        let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
        if kind == CommitKind::QualifiedFork {
            ic3_disk_fixture::local_authorization(&f, &mut session, 92);
            let mut first = Candidate {
                instance: f.query.instance.clone(),
                operation: f.query.operation.clone(),
                evidence: f.query.evidence.clone(),
            };
            first.evidence.admission = None;
            session.submit_local(first, [90; 32], [91; 32]).unwrap();
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
        let mut effects =
            super::super::runtime::event(&mut session, Event::OfferReplica(Box::new(candidate)))
                .unwrap();
        let mut found = false;
        for _ in 0..16 {
            let effect = effects
                .into_iter()
                .find(|effect| {
                    matches!(
                        effect,
                        Effect::Verify(_) | Effect::Seal(_) | Effect::Prepare { .. }
                    )
                })
                .expect("bounded signed core preparation path");
            let callback = match effect {
                Effect::Verify(query) | Effect::Seal(query) => {
                    super::super::authentication::begin(&mut session).unwrap();
                    let reply = glade_crdt_evidence_api::EvidencePort::verify(
                        &mut session.evidence,
                        glade_crdt_evidence_api::BoundQuery {
                            query,
                            identity: f.identity.clone(),
                        },
                    );
                    super::super::runtime::retain_observed(&mut session).unwrap();
                    Event::EvidenceResolved(Box::new(reply))
                }
                Effect::Prepare { request, .. } => {
                    let reply = session.prepare(request);
                    assert!(matches!(reply.result, PrepareResult::Prepared(_)));
                    let LoadResult::Validated(live) = session.load() else {
                        panic!("live exact original request")
                    };
                    retained_prepare_replays(&mut session, &live.image);
                    assert!(matches!(
                        live.image.recovery.attempts.last().unwrap().phase,
                        AttemptPhase::Reserved
                    ));
                    super::super::runtime::event(&mut session, Event::PrepareResolved(reply))
                        .unwrap();
                    let LoadResult::Validated(retired) = session.load() else {
                        panic!("retired exact request before Begin")
                    };
                    retained_prepare_replays(&mut session, &retired.image);
                    assert!(matches!(
                        retired.image.recovery.attempts.last().unwrap().phase,
                        AttemptPhase::Reserved
                    ));
                    found = true;
                    break;
                }
                _ => unreachable!(),
            };
            effects = super::super::runtime::event(&mut session, callback).unwrap();
        }
        assert!(found);
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}
