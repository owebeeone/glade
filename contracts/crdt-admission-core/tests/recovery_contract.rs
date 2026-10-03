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
    register_attempt(&mut state, &plan);
    state.next_effect_id = 40;
    (state, plan)
}
fn request(plan: &CommitPlan, lookup_id: u64) -> LookupRequest {
    resolve_request(plan, lookup_id, OperationKind::Inspect)
}
fn reply(q: &LookupRequest, result: LookupResult) -> Event {
    Event::LookupResolved(ResolveReply {
        request: q.clone(),
        result,
    })
}
fn committed(plan: &CommitPlan) -> LookupResult {
    committed_result(plan)
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
    let unknown = step(&l1.state, reply(&q1, ResolveResult::Pending));
    assert!(
        !unknown.state.lookups.contains_key(&q1.invocation.number),
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
    assert!(q2.invocation.number > q1.invocation.number);
    assert_eq!(
        q2.attempt.binding.plan.number,
        q1.attempt.binding.plan.number
    );
    for result in [
        ResolveResult::Pending,
        noncommit_result(&plan),
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
    assert!(!recovered.effects.iter().any(|e| matches!(
        e,
        Effect::Seal(_) | Effect::Prepare { .. } | Effect::Begin(_)
    )));
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
            register_attempt(&mut state, &plan);
            let uncertain = step(
                &state,
                Event::CommitResolved({
                    let mut reply = commit_reply(&plan, 8, ResolveResult::Pending);
                    reply.request.attempt.binding.plan.number = plan.id;
                    reply.request.attempt.id.instance = plan.instance.clone();
                    reply.request.attempt.binding.plan.instance =
                        reply.request.attempt.id.instance.clone();
                    if plan.batch.digest() != plan.batch.digest() {
                        reply.request.attempt.binding.batch =
                            Bytes::new(b"changed-callback-batch".to_vec());
                    }
                    reply
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
            register_lookup(&mut state, q.clone()).unwrap();
            state.next_effect_id = 41;
            let resolution = match outcome {
                0 => committed(&plan),
                1 => noncommit_result(&plan),
                _ => ResolveResult::Pending,
            };
            let result = step(&state, reply(&q, resolution));
            assert!(
                !result.state.lookups.contains_key(&q.invocation.number),
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
    register_lookup(&mut state, q.clone()).unwrap();
    for mutation in 0..4 {
        let mut terminal = match committed_result(&plan) {
            ResolveResult::Terminal(t) => t,
            _ => unreachable!(),
        };
        match mutation {
            0 => terminal.attempt.binding.batch = Bytes::new(b"changed".to_vec()),
            1 => {
                if let TerminalOutcome::Committed { revision, .. } = &mut terminal.outcome {
                    *revision = 0;
                }
            }
            2 => {
                if let TerminalOutcome::Committed { storage, .. } = &mut terminal.outcome {
                    *storage = StorageClass::LocalProcessRestart;
                }
            }
            _ => {
                if let TerminalOutcome::Committed { receipts, .. } = &mut terminal.outcome {
                    *receipts = vec![
                        accepted(candidate(
                            &d,
                            "foreign",
                            0,
                            None,
                            vec![],
                            insert("z", None, "Z"),
                        ))
                        .receipt,
                    ];
                }
            }
        }
        let mismatch = step(&state, reply(&q, ResolveResult::Terminal(terminal)));
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
        let mut replica = FixtureReplica::new(state.clone()).unwrap();
        let forked = drive_with_port(
            state,
            Event::OfferFork(Box::new(ForkEvidence {
                first,
                second: second.clone(),
            })),
            facts(&second),
            &mut replica.session,
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
        let recovered = drive_with_port(
            forked.state,
            Event::SubmitLocal(Box::new(fresh.clone())),
            facts(&fresh),
            &mut replica.session,
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
        let refused = drive_with_port(
            recovered.state,
            Event::SubmitLocal(Box::new(reused)),
            invalid,
            &mut replica.session,
        );
        assert!(refused.effects.contains(&Effect::Report(Outcome::Invalid)));
    }
}

#[test]
fn trusted_restoration_invalidates_live_lookup_and_preserves_identity_high_water() {
    let d = descriptor("x");
    let (mut restored, plan) = retention_plan(&d, CommitKind::Candidate);
    let retired = request(&plan, 40);
    register_lookup(&mut restored, retired.clone()).unwrap();
    let issued = restored
        .storage_invocations
        .remove(&retired.invocation)
        .unwrap();
    restored
        .retired_storage_invocations
        .insert(retired.invocation.clone(), issued);
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
    assert!(current.invocation.number > retired.invocation.number);
    let old = step(&renewed.state, reply(&retired, noncommit_result(&plan)));
    assert_eq!(old.state, renewed.state);
    assert!(
        old.effects
            .contains(&Effect::Report(Outcome::CallbackMismatch))
    );
}
