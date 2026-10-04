use super::*;
fn persists(t: &Transition, d: &Descriptor) {
    assert!(t.state.instances[&d.instance].recovery_incomplete);
    assert!(!cut(&t.state, d.instance.clone()).complete_local);
    let resumed = step(&t.state, Event::ResumeRecovery(d.instance.clone()));
    assert!(resumed.state.instances[&d.instance].recovery_incomplete);
    assert!(!cut(&resumed.state, d.instance.clone()).complete_local);
    let restored =
        FixtureReplica::new(resumed.state).expect("validated trusted fixture restoration");
    assert!(!cut(&restored.state, d.instance.clone()).complete_local);
}
fn historical_exhaustion(floor: u64) {
    let d = descriptor("x");
    let prior = candidate(&d, "prior", 0, None, vec![], insert("p", None, "P"));
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut s = initial(d.clone());
    seed(&mut s, prior);
    s.next_effect_id = floor;
    let offered = step(&s, Event::OfferReplica(Box::new(c.clone())));
    let t = if floor == u64::MAX {
        offered
    } else {
        verified(&offered.state, verify(&offered), &c)
    };
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert_eq!(t.state.next_effect_id, u64::MAX);
    assert!(t.state.queries.is_empty());
    no_new_work(&t);
    assert_eq!(
        t.state.instances[&d.instance].accepted,
        s.instances[&d.instance].accepted
    );
    assert_eq!(
        t.state.instances[&d.instance].used,
        s.instances[&d.instance].used
    );
    assert_eq!(t.state.instances[&d.instance].revision, 0);
    persists(&t, &d);
}
#[test]
fn historical_query_exhaustion_remains_sticky_through_resume_restore() {
    historical_exhaustion(u64::MAX);
}
#[test]
fn historical_plan_exhaustion_remains_sticky_through_resume_restore() {
    historical_exhaustion(u64::MAX - 1);
}
#[test]
fn local_intent_counter_refusal_alone_does_not_taint_known_history() {
    for floor in [u64::MAX, u64::MAX - 1] {
        let d = descriptor("x");
        let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
        let mut s = initial(d.clone());
        s.next_effect_id = floor;
        let offered = step(&s, Event::SubmitLocal(Box::new(c.clone())));
        let t = if floor == u64::MAX {
            offered
        } else {
            verified(&offered.state, verify(&offered), &c)
        };
        assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
        assert!(!t.state.instances[&d.instance].recovery_incomplete);
        assert!(cut(&t.state, d.instance).complete_local);
        no_new_work(&t);
    }
}
