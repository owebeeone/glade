//! Test-first shared port journeys. Success here is bounded memory conformance only.
#[allow(dead_code)]
mod support;
use glade_crdt_storage_attempt_api::*;
use support::*;

#[test]
fn sta001_exact_binding_exhaustion_and_invocation_identity() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    let b = binding("x", CommitKind::AcceptedBatch, 1);
    let a = prepared(s.prepare(prepare_request(b.clone(), 1)));
    for mutation in 0..5 {
        let mut wrong = b.clone();
        match mutation {
            0 => wrong.batch = Bytes::new(b"changed".to_vec()),
            1 => wrong.expected_revision = 1,
            2 => wrong.storage = StorageClass::LocalProcessRestart,
            3 => wrong.charges.edit = 1,
            _ => wrong.receipts[0].stable_id = [9; 32],
        }
        assert_eq!(
            s.prepare(prepare_request(wrong, 2 + mutation)).result,
            PrepareResult::Refused(Fault::BindingMismatch)
        );
    }
    assert_eq!(
        s.recover_plan(prepare_request(b, 7)).result,
        PrepareResult::Prepared(Box::new(a.clone()))
    );
    let q = resolution(a, 8, OperationKind::Inspect);
    s.inspect(q.clone());
    assert_eq!(
        s.inspect(q).result,
        ResolveResult::Refused(Fault::CallbackMismatch)
    );
}
#[test]
fn sta002_exact_plan_dedup_no_overlap_and_independent_instances() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    let x = binding("x", CommitKind::Candidate, 1);
    let a = prepared(s.prepare(prepare_request(x.clone(), 1)));
    assert_eq!(prepared(s.recover_plan(prepare_request(x, 2))), a);
    assert_eq!(
        s.prepare(prepare_request(binding("x", CommitKind::Candidate, 2), 3))
            .result,
        PrepareResult::Refused(Fault::Unavailable)
    );
    prepared(s.prepare(prepare_request(binding("y", CommitKind::Candidate, 1), 4)));
}
#[test]
fn sta003_absent_terminal_is_pending_for_all_kinds() {
    for kind in [
        CommitKind::AcceptedBatch,
        CommitKind::Candidate,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
    ] {
        let mut host = MemoryHost::new();
        let mut s = host.open(open_request()).unwrap();
        let a = prepared(s.prepare(prepare_request(binding("x", kind, 1), 1)));
        assert_eq!(
            s.inspect(resolution(a.clone(), 2, OperationKind::Inspect))
                .result,
            ResolveResult::Pending
        );
        assert_eq!(s.close(), CloseResult::Pending);
        let committed = s.begin(BeginRequest {
            request: resolution(a.clone(), 3, OperationKind::Begin),
            current: cut(),
        });
        let ResolveResult::Terminal(t) = committed.result else {
            panic!("late write must recover whole original terminal")
        };
        assert_eq!(t.attempt, a);
        assert_eq!(
            s.inspect(resolution(a, 4, OperationKind::Inspect)).result,
            ResolveResult::Terminal(t)
        );
    }
}
#[test]
fn sta004_fence_excludes_late_begin_and_reprepare_uses_new_identity() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    let b = binding("x", CommitKind::AcceptedBatch, 1);
    let a = prepared(s.prepare(prepare_request(b.clone(), 1)));
    let negative = s
        .request_fence(resolution(a.clone(), 2, OperationKind::Fence))
        .result;
    assert!(matches!(
        &negative,
        ResolveResult::Terminal(t) if matches!(t.outcome,TerminalOutcome::NonCommit {revision:0,..})
    ));
    assert_eq!(
        s.begin(BeginRequest {
            request: resolution(a.clone(), 3, OperationKind::Begin),
            current: cut()
        })
        .result,
        negative
    );
    assert_eq!(prepared(s.recover_plan(prepare_request(b, 4))), a);
    let new = prepared(s.prepare(prepare_request(
        binding("x", CommitKind::AcceptedBatch, 2),
        5,
    )));
    assert_ne!(new.id, a.id);
}
#[test]
fn sta005_commit_wins_fence_race_and_receipts_are_original() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    let b = binding("x", CommitKind::AcceptedBatch, 1);
    let a = prepared(s.prepare(prepare_request(b.clone(), 1)));
    let terminal = s
        .begin(BeginRequest {
            request: resolution(a.clone(), 2, OperationKind::Begin),
            current: cut(),
        })
        .result;
    assert!(
        matches!(&terminal,ResolveResult::Terminal(t) if matches!(&t.outcome,TerminalOutcome::Committed {revision:1,receipts,..} if receipts==&b.receipts))
    );
    assert_eq!(
        s.request_fence(resolution(a, 3, OperationKind::Fence))
            .result,
        terminal
    );
}
#[test]
fn sta006_prestart_checks_entire_window_and_historical_has_no_current_admit_check() {
    for changed in 0..3 {
        let mut host = MemoryHost::new();
        let mut s = host.open(open_request()).unwrap();
        let a = prepared(s.prepare(prepare_request(
            binding("x", CommitKind::AcceptedBatch, 1),
            1,
        )));
        let mut current = cut();
        match changed {
            0 => current.policy = [9; 32],
            1 => current.time.latest_ms = 101,
            _ => current.time.earliest_ms = 22,
        }
        assert!(matches!(
            s.begin(BeginRequest {
                request: resolution(a, 2, OperationKind::Begin),
                current
            })
            .result,
            ResolveResult::Terminal(t) if matches!(t.outcome,TerminalOutcome::NonCommit {..})
        ));
    }
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    let a = prepared(s.prepare(prepare_request(
        binding("x", CommitKind::QualifiedFork, 1),
        1,
    )));
    assert!(matches!(
        s.begin(BeginRequest {
            request: resolution(a, 2, OperationKind::Begin),
            current: LocalPrecondition {
                policy: [9; 32],
                time: TimeInterval {
                    earliest_ms: 1000,
                    latest_ms: 1001
                }
            }
        })
        .result,
        ResolveResult::Terminal(t) if matches!(t.outcome,TerminalOutcome::Committed {start_cut:None,..})
    ));
}
#[test]
fn sta007_retention_receipts_cannot_be_synthetic() {
    for kind in [CommitKind::Candidate, CommitKind::SecurityEvidence] {
        let mut host = MemoryHost::new();
        let mut s = host.open(open_request()).unwrap();
        let mut b = binding("x", kind, 1);
        b.receipts = binding("x", CommitKind::AcceptedBatch, 1).receipts;
        assert_eq!(
            s.prepare(prepare_request(b, 1)).result,
            PrepareResult::Refused(Fault::BindingMismatch)
        );
    }
}
#[test]
fn sta008_critical_reserve_exhaustion_cannot_manufacture_negative_finality() {
    let mut host = MemoryHost::new();
    let mut open = open_request();
    open.limits.critical_bytes = 4096;
    let mut s = host.open(open).unwrap();
    let a = prepared(s.prepare(prepare_request(binding("x", CommitKind::Candidate, 1), 1)));
    assert_eq!(
        s.prepare(prepare_request(binding("y", CommitKind::Candidate, 1), 2))
            .result,
        PrepareResult::Refused(Fault::Capacity)
    );
    assert_eq!(
        s.inspect(resolution(a, 3, OperationKind::Inspect)).result,
        ResolveResult::Pending
    );
}
#[test]
fn sta009_exclusive_recovery_and_close_keep_issuance_floors() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    assert!(matches!(host.open(open_request()), Err(Fault::Ownership)));
    let a = prepared(s.prepare(prepare_request(binding("x", CommitKind::Candidate, 1), 1)));
    s.request_fence(resolution(a.clone(), 2, OperationKind::Fence));
    assert_eq!(s.close(), CloseResult::Closed);
    let mut next = open_request();
    next.owner.generation = 2;
    let mut reopened = host.open(next).unwrap();
    let RecoveryResult::Validated(r) = reopened.recover() else {
        panic!("retained trusted ledger")
    };
    assert_eq!(r.next_attempt, 2);
    assert_eq!(r.invocation_high_water, 2);
    assert_eq!(r.attempts[0].attempt, a);
    assert_eq!(
        s.begin(BeginRequest {
            request: resolution(r.attempts[0].attempt.clone(), 3, OperationKind::Begin),
            current: cut()
        })
        .result,
        ResolveResult::Refused(Fault::Ownership)
    );
}
#[test]
fn lost_prepare_ack_and_queued_late_prepare_recover_exactly_one_attempt() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    s.hold_preparation(true);
    let b = binding("x", CommitKind::Candidate, 1);
    assert_eq!(
        s.prepare(prepare_request(b.clone(), 1)).result,
        PrepareResult::Pending
    );
    assert_eq!(
        s.recover_plan(prepare_request(b.clone(), 2)).result,
        PrepareResult::Pending
    );
    assert_eq!(
        s.prepare(prepare_request(binding("x", CommitKind::Candidate, 2), 3))
            .result,
        PrepareResult::Refused(Fault::Unavailable)
    );
    prepared(s.prepare(prepare_request(binding("y", CommitKind::Candidate, 1), 4)));
    s.hold_preparation(false);
    s.lose_prepare_ack(true);
    assert_eq!(
        s.prepare(prepare_request(b.clone(), 5)).result,
        PrepareResult::Pending
    );
    s.lose_prepare_ack(false);
    let a = prepared(s.recover_plan(prepare_request(b.clone(), 6)));
    assert_eq!(a.id.number, 2);
    assert_eq!(prepared(s.recover_plan(prepare_request(b, 7))), a);
    let RecoveryResult::Validated(r) = s.recover() else {
        panic!("recovery")
    };
    assert_eq!(r.attempts.len(), 2);
}
#[test]
fn started_absent_lookup_late_publication_and_fence_use_same_predicate() {
    for fence_wins in [false, true] {
        let mut host = MemoryHost::new();
        let mut s = host.open(open_request()).unwrap();
        s.hold_publication(true);
        let a = prepared(s.prepare(prepare_request(
            binding("x", CommitKind::AcceptedBatch, 1),
            1,
        )));
        assert!(matches!(
            s.begin(BeginRequest {
                request: resolution(a.clone(), 2, OperationKind::Begin),
                current: cut()
            })
            .result,
            ResolveResult::Started { cut: Some(_) }
        ));
        assert_eq!(
            s.inspect(resolution(a.clone(), 3, OperationKind::Inspect))
                .result,
            ResolveResult::Pending
        );
        let result = if fence_wins {
            s.request_fence(resolution(a.clone(), 4, OperationKind::Fence))
                .result
        } else {
            s.publish(&a, open_request().owner)
        };
        assert_eq!(s.publish(&a, open_request().owner), result);
        assert_eq!(
            s.inspect(resolution(a, 5, OperationKind::Inspect)).result,
            result
        );
    }
}
#[test]
fn finite_attempt_invocation_and_name_bounds_refuse_without_wrap() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    s.set_attempt_floor(u64::MAX);
    assert_eq!(
        s.prepare(prepare_request(binding("x", CommitKind::Candidate, 1), 1))
            .result,
        PrepareResult::Refused(Fault::Capacity)
    );
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    assert_eq!(
        s.prepare(prepare_request(
            binding("x", CommitKind::Candidate, 1),
            u64::MAX
        ))
        .result,
        PrepareResult::Refused(Fault::Capacity)
    );
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    assert_eq!(
        s.prepare(prepare_request(
            binding(&"x".repeat(65), CommitKind::Candidate, 1),
            1
        ))
        .result,
        PrepareResult::Refused(Fault::Capacity)
    );
}
#[test]
fn started_cut_is_not_rechecked_after_revocation_or_delayed_begin_ack() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    s.hold_publication(true);
    let a = prepared(s.prepare(prepare_request(
        binding("x", CommitKind::AcceptedBatch, 1),
        1,
    )));
    let started = s
        .begin(BeginRequest {
            request: resolution(a.clone(), 2, OperationKind::Begin),
            current: cut(),
        })
        .result;
    assert!(matches!(started, ResolveResult::Started { cut: Some(_) }));
    let replay = s
        .begin(BeginRequest {
            request: resolution(a.clone(), 3, OperationKind::Begin),
            current: LocalPrecondition {
                policy: [9; 32],
                time: TimeInterval {
                    earliest_ms: 200,
                    latest_ms: 201,
                },
            },
        })
        .result;
    assert_eq!(replay, started, "valid Started cut survives later policy");
    let result = s.publish(&a, open_request().owner);
    assert!(
        matches!(result,ResolveResult::Terminal(t) if matches!(&t.outcome,TerminalOutcome::Committed {start_cut:Some(original),..} if original==&cut()))
    );
}
#[test]
fn validated_restore_retains_all_attempts_and_invocation_namespace_without_auto_commit() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    s.hold_publication(true);
    let a = prepared(s.prepare(prepare_request(
        binding("x", CommitKind::AcceptedBatch, 1),
        1,
    )));
    s.begin(BeginRequest {
        request: resolution(a.clone(), 2, OperationKind::Begin),
        current: cut(),
    });
    let RecoveryResult::Validated(recovery) = s.recover() else {
        panic!("recovery")
    };
    let mut restored = MemoryHost::restore(*recovery.clone()).unwrap();
    let mut next = open_request();
    next.owner.generation = 2;
    let mut current = restored.open(next.clone()).unwrap();
    let q = ResolveRequest {
        owner: next.owner.clone(),
        ..resolution(a.clone(), 3, OperationKind::Inspect)
    };
    assert_eq!(current.inspect(q).result, ResolveResult::Pending);
    let old = current.inspect(ResolveRequest {
        owner: next.owner,
        ..resolution(a, 2, OperationKind::Inspect)
    });
    assert_eq!(old.result, ResolveResult::Refused(Fault::CallbackMismatch));
}
#[test]
fn host_serialized_observation_cannot_be_replaced_by_caller_current_claim() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    let a = prepared(s.prepare(prepare_request(
        binding("x", CommitKind::AcceptedBatch, 1),
        1,
    )));
    s.observe_policy(
        a.id.instance.clone(),
        LocalPrecondition {
            policy: [9; 32],
            time: TimeInterval {
                earliest_ms: 22,
                latest_ms: 23,
            },
        },
    )
    .unwrap();
    let result = s.begin(BeginRequest {
        request: resolution(a, 2, OperationKind::Begin),
        current: cut(),
    });
    assert!(
        matches!(result.result,ResolveResult::Terminal(t) if matches!(t.outcome,TerminalOutcome::NonCommit {..})),
        "caller cannot restore stale observation"
    );
}
#[test]
fn historical_accepted_batch_keeps_original_receipt_without_local_prestart() {
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    let mut b = binding("x", CommitKind::AcceptedBatch, 1);
    b.prestart = None;
    let a = prepared(s.prepare(prepare_request(b.clone(), 1)));
    let result = s.begin(BeginRequest {
        request: resolution(a, 2, OperationKind::Begin),
        current: LocalPrecondition {
            policy: [9; 32],
            time: TimeInterval {
                earliest_ms: 200,
                latest_ms: 201,
            },
        },
    });
    assert!(
        matches!(result.result,ResolveResult::Terminal(t) if matches!(t.outcome,TerminalOutcome::Committed {ref receipts,start_cut:None,..} if receipts==&b.receipts))
    );
}

