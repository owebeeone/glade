use super::*;

#[test]
fn outstanding_verification_prevents_complete_local_and_is_bounded() {
    let d = descriptor("x");
    let mut state = initial(d.clone());
    state
        .instances
        .get_mut(&d.instance)
        .unwrap()
        .descriptor
        .bounds
        .pending_items = 1;
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let first = step(&state, Event::SubmitLocal(Box::new(c)));
    assert!(!cut(&first.state, d.instance.clone()).complete_local);
    let second = step(
        &first.state,
        Event::SubmitLocal(Box::new(candidate(
            &d,
            "b",
            0,
            None,
            vec![],
            insert("b", None, "B"),
        ))),
    );
    assert!(second.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert_eq!(second.state.queries.len(), 1);
}
#[test]
fn restored_fork_cut_derives_one_coherent_quarantine_and_eligible_set() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let b = candidate(&d, "a", 0, None, vec![], insert("b", None, "B"));
    seed(&mut s, a.clone());
    seed(&mut s, b.clone());
    s.instances
        .get_mut(&d.instance)
        .unwrap()
        .qualified_forks
        .push(ForkEvidence {
            first: a,
            second: b,
        });
    let read = cut(&s, d.instance);
    assert_eq!(read.quarantine["a"], 0);
    assert!(read.eligible.is_empty());
}
#[test]
fn signed_invalid_payload_is_security_custody_without_conviction() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    seed(&mut s, a.clone());
    let rival = candidate(&d, "a", 0, None, vec![], b"malformed".to_vec());
    let mut f = facts(&rival);
    f.payload_valid = false;
    let t = drive(s, Event::OfferReplica(Box::new(rival.clone())), f);
    assert!(
        t.state.instances[&d.instance]
            .security_evidence
            .contains(&rival)
    );
    assert_eq!(
        cut(&t.state, d.instance).eligible,
        vec![a.operation.canonical().digest()]
    );
}
#[test]
fn pending_origins_reserve_the_origin_limit_before_verification() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    s.instances
        .get_mut(&d.instance)
        .unwrap()
        .descriptor
        .bounds
        .max_origins = 1;
    let first = step(
        &s,
        Event::SubmitLocal(Box::new(candidate(
            &d,
            "a",
            0,
            None,
            vec![],
            insert("a", None, "A"),
        ))),
    );
    let second = step(
        &first.state,
        Event::SubmitLocal(Box::new(candidate(
            &d,
            "b",
            0,
            None,
            vec![],
            insert("b", None, "B"),
        ))),
    );
    assert!(second.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert_eq!(second.state.queries.len(), 1);
}
#[test]
fn retained_candidates_and_verifications_share_pending_capacity() {
    let d = descriptor("x");
    let mut s = initial(d.clone());
    let v = s.instances.get_mut(&d.instance).unwrap();
    v.used.pending = v.descriptor.bounds.pending_bytes;
    let t = step(
        &s,
        Event::OfferReplica(Box::new(candidate(
            &d,
            "a",
            0,
            None,
            vec![],
            insert("a", None, "A"),
        ))),
    );
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert!(t.state.queries.is_empty());
}
#[test]
fn oversized_proof_cannot_allocate_a_verification_continuation() {
    let d = descriptor("x");
    let s = initial(d.clone());
    let mut c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    c.evidence.permit = Bytes::new(vec![0; 1025]);
    let t = step(&s, Event::OfferReplica(Box::new(c)));
    assert!(t.effects.contains(&Effect::Report(Outcome::Capacity)));
    assert!(t.state.queries.is_empty());
}
#[test]
fn committed_local_custody_retains_the_actual_seal_named_by_original_receipt() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let t = drive(
        initial(d.clone()),
        Event::SubmitLocal(Box::new(c.clone())),
        facts(&c),
    );
    let a = &t.state.instances[&d.instance].accepted[0];
    assert_eq!(
        a.candidate.evidence.admission.as_ref().unwrap().digest(),
        a.receipt.admission
    );
}
#[test]
fn no_prior_admission_local_intent_keeps_real_seal_for_historical_replay() {
    let d = descriptor("x");
    let mut c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    c.evidence.admission = None;
    let local = drive(
        initial(d.clone()),
        Event::SubmitLocal(Box::new(c.clone())),
        facts(&c),
    );
    let a = local.state.instances[&d.instance].accepted[0].clone();
    assert_eq!(
        a.candidate.evidence.admission.as_ref().unwrap().digest(),
        a.receipt.admission
    );
    let replay = drive(
        initial(d.clone()),
        Event::OfferReplica(Box::new(a.candidate.clone())),
        facts(&a.candidate),
    );
    assert_eq!(
        replay.state.instances[&d.instance].accepted[0].receipt,
        a.receipt
    );
}
#[test]
fn signed_historical_rival_outside_permit_sequence_is_security_not_conviction() {
    let d = descriptor("x");
    let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let rival = candidate(&d, "a", 0, None, vec![], insert("rival", None, "X"));
    let mut s = initial(d.clone());
    seed(&mut s, a.clone());
    let mut denied = facts(&rival);
    denied.permit_first_seq = 1;
    let t = drive(s, Event::OfferReplica(Box::new(rival.clone())), denied);
    assert!(
        t.state.instances[&d.instance]
            .security_evidence
            .contains(&rival)
    );
    let cut = cut(&t.state, d.instance);
    assert!(cut.quarantine.is_empty());
    assert_eq!(cut.eligible, vec![a.operation.canonical().digest()]);
}
