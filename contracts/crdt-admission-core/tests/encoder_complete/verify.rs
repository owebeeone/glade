use super::*;

#[test]
fn full_verification_query_continuations() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.id += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .instance
            .incarnation
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.mode = Mode::Historical;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.validated_under = Some(Mode::Historical);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.verified_facts = Some(Box::new(facts(&p.records[0])));
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .instance
            .descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .instance
            .incarnation
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.canonical = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.creation_root = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .resource
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.share.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .glade_id
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.key.push(8);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.engine.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .payload_profile
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .admission_profile
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .authorization_profile
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.mode = AdmissionMode::Strong;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .bounds
            .max_operation_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .bounds
            .max_origins += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.bounds.max_refs += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.descriptor.bounds.edit_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .bounds
            .recovery_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .bounds
            .evidence_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .bounds
            .pending_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .verified_query
            .descriptor
            .bounds
            .pending_items += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.evidence.certificate = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.evidence.permit = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.evidence.policy = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.evidence.admission = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].verified_query.evidence.admission = Some(Bytes::new(vec![8]));
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        let mut o = p.acceptances[0].verified_query.operation.inner().clone();
        o.lamport += 1;
        p.acceptances[0].verified_query.operation = Operation::new(o);
    });
}