#[test]
fn trusted_observation_injection_has_finite_instance_and_name_bounds() {
    let mut h = MemoryHost::new();
    let mut open = open_request();
    open.limits.instances = 1;
    let mut s = h.open(open).unwrap();
    assert_eq!(
        s.observe_policy(binding("x", CommitKind::Candidate, 1).plan.instance, cut()),
        Ok(())
    );
    assert_eq!(
        s.observe_policy(binding("y", CommitKind::Candidate, 1).plan.instance, cut()),
        Err(Fault::Capacity)
    );
    assert_eq!(
        s.observe_policy(
            binding(&"z".repeat(65), CommitKind::Candidate, 1)
                .plan
                .instance,
            cut()
        ),
        Err(Fault::Capacity)
    );
}

#[test]
fn sparse_invocation_restore_preserves_actual_remaining_capacity() {
    let mut h = MemoryHost::new();
    let mut s = h.open(open_request()).unwrap();
    let a = prepared(s.prepare(prepare_request(binding("x", CommitKind::Candidate, 1), 128)));
    let RecoveryResult::Validated(r) = s.recover() else {
        panic!("authentic recovery")
    };
    let mut restored = MemoryHost::restore(*r).unwrap();
    let mut open = open_request();
    open.owner.generation = 2;
    let mut r = restored.open(open.clone()).unwrap();
    let mut stale = resolution(a.clone(), 128, OperationKind::Inspect);
    stale.owner = open.owner.clone();
    assert_eq!(
        r.inspect(stale).result,
        ResolveResult::Refused(Fault::CallbackMismatch)
    );
    let mut q = resolution(a.clone(), 129, OperationKind::Inspect);
    q.owner = open.owner.clone();
    assert_eq!(r.inspect(q).result, ResolveResult::Pending);
    assert_eq!(
        s.inspect(resolution(a.clone(), 129, OperationKind::Inspect))
            .result,
        ResolveResult::Pending
    );
    let live_terminal = s
        .request_fence(resolution(a.clone(), 130, OperationKind::Fence))
        .result;
    let mut q = resolution(a.clone(), 130, OperationKind::Fence);
    q.owner = open.owner.clone();
    assert_eq!(r.request_fence(q).result, live_terminal);
    assert!(
        matches!(live_terminal,ResolveResult::Terminal(ref t) if matches!(t.outcome,TerminalOutcome::NonCommit{..}))
    );
    for n in 131..256 {
        let live = s.inspect(resolution(a.clone(), n, OperationKind::Inspect));
        let mut q = resolution(a.clone(), n, OperationKind::Inspect);
        q.owner = open.owner.clone();
        let recovered = r.inspect(q);
        assert_eq!(live.result, recovered.result);
        assert_eq!(recovered.result, live_terminal);
    }
    let mut q = resolution(a.clone(), 256, OperationKind::Fence);
    q.owner = open.owner;
    assert_eq!(
        r.request_fence(q).result,
        ResolveResult::Refused(Fault::Capacity)
    );
    assert_eq!(
        s.request_fence(resolution(a, 256, OperationKind::Fence))
            .result,
        ResolveResult::Refused(Fault::Capacity)
    );
}

