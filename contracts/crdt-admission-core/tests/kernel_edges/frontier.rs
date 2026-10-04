use super::*;

#[test]
fn announced_missing_frontier_survives_unrelated_publication() {
    let d = descriptor("x");
    let t = step(
        &initial(d.clone()),
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: std::collections::BTreeMap::from([("missing".into(), (0, [7; 32]))]),
        },
    );
    assert!(!cut(&t.state, d.instance.clone()).complete_local);
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let published = drive(t.state, Event::SubmitLocal(Box::new(c.clone())), facts(&c));
    assert!(!cut(&published.state, d.instance).complete_local);
}
#[test]
fn empty_peer_frontier_offers_all_local_history() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    seed(&mut s, c.clone());
    let t = step(
        &s,
        Event::ObserveFrontier {
            instance: d.instance,
            heads: std::collections::BTreeMap::new(),
        },
    );
    assert!(t.effects.contains(&Effect::OfferMissing(vec![c])));
}
#[test]
fn frontier_union_cannot_exceed_pending_items_or_clear_previous_observation() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    s.instances
        .get_mut(&d.instance)
        .unwrap()
        .descriptor
        .bounds
        .pending_items = 1;
    let a = step(
        &s,
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: std::collections::BTreeMap::from([("a".into(), (0, [7; 32]))]),
        },
    );
    let b = step(
        &a.state,
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: std::collections::BTreeMap::from([("b".into(), (0, [8; 32]))]),
        },
    );
    assert!(b.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert_eq!(b.state.instances[&d.instance].pending.len(), 1);
    assert!(b.state.instances[&d.instance].recovery_incomplete);
}
#[test]
fn different_arriving_hash_cannot_complete_an_announced_missing_slot() {
    let d = descriptor("x");
    let t = step(
        &initial(d.clone()),
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: std::collections::BTreeMap::from([("a".into(), (0, [7; 32]))]),
        },
    );
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    assert_ne!(c.operation.canonical().digest(), [7; 32]);
    let t = drive(t.state, Event::OfferReplica(Box::new(c.clone())), facts(&c));
    assert!(!cut(&t.state, d.instance).complete_local);
}
#[test]
fn already_retained_exact_frontier_hash_preserves_complete_local() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    seed(&mut s, c.clone());
    let t = step(
        &s,
        Event::ObserveFrontier {
            instance: d.instance.clone(),
            heads: std::collections::BTreeMap::from([(
                "a".into(),
                (0, c.operation.canonical().digest()),
            )]),
        },
    );
    assert!(cut(&t.state, d.instance).complete_local);
}
