use super::*;

#[test]
fn fork_pairs_have_complete_candidate_identity_and_evidence() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].first.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].first.instance.incarnation.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].first.evidence.certificate = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].first.evidence.permit = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].first.evidence.policy = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].first.evidence.admission = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].first.evidence.admission = Some(Bytes::new(vec![8]));
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].second.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].second.instance.incarnation.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].second.evidence.certificate = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].second.evidence.permit = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].second.evidence.policy = Bytes::new(vec![8]);
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].second.evidence.admission = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks[0].second.evidence.admission = Some(Bytes::new(vec![8]));
    });
}