#[test]
fn restoration_rejects_corrupt_cardinality_floor_and_history_limit() {
    let mut h = MemoryHost::new();
    let mut s = h.open(open_request()).unwrap();
    prepared(s.prepare(prepare_request(binding("x", CommitKind::Candidate, 1), 128)));
    let RecoveryResult::Validated(r) = s.recover() else {
        panic!("authentic recovery")
    };
    assert_eq!(r.invocation_count, 1);
    assert_eq!(r.invocation_high_water, 128);
    for mutation in 0..5 {
        let mut bad = (*r).clone();
        match mutation {
            0 => bad.invocation_count = 0,
            1 => bad.invocation_count = 129,
            2 => bad.invocation_high_water = 0,
            3 => bad.limits.invocation_history = 0,
            _ => bad.next_attempt = 1,
        }
        assert!(
            MemoryHost::restore(bad).is_err(),
            "corrupt restored history must refuse"
        );
    }
}

#[test]
fn restored_capacity_and_binding_phase_consistency_are_validated() {
    let mut h = MemoryHost::new();
    let mut s = h.open(open_request()).unwrap();
    prepared(s.prepare(prepare_request(
        binding("x", CommitKind::AcceptedBatch, 1),
        128,
    )));
    let RecoveryResult::Validated(r) = s.recover() else {
        panic!("authentic recovery")
    };
    for mutation in 0..8 {
        let mut bad = (*r).clone();
        match mutation {
            0 => bad.limits.max_batch_bytes = 1,
            1 => bad.limits.max_receipts = 0,
            2 => bad.limits.max_windows = 0,
            3 => bad.limits.max_name_bytes = 0,
            4 => bad.revisions.clear(),
            5 => bad.attempts[0].attempt.id.instance.incarnation = "foreign".into(),
            6 => {
                bad.attempts[0].phase = AttemptPhase::Terminal(TerminalOutcome::NonCommit {
                    revision: 1,
                    storage: StorageClass::VolatileTest,
                })
            }
            _ => {
                bad.attempts[0].phase = AttemptPhase::Started {
                    cut: Some(LocalPrecondition {
                        policy: [9; 32],
                        time: cut().time,
                    }),
                }
            }
        }
        assert!(
            MemoryHost::restore(bad).is_err(),
            "mutation {mutation} cannot be validated recovery"
        );
    }
}

