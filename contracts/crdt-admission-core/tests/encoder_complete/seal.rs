use super::*;

#[test]
fn full_sealing_query_continuations() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].seal_query.as_mut().unwrap().id += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .instance
            .descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .instance
            .incarnation
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].seal_query.as_mut().unwrap().mode = Mode::Historical;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .validated_under = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .validated_under = Some(Mode::Historical);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].seal_query.as_mut().unwrap().verified_facts = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .instance
            .descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .instance
            .incarnation
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .canonical_origin
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .epoch
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .display_name
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .writer_key = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .requester
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .certificate
            .canonical = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .descriptor_valid = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .payload_valid = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .signatures_valid = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .issuance_unique = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .current_allowed = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .historical_admitter_authorized = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .historical_policy_allowed = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .permit_window
            .not_before_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .permit_window
            .not_after_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .permit_first_seq += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .permit_last_seq += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .historical_time = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .historical_time
            .as_mut()
            .unwrap()
            .earliest_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .verified_facts
            .as_mut()
            .unwrap()
            .historical_time
            .as_mut()
            .unwrap()
            .latest_ms += 1;
    });
}
