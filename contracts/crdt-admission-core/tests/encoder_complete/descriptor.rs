use super::*;

#[test]
fn all_descriptor_parsed_fields_and_bounds() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.instance.incarnation.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.canonical = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.creation_root = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.resource.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.share.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.glade_id.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.key.push(8);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.engine.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.payload_profile.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.admission_profile.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.authorization_profile.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.mode = AdmissionMode::Strong;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.max_operation_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.max_origins += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.max_refs += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.edit_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.recovery_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.evidence_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.pending_bytes += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        d.bounds.pending_items += 1;
    });
}