#[test]
fn fresh_revision_mismatch_refuses_before_reserved_or_queued_for_every_kind() {
    for kind in [
        CommitKind::Candidate,
        CommitKind::AcceptedBatch,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
    ] {
        for expected in [0, 2] {
            for fence in [false, true] {
                for hold in [false, true] {
                    let mut h = MemoryHost::new();
                    let mut s = h.open(open_request()).unwrap();
                    let original = binding("x", kind.clone(), 1);
                    let a = prepared(s.prepare(prepare_request(original.clone(), 1)));
                    let committed = s
                        .begin(BeginRequest {
                            request: resolution(a.clone(), 2, OperationKind::Begin),
                            current: cut(),
                        })
                        .result;
                    assert!(
                        matches!(committed, ResolveResult::Terminal(ref t) if matches!(t.outcome, TerminalOutcome::Committed { revision: 1, .. }))
                    );
                    assert_eq!(
                        s.recover_plan(prepare_request(original, 3)).result,
                        PrepareResult::Prepared(Box::new(a.clone()))
                    );
                    let before = s.fixture_seed().recovery;
                    s.hold_preparation(hold);
                    let mut bad = binding("x", kind.clone(), 2);
                    bad.expected_revision = expected;
                    assert_eq!(
                        s.prepare(prepare_request(bad.clone(), 4)).result,
                        PrepareResult::Refused(Fault::BindingMismatch),
                        "kind {kind:?}, expected {expected}, fence {fence}, hold {hold}"
                    );
                    let fake = AttemptBinding {
                        id: AttemptId {
                            store: open_request().owner.store,
                            instance: bad.plan.instance.clone(),
                            number: before.next_attempt,
                        },
                        binding: bad.clone(),
                    };
                    let request = resolution(
                        fake,
                        5,
                        if fence {
                            OperationKind::Fence
                        } else {
                            OperationKind::Begin
                        },
                    );
                    let result = if fence {
                        s.request_fence(request).result
                    } else {
                        s.begin(BeginRequest {
                            request,
                            current: cut(),
                        })
                        .result
                    };
                    assert_eq!(result, ResolveResult::Pending);
                    assert_eq!(
                        s.recover_plan(prepare_request(bad, 6)).result,
                        PrepareResult::Pending
                    );
                    let after = s.fixture_seed().recovery;
                    assert_eq!(after.attempts, before.attempts);
                    assert_eq!(after.preparing, before.preparing);
                    assert_eq!(after.revisions, before.revisions);
                    assert_eq!(after.next_attempt, before.next_attempt);
                    MemoryHost::restore(after).unwrap();
                    assert_eq!(
                        s.inspect(resolution(a, 7, OperationKind::Inspect)).result,
                        committed
                    );
                    s.hold_preparation(false);
                    let mut valid = binding("x", kind.clone(), 3);
                    valid.expected_revision = 1;
                    let pending = prepared(s.prepare(prepare_request(valid.clone(), 8)));
                    s.hold_publication(true);
                    assert!(matches!(
                        s.begin(BeginRequest {
                            request: resolution(pending.clone(), 9, OperationKind::Begin),
                            current: cut()
                        })
                        .result,
                        ResolveResult::Started { .. }
                    ));
                    let mut rival = valid.clone();
                    rival.plan.number = 4;
                    rival.expected_revision = 2;
                    assert_eq!(
                        s.prepare(prepare_request(rival, 10)).result,
                        PrepareResult::Refused(Fault::Unavailable)
                    );
                    assert_eq!(
                        s.recover_plan(prepare_request(valid, 11)).result,
                        PrepareResult::Prepared(Box::new(pending.clone()))
                    );
                    assert_eq!(
                        s.publish(&pending, open_request().owner),
                        ResolveResult::Terminal(Box::new(Terminal {
                            attempt: pending.clone(),
                            outcome: TerminalOutcome::Committed {
                                revision: 2,
                                storage: pending.binding.storage.clone(),
                                receipts: pending.binding.receipts.clone(),
                                start_cut: pending.binding.prestart.as_ref().map(|p| p.cut.clone())
                            }
                        }))
                    );
                }
            }
        }
    }
}

