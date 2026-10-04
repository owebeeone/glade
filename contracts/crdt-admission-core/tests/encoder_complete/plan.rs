use super::*;

#[test]
fn complete_plan_header_and_optional_local_cut() {
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.id += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.instance.descriptor = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.instance.incarnation.push('x');
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.expected_revision += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.kind = CommitKind::QualifiedFork;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.local_precondition = None;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.local_precondition.as_mut().unwrap().policy = [8; 32];
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.local_precondition.as_mut().unwrap().time.earliest_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.local_precondition.as_mut().unwrap().time.latest_ms += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.charges.edit += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.charges.recovery += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.charges.evidence += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.charges.pending += 1;
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.records.clear();
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.acceptances.clear();
    });
    changed(|s, d, p| {
        let _ = (&s, &d, &p);
        p.qualified_forks.clear();
    });
}
#[test]
fn namespaces_issuance_owner_and_storage_promise_are_part_of_bytes() {
    changed(|s, _, _| s.plan_namespace = [8; 32]);
    changed(|s, _, _| s.invocation_namespace = [8; 32]);
    changed(|s, _, _| s.storage_owner.as_mut().unwrap().generation += 1);
    changed(|s, _, _| s.storage_owner.as_mut().unwrap().store.0 = [8; 32]);
    changed(|s, _, _| s.storage_owner = None);
    changed(|s, _, _| s.next_effect_id += 1);
    let (mut s, d, mut p) = base();
    p.acceptances.clear();
    let before = encoder::encode(&s, &d, &p);
    s.instances.get_mut(&p.instance).unwrap().storage = StorageClass::LocalProcessRestart;
    assert_ne!(before, encoder::encode(&s, &d, &p));
}
