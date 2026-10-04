use super::*;

fn actual_schedule(kind: CommitKind, committed: bool, terminal_first: bool) {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let c = match kind {
        CommitKind::Candidate => {
            candidate(&d, "b", 0, None, refs(&["missing"]), insert("b", None, "B"))
        }
        CommitKind::QualifiedFork => {
            seed(
                &mut s,
                candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
            );
            candidate(&d, "a", 0, None, vec![], insert("rival", None, "R"))
        }
        _ => candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    };
    let mut fixture = FixtureReplica::new(s.clone()).unwrap();
    fixture.session.hold_publication(true);
    let mut f = facts(&c);
    if kind == CommitKind::SecurityEvidence {
        f.payload_valid = false;
    }
    let t = staged(&s, &c, kind == CommitKind::AcceptedBatch, f);
    let (p, request) = prepare(&t);
    assert_eq!(p.kind, kind);
    let prepared = fixture.session.prepare(request);
    let t = step(&t.state, Event::PrepareResolved(prepared));
    let begin = t
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Begin(q) = e {
                Some(q.clone())
            } else {
                None
            }
        })
        .unwrap();
    let delayed = fixture.session.begin(begin.clone());
    assert!(matches!(delayed.result, ResolveResult::Started { .. }));
    s = t.state;
    if !terminal_first {
        s = step(&s, Event::CommitResolved(delayed.clone())).state;
    }
    if committed {
        assert!(matches!(
            fixture
                .session
                .publish(&begin.request.attempt, begin.request.owner.clone()),
            ResolveResult::Terminal(_)
        ));
    } else {
        // Explicit fixture issuance records the full Fence request before execution.
        let q = ResolveRequest {
            operation: OperationKind::Fence,
            invocation: InvocationId {
                namespace: s.invocation_namespace,
                number: s.next_effect_id,
            },
            ..begin.request.clone()
        };
        s.next_effect_id += 1;
        s.storage_invocations
            .insert(q.invocation.clone(), StorageInvocation::Resolve(q.clone()));
        let reply = fixture.session.request_fence(q);
        assert!(matches!(reply.result, ResolveResult::Terminal(_)));
        // Keep this authentic result undelivered: terminal knowledge comes from Inspect.
    }
    let resumed = step(&s, Event::ResumeRecovery(d.instance.clone()));
    let inspect = resumed
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Lookup(q) = e {
                Some(q.clone())
            } else {
                None
            }
        })
        .unwrap();
    let inspected = fixture.session.inspect(inspect);
    let ResolveResult::Terminal(terminal) = &inspected.result else {
        panic!("actual terminal inspect")
    };
    let expected = terminal.outcome.clone();
    let published = step(&resumed.state, Event::LookupResolved(inspected));
    let custody = published.state.instances[&d.instance].clone();
    let terminal_count = published.state.terminal_results.len();
    let final_t = if terminal_first {
        step(&published.state, Event::CommitResolved(delayed))
    } else {
        published
    };
    assert_eq!(
        final_t.state.attempts[&begin.request.attempt.binding.plan].phase,
        AttemptPhase::Terminal(expected)
    );
    assert_eq!(final_t.state.terminal_results.len(), terminal_count);
    assert_eq!(final_t.state.instances[&d.instance], custody);
    assert!(
        !final_t
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Report(Outcome::OutcomeUnknown(_))))
    );
    assert!(
        FixtureReplica::new(final_t.state.clone()).is_ok(),
        "validated restore must preserve terminal grammar"
    );
    let resumed = step(&final_t.state, Event::ResumeRecovery(d.instance));
    assert!(
        !resumed
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Lookup(_) | Effect::RecoverPlan(_)))
    );
}
#[test]
fn held_started_and_terminal_inspect_both_orders_all_four_kinds_and_outcomes() {
    for kind in [
        CommitKind::AcceptedBatch,
        CommitKind::Candidate,
        CommitKind::SecurityEvidence,
        CommitKind::QualifiedFork,
    ] {
        for committed in [true, false] {
            for terminal_first in [true, false] {
                actual_schedule(kind.clone(), committed, terminal_first);
            }
        }
    }
}
#[test]
fn late_pending_and_refused_never_reopen_an_authentic_terminal() {
    for result in [
        ResolveResult::Pending,
        ResolveResult::Refused(Fault::Unavailable),
    ] {
        let d = descriptor("x");
        let p = staged_plan(
            &d,
            candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
        );
        let mut s = initial(d.clone());
        register_attempt(&mut s, &p);
        s.commits.insert(p.id, p.clone());
        let lookup = resolve_request(&p, s.next_effect_id, OperationKind::Inspect);
        register_lookup(&mut s, lookup.clone()).unwrap();
        let committed = step(
            &s,
            Event::LookupResolved(ResolveReply {
                request: lookup,
                result: committed_result(&p),
            }),
        );
        assert_eq!(committed.state.terminal_results.len(), 1);
        let q = resolve_request(&p, 8, OperationKind::Begin);
        let late = step(
            &committed.state,
            Event::CommitResolved(ResolveReply { request: q, result }),
        );
        assert_eq!(late.state.instances, committed.state.instances);
        assert_eq!(late.state.attempts, committed.state.attempts);
        assert!(
            !late
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Report(Outcome::OutcomeUnknown(_))))
        );
    }
}