#[test]
fn restoration_rejects_identity_and_unresolved_overlap_but_keeps_history_and_y() {
    let mut h = MemoryHost::new();
    let mut s = h.open(open_request()).unwrap();
    let first = prepared(s.prepare(prepare_request(binding("x", CommitKind::Candidate, 1), 1)));
    s.request_fence(resolution(first, 2, OperationKind::Fence));
    let second = prepared(s.prepare(prepare_request(binding("x", CommitKind::Candidate, 2), 3)));
    s.request_fence(resolution(second, 4, OperationKind::Fence));
    prepared(s.prepare(prepare_request(binding("x", CommitKind::Candidate, 3), 5)));
    let good = s.fixture_seed().recovery;
    for mutation in 0..8 {
        let mut bad = good.clone();
        let mut extra = bad.attempts[2].clone();
        extra.attempt.binding.plan.number = 4;
        match mutation {
            0 => {
                bad.attempts.push(extra);
            }
            1 => {
                extra.attempt.id.number = bad.next_attempt;
                bad.next_attempt += 1;
                bad.attempts.push(extra);
            }
            2 => {
                bad.preparing.push(extra.attempt.binding);
            }
            3 => {
                bad.attempts.pop();
                bad.preparing.push(binding("x", CommitKind::Candidate, 3));
                bad.preparing.push(binding("x", CommitKind::Candidate, 4));
            }
            4 => {
                bad.attempts[1].attempt.id = bad.attempts[0].attempt.id.clone();
            }
            5 => {
                bad.attempts[2].attempt.binding.expected_revision = 1;
            }
            6 => {
                bad.preparing.push(binding("y", CommitKind::Candidate, 1));
                bad.limits.instances = 1;
            }
            _ => {
                bad.attempts.pop();
                let mut queued = binding("x", CommitKind::Candidate, 3);
                queued.expected_revision = 1;
                bad.preparing.push(queued);
            }
        }
        bad.invocation_count += 2;
        bad.invocation_high_water += 2;
        assert!(
            MemoryHost::restore(bad).is_err(),
            "overlap mutation {mutation}"
        );
    }
    let mut restored = MemoryHost::restore(good.clone()).unwrap();
    let mut open = open_request();
    open.owner.generation = 2;
    let mut r = restored.open(open.clone()).unwrap();
    let mut q = prepare_request(binding("y", CommitKind::Candidate, 1), 6);
    q.owner = open.owner.clone();
    let y = prepared(r.prepare(q));
    let mut q = resolution(y, 7, OperationKind::Begin);
    q.owner = open.owner;
    assert!(
        matches!(r.begin(BeginRequest { request: q, current: cut() }).result, ResolveResult::Terminal(t) if matches!(t.outcome, TerminalOutcome::Committed { revision: 1, .. }))
    );
    let recovered = r.fixture_seed().recovery;
    assert_eq!(&recovered.attempts[..3], &good.attempts);
    MemoryHost::restore(recovered).unwrap();
}

