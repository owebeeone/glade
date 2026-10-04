use super::*;

#[test]
fn encoder_is_deterministic_and_binds_descriptor_facts_and_seal_continuations() {
    let d = descriptor("x");
    let p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let base = sealed_plan(initial(d.clone()), p.clone());
    assert_eq!(
        emitted(&base).batch,
        emitted(&sealed_plan(initial(d.clone()), p.clone())).batch
    );
    for field in 0..9 {
        let mut state = initial(d.clone());
        let mut changed = p.clone();
        let a = &mut changed.acceptances[0];
        match field {
            0 => {
                state
                    .instances
                    .get_mut(&d.instance)
                    .unwrap()
                    .descriptor
                    .creation_root = [7; 32];
            }
            1 => {
                state
                    .instances
                    .get_mut(&d.instance)
                    .unwrap()
                    .descriptor
                    .resource = "another-resource".into();
            }
            2 => {
                a.facts.certificate.requester = "bob".into();
            }
            3 => {
                a.facts.certificate.writer_key = [7; 32];
            }
            4 => {
                a.facts.certificate.display_name = "another-display".into();
            }
            5 => {
                a.facts.permit_last_seq = 99;
            }
            6 => {
                a.facts.historical_policy_allowed = false;
            }
            7 => {
                a.sealed_admission = Bytes::new(b"another-seal".to_vec());
            }
            _ => {
                a.verified_query.id = 4;
            }
        }
        a.seal_query.as_mut().unwrap().verified_facts = Some(Box::new(a.facts.clone()));
        let result = sealed_plan(state, changed);
        assert_ne!(
            emitted(&result).batch,
            emitted(&base).batch,
            "semantic field {field}"
        );
    }
}
#[test]
fn storage_batch_limit_refuses_before_any_prepare_or_invocation() {
    let d = descriptor("x");
    let mut state = initial(d.clone());
    state.storage_limits.as_mut().unwrap().max_batch_bytes = 1;
    let p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let t = sealed_plan(state, p);
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert!(t.state.preparing.is_empty());
    assert!(t.state.storage_invocations.is_empty());
}
#[test]
fn full_encoded_continuation_footprint_consumes_edit_quota() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    s.instances
        .get_mut(&d.instance)
        .unwrap()
        .descriptor
        .bounds
        .edit_bytes = 400;
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let t = drive(s, Event::SubmitLocal(Box::new(c.clone())), facts(&c));
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert!(t.state.instances[&d.instance].accepted.is_empty());
}
#[test]
fn unretained_historical_batch_due_to_encoder_limit_is_sticky_incomplete() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    s.storage_limits.as_mut().unwrap().max_batch_bytes = 400;
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let t = drive(s, Event::OfferReplica(Box::new(c.clone())), facts(&c));
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert!(t.state.instances[&d.instance].recovery_incomplete);
    assert!(!cut(&t.state, d.instance).complete_local);
}
