use super::*;
fn bounded(s: &State, d: &Descriptor, effects: &[Effect]) {
    let limit = s.instances[&d.instance].descriptor.bounds.pending_items;
    assert!(s.instances[&d.instance].pending.len() <= limit);
    for e in effects {
        if let Effect::RequestMissing(slots) | Effect::Report(Outcome::MissingHistory(slots)) = e {
            assert!(slots.len() <= limit);
        }
    }
    assert!(cut(s, d.instance.clone()).pending.len() <= limit);
}
fn limited(d: &Descriptor, n: usize) -> State {
    let mut s = initial(d.clone());
    s.instances
        .get_mut(&d.instance)
        .unwrap()
        .descriptor
        .bounds
        .pending_items = n;
    s
}
#[test]
fn prospective_two_missing_slots_refuse_before_emission_or_candidate_staging() {
    let d = descriptor("x");
    let s = limited(&d, 1);
    let c = candidate(&d, "a", 0, None, refs(&["b", "c"]), insert("a", None, "A"));
    let t = staged(&s, &c, false, facts(&c));
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    no_new_work(&t);
    assert!(!t.effects.iter().any(|e| matches!(
        e,
        Effect::RequestMissing(_) | Effect::Report(Outcome::MissingHistory(_))
    )));
    assert_eq!(
        t.state.instances[&d.instance].used,
        s.instances[&d.instance].used
    );
    assert!(t.state.instances[&d.instance].candidates.is_empty());
    assert!(t.state.instances[&d.instance].recovery_incomplete);
    bounded(&t.state, &d, &t.effects);
}
#[test]
fn exact_slot_limit_and_overlap_union_commit_without_duplicate_reservations() {
    let d = descriptor("x");
    let s = limited(&d, 2);
    let c = candidate(&d, "a", 0, None, refs(&["b", "c"]), insert("a", None, "A"));
    let t = drive(s, Event::OfferReplica(Box::new(c.clone())), facts(&c));
    assert_eq!(t.state.instances[&d.instance].candidates, vec![c]);
    assert_eq!(t.state.instances[&d.instance].pending.len(), 2);
    bounded(&t.state, &d, &t.effects);
    let second = candidate(&d, "d", 0, None, refs(&["b", "c"]), insert("d", None, "D"));
    let t = drive(
        t.state,
        Event::OfferReplica(Box::new(second.clone())),
        facts(&second),
    );
    assert_eq!(t.state.instances[&d.instance].candidates.len(), 2);
    assert_eq!(t.state.instances[&d.instance].pending.len(), 2);
    bounded(&t.state, &d, &t.effects);
}
#[test]
fn multiple_candidates_and_frontier_slots_share_deduplicated_prospective_cap() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, refs(&["b"]), insert("a", None, "A"));
    let t = drive(
        limited(&d, 2),
        Event::OfferReplica(Box::new(c.clone())),
        facts(&c),
    );
    let frontier = step(
        &t.state,
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: BTreeMap::from([("c".into(), (0, [7; 32]))]),
        },
    );
    bounded(&frontier.state, &d, &frontier.effects);
    let second = candidate(&d, "d", 0, None, refs(&["e"]), insert("d", None, "D"));
    let refused = staged(&frontier.state, &second, false, facts(&second));
    assert!(refused.effects.contains(&Effect::Report(Outcome::Capacity)));
    no_new_work(&refused);
    assert_eq!(
        refused.state.instances[&d.instance].candidates,
        frontier.state.instances[&d.instance].candidates
    );
    assert_eq!(
        refused.state.instances[&d.instance].used,
        frontier.state.instances[&d.instance].used
    );
    bounded(&refused.state, &d, &refused.effects);
}
#[test]
fn frontier_cannot_steal_slots_reserved_by_an_unpublished_candidate_plan() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, refs(&["b"]), insert("a", None, "A"));
    let staged = staged(&limited(&d, 1), &c, false, facts(&c));
    prepare(&staged);
    let t = step(
        &staged.state,
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: BTreeMap::from([("c".into(), (0, [7; 32]))]),
        },
    );
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert!(t.state.instances[&d.instance].pending.is_empty());
    assert_eq!(t.state.commits, staged.state.commits);
    assert_eq!(t.state.preparing, staged.state.preparing);
    bounded(&t.state, &d, &t.effects);
}
#[test]
fn predecessor_and_ref_to_the_same_missing_slot_emit_one_request() {
    let d = descriptor("x");
    let c = candidate(
        &d,
        "a",
        1,
        Some(vec![7; 32]),
        refs(&["a"]),
        insert("a1", None, "A"),
    );
    let t = staged(&limited(&d, 1), &c, false, facts(&c));
    let (_, _) = prepare(&t);
    let missing = t
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::RequestMissing(slots) = e {
                Some(slots)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(missing.len(), 1);
    bounded(&t.state, &d, &t.effects);
}