#[test]
fn reopen_validates_retained_consumption_before_changing_owner_or_floors() {
    for reduction in 0..9 {
        let mut h = MemoryHost::new();
        let mut s = h.open(open_request()).unwrap();
        for (name, n) in [("x", 1), ("y", 3)] {
            let a = prepared(s.prepare(prepare_request(
                binding(name, CommitKind::AcceptedBatch, 1),
                n,
            )));
            s.request_fence(resolution(a, n + 1, OperationKind::Fence));
        }
        let before = s.fixture_seed().recovery;
        assert_eq!(s.close(), CloseResult::Closed);
        let mut smaller = open_request();
        smaller.owner.generation = 2;
        match reduction {
            0 => smaller.limits.max_batch_bytes = 1,
            1 => smaller.limits.max_receipts = 0,
            2 => smaller.limits.max_windows = 0,
            3 => smaller.limits.max_name_bytes = 0,
            4 => smaller.limits.critical_per_attempt = 1,
            5 => smaller.limits.critical_bytes = 8191,
            6 => smaller.limits.attempts = 1,
            7 => smaller.limits.invocation_history = 3,
            _ => smaller.limits.instances = 1,
        }
        assert!(
            matches!(h.open(smaller), Err(Fault::Capacity)),
            "bound reduction {reduction}"
        );
        assert_eq!(s.fixture_seed().recovery, before);
        let mut compatible = open_request();
        compatible.owner.generation = 2;
        let mut next = h.open(compatible.clone()).unwrap();
        let recovered = next.fixture_seed().recovery;
        assert_eq!(recovered.owner, compatible.owner);
        assert_eq!(recovered.attempts, before.attempts);
        assert_eq!(recovered.revisions, before.revisions);
        assert_eq!(recovered.next_attempt, before.next_attempt);
        assert_eq!(recovered.invocation_count, before.invocation_count);
        assert_eq!(
            recovered.invocation_high_water,
            before.invocation_high_water
        );
        MemoryHost::restore(recovered).unwrap();
        let mut q = resolution(
            before.attempts[0].attempt.clone(),
            5,
            OperationKind::Inspect,
        );
        q.owner = compatible.owner;
        assert_eq!(
            next.inspect(q).result,
            ResolveResult::Terminal(Box::new(Terminal {
                attempt: before.attempts[0].attempt.clone(),
                outcome: match &before.attempts[0].phase {
                    AttemptPhase::Terminal(t) => t.clone(),
                    _ => unreachable!(),
                }
            }))
        );
    }
}

