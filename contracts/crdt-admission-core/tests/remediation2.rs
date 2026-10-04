//! Code-2 P2-5: busy-instance refusal must retain honest history completeness.
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use support::*;

fn verification(t: &Transition) -> Query {
    t.effects
        .iter()
        .find_map(|e| {
            if let Effect::Verify(q) = e {
                Some(q.clone())
            } else {
                None
            }
        })
        .expect("issued full verification")
}
fn verify(s: &State, q: Query, c: &Candidate) -> Transition {
    step(
        s,
        Event::EvidenceResolved(Box::new(EvidenceReply {
            query: q,
            verdict: Verdict::Verified(Box::new(facts(c))),
        })),
    )
}
fn begin_held(f: &mut FixtureReplica, t: Transition) -> (CommitPlan, AttemptBinding) {
    let (plan, request) = t
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Prepare { plan, request } = e {
                Some((plan.clone(), request.clone()))
            } else {
                None
            }
        })
        .expect("actual immutable A prepare");
    let prepared = f.session.prepare(request);
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
        .expect("actual issued Begin");
    let started = f.session.begin(begin.clone());
    assert!(matches!(started.result, ResolveResult::Started { .. }));
    f.state = step(&t.state, Event::CommitResolved(started)).state;
    assert_eq!(f.state.unknown_commits.get(&plan.id), Some(&plan));
    (plan, begin.request.attempt)
}
fn settle_and_check(
    f: &mut FixtureReplica,
    d: &Descriptor,
    a: &Candidate,
    b: &Candidate,
    plan: &CommitPlan,
    attempt: &AttemptBinding,
    expect_incomplete: bool,
) {
    assert!(matches!(
        f.session
            .publish(attempt, f.state.storage_owner.clone().unwrap()),
        ResolveResult::Terminal(_)
    ));
    let resumed = step(&f.state, Event::ResumeRecovery(d.instance.clone()));
    f.state = resumed.state;
    // The busy branch may already have issued A's Inspect. Use that exact owned
    // request instead of inventing a callback or reconstructing a new session.
    let inspect = f
        .state
        .lookups
        .values()
        .find(|q| q.attempt == *attempt)
        .unwrap()
        .clone();
    let reply = f.session.inspect(inspect);
    assert!(matches!(reply.result, ResolveResult::Terminal(_)));
    f.state = step(&f.state, Event::LookupResolved(reply)).state;
    let v = &f.state.instances[&d.instance];
    assert_eq!(v.revision, plan.expected_revision + 1);
    assert_eq!(v.used, plan.charges);
    assert_eq!(v.accepted.len(), 1);
    assert_eq!(v.accepted[0].candidate.operation, a.operation);
    assert_eq!(v.accepted[0].receipt, plan.acceptances[0].receipt);
    assert!(
        !v.accepted
            .iter()
            .any(|old| old.candidate.operation == b.operation)
    );
    assert!(f.state.commits.is_empty());
    assert!(f.state.unknown_commits.is_empty());
    assert_eq!(
        cut(&f.state, d.instance.clone()).complete_local,
        !expect_incomplete
    );
    let custody = v.clone();
    let retry = step(&f.state, Event::OfferReplica(Box::new(a.clone())));
    assert!(retry.effects.contains(&Effect::Report(Outcome::ExactRetry(
        plan.acceptances[0].receipt.clone()
    ))));
    assert_eq!(retry.state.instances[&d.instance], custody);
    let resumed = step(&f.state, Event::ResumeRecovery(d.instance.clone()));
    assert!(
        !resumed
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Lookup(_) | Effect::RecoverPlan(_)))
    );
    assert_eq!(resumed.state.instances[&d.instance], custody);
    assert_eq!(
        cut(&resumed.state, d.instance.clone()).complete_local,
        !expect_incomplete
    );
    let mut restored =
        FixtureReplica::new(resumed.state).expect("validated exact terminal/custody restoration");
    restored.advance(Event::ResumeRecovery(d.instance.clone()), facts(a));
    assert_eq!(restored.state.instances[&d.instance], custody);
    assert_eq!(
        cut(&restored.state, d.instance.clone()).complete_local,
        !expect_incomplete
    );
}
fn schedule(park_verified: bool, historical: bool) {
    let d = descriptor("x");
    let y = descriptor("y");
    let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let b = candidate(&d, "b", 0, None, vec![], insert("b", None, "B"));
    let mut s = initial(d.clone());
    s.instances.extend(initial(y.clone()).instances);
    let mut f = FixtureReplica::new(s.clone()).unwrap();
    f.session.hold_publication(true);
    let offered_a = step(&s, Event::OfferReplica(Box::new(a.clone())));
    let aq = verification(&offered_a);
    let (before_a, bq) = if park_verified {
        let offered_b = step(
            &offered_a.state,
            if historical {
                Event::OfferReplica(Box::new(b.clone()))
            } else {
                Event::SubmitLocal(Box::new(b.clone()))
            },
        );
        assert_eq!(offered_b.state.queries.len(), 2);
        let q = verification(&offered_b);
        (offered_b.state, Some(q))
    } else {
        (offered_a.state, None)
    };
    let staged_a = verify(&before_a, aq, &a);
    let (plan, attempt) = begin_held(&mut f, staged_a);
    let before_b = f.state.clone();
    let refused = if let Some(q) = bq {
        let verified_b = verify(&before_b, q, &b);
        if !historical {
            let seal = verified_b
                .effects
                .iter()
                .find_map(|e| {
                    if let Effect::Seal(q) = e {
                        Some(q.clone())
                    } else {
                        None
                    }
                })
                .expect("local verification still owns its seal continuation");
            step(
                &verified_b.state,
                Event::EvidenceResolved(Box::new(EvidenceReply {
                    query: seal,
                    verdict: Verdict::Sealed(b.evidence.admission.clone().unwrap()),
                })),
            )
        } else {
            verified_b
        }
    } else {
        step(
            &before_b,
            if historical {
                Event::OfferReplica(Box::new(b.clone()))
            } else {
                Event::SubmitLocal(Box::new(b.clone()))
            },
        )
    };
    assert_eq!(refused.state.unknown_commits.get(&plan.id), Some(&plan));
    assert_eq!(
        refused.state.instances[&d.instance].used,
        before_b.instances[&d.instance].used
    );
    assert_eq!(refused.state.attempts, before_b.attempts);
    assert!(!refused.effects.iter().any(|e| matches!(
        e,
        Effect::Prepare { .. } | Effect::Begin(_) | Effect::Verify(_) | Effect::Seal(_)
    )));
    if !historical {
        assert!(!refused.state.instances[&d.instance].recovery_incomplete);
    }
    let refusal_effects = refused.effects.clone();
    f.state = refused.state;
    assert!(
        !cut(&f.state, d.instance.clone()).complete_local,
        "A remains unresolved"
    );
    // Execute the initial lookup while A is Pending before Y advances the
    // actual provider invocation floor. No reply acquires invented authority.
    let pending = f
        .state
        .lookups
        .values()
        .find(|q| q.attempt == attempt)
        .unwrap()
        .clone();
    let pending_reply = f.session.inspect(pending);
    assert_eq!(pending_reply.result, ResolveResult::Pending);
    f.state = step(&f.state, Event::LookupResolved(pending_reply)).state;
    // Y commits through the same caller-owned actual port while A is unresolved.
    f.session.hold_publication(false);
    let yc = candidate(&y, "y", 0, None, vec![], insert("y", None, "Y"));
    f.advance(Event::SubmitLocal(Box::new(yc.clone())), facts(&yc));
    assert_eq!(f.state.instances[&y.instance].accepted.len(), 1);
    assert_eq!(f.state.unknown_commits.get(&plan.id), Some(&plan));
    settle_and_check(&mut f, &d, &a, &b, &plan, &attempt, historical);
    if historical {
        assert!(
            refusal_effects.contains(&Effect::Report(Outcome::Capacity)),
            "busy historical B must receive an explicit bounded refusal"
        );
        assert!(f.state.instances[&d.instance].recovery_incomplete);
    }
}
#[test]
fn two_issued_queries_verified_historical_b_during_held_started_a_is_explicitly_incomplete() {
    schedule(true, true);
}
#[test]
fn fresh_historical_offer_during_held_started_a_is_explicitly_incomplete() {
    schedule(false, true);
}
#[test]
fn local_intent_only_busy_refusal_does_not_taint_known_history() {
    for park_verified in [false, true] {
        schedule(park_verified, false);
    }
}
