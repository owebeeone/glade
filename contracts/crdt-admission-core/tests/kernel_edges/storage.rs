use super::*;

#[test]
fn prepared_reply_cannot_replace_original_attempt_mapping() {
    let d = descriptor("x");
    let p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let mut s = initial(d);
    register_attempt(&mut s, &p);
    let q = PrepareRequest {
        owner: s.storage_owner.clone().unwrap(),
        invocation: InvocationId {
            namespace: s.invocation_namespace,
            number: 10,
        },
        binding: binding_for(&p),
    };
    s.storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Prepare(q.clone()));
    let mut changed = attempt(&p);
    changed.id.number = 2;
    let t = step(
        &s,
        Event::PrepareResolved(PrepareReply {
            request: q,
            result: PrepareResult::Prepared(Box::new(changed)),
        }),
    );
    assert_eq!(t.state, s);
    assert!(
        t.effects
            .contains(&Effect::Report(Outcome::CallbackMismatch))
    );
}
#[test]
fn lost_prepare_ack_can_recover_original_plan_while_original_invocation_is_live() {
    let d = descriptor("x");
    let p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let t = sealed_plan(initial(d.clone()), p);
    let original = emitted(&t).clone();
    let resumed = step(&t.state, Event::ResumeRecovery(d.instance));
    assert!(resumed.effects.iter().any(|e|matches!(e,Effect::RecoverPlan(q) if q.binding.plan.number==original.id && q.binding.batch==original.batch)));
    assert!(
        t.state
            .storage_invocations
            .keys()
            .all(|id| resumed.state.storage_invocations.contains_key(id))
    );
}
#[test]
fn prepare_request_from_retained_old_owner_has_no_current_callback_authority() {
    let d = descriptor("x");
    let p = staged_plan(
        &d,
        candidate(&d, "a", 0, None, vec![], insert("a", None, "A")),
    );
    let t = sealed_plan(initial(d), p);
    let q = t
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::Prepare { request, .. } = e {
                Some(request.clone())
            } else {
                None
            }
        })
        .unwrap();
    let mut s = t.state;
    s.storage_owner.as_mut().unwrap().generation += 1;
    let t = step(
        &s,
        Event::PrepareResolved(PrepareReply {
            request: q,
            result: PrepareResult::Pending,
        }),
    );
    assert_eq!(t.state, s);
    assert!(
        t.effects
            .contains(&Effect::Report(Outcome::CallbackMismatch))
    );
}