fn authentic_revision_history(commits: u64) -> Recovery {
    let mut host = MemoryHost::new();
    let mut session = host.open(open_request()).unwrap();
    for number in 1..=commits {
        let mut b = binding("x", CommitKind::Candidate, number);
        b.expected_revision = number - 1;
        let a = prepared(session.prepare(prepare_request(b, number * 2 - 1)));
        assert!(
            matches!(session.begin(BeginRequest { request: resolution(a, number * 2, OperationKind::Begin), current: cut() }).result, ResolveResult::Terminal(t) if matches!(t.outcome, TerminalOutcome::Committed { revision, .. } if revision == number))
        );
    }
    let RecoveryResult::Validated(r) = session.recover() else {
        panic!("authentic retained history required")
    };
    *r
}

#[test]
fn authentic_two_commit_recovery_rejects_distinct_duplicate_revision() {
    let authentic = authentic_revision_history(2);
    assert_eq!(
        authentic.revisions.values().copied().collect::<Vec<_>>(),
        vec![2]
    );
    assert_ne!(
        authentic.attempts[0].attempt.id,
        authentic.attempts[1].attempt.id
    );
    assert_ne!(
        authentic.attempts[0].attempt.binding.batch,
        authentic.attempts[1].attempt.binding.batch
    );
    MemoryHost::restore(authentic.clone()).unwrap();
    let mut bad = authentic.clone();
    bad.attempts[1].attempt.binding.expected_revision = 0;
    let AttemptPhase::Terminal(TerminalOutcome::Committed { revision, .. }) =
        &mut bad.attempts[1].phase
    else {
        panic!("second authentic terminal")
    };
    *revision = 1;
    assert!(
        matches!(MemoryHost::restore(bad), Err(Fault::Integrity)),
        "distinct commits cannot own the same resulting revision"
    );
}

#[test]
fn full_retained_commit_history_rejects_missing_head_gaps_and_unbacked_current_revision() {
    let authentic = authentic_revision_history(3);
    for mutation in 0..6 {
        let mut bad = authentic.clone();
        match mutation {
            0 => {
                bad.attempts.remove(2);
            }
            1 => {
                bad.attempts.remove(0);
            }
            2 => {
                bad.attempts.remove(1);
            }
            3 => {
                *bad.revisions.values_mut().next().unwrap() = 4;
            }
            4 => {
                bad.attempts.clear();
            }
            _ => {
                bad.revisions
                    .insert(binding("y", CommitKind::Candidate, 1).plan.instance, 1);
            }
        }
        assert!(
            matches!(MemoryHost::restore(bad), Err(Fault::Integrity)),
            "full-history inconsistency {mutation}"
        );
    }
    MemoryHost::restore(authentic).unwrap();
}

