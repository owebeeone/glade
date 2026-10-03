//! Remediation regressions for trusted development assembly, independently of step.
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use support::*;

fn publish(
    s: &mut storage_host::MemorySession,
    p: &CommitPlan,
    prepare_id: u64,
    begin_id: u64,
) -> Terminal {
    let owner = storage_host::open_request().owner;
    let prepared = s.prepare(PrepareRequest {
        owner: owner.clone(),
        invocation: InvocationId {
            namespace: [2; 32],
            number: prepare_id,
        },
        binding: binding_for(p),
    });
    let PrepareResult::Prepared(a) = prepared.result else {
        panic!("actual preparation required")
    };
    let reply = s.begin(BeginRequest {
        request: ResolveRequest {
            owner,
            invocation: InvocationId {
                namespace: [2; 32],
                number: begin_id,
            },
            operation: OperationKind::Begin,
            attempt: *a,
        },
        current: p.local_precondition.clone().unwrap(),
    });
    let ResolveResult::Terminal(t) = reply.result else {
        panic!("terminal required")
    };
    *t
}
#[test]
fn configured_provider_matches_core_cut_and_changes_independently() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let p = staged_plan(&d, c);
    let mut fixture = FixtureReplica::new(initial(d.clone())).unwrap();
    let t = publish(&mut fixture.session, &p, 1, 2);
    assert!(
        matches!(t.outcome,TerminalOutcome::Committed{ref receipts,ref start_cut,..} if receipts==&p.acceptances.iter().map(|a|a.receipt.clone()).collect::<Vec<_>>() && start_cut==&p.local_precondition)
    );
    let mut stale = FixtureReplica::new(initial(d)).unwrap();
    stale
        .observe_policy(
            p.instance.clone(),
            LocalPrecondition {
                policy: [9; 32],
                time: TimeInterval {
                    earliest_ms: 22,
                    latest_ms: 23,
                },
            },
        )
        .unwrap();
    assert!(matches!(
        publish(&mut stale.session, &p, 1, 2).outcome,
        TerminalOutcome::NonCommit { revision: 0, .. }
    ));
}
#[test]
fn seeded_custody_is_explicit_and_consecutive_port_publications_keep_history() {
    let d = descriptor("x");
    let a = candidate(&d, "seed", 0, None, vec![], insert("s", None, "S"));
    let mut state = initial(d.clone());
    seed(&mut state, a.clone());
    let expected = Bytes::new(format!("{:?}", state.instances[&d.instance]).into_bytes());
    let mut fixture = FixtureReplica::new(state).unwrap();
    let RecoveryResult::Validated(r) = fixture.session.recover() else {
        panic!("trusted fixture seed")
    };
    assert_eq!(
        fixture.session.fixture_seed().genesis_custody[&d.instance],
        expected
    );
    assert_eq!(r.revisions[&d.instance], 0);
    let mut p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let first = publish(&mut fixture.session, &p, 1, 2);
    p.id = 9;
    p.expected_revision = 1;
    p.batch = Bytes::new(b"second-exact-batch".to_vec());
    let second = publish(&mut fixture.session, &p, 3, 4);
    assert!(matches!(
        first.outcome,
        TerminalOutcome::Committed { revision: 1, .. }
    ));
    assert!(matches!(
        second.outcome,
        TerminalOutcome::Committed { revision: 2, .. }
    ));
    let RecoveryResult::Validated(r) = fixture.session.recover() else {
        panic!("retained ledger")
    };
    assert_eq!(r.revisions[&d.instance], 2);
    assert_eq!(
        fixture.session.fixture_seed().genesis_custody[&d.instance],
        expected
    );
    assert!(
        r.attempts.iter().any(|a| a.attempt == first.attempt
            && a.phase == AttemptPhase::Terminal(first.outcome.clone()))
    );
}
#[test]
fn lookup_fixture_owns_full_request_and_bounded_history_not_only_secondary_index() {
    let d = descriptor("x");
    let p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let mut state = initial(d);
    register_attempt(&mut state, &p);
    let issued_prepare = PrepareRequest {
        owner: state.storage_owner.clone().unwrap(),
        invocation: InvocationId {
            namespace: state.invocation_namespace,
            number: 7,
        },
        binding: binding_for(&p),
    };
    assert_eq!(
        state.retired_storage_invocations[&issued_prepare.invocation],
        StorageInvocation::Prepare(issued_prepare.clone())
    );
    assert_eq!(state.next_effect_id, 9);
    let q = resolve_request(&p, 40, OperationKind::Inspect);
    register_lookup(&mut state, q.clone()).unwrap();
    assert_eq!(
        state.storage_invocations[&q.invocation],
        StorageInvocation::Resolve(q.clone())
    );
    assert_eq!(state.lookups[&40], q);
    assert_eq!(state.next_effect_id, 41);
    let old = state.clone();
    let mut wrong = q;
    wrong.invocation.namespace = [9; 32];
    assert!(register_lookup(&mut state, wrong).is_err());
    assert_eq!(state, old);
    state.storage_limits.as_mut().unwrap().invocation_history =
        state.storage_invocations.len() + state.retired_storage_invocations.len();
    let before = state.clone();
    let q = resolve_request(&p, 128, OperationKind::Inspect);
    assert_eq!(register_lookup(&mut state, q), Err(Fault::Capacity));
    assert_eq!(state, before);
}
#[test]
fn mandatory_text_replica_journeys_do_not_construct_fresh_host_per_drive_call() {
    let source = include_str!("../examples/text_admission_trace.rs");
    assert!(
        !source.contains("drive("),
        "every text journey must use its persistent replica assembly"
    );
    assert!(source.contains("FixtureReplica::new"));
}

