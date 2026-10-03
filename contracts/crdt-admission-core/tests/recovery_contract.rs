//! Regression consumers for invocation identity and non-admission atomic recovery.
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use glade_wire::generated::Head;
use support::*;

fn retention_plan(d: &Descriptor, kind: CommitKind) -> (State, CommitPlan) {
    let a = candidate(d, "writer-e0", 0, None, vec![], insert("a", None, "A"));
    let mut state = initial(d.clone());
    seed(&mut state, a.clone());
    let mut c = candidate(
        d,
        "writer-e0",
        1,
        Some(a.operation.canonical().digest().to_vec()),
        vec![],
        insert("b", Some("a"), "B"),
    );
    let mut plan = staged_plan(d, c.clone());
    plan.kind = kind.clone();
    if kind != CommitKind::AcceptedBatch {
        plan.local_precondition = None;
        plan.charges = Charges {
            edit: 0,
            recovery: 30,
            evidence: 20,
            pending: 10,
        };
        plan.acceptances.clear();
        if kind == CommitKind::Candidate || kind == CommitKind::SecurityEvidence {
            c.evidence.admission = None;
            if kind == CommitKind::SecurityEvidence {
                c = candidate(d, "writer-e0", 0, None, vec![], insert("x", None, "X"));
                c.evidence.admission = None;
            }
            if kind == CommitKind::Candidate {
                let mut op = c.operation.inner().clone();
                op.seq = 2;
                op.prev = Some(vec![9; 32]);
                c.operation = Operation::new(op);
            }
            plan.records = vec![c];
        } else {
            let rival = candidate(
                d,
                "writer-e0",
                1,
                Some(a.operation.canonical().digest().to_vec()),
                vec![],
                insert("x", Some("a"), "X"),
            );
            seed(&mut state, c.clone());
            seed(&mut state, rival.clone());
            plan.records = vec![c.clone(), rival.clone()];
            plan.qualified_forks = vec![ForkEvidence {
                first: c,
                second: rival,
            }];
        }
    }
    state.unknown_commits.insert(plan.id, plan.clone());
    state.next_effect_id = 40;
    (state, plan)
}
fn request(plan: &CommitPlan, lookup_id: u64) -> LookupRequest {
    LookupRequest {
        lookup_id,
        plan_id: plan.id,
        expected_revision: plan.expected_revision,
        instance: plan.instance.clone(),
        operation: plan.acceptances.first().map(|a| a.receipt.operation),
        batch_digest: plan.batch.digest(),
    }
}
fn reply(q: &LookupRequest, result: LookupResult) -> Event {
    Event::LookupResolved(LookupReply {
        lookup_id: q.lookup_id,
        plan_id: q.plan_id,
        expected_revision: q.expected_revision,
        instance: q.instance.clone(),
        operation: q.operation,
        batch_digest: q.batch_digest,
        result,
    })
}
fn committed(plan: &CommitPlan) -> LookupResult {
    LookupResult::Committed {
        batch: BatchCommitted {
            batch_digest: plan.batch.digest(),
            revision: plan.expected_revision + 1,
            storage: StorageClass::VolatileTest,
            receipts: plan.acceptances.iter().map(|a| a.receipt.clone()).collect(),
        },
    }
}
#[test]
fn repeated_unknown_lookup_retires_invocation_and_old_replies_preserve_new_lookup() {
    let d = descriptor("x");
    let (state, plan) = retention_plan(&d, CommitKind::AcceptedBatch);
    let l1 = step(&state, Event::ResumeRecovery(d.instance.clone()));
    let q1 = l1
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Lookup(q) = e {
                Some(q.clone())
            } else {
                None
            }
        })
        .expect("first lookup invocation");
    let unknown = step(&l1.state, reply(&q1, LookupResult::Unknown));
    assert!(
        !unknown.state.lookups.contains_key(&q1.lookup_id),
        "Unknown retires only invocation"
    );
    assert_eq!(unknown.state.unknown_commits.get(&plan.id), Some(&plan));
    let l2 = step(&unknown.state, Event::ResumeRecovery(d.instance.clone()));
    let q2 = l2
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Lookup(q) = e {
                Some(q.clone())
            } else {
                None
            }
        })
        .expect("fresh lookup invocation");
    assert!(q2.lookup_id > q1.lookup_id);
    assert_eq!(q2.plan_id, q1.plan_id);
    for result in [
        LookupResult::Unknown,
        LookupResult::KnownAbsent,
        committed(&plan),
    ] {
        let stale = step(&l2.state, reply(&q1, result));
        assert!(
            stale
                .effects
                .contains(&Effect::Report(Outcome::CallbackMismatch))
        );
        assert_eq!(
            stale.state, l2.state,
            "retired reply cannot consume L2 or reservation"
        );
    }
    let recovered = step(&l2.state, reply(&q2, committed(&plan)));
    assert!(
        recovered
            .effects
            .contains(&Effect::Report(Outcome::ExactRetry(
                plan.acceptances[0].receipt.clone()
            )))
    );
    assert!(
        !recovered
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Seal(_) | Effect::Commit(_)))
    );
}
#[test]
fn lookup_invocation_counter_exhaustion_refuses_without_wrap_or_reservation_loss() {
    let d = descriptor("x");
    let (mut state, plan) = retention_plan(&d, CommitKind::Candidate);
    state.next_effect_id = u64::MAX;
    let result = step(&state, Event::ResumeRecovery(d.instance));
    assert!(result.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert_eq!(result.state.unknown_commits.get(&plan.id), Some(&plan));
    assert_eq!(result.state.next_effect_id, u64::MAX);
    assert!(result.state.lookups.is_empty());
}
#[test]
fn every_atomic_kind_recovers_committed_absent_and_unknown_without_fake_admission() {
    for kind in [
        CommitKind::Candidate,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
        CommitKind::AcceptedBatch,
    ] {
        for outcome in 0..3 {
            let d = descriptor("x");
            let y = descriptor("y");
            let (mut state, plan) = retention_plan(&d, kind.clone());
            state.instances.extend(initial(y.clone()).instances);
            let old = state.instances[&d.instance].accepted.clone();
            state.unknown_commits.remove(&plan.id);
            state.commits.insert(plan.id, plan.clone());
            let uncertain = step(
                &state,
                Event::CommitResolved(CommitReply {
                    plan_id: plan.id,
                    instance: plan.instance.clone(),
                    batch_digest: plan.batch.digest(),
                    result: CommitResult::OutcomeUnknown,
                }),
            );
            assert_eq!(
                uncertain.state.unknown_commits.get(&plan.id),
                Some(&plan),
                "every kind retains its complete uncertain reservation"
            );
            assert!(!uncertain.state.commits.contains_key(&plan.id));
            state = uncertain.state;
            let q = request(&plan, 40);
            state.lookups.insert(q.lookup_id, q.clone());
            state.next_effect_id = 41;
            let resolution = match outcome {
                0 => committed(&plan),
                1 => LookupResult::KnownAbsent,
                _ => LookupResult::Unknown,
            };
            let result = step(&state, reply(&q, resolution));
            assert!(
                !result.state.lookups.contains_key(&q.lookup_id),
                "matching response retires invocation"
            );
            assert_eq!(
                result.state.instances[&y.instance],
                state.instances[&y.instance]
            );
            if outcome == 2 {
                assert_eq!(result.state.unknown_commits.get(&plan.id), Some(&plan));
                assert_eq!(
                    result.state.instances[&d.instance],
                    state.instances[&d.instance]
                );
            } else {
                assert!(
                    !result.state.unknown_commits.contains_key(&plan.id),
                    "resolved reservation released"
                );
                if outcome == 1 {
                    assert!(!result.effects.iter().any(|e| matches!(
                        e,
                        Effect::Report(
                            Outcome::AcceptedLocal(_)
                                | Outcome::ExactRetry(_)
                                | Outcome::BatchRetained { .. }
                        )
                    )));
                    assert_eq!(
                        result.state.instances[&d.instance], state.instances[&d.instance],
                        "absence cannot delete old custody"
                    );
                } else {
                    let value = &result.state.instances[&d.instance];
                    assert_eq!(value.revision, 1);
                    assert_eq!(
                        value.used, plan.charges,
                        "committed charges installed exactly once"
                    );
                    for accepted in &old {
                        assert!(value.accepted.contains(accepted));
                    }
                    if kind == CommitKind::AcceptedBatch {
                        assert!(result.effects.contains(&Effect::Report(Outcome::ExactRetry(
                            plan.acceptances[0].receipt.clone()
                        ))));
                        assert_eq!(
                            value.accepted.last().unwrap().candidate,
                            plan.acceptances[0].candidate
                        );
                    } else {
                        assert_eq!(
                            value.accepted, old,
                            "retention-only plans invent no accepted record"
                        );
                        assert!(!result.effects.iter().any(|e| matches!(
                            e,
                            Effect::Report(Outcome::AcceptedLocal(_) | Outcome::ExactRetry(_))
                        )));
                        assert!(
                            result
                                .effects
                                .contains(&Effect::Report(Outcome::BatchRetained {
                                    instance: d.instance.clone(),
                                    kind: kind.clone(),
                                    batch_digest: plan.batch.digest(),
                                    revision: 1,
                                    storage: StorageClass::VolatileTest
                                }))
                        );
                        match kind {
                            CommitKind::Candidate => {
                                assert!(value.candidates.contains(&plan.records[0]));
                                assert!(
                                    value
                                        .pending
                                        .iter()
                                        .any(|s| s.origin == "writer-e0" && s.seq == 1)
                                );
                                let read = cut(&result.state, d.instance.clone());
                                assert_eq!(read.contiguous_common_heads["writer-e0"].0, 0);
                                assert!(
                                    !read.complete_local,
                                    "candidate gap prevents completeness"
                                );
                            }
                            CommitKind::SecurityEvidence => {
                                assert!(value.security_evidence.contains(&plan.records[0]));
                                assert!(value.quarantine.is_empty());
                                let read = cut(&result.state, d.instance.clone());
                                assert_eq!(
                                    read.eligible,
                                    vec![old[0].candidate.operation.canonical().digest()]
                                );
                                assert_eq!(read.contiguous_common_heads["writer-e0"].0, 0);
                            }
                            CommitKind::QualifiedFork => {
                                assert_eq!(value.qualified_forks, plan.qualified_forks);
                                let read = cut(&result.state, d.instance.clone());
                                assert_eq!(read.quarantine["writer-e0"], 1);
                                assert_eq!(read.contiguous_common_heads["writer-e0"].0, 0);
                            }
                            CommitKind::AcceptedBatch => unreachable!(),
                        }
                    }
                    let duplicate = step(&result.state, reply(&q, committed(&plan)));
                    assert!(
                        duplicate
                            .effects
                            .contains(&Effect::Report(Outcome::CallbackMismatch))
                    );
                    assert_eq!(
                        duplicate.state, result.state,
                        "duplicate cannot double-charge"
                    );
                }
            }
            let allowed = candidate(&y, "writer-e0", 0, None, vec![], insert("y", None, "Y"));
            let independent = drive(
                result.state,
                Event::SubmitLocal(Box::new(allowed.clone())),
                facts(&allowed),
            );
            assert!(
                independent
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_)))),
                "uncertain X cannot stall Y"
            );
        }
    }
}
#[test]
fn retention_committed_lookup_binds_batch_storage_revision_and_empty_receipts() {
    let d = descriptor("x");
    let (mut state, plan) = retention_plan(&d, CommitKind::SecurityEvidence);
    let q = request(&plan, 40);
    state.lookups.insert(q.lookup_id, q.clone());
    for mutation in 0..4 {
        let mut batch = BatchCommitted {
            batch_digest: plan.batch.digest(),
            revision: 1,
            storage: StorageClass::VolatileTest,
            receipts: vec![],
        };
        match mutation {
            0 => batch.batch_digest = [9; 32],
            1 => batch.revision = 0,
            2 => batch.storage = StorageClass::LocalProcessRestart,
            _ => {
                batch.receipts = vec![
                    accepted(candidate(
                        &d,
                        "foreign",
                        0,
                        None,
                        vec![],
                        insert("z", None, "Z"),
                    ))
                    .receipt,
                ]
            }
        };
        let mismatch = step(&state, reply(&q, LookupResult::Committed { batch }));
        assert!(
            mismatch
                .effects
                .contains(&Effect::Report(Outcome::CallbackMismatch))
        );
        assert_eq!(
            mismatch.state, state,
            "foreign batch result preserves reservation"
        );
    }
}
#[test]
fn genuine_e0_fork_then_fresh_e1_local_recovery_keeps_ab_and_old_receipts_both_orders() {
    for reverse in [false, true] {
        let d = descriptor("x");
        let a = candidate(&d, "writer-e0", 0, None, vec![], insert("a", None, "A"));
        let b = candidate(
            &d,
            "writer-e0",
            1,
            Some(a.operation.canonical().digest().to_vec()),
            vec![],
            insert("old-b", Some("a"), "X"),
        );
        let rival = candidate(
            &d,
            "writer-e0",
            1,
            Some(a.operation.canonical().digest().to_vec()),
            vec![],
            insert("old-x", Some("a"), "Z"),
        );
        let (first, second) = if reverse { (rival, b) } else { (b, rival) };
        let mut state = initial(d.clone());
        seed(&mut state, a.clone());
        seed(&mut state, first.clone());
        let forked = drive(
            state,
            Event::OfferFork(Box::new(ForkEvidence {
                first,
                second: second.clone(),
            })),
            facts(&second),
        );
        let before = cut(&forked.state, d.instance.clone());
        assert_eq!(before.quarantine["writer-e0"], 1);
        assert_eq!(before.eligible, vec![a.operation.canonical().digest()]);
        let old = forked.state.instances[&d.instance].accepted.clone();
        let fresh = candidate(
            &d,
            "writer-e1",
            0,
            None,
            vec![Head {
                origin: "writer-e0".into(),
                seq: 0,
                hash: Some(a.operation.canonical().digest().to_vec()),
            }],
            insert("fresh-b", Some("a"), "B"),
        );
        let recovered = drive(
            forked.state,
            Event::SubmitLocal(Box::new(fresh.clone())),
            facts(&fresh),
        );
        assert!(
            recovered
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_)))),
            "quarantine elsewhere cannot forbid fresh origin"
        );
        let mut eligible = cut(&recovered.state, d.instance.clone()).eligible;
        eligible.sort();
        let mut expected = vec![
            a.operation.canonical().digest(),
            fresh.operation.canonical().digest(),
        ];
        expected.sort();
        assert_eq!(eligible, expected);
        for record in old {
            assert!(
                recovered.state.instances[&d.instance]
                    .accepted
                    .contains(&record)
            );
            let retry = step(
                &recovered.state,
                Event::SubmitLocal(Box::new(record.candidate)),
            );
            assert!(
                retry
                    .effects
                    .contains(&Effect::Report(Outcome::ExactRetry(record.receipt)))
            );
        }
        let mut reused = candidate(
            &d,
            "writer-e0",
            0,
            None,
            vec![],
            insert("illegal", None, "Q"),
        );
        reused.evidence.certificate = Bytes::new(b"reused-origin-epoch-1".to_vec());
        let mut invalid = facts(&reused);
        invalid.certificate.epoch = "epoch-1".into();
        invalid.issuance_unique = false;
        let refused = drive(
            recovered.state,
            Event::SubmitLocal(Box::new(reused)),
            invalid,
        );
        assert!(refused.effects.contains(&Effect::Report(Outcome::Invalid)));
    }
}

#[test]
fn trusted_restoration_invalidates_live_lookup_and_preserves_identity_high_water() {
    let d = descriptor("x");
    let (mut restored, plan) = retention_plan(&d, CommitKind::Candidate);
    let retired = request(&plan, 40);
    // Host restoration must invalidate outstanding transport invocations while
    // retaining validated high-water state; this is not a disk-restore proof.
    restored.lookups.clear();
    restored.next_effect_id = 41;
    let renewed = step(&restored, Event::ResumeRecovery(d.instance));
    let current = renewed
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Lookup(q) = e {
                Some(q)
            } else {
                None
            }
        })
        .expect("new restoration invocation");
    assert!(current.lookup_id > retired.lookup_id);
    let old = step(&renewed.state, reply(&retired, LookupResult::KnownAbsent));
    assert_eq!(old.state, renewed.state);
    assert!(
        old.effects
            .contains(&Effect::Report(Outcome::CallbackMismatch))
    );
}