#[test]
fn sequential_commits_repeated_noncommit_and_independent_revision_spaces_restore() {
    let mut independent = MemoryHost::new();
    let mut both = independent.open(open_request()).unwrap();
    for (name, invocation) in [("x", 1), ("y", 3)] {
        let a = prepared(both.prepare(prepare_request(
            binding(name, CommitKind::Candidate, 1),
            invocation,
        )));
        both.begin(BeginRequest {
            request: resolution(a, invocation + 1, OperationKind::Begin),
            current: cut(),
        });
    }
    let RecoveryResult::Validated(independent) = both.recover() else {
        panic!("independent histories")
    };
    assert_eq!(
        independent.revisions.values().copied().collect::<Vec<_>>(),
        vec![1, 1]
    );
    MemoryHost::restore(*independent).unwrap();
    let mut host = MemoryHost::new();
    let mut s = host.open(open_request()).unwrap();
    for (name, plan, expected, invocation, commit) in [
        ("x", 1, 0, 1, true),
        ("x", 2, 1, 3, true),
        ("x", 3, 2, 5, false),
        ("x", 4, 2, 7, false),
        ("y", 1, 0, 9, true),
    ] {
        let mut b = binding(name, CommitKind::Candidate, plan);
        b.expected_revision = expected;
        let a = prepared(s.prepare(prepare_request(b, invocation)));
        let reply = if commit {
            s.begin(BeginRequest {
                request: resolution(a, invocation + 1, OperationKind::Begin),
                current: cut(),
            })
        } else {
            s.request_fence(resolution(a, invocation + 1, OperationKind::Fence))
        };
        assert!(
            matches!(reply.result, ResolveResult::Terminal(ref t) if match t.outcome { TerminalOutcome::Committed { revision, .. } => commit && revision == expected + 1, TerminalOutcome::NonCommit { revision, .. } => !commit && revision == expected })
        );
    }
    let RecoveryResult::Validated(authentic) = s.recover() else {
        panic!("complete history")
    };
    assert_eq!(
        authentic.revisions[&binding("x", CommitKind::Candidate, 1).plan.instance],
        2
    );
    assert_eq!(
        authentic.revisions[&binding("y", CommitKind::Candidate, 1).plan.instance],
        1
    );
    assert_eq!(
        authentic
            .attempts
            .iter()
            .filter(|a| matches!(
                a.phase,
                AttemptPhase::Terminal(TerminalOutcome::NonCommit { revision: 2, .. })
            ))
            .count(),
        2
    );
    let mut restored = MemoryHost::restore((*authentic).clone()).unwrap();
    let mut open = open_request();
    open.owner.generation = 2;
    let mut r = restored.open(open.clone()).unwrap();
    let RecoveryResult::Validated(exported) = r.recover() else {
        panic!("restored history")
    };
    assert_eq!(exported.attempts, authentic.attempts);
    assert_eq!(exported.revisions, authentic.revisions);
    for (offset, retained) in authentic.attempts.iter().enumerate() {
        let mut q = resolution(
            retained.attempt.clone(),
            11 + u64::try_from(offset).unwrap(),
            OperationKind::Inspect,
        );
        q.owner = open.owner.clone();
        let AttemptPhase::Terminal(outcome) = &retained.phase else {
            panic!("historical terminal")
        };
        assert_eq!(
            r.inspect(q).result,
            ResolveResult::Terminal(Box::new(Terminal {
                attempt: retained.attempt.clone(),
                outcome: outcome.clone()
            }))
        );
    }
}

#[test]
fn restored_started_and_committed_cuts_must_fit_their_immutable_windows() {
    for hold in [false, true] {
        let mut host = MemoryHost::new();
        let mut s = host.open(open_request()).unwrap();
        let a = prepared(s.prepare(prepare_request(
            binding("x", CommitKind::AcceptedBatch, 1),
            1,
        )));
        s.hold_publication(hold);
        let result = s
            .begin(BeginRequest {
                request: resolution(a, 2, OperationKind::Begin),
                current: cut(),
            })
            .result;
        assert!(matches!(
            result,
            ResolveResult::Started { .. } | ResolveResult::Terminal(_)
        ));
        let RecoveryResult::Validated(authentic) = s.recover() else {
            panic!("authentic protected start")
        };
        MemoryHost::restore((*authentic).clone()).unwrap();
        for before in [true, false] {
            let mut bad = (*authentic).clone();
            let window = &mut bad.attempts[0]
                .attempt
                .binding
                .prestart
                .as_mut()
                .unwrap()
                .windows[0];
            if before {
                window.not_before_ms = 22;
            } else {
                window.not_after_ms = 20;
            }
            assert!(
                matches!(MemoryHost::restore(bad), Err(Fault::Integrity)),
                "protected phase hold{hold} before{before} cannot claim a cut outside immutable windows"
            );
        }
    }
}

#[test]
fn restored_noncommit_must_retain_its_bindings_unchanged_revision() {
    let mut host = MemoryHost::restore(authentic_revision_history(1)).unwrap();
    let mut open = open_request();
    open.owner.generation = 2;
    let mut s = host.open(open.clone()).unwrap();
    let mut b = binding("x", CommitKind::Candidate, 2);
    b.expected_revision = 1;
    let mut q = prepare_request(b, 3);
    q.owner = open.owner.clone();
    let a = prepared(s.prepare(q));
    let mut q = resolution(a, 4, OperationKind::Fence);
    q.owner = open.owner;
    assert!(
        matches!(s.request_fence(q).result, ResolveResult::Terminal(t) if matches!(t.outcome, TerminalOutcome::NonCommit { revision: 1, .. }))
    );
    let RecoveryResult::Validated(authentic) = s.recover() else {
        panic!("authentic fence history")
    };
    MemoryHost::restore((*authentic).clone()).unwrap();
    let mut bad = *authentic;
    let AttemptPhase::Terminal(TerminalOutcome::NonCommit { revision, .. }) =
        &mut bad.attempts[1].phase
    else {
        panic!("original negative terminal")
    };
    *revision = 0;
    assert!(
        matches!(MemoryHost::restore(bad), Err(Fault::Integrity)),
        "NonCommit cannot change the binding's application revision"
    );
}
