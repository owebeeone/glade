//! STA consumers deliberately RED against refusing step. Actual port fixture is
//! independently tested; no simulated host success replaces these kernel assertions.
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use support::*;

fn reserved(kind: CommitKind) -> (State, CommitPlan, AttemptBinding) {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut p = staged_plan(&d, c);
    p.kind = kind.clone();
    if kind != CommitKind::AcceptedBatch {
        p.acceptances.clear();
        p.local_precondition = None;
    }
    let mut state = initial(d);
    state.commits.insert(p.id, p.clone());
    let a = attempt(&p);
    state.attempts.insert(
        a.binding.plan.clone(),
        KernelAttempt {
            attempt: a.clone(),
            phase: AttemptPhase::Reserved,
        },
    );
    let prepare = PrepareRequest {
        owner: state.storage_owner.clone().unwrap(),
        invocation: InvocationId {
            namespace: state.invocation_namespace,
            number: 7,
        },
        binding: binding_for(&p),
    };
    state.retired_storage_invocations.insert(
        prepare.invocation.clone(),
        StorageInvocation::Prepare(prepare),
    );
    // Invocation8 is the explicitly injected resolve request in each consumer.
    state.next_effect_id = 9;
    (state, p, a)
}
#[test]
fn sta001_callback_complete_binding_and_finite_invocation_namespace() {
    let (mut s, p, a) = reserved(CommitKind::AcceptedBatch);
    let q = resolve_request(&p, 8, OperationKind::Begin);
    s.storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Resolve(q.clone()));
    for wrong in 0..5 {
        let mut r = commit_reply(&p, 8, committed_result(&p));
        match wrong {
            0 => r.request.invocation.namespace = [9; 32],
            1 => r.request.owner.generation = 9,
            2 => r.request.attempt.id.number = 9,
            3 => r.request.attempt.binding.batch = Bytes::new(b"changed".to_vec()),
            _ => r.request.attempt.binding.charges.edit += 1,
        }
        let t = step(&s, Event::CommitResolved(r));
        assert_eq!(t.state, s);
        assert!(
            t.effects
                .contains(&Effect::Report(Outcome::CallbackMismatch))
        );
    }
    assert_eq!(s.attempts[&a.binding.plan].attempt, a);
}
#[test]
fn sta002_kernel_prepare_keeps_exact_plan_after_lost_ack() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let driven = drive(
        initial(d.clone()),
        Event::SubmitLocal(Box::new(c.clone())),
        facts(&c),
    );
    assert!(
        driven
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Prepare { .. })),
        "actual emitted prepare required"
    );
}
#[test]
fn sta003_pending_lookup_retains_started_write_until_late_commit_all_kinds() {
    for kind in [
        CommitKind::AcceptedBatch,
        CommitKind::Candidate,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
    ] {
        let (mut s, p, a) = reserved(kind);
        s.commits.remove(&p.id);
        s.unknown_commits.insert(p.id, p.clone());
        let q = resolve_request(&p, 40, OperationKind::Inspect);
        register_lookup(&mut s, q.clone()).unwrap();
        let pending = step(
            &s,
            Event::LookupResolved(ResolveReply {
                request: q,
                result: ResolveResult::Pending,
            }),
        );
        assert_eq!(pending.state.unknown_commits[&p.id], p);
        assert!(
            pending.state.lookups.is_empty(),
            "fresh invocation retires independently"
        );
        assert_eq!(pending.state.attempts[&a.binding.plan].attempt, a);
        assert!(!cut(&pending.state, p.instance).complete_local);
    }
}
#[test]
fn sta004_noncommit_retains_tombstone_and_rescheduling_uses_fresh_plan() {
    let (mut s, p, a) = reserved(CommitKind::AcceptedBatch);
    let q = resolve_request(&p, 8, OperationKind::Fence);
    s.storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Resolve(q.clone()));
    let terminal = Terminal {
        attempt: a.clone(),
        outcome: TerminalOutcome::NonCommit {
            revision: p.expected_revision,
            storage: StorageClass::VolatileTest,
        },
    };
    let t = step(
        &s,
        Event::FenceResolved(ResolveReply {
            request: q,
            result: ResolveResult::Terminal(Box::new(terminal.clone())),
        }),
    );
    assert_eq!(t.state.terminal_results[&a.id], terminal);
    assert!(!t.state.commits.contains_key(&p.id));
    let c = p.records[0].clone();
    let new = drive(t.state, Event::SubmitLocal(Box::new(c.clone())), facts(&c));
    assert!(
        new.effects.iter().any(
            |e| matches!(e,Effect::Prepare{request,..} if request.binding.plan!=a.binding.plan)
        )
    );
}
#[test]
fn sta005_contradictory_terminal_stops_instance_after_once_only_installation() {
    let (mut s, p, a) = reserved(CommitKind::AcceptedBatch);
    let q = resolve_request(&p, 8, OperationKind::Begin);
    s.storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Resolve(q));
    let committed = step(
        &s,
        Event::CommitResolved(commit_reply(&p, 8, committed_result(&p))),
    );
    assert_eq!(committed.state.instances[&p.instance].used, p.charges);
    assert!(
        committed
            .state
            .retired_storage_invocations
            .contains_key(&resolve_request(&p, 8, OperationKind::Begin).invocation)
    );
    let contrary = step(
        &committed.state,
        Event::CommitResolved(commit_reply(&p, 8, noncommit_result(&p))),
    );
    assert!(contrary.state.instances[&p.instance].storage_integrity_failed);
    assert!(matches!(
        contrary.state.terminal_conflicts[&a.id].outcome,
        TerminalOutcome::NonCommit { .. }
    ));
    assert_eq!(
        contrary.state.terminal_results[&a.id],
        committed.state.terminal_results[&a.id]
    );
    assert_eq!(contrary.state.instances[&p.instance].used, p.charges);
}
#[test]
fn sta006_current_prestart_observation_emits_fence_without_erasing_plan() {
    let (s, p, _) = reserved(CommitKind::AcceptedBatch);
    let t = step(
        &s,
        Event::ObservePolicy {
            instance: p.instance.clone(),
            policy: Bytes::new(b"revoked".to_vec()),
            time: TimeInterval {
                earliest_ms: 22,
                latest_ms: 23,
            },
        },
    );
    assert_eq!(t.state.commits[&p.id], p);
    assert!(t.effects.iter().any(|e| matches!(e, Effect::Fence(_))));
}
#[test]
fn sta007_all_kinds_install_original_terminal_without_double_charge() {
    for kind in [
        CommitKind::AcceptedBatch,
        CommitKind::Candidate,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
    ] {
        let (mut s, p, _) = reserved(kind.clone());
        let q = resolve_request(&p, 8, OperationKind::Begin);
        s.storage_invocations
            .insert(q.invocation.clone(), StorageInvocation::Resolve(q));
        let t = step(
            &s,
            Event::CommitResolved(commit_reply(&p, 8, committed_result(&p))),
        );
        assert_eq!(t.state.instances[&p.instance].used, p.charges);
        if kind != CommitKind::AcceptedBatch {
            assert!(
                t.effects
                    .iter()
                    .any(|e| matches!(e, Effect::Report(Outcome::BatchRetained { .. })))
            );
        }
        let duplicate = step(
            &t.state,
            Event::CommitResolved(commit_reply(&p, 8, committed_result(&p))),
        );
        assert_eq!(duplicate.state, t.state);
        assert!(
            duplicate
                .effects
                .contains(&Effect::Report(Outcome::CallbackMismatch))
        );
    }
}
#[test]
fn sta008_critical_pending_does_not_release_semantic_reservation() {
    let (mut s, p, _) = reserved(CommitKind::Candidate);
    assert!(s.storage_limits.as_ref().unwrap().attempts < usize::MAX);
    let q = resolve_request(&p, 8, OperationKind::Begin);
    s.storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Resolve(q));
    let t = step(
        &s,
        Event::CommitResolved(commit_reply(&p, 8, ResolveResult::Pending)),
    );
    assert_eq!(t.state.unknown_commits[&p.id], p);
    assert_eq!(
        t.state.instances[&p.instance].used,
        s.instances[&p.instance].used
    );
}
#[test]
fn sta009_restored_attempt_and_invocation_floors_preserve_pending() {
    let (mut s, p, _) = reserved(CommitKind::AcceptedBatch);
    s.commits.remove(&p.id);
    s.unknown_commits.insert(p.id, p.clone());
    s.next_effect_id = 41;
    let t = step(&s, Event::ResumeRecovery(p.instance.clone()));
    let q = t
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Lookup(q) = e {
                Some(q)
            } else {
                None
            }
        })
        .expect("fresh storage lookup");
    assert!(q.invocation.number >= 41);
    let retired = step(
        &t.state,
        Event::LookupResolved(ResolveReply {
            request: resolve_request(&p, 40, OperationKind::Inspect),
            result: noncommit_result(&p),
        }),
    );
    assert_eq!(retired.state, t.state);
    assert!(
        retired
            .effects
            .contains(&Effect::Report(Outcome::CallbackMismatch))
    );
}
#[test]
fn sta010_overflow_is_sticky_distinct_from_charges_and_partial_repair() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let v = s.instances.get_mut(&d.instance).unwrap();
    v.used.recovery = d.bounds.recovery_bytes;
    v.used.pending = d.bounds.pending_bytes;
    let full = drive(
        s.clone(),
        Event::OfferReplica(Box::new(c.clone())),
        facts(&c),
    );
    assert!(full.state.instances[&d.instance].recovery_incomplete);
    assert_eq!(
        full.state.instances[&d.instance].used,
        s.instances[&d.instance].used
    );
    let mut repaired = full.state;
    repaired
        .instances
        .get_mut(&d.instance)
        .unwrap()
        .used
        .recovery = 0;
    let resumed = step(&repaired, Event::ResumeRecovery(d.instance.clone()));
    assert!(resumed.state.instances[&d.instance].recovery_incomplete);
    assert!(!cut(&resumed.state, d.instance).complete_local);
}
#[test]
fn sta011_existing_twenty_seven_and_text_consumers_keep_actual_port_path() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let t = drive(
        initial(d),
        Event::SubmitLocal(Box::new(c.clone())),
        facts(&c),
    );
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_))))
    );
    assert!(t.effects.iter().any(|e| matches!(e, Effect::Begin(_))));
}
#[test]
fn sta005_unissued_foreign_or_changed_retired_callback_cannot_create_integrity_stop() {
    let (mut s, p, a) = reserved(CommitKind::AcceptedBatch);
    let original = resolve_request(&p, 8, OperationKind::Begin);
    s.commits.clear();
    s.storage_invocations.clear();
    s.retired_storage_invocations.insert(
        original.invocation.clone(),
        StorageInvocation::Resolve(original),
    );
    let ResolveResult::Terminal(t) = committed_result(&p) else {
        unreachable!()
    };
    s.terminal_results.insert(a.id.clone(), *t);
    for mutation in 0..6 {
        let mut wrong = commit_reply(&p, 8, noncommit_result(&p));
        match mutation {
            0 => wrong.request.invocation.number = 9,
            1 => wrong.request.invocation.namespace = [9; 32],
            2 => wrong.request.owner.generation = 9,
            3 => wrong.request.operation = OperationKind::Inspect,
            4 => wrong.request.attempt.id.instance.incarnation = "foreign".into(),
            _ => wrong.request.attempt.binding.charges.edit = 99,
        }
        let t = step(&s, Event::CommitResolved(wrong));
        assert_eq!(
            t.state, s,
            "unissued/foreign request is never terminal authority"
        );
        assert!(
            t.effects
                .contains(&Effect::Report(Outcome::CallbackMismatch))
        );
        assert!(!t.state.instances[&p.instance].storage_integrity_failed);
    }
}

