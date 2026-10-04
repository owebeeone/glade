use super::*;

#[test]
fn full_seal_descriptor_and_evidence() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .instance
            .descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
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
            .descriptor
            .canonical = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .creation_root = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .resource
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .share
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .glade_id
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .key
            .push(8);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .engine
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .payload_profile
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .admission_profile
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .authorization_profile
            .push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .mode = AdmissionMode::Strong;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .max_operation_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .max_origins += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .max_refs += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .edit_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .recovery_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .evidence_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .pending_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .descriptor
            .bounds
            .pending_items += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .evidence
            .certificate = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .evidence
            .permit = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .evidence
            .policy = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .evidence
            .admission = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0]
            .seal_query
            .as_mut()
            .unwrap()
            .evidence
            .admission = Some(Bytes::new(vec![8]));
    });
}
