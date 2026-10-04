//! Owned request issuance and recovery. Invocation consumption never cancels a
//! plan or releases its reservation; only a retained terminal result can do so.
use crate::*;

pub(crate) fn allocate(s: &mut State) -> Option<u64> {
    let id = s.next_effect_id;
    s.next_effect_id = id.checked_add(1)?;
    Some(id)
}
pub(crate) fn binding(s: &State, p: &CommitPlan) -> Option<Binding> {
    Some(Binding {
        plan: PlanKey {
            namespace: s.plan_namespace,
            instance: p.instance.clone(),
            number: p.id,
        },
        batch: p.batch.clone(),
        expected_revision: p.expected_revision,
        storage: s.instances.get(&p.instance)?.storage.clone(),
        kind: p.kind.clone(),
        charges: p.charges.clone(),
        receipts: p.acceptances.iter().map(|a| a.receipt.clone()).collect(),
        prestart: p.local_precondition.clone().map(|cut| StartPrecondition {
            cut,
            windows: p
                .acceptances
                .iter()
                .map(|a| a.facts.permit_window.clone())
                .collect(),
        }),
    })
}
fn invocation(s: &mut State) -> Option<InvocationId> {
    let z = s.storage_limits.as_ref()?;
    if s.storage_invocations
        .len()
        .checked_add(s.retired_storage_invocations.len())?
        >= z.invocation_history
    {
        return None;
    }
    Some(InvocationId {
        namespace: s.invocation_namespace,
        number: allocate(s)?,
    })
}
pub(crate) fn issue_resolve(
    s: &mut State,
    a: AttemptBinding,
    operation: OperationKind,
) -> Option<ResolveRequest> {
    let owner = s.storage_owner.clone()?;
    let invocation = invocation(s)?;
    let q = ResolveRequest {
        owner,
        invocation,
        operation,
        attempt: a,
    };
    s.storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Resolve(q.clone()));
    Some(q)
}
pub(crate) fn stage(s: &mut State, p: CommitPlan, effects: &mut Vec<Effect>) -> bool {
    let Some(b) = binding(s, &p) else {
        return false;
    };
    let Some(z) = s.storage_limits.as_ref() else {
        return false;
    };
    let history = s.attempts.len().checked_add(s.preparing.len());
    let footprint = 256usize
        .checked_add(b.batch.value().len())
        .and_then(|n| n.checked_add(b.plan.instance.incarnation.len()))
        .and_then(|n| {
            b.receipts
                .len()
                .checked_mul(160 + b.plan.instance.incarnation.len())
                .and_then(|r| n.checked_add(r))
        })
        .and_then(|n| {
            b.prestart.as_ref().map_or(Some(n), |p| {
                p.windows
                    .len()
                    .checked_mul(16)
                    .and_then(|w| n.checked_add(w))
            })
        });
    if z.instances == 0
        || s.instances.len() > z.instances
        || history.is_none_or(|n| n >= z.attempts)
        || history
            .and_then(|n| n.checked_add(1))
            .and_then(|n| n.checked_mul(z.critical_per_attempt))
            .is_none_or(|n| n > z.critical_bytes)
        || footprint.is_none_or(|n| n > z.critical_per_attempt)
        || b.batch.value().len() > z.max_batch_bytes
        || b.receipts.len() > z.max_receipts
        || b.plan.instance.incarnation.len() > z.max_name_bytes
        || b.prestart
            .as_ref()
            .is_some_and(|p| p.windows.len() > z.max_windows)
    {
        return false;
    }
    let Some(owner) = s.storage_owner.clone() else {
        return false;
    };
    let Some(invocation) = invocation(s) else {
        return false;
    };
    let q = PrepareRequest {
        owner,
        invocation,
        binding: b,
    };
    s.storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Prepare(q.clone()));
    s.preparing.insert(q.binding.plan.clone(), q.clone());
    s.commits.insert(p.id, p.clone());
    effects.push(Effect::Prepare {
        plan: p,
        request: q,
    });
    true
}
pub(crate) fn resume(s: &mut State, instance: &Instance, effects: &mut Vec<Effect>) {
    let preparing: Vec<_> = s
        .preparing
        .values()
        .filter(|p| &p.binding.plan.instance == instance)
        .cloned()
        .collect();
    for mut q in preparing {
        if s.storage_invocations.values().any(
            |i| matches!(i,StorageInvocation::RecoverPlan(r) if r.binding.plan==q.binding.plan),
        ) {
            continue;
        }
        let Some(id) = invocation(s) else {
            effects.push(Effect::Report(Outcome::Capacity));
            return;
        };
        q.invocation = id;
        s.storage_invocations.insert(
            q.invocation.clone(),
            StorageInvocation::RecoverPlan(q.clone()),
        );
        effects.push(Effect::RecoverPlan(q));
    }
    let attempts: Vec<_> = s
        .attempts
        .values()
        .filter(|a| {
            &a.attempt.id.instance == instance && !matches!(a.phase, AttemptPhase::Terminal(_))
        })
        .cloned()
        .collect();
    for a in attempts {
        if s.storage_invocations.values().any(|i|matches!(i,StorageInvocation::Resolve(r) if r.attempt.id==a.attempt.id && r.operation==OperationKind::Inspect)) { continue; }
        let Some(q) = issue_resolve(s, a.attempt, OperationKind::Inspect) else {
            effects.push(Effect::Report(Outcome::Capacity));
            return;
        };
        s.lookups.insert(q.invocation.number, q.clone());
        effects.push(Effect::Lookup(q));
    }
}
pub(crate) fn prepare_reply(s: &mut State, r: PrepareReply, effects: &mut Vec<Effect>) {
    let valid = s.storage_invocations.get(&r.request.invocation).is_some_and(|q| matches!(q,StorageInvocation::Prepare(q)|StorageInvocation::RecoverPlan(q) if q==&r.request));
    if !valid
        || s.storage_owner.as_ref() != Some(&r.request.owner)
        || r.request.invocation.namespace != s.invocation_namespace
    {
        effects.push(Effect::Report(Outcome::CallbackMismatch));
        return;
    }
    if let PrepareResult::Prepared(a) = &r.result {
        if a.binding != r.request.binding
            || a.id.instance != a.binding.plan.instance
            || Some(&a.id.store) != s.storage_owner.as_ref().map(|o| &o.store)
            || s.attempts
                .get(&a.binding.plan)
                .is_some_and(|old| old.attempt != **a)
            || s.attempts
                .values()
                .any(|old| old.attempt.id == a.id && old.attempt != **a)
        {
            effects.push(Effect::Report(Outcome::CallbackMismatch));
            return;
        }
    }
    let issued = s
        .storage_invocations
        .remove(&r.request.invocation)
        .expect("matched issuance");
    s.retired_storage_invocations
        .insert(r.request.invocation.clone(), issued);
    match r.result {
        PrepareResult::Prepared(a) => {
            s.preparing.remove(&a.binding.plan);
            s.attempts
                .entry(a.binding.plan.clone())
                .or_insert(KernelAttempt {
                    attempt: (*a).clone(),
                    phase: AttemptPhase::Reserved,
                });
            let operation = if a
                .binding
                .prestart
                .as_ref()
                .is_some_and(|p| !current_matches(s, &a.id.instance, &p.cut))
            {
                OperationKind::Fence
            } else {
                OperationKind::Begin
            };
            let Some(q) = issue_resolve(s, *a, operation.clone()) else {
                effects.push(Effect::Report(Outcome::Capacity));
                return;
            };
            if operation == OperationKind::Fence {
                effects.push(Effect::Fence(q));
            } else {
                let v = &s.instances[&q.attempt.id.instance];
                effects.push(Effect::Begin(BeginRequest {
                    request: q,
                    current: LocalPrecondition {
                        policy: v.policy.digest(),
                        time: v.policy_time.clone(),
                    },
                }));
            }
        }
        PrepareResult::Pending | PrepareResult::Refused(_) => {
            if let Some(p) = s.commits.remove(&r.request.binding.plan.number) {
                s.unknown_commits.insert(p.id, p);
            }
            effects.push(Effect::Report(Outcome::OutcomeUnknown(
                r.request.binding.batch.digest(),
            )));
        }
    }
}
pub(crate) fn current_matches(s: &State, i: &Instance, p: &LocalPrecondition) -> bool {
    s.instances.get(i).is_some_and(|v| {
        v.policy.digest() == p.policy
            && v.policy_time == p.time
            && p.time.earliest_ms >= v.time_floor_ms
            && p.time.earliest_ms <= p.time.latest_ms
    })
}
pub(crate) fn observe(
    s: &mut State,
    instance: Instance,
    policy: Bytes,
    time: TimeInterval,
    effects: &mut Vec<Effect>,
) {
    let Some(v) = s.instances.get_mut(&instance) else {
        effects.push(Effect::Report(Outcome::Invalid));
        return;
    };
    if time.earliest_ms > time.latest_ms || time.earliest_ms < v.time_floor_ms {
        effects.push(Effect::Report(Outcome::PolicyPending));
        return;
    }
    v.time_floor_ms = time.earliest_ms;
    v.policy = policy;
    v.policy_time = time;
    let attempts: Vec<_> = s
        .attempts
        .values()
        .filter(|a| {
            a.attempt.id.instance == instance
                && matches!(a.phase, AttemptPhase::Reserved)
                && a.attempt
                    .binding
                    .prestart
                    .as_ref()
                    .is_some_and(|p| !current_matches(s, &instance, &p.cut))
        })
        .map(|a| a.attempt.clone())
        .collect();
    for a in attempts {
        if s.storage_invocations.values().any(|i|matches!(i,StorageInvocation::Resolve(q) if q.attempt.id==a.id && q.operation==OperationKind::Fence)) { continue; }
        if let Some(q) = issue_resolve(s, a, OperationKind::Fence) {
            effects.push(Effect::Fence(q));
        } else {
            effects.push(Effect::Report(Outcome::Capacity));
        }
    }
}
