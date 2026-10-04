use super::*;
fn stop(s: &State, instance: &Instance) -> Transition {
    let original = s
        .terminal_results
        .values()
        .find(|t| &t.attempt.id.instance == instance)
        .unwrap();
    let request = s
        .retired_storage_invocations
        .values()
        .find_map(|q| match q {
            StorageInvocation::Resolve(q)
                if q.attempt == original.attempt && q.operation == OperationKind::Begin =>
            {
                Some(q.clone())
            }
            _ => None,
        })
        .unwrap();
    step(
        s,
        Event::CommitResolved(ResolveReply {
            request,
            result: ResolveResult::Terminal(Box::new(Terminal {
                attempt: original.attempt.clone(),
                outcome: TerminalOutcome::NonCommit {
                    revision: original.attempt.binding.expected_revision,
                    storage: original.attempt.binding.storage.clone(),
                },
            })),
        }),
    )
}
#[test]
fn parked_verification_and_seal_cannot_create_new_work_after_authenticated_stop() {
    for seal_phase in [false, true] {
        let d = descriptor("x");
        let y = descriptor("y");
        let mut s = initial(d.clone());
        s.instances.extend(initial(y.clone()).instances);
        let b = candidate(&d, "b", 0, None, vec![], insert("b", None, "B"));
        let offered = step(&s, Event::SubmitLocal(Box::new(b.clone())));
        let q = verify(&offered);
        let (parked, reply) = if seal_phase {
            let t = verified(&offered.state, q, &b);
            let q = t
                .effects
                .iter()
                .find_map(|e| {
                    if let Effect::Seal(q) = e {
                        Some(q.clone())
                    } else {
                        None
                    }
                })
                .unwrap();
            (
                t.state,
                EvidenceReply {
                    query: q,
                    verdict: Verdict::Sealed(b.evidence.admission.clone().unwrap()),
                },
            )
        } else {
            (
                offered.state,
                EvidenceReply {
                    query: q,
                    verdict: Verdict::Verified(Box::new(facts(&b))),
                },
            )
        };
        let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
        let committed = drive(parked, Event::SubmitLocal(Box::new(a.clone())), facts(&a));
        let stopped = stop(&committed.state, &d.instance);
        assert!(stopped.state.instances[&d.instance].storage_integrity_failed);
        let continued = step(&stopped.state, Event::EvidenceResolved(Box::new(reply)));
        no_new_work(&continued);
        assert!(
            continued
                .effects
                .contains(&Effect::Report(Outcome::IntegrityConflict))
        );
        assert_eq!(
            continued.state, stopped.state,
            "parked continuation and original custody stay bounded and retained"
        );
        let retry = step(&continued.state, Event::SubmitLocal(Box::new(a.clone())));
        assert!(
            retry.effects.contains(&Effect::Report(Outcome::ExactRetry(
                committed.state.instances[&d.instance].accepted[0]
                    .receipt
                    .clone()
            )))
        );
        let c = candidate(&y, "c", 0, None, vec![], insert("c", None, "C"));
        let yprogress = drive(
            continued.state,
            Event::SubmitLocal(Box::new(c.clone())),
            facts(&c),
        );
        assert_eq!(yprogress.state.instances[&y.instance].accepted.len(), 1);
        assert_eq!(
            yprogress.state.instances[&d.instance],
            stopped.state.instances[&d.instance]
        );
    }
}
#[test]
fn already_qualified_fork_cannot_stage_new_work_after_stop() {
    let d = descriptor("x");
    let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let b = candidate(&d, "a", 0, None, vec![], insert("rival", None, "B"));
    let mut s = initial(d.clone());
    seed(&mut s, a.clone());
    let qualified = drive(s, Event::OfferReplica(Box::new(b.clone())), facts(&b));
    let stopped = stop(&qualified.state, &d.instance);
    let t = step(
        &stopped.state,
        Event::OfferFork(Box::new(ForkEvidence {
            first: a,
            second: b,
        })),
    );
    no_new_work(&t);
    assert_eq!(t.state, stopped.state);
    assert!(
        t.effects
            .contains(&Effect::Report(Outcome::IntegrityConflict))
    );
}
#[test]
fn integrity_stop_preserves_owned_started_attempt_and_genuine_late_commit() {
    let d = descriptor("x");
    let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let b = candidate(&d, "b", 0, None, vec![], insert("b", None, "B"));
    let mut f = FixtureReplica::new(initial(d.clone())).unwrap();
    f.advance(Event::SubmitLocal(Box::new(a.clone())), facts(&a));
    f.session.hold_publication(true);
    f.advance(Event::SubmitLocal(Box::new(b.clone())), facts(&b));
    let owned = f
        .state
        .attempts
        .values()
        .find(|a| !matches!(a.phase, AttemptPhase::Terminal(_)))
        .unwrap()
        .attempt
        .clone();
    let stopped = stop(&f.state, &d.instance);
    assert!(!stopped.state.unknown_commits.is_empty());
    f.state = stopped.state;
    assert!(matches!(
        f.session
            .publish(&owned, f.state.storage_owner.clone().unwrap()),
        ResolveResult::Terminal(_)
    ));
    f.advance(Event::ResumeRecovery(d.instance.clone()), facts(&b));
    assert_eq!(f.state.instances[&d.instance].accepted.len(), 2);
    assert_eq!(f.state.instances[&d.instance].revision, 2);
    assert!(f.state.instances[&d.instance].storage_integrity_failed);
    assert!(f.state.unknown_commits.is_empty());
    assert!(FixtureReplica::new(f.state).is_ok());
}
