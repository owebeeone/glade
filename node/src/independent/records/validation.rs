//! Validate full original bindings and reconstruct installed projection through Pure callbacks.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn state<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    image: &ReplicaImage,
    floor: &ReplicaFloor,
) -> Result<(), Fault> {
    let state = &image.state;
    let recovery = &image.recovery;
    let limits = &request.limits.storage;
    let value = state
        .instances
        .get(&request.instance)
        .ok_or(Fault::Integrity)?;
    if state.instances.len() != 1
        || state.storage_owner.as_ref() != Some(&request.owner)
        || state.storage_limits.as_ref() != Some(limits)
        || state.plan_namespace != request.plan_namespace
        || state.invocation_namespace != request.invocation_namespace
        || state.next_effect_id > floor.issuance.next_effect_id
        || recovery.next_attempt > floor.next_attempt
        || recovery.attempts.len() > limits.attempts
        || recovery.preparing.len() > limits.attempts
        || recovery.revisions.len() != 1
        || recovery.invocation_count
            != state
                .storage_invocations
                .len()
                .checked_add(state.retired_storage_invocations.len())
                .ok_or(Fault::Capacity)?
        || recovery.invocation_count > limits.invocation_history
        || recovery.invocation_high_water > floor.invocation_high_water
        || state.queries.len() > request.descriptor.bounds.pending_items
        || state.attempts.len() > limits.attempts
        || state.preparing.len() > limits.attempts
        || state.terminal_conflicts.len() > limits.attempts
        || state.terminal_results.len() > limits.attempts
        || !recovery.preparing.is_empty()
    {
        return Err(Fault::Integrity);
    }
    let mut plans = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut completed = BTreeMap::new();
    for retained in &recovery.attempts {
        let attempt = &retained.attempt;
        if attempt.id.store != request.owner.store
            || attempt.id.instance != request.instance
            || attempt.id.number == 0
            || attempt.id.number >= recovery.next_attempt
            || !ids.insert(attempt.id.clone())
            || plans.contains_key(&attempt.binding.plan)
        {
            return Err(Fault::Integrity);
        }
        if !state.storage_invocations.values().chain(state.retired_storage_invocations.values()).any(|invocation| matches!(invocation, StorageInvocation::Prepare(q) | StorageInvocation::RecoverPlan(q) if q.binding == attempt.binding && q.owner == request.owner)) {
            return Err(Fault::Integrity);
        }
        let plan = bindings::check(evidence, request, &attempt.binding, state.next_effect_id)?;
        if let AttemptPhase::Started { cut } = &retained.phase {
            if cut != &attempt.binding.prestart.as_ref().map(|p| p.cut.clone()) {
                return Err(Fault::Integrity);
            }
        }
        if let AttemptPhase::Terminal(outcome) = &retained.phase {
            terminal(&attempt.binding, outcome)?;
            if let TerminalOutcome::Committed { revision, .. } = outcome {
                if completed
                    .insert(*revision, (retained, plan.clone()))
                    .is_some()
                {
                    return Err(Fault::Integrity);
                }
            }
        }
        plans.insert(attempt.binding.plan.clone(), plan);
    }
    let physical_revision = *recovery
        .revisions
        .get(&request.instance)
        .ok_or(Fault::Integrity)?;
    if usize::try_from(physical_revision).ok() != Some(completed.len())
        || value.revision > physical_revision
    {
        return Err(Fault::Integrity);
    }
    let mut expected_revision = 1u64;
    for revision in completed.keys() {
        if *revision != expected_revision {
            return Err(Fault::Integrity);
        }
        expected_revision = expected_revision.checked_add(1).ok_or(Fault::Capacity)?;
    }
    for (key, preparing) in &state.preparing {
        if key != &preparing.binding.plan
            || preparing.owner != request.owner
            || preparing.invocation.namespace != request.invocation_namespace
        {
            return Err(Fault::Integrity);
        }
        let plan = bindings::check(evidence, request, &preparing.binding, state.next_effect_id)?;
        if state
            .commits
            .get(&key.number)
            .or_else(|| state.unknown_commits.get(&key.number))
            != Some(&plan)
        {
            return Err(Fault::Integrity);
        }
    }
    for (key, attempt) in &state.attempts {
        let physical = recovery
            .attempts
            .iter()
            .find(|a| a.attempt == attempt.attempt)
            .ok_or(Fault::Integrity)?;
        if let AttemptPhase::Terminal(outcome) = &attempt.phase {
            if state.terminal_results.get(&attempt.attempt.id)
                != Some(&Terminal {
                    attempt: attempt.attempt.clone(),
                    outcome: outcome.clone(),
                })
            {
                return Err(Fault::Integrity);
            }
            if matches!(outcome, TerminalOutcome::Committed { revision, .. } if *revision > value.revision)
            {
                return Err(Fault::Integrity);
            }
        }
        if key != &attempt.attempt.binding.plan || !phase_prefix(&attempt.phase, &physical.phase) {
            return Err(Fault::Integrity);
        }
    }
    for (id, terminal_value) in &state.terminal_results {
        let actual = recovery
            .attempts
            .iter()
            .find(|a| a.attempt == terminal_value.attempt)
            .ok_or(Fault::Integrity)?;
        if id != &terminal_value.attempt.id
            || actual.phase != AttemptPhase::Terminal(terminal_value.outcome.clone())
            || state
                .attempts
                .get(&actual.attempt.binding.plan)
                .is_none_or(|a| a.phase != actual.phase)
        {
            return Err(Fault::Integrity);
        }
    }
    for (id, conflict) in &state.terminal_conflicts {
        if conflict.attempt.id != *id
            || state
                .terminal_results
                .get(id)
                .is_none_or(|original| original == conflict || original.attempt != conflict.attempt)
        {
            return Err(Fault::Integrity);
        }
        terminal(&conflict.attempt.binding, &conflict.outcome)?;
    }
    for (number, plan) in state.commits.iter().chain(&state.unknown_commits) {
        let key = PlanKey {
            namespace: request.plan_namespace,
            instance: request.instance.clone(),
            number: *number,
        };
        let actual = plans
            .get(&key)
            .or_else(|| state.preparing.get(&key).map(|_| plan))
            .ok_or(Fault::Integrity)?;
        if plan.id != *number
            || actual != plan
            || state.commits.contains_key(number) && state.unknown_commits.contains_key(number)
        {
            return Err(Fault::Integrity);
        }
    }
    for (id, invocation) in state
        .storage_invocations
        .iter()
        .chain(&state.retired_storage_invocations)
    {
        if id.namespace != request.invocation_namespace
            || id.number >= state.next_effect_id
            || id.number >= recovery.invocation_high_water
            || state.storage_invocations.contains_key(id)
                && state.retired_storage_invocations.contains_key(id)
        {
            return Err(Fault::Integrity);
        }
        match invocation {
            StorageInvocation::Prepare(q) | StorageInvocation::RecoverPlan(q) => {
                if q.invocation != *id || q.owner != request.owner {
                    return Err(Fault::Integrity);
                }
                bindings::check(evidence, request, &q.binding, state.next_effect_id)?;
            }
            StorageInvocation::Resolve(q) => {
                if q.invocation != *id
                    || q.owner != request.owner
                    || !recovery.attempts.iter().any(|a| a.attempt == q.attempt)
                {
                    return Err(Fault::Integrity);
                }
            }
        }
    }
    for (id, lookup) in &state.lookups {
        if *id != lookup.invocation.number
            || lookup.operation != OperationKind::Inspect
            || state.storage_invocations.get(&lookup.invocation)
                != Some(&StorageInvocation::Resolve(lookup.clone()))
        {
            return Err(Fault::Integrity);
        }
    }
    for (id, pending) in &state.queries {
        if *id != pending.query.id
            || *id >= state.next_effect_id
            || pending.candidate.instance != request.instance
            || pending.query.instance != request.instance
            || pending.query.descriptor != request.descriptor
            || pending.query.operation != pending.candidate.operation
            || pending.query.evidence != pending.candidate.evidence
        {
            return Err(Fault::Integrity);
        }
        bindings::record(evidence, request, &pending.candidate)?;
        match (&pending.verified, &pending.verified_query) {
            (None, None) => {
                if pending.query.verified_facts.is_some()
                    || pending.query.validated_under.is_some()
                    || pending.query.mode == Mode::AdmissionSeal
                {
                    return Err(Fault::Integrity);
                }
            }
            (Some(facts), Some(original)) => {
                let derived = match &original.mode {
                    Mode::Local { .. } => bindings::local_facts(evidence, request, original)?,
                    Mode::Historical => bindings::record(evidence, request, &pending.candidate)?,
                    Mode::AdmissionSeal => return Err(Fault::Integrity),
                };
                if original.instance != request.instance
                    || original.descriptor != request.descriptor
                    || original.operation != pending.candidate.operation
                    || original.evidence != pending.candidate.evidence
                    || original.id >= pending.query.id
                    || original.verified_facts.is_some()
                    || original.validated_under.is_some()
                    || facts != &derived
                    || pending.query.mode != Mode::AdmissionSeal
                    || pending.query.verified_facts.as_deref() != Some(facts)
                    || pending.query.validated_under.as_ref() != Some(&original.mode)
                {
                    return Err(Fault::Integrity);
                }
            }
            _ => return Err(Fault::Integrity),
        }
    }
    // Original complete, cryptographically validated batches drive the existing
    // installation callback. No cached heads/eligible set is trusted as proof.
    let mut replay = genesis::state(
        request,
        value.policy.clone(),
        value.policy_time.clone(),
        value.time_floor_ms,
    );
    replay.next_effect_id = state.next_effect_id;
    for (revision, (retained, plan)) in completed {
        if revision > value.revision {
            break;
        }
        let request_value = state
            .storage_invocations
            .values()
            .chain(state.retired_storage_invocations.values())
            .find_map(|invocation| match invocation {
                StorageInvocation::Resolve(q) if q.attempt == retained.attempt => Some(q.clone()),
                _ => None,
            })
            .ok_or(Fault::Integrity)?;
        let AttemptPhase::Terminal(outcome) = &retained.phase else {
            return Err(Fault::Integrity);
        };
        replay.commits.insert(plan.id, plan);
        replay.attempts.insert(
            retained.attempt.binding.plan.clone(),
            KernelAttempt {
                attempt: retained.attempt.clone(),
                phase: AttemptPhase::Reserved,
            },
        );
        replay.storage_invocations.insert(
            request_value.invocation.clone(),
            StorageInvocation::Resolve(request_value.clone()),
        );
        let reply = ResolveReply {
            request: request_value.clone(),
            result: ResolveResult::Terminal(Box::new(Terminal {
                attempt: retained.attempt.clone(),
                outcome: outcome.clone(),
            })),
        };
        let event = match request_value.operation {
            OperationKind::Begin => Event::CommitResolved(reply),
            OperationKind::Inspect => Event::LookupResolved(reply),
            OperationKind::Fence => Event::FenceResolved(reply),
        };
        replay = glade_crdt_admission_core::step(&replay, event).state;
    }
    let rebuilt = replay
        .instances
        .get_mut(&request.instance)
        .ok_or(Fault::Integrity)?;
    // Sticky loss is not cleared by projection reconstruction. Its durable
    // source remains the selected kernel image plus the floor/inbox cut.
    rebuilt.recovery_incomplete = value.recovery_incomplete;
    rebuilt.storage_integrity_failed = !state.terminal_conflicts.is_empty();
    if rebuilt != value {
        return Err(Fault::Integrity);
    }
    Ok(())
}
fn phase_prefix(cached: &AttemptPhase, actual: &AttemptPhase) -> bool {
    match cached {
        AttemptPhase::Reserved => true,
        AttemptPhase::Started { cut } => match actual {
            AttemptPhase::Started { cut: actual_cut } => cut == actual_cut,
            AttemptPhase::Terminal(TerminalOutcome::Committed { start_cut, .. }) => {
                cut == start_cut
            }
            _ => false,
        },
        AttemptPhase::Terminal(_) => cached == actual,
    }
}
fn terminal(binding: &Binding, outcome: &TerminalOutcome) -> Result<(), Fault> {
    let valid = match outcome {
        TerminalOutcome::NonCommit { revision, storage } => {
            *revision == binding.expected_revision && *storage == binding.storage
        }
        TerminalOutcome::Committed {
            revision,
            storage,
            receipts,
            start_cut,
        } => {
            Some(*revision) == binding.expected_revision.checked_add(1)
                && *storage == binding.storage
                && receipts == &binding.receipts
                && start_cut == &binding.prestart.as_ref().map(|p| p.cut.clone())
        }
    };
    if valid {
        Ok(())
    } else {
        Err(Fault::Integrity)
    }
}
