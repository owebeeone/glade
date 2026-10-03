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
