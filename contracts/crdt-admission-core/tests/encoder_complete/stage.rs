use super::*;

#[test]
fn all_staged_candidate_seal_and_receipt_fields() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].candidate.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].candidate.instance.incarnation.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].candidate.evidence.certificate = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].candidate.evidence.permit = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].candidate.evidence.policy = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].candidate.evidence.admission = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].candidate.evidence.admission = Some(Bytes::new(vec![8]));
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].seal_query = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].sealed_admission = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].receipt.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].receipt.instance.incarnation.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].receipt.operation = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].receipt.admission = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].receipt.stable_id = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances[0].receipt.storage = StorageClass::LocalProcessRestart;
    });
}