#[test]
fn trusted_seed_retains_reservations_outcomes_and_rejects_inconsistent_history() {
    let d = descriptor("x");
    let p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let mut state = initial(d.clone());
    state.unknown_commits.insert(p.id, p.clone());
    register_attempt(&mut state, &p);
    let fixture = FixtureReplica::new(state.clone()).unwrap();
    let seed = fixture.session.fixture_seed();
    assert_eq!(seed.recovery.attempts.len(), 1);
    assert_eq!(seed.recovery.attempts[0].attempt, attempt(&p));
    assert_eq!(seed.recovery.attempts[0].phase, AttemptPhase::Reserved);
    assert!(
        seed.recovery.invocation_count
            >= state.storage_invocations.len() + state.retired_storage_invocations.len()
    );
    let mut corrupt = state.clone();
    corrupt.next_effect_id = 8;
    assert!(
        FixtureReplica::new(corrupt).is_err(),
        "retained issuance counter cannot regress below an issued request"
    );
    let mut corrupt = state.clone();
    corrupt
        .lookups
        .insert(40, resolve_request(&p, 40, OperationKind::Inspect));
    assert!(
        FixtureReplica::new(corrupt).is_err(),
        "secondary index cannot create issuance"
    );
    let mut corrupt = state;
    corrupt.instances.get_mut(&d.instance).unwrap().revision = 1;
    assert!(
        FixtureReplica::new(corrupt).is_err(),
        "revision cannot manufacture committed custody/outcome"
    );
}

#[test]
fn seeded_terminal_requires_original_custody_and_accounting() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let p = staged_plan(&d, c.clone());
    let mut state = initial(d.clone());
    register_attempt(&mut state, &p);
    let ResolveResult::Terminal(t) = committed_result(&p) else {
        unreachable!()
    };
    state
        .attempts
        .get_mut(&t.attempt.binding.plan)
        .unwrap()
        .phase = AttemptPhase::Terminal(t.outcome.clone());
    state.terminal_results.insert(t.attempt.id.clone(), *t);
    let q = resolve_request(&p, 8, OperationKind::Begin);
    let issued = state.storage_invocations.remove(&q.invocation).unwrap();
    state
        .retired_storage_invocations
        .insert(q.invocation, issued);
    state.instances.get_mut(&d.instance).unwrap().revision = 1;
    assert!(
        FixtureReplica::new(state.clone()).is_err(),
        "terminal without original custody cannot seed a usable host"
    );
    seed(&mut state, c);
    assert!(
        FixtureReplica::new(state.clone()).is_err(),
        "terminal charges cannot disappear on fixture recovery"
    );
    state.instances.get_mut(&d.instance).unwrap().used = p.charges.clone();
    let mut fixture = FixtureReplica::new(state).unwrap();
    let old = fixture
        .session
        .inspect(resolve_request(&p, 40, OperationKind::Inspect));
    assert_eq!(old.result, committed_result(&p));
    let seed = fixture.session.fixture_seed();
    let mut restored = storage_host::MemoryHost::from_owned_fixture(seed).unwrap();
    assert_eq!(
        restored
            .inspect(resolve_request(&p, 41, OperationKind::Inspect))
            .result,
        old.result
    );
}

#[test]
fn historical_prior_cut_fixture_registers_multiline_inspect_through_full_helper() {
    let source = include_str!("records_host_contract.rs");
    let historical = source
        .split("fn icd006_delayed_prior_cut_committed_callback_retains_historical_admission()")
        .nth(1)
        .unwrap();
    assert!(
        !historical.contains("unknown\n        .lookups\n        .insert"),
        "secondary-only multiline lookup insertion leaves Inspect40 unissued"
    );
    assert!(historical.contains("register_lookup(&mut unknown"));
    assert!(historical.contains("unknown.storage_invocations"));
}
