use super::*;

#[test]
fn all_staged_facts_and_certificate_fields() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.certificate.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .facts
            .certificate
            .instance
            .incarnation
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .facts
            .certificate
            .canonical_origin
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.certificate.epoch.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.certificate.display_name.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.certificate.writer_key = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.certificate.requester.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.certificate.canonical = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.descriptor_valid = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.payload_valid = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.signatures_valid = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.issuance_unique = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.current_allowed = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.historical_admitter_authorized = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.historical_policy_allowed = false;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.permit_window.not_before_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.permit_window.not_after_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.permit_first_seq += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.permit_last_seq += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].facts.historical_time = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .facts
            .historical_time
            .as_mut()
            .unwrap()
            .earliest_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .facts
            .historical_time
            .as_mut()
            .unwrap()
            .latest_ms += 1;
    });
}