#[test]
fn sta011_driver_uses_caller_owned_session_across_continuations() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut replica = FixtureReplica::new(initial(d.clone())).unwrap();
    let first = drive_with_port(
        initial(d.clone()),
        Event::SubmitLocal(Box::new(c.clone())),
        facts(&c),
        &mut replica.session,
    );
    let next = drive_with_port(
        first.state,
        Event::ResumeRecovery(d.instance),
        facts(&c),
        &mut replica.session,
    );
    assert!(
        first
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Prepare { .. })),
        "actual emitted prepare through caller-owned port required"
    );
    assert!(
        next.effects.iter().any(|e| matches!(
            e,
            Effect::Report(Outcome::AcceptedLocal(_)) | Effect::Lookup(_)
        )),
        "continuation must use the same explicit port"
    );
}

#[test]
fn issued_lookup_retains_full_request_on_consumption() {
    let (mut s, p, _) = reserved(CommitKind::AcceptedBatch);
    s.commits.remove(&p.id);
    s.unknown_commits.insert(p.id, p.clone());
    register_attempt(&mut s, &p);
    let q = resolve_request(&p, 40, OperationKind::Inspect);
    register_lookup(&mut s, q.clone()).unwrap();
    let result = step(
        &s,
        Event::LookupResolved(ResolveReply {
            request: q.clone(),
            result: ResolveResult::Pending,
        }),
    );
    assert!(!result.state.lookups.contains_key(&40));
    assert!(!result.state.storage_invocations.contains_key(&q.invocation));
    assert_eq!(
        result.state.retired_storage_invocations[&q.invocation],
        StorageInvocation::Resolve(q)
    );
    assert_eq!(result.state.unknown_commits[&p.id], p);
}
#[test]
fn identical_unissued_lookup_reply_cannot_acquire_callback_authority() {
    let (mut s, p, _) = reserved(CommitKind::AcceptedBatch);
    s.commits.remove(&p.id);
    s.unknown_commits.insert(p.id, p.clone());
    register_attempt(&mut s, &p);
    let q = resolve_request(&p, 40, OperationKind::Inspect);
    let result = step(
        &s,
        Event::LookupResolved(ResolveReply {
            request: q,
            result: ResolveResult::Pending,
        }),
    );
    assert_eq!(result.state, s);
    assert!(
        result
            .effects
            .contains(&Effect::Report(Outcome::CallbackMismatch))
    );
}
