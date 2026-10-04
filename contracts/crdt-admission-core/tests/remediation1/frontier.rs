use super::*;
fn observed(s: &State, d: &Descriptor, names: &[&str]) -> Transition {
    step(
        s,
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: names.iter().map(|n| ((*n).into(), (0, [7; 32]))).collect(),
        },
    )
}
fn footprint(d: &Descriptor, origin: &str) -> usize {
    std::mem::size_of::<Slot>() + d.instance.incarnation.len() + origin.len()
}
#[test]
fn frontier_names_empty_exact_limit_over_limit_and_huge_are_bounded() {
    let d = descriptor("x");
    let s = initial(d.clone());
    let limit = s.storage_limits.as_ref().unwrap().max_name_bytes;
    let empty = observed(&s, &d, &[""]);
    assert!(empty.effects.contains(&Effect::Report(Outcome::Invalid)));
    assert_eq!(empty.state, s);
    let exact = "a".repeat(limit);
    let t = observed(&s, &d, &[&exact]);
    assert_eq!(t.state.instances[&d.instance].pending[0].origin, exact);
    for name in ["b".repeat(limit + 1), "c".repeat(1_000_000)] {
        let t = observed(&s, &d, &[&name]);
        assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
        assert!(t.state.instances[&d.instance].pending.is_empty());
        assert!(t.state.instances[&d.instance].recovery_incomplete);
        assert_eq!(
            t.state.instances[&d.instance].used,
            s.instances[&d.instance].used
        );
        assert!(
            !t.effects
                .iter()
                .any(|e| matches!(e, Effect::RequestMissing(_)))
        );
        assert!(!cut(&t.state, d.instance.clone()).complete_local);
    }
}
#[test]
fn frontier_retained_slot_aggregate_bytes_exact_boundary_and_one_over() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let exact = footprint(&d, "a") + footprint(&d, "b");
    s.instances
        .get_mut(&d.instance)
        .unwrap()
        .descriptor
        .bounds
        .pending_bytes = exact;
    let a = observed(&s, &d, &["a"]);
    let b = observed(&a.state, &d, &["b"]);
    assert_eq!(b.state.instances[&d.instance].pending.len(), 2);
    let mut tight = a.state.clone();
    tight
        .instances
        .get_mut(&d.instance)
        .unwrap()
        .descriptor
        .bounds
        .pending_bytes = exact - 1;
    let refused = observed(&tight, &d, &["b"]);
    assert!(refused.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert_eq!(
        refused.state.instances[&d.instance].pending,
        tight.instances[&d.instance].pending
    );
    assert_eq!(
        refused.state.instances[&d.instance].used,
        tight.instances[&d.instance].used
    );
    assert!(
        !refused
            .effects
            .iter()
            .any(|e| matches!(e, Effect::RequestMissing(_)))
    );
    assert_eq!(
        observed(&a.state, &d, &["a"]).state.instances[&d.instance]
            .pending
            .len(),
        1,
        "duplicate slots don't consume bytes twice"
    );
}
#[test]
fn frontier_bytes_include_existing_candidate_charge_and_parked_verification() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut s = step(
        &initial(d.clone()),
        Event::OfferReplica(Box::new(c.clone())),
    )
    .state;
    let evidence = &c.evidence;
    let query_bytes = c.operation.canonical().value().len()
        + evidence.certificate.value().len()
        + evidence.permit.value().len()
        + evidence.policy.value().len()
        + evidence.admission.as_ref().unwrap().value().len();
    let v = s.instances.get_mut(&d.instance).unwrap();
    v.used.pending = 5;
    v.descriptor.bounds.pending_bytes = 5 + query_bytes + footprint(&d, "b") - 1;
    let t = observed(&s, &d, &["b"]);
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert_eq!(t.state.queries, s.queries);
    assert_eq!(
        t.state.instances[&d.instance].used,
        s.instances[&d.instance].used
    );
    assert!(t.state.instances[&d.instance].pending.is_empty());
}
