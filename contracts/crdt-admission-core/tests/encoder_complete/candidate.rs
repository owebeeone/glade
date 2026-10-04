use super::*;

#[test]
fn all_candidate_evidence_and_inner_op_bytes() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records[0].instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records[0].instance.incarnation.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records[0].evidence.certificate = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records[0].evidence.permit = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records[0].evidence.policy = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records[0].evidence.admission = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records[0].evidence.admission = Some(Bytes::new(vec![8]));
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        let mut o = p.records[0].operation.inner().clone();
        o.lamport += 1;
        p.records[0].operation = Operation::new(o);
    });
}
