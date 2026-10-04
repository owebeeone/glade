//! Retained native attempts and monotone terminal publication; no memory-host success.
use super::*;
pub(super) fn prepare<P: PolicyClock>(
    session: &mut DiskSession<P>,
    request: PrepareRequest,
) -> PrepareReply {
    let result = (|| {
        let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        let state = &pair.image.state;
        let issued = state
            .storage_invocations
            .get(&request.invocation)
            .or_else(|| state.retired_storage_invocations.get(&request.invocation));
        if request.owner != session.request.owner
            || request.invocation.namespace != session.request.invocation_namespace
            || !matches!(issued, Some(StorageInvocation::Prepare(q) | StorageInvocation::RecoverPlan(q)) if q == &request)
        {
            return Err(Fault::CallbackMismatch);
        }
        bindings::check(
            &mut session.evidence,
            &session.request,
            &request.binding,
            state.next_effect_id,
        )?;
        let mut recovery = pair.image.recovery.clone();
        if let Some(old) = recovery
            .attempts
            .iter()
            .find(|a| a.attempt.binding.plan == request.binding.plan)
        {
            return if old.attempt.binding == request.binding {
                Ok(old.attempt.clone())
            } else {
                Err(Fault::Integrity)
            };
        }
        let limits = &session.request.limits.storage;
        let total = recovery
            .attempts
            .len()
            .checked_add(1)
            .ok_or(Fault::Capacity)?;
        let footprint = 256usize
            .checked_add(request.binding.batch.value().len())
            .and_then(|n| n.checked_add(request.binding.plan.instance.incarnation.len()))
            .and_then(|n| {
                request
                    .binding
                    .receipts
                    .len()
                    .checked_mul(160 + request.binding.plan.instance.incarnation.len())
                    .and_then(|r| n.checked_add(r))
            })
            .and_then(|n| {
                request.binding.prestart.as_ref().map_or(Some(n), |p| {
                    p.windows
                        .len()
                        .checked_mul(16)
                        .and_then(|w| n.checked_add(w))
                })
            })
            .ok_or(Fault::Capacity)?;
        if total > limits.attempts
            || total
                .checked_mul(limits.critical_per_attempt)
                .is_none_or(|n| n > limits.critical_bytes)
            || footprint > limits.critical_per_attempt
            || recovery.revisions.get(&request.binding.plan.instance)
                != Some(&request.binding.expected_revision)
        {
            return Err(Fault::Capacity);
        }
        let number = pair.envelope.floor.next_attempt.max(recovery.next_attempt);
        let attempt = AttemptBinding {
            id: AttemptId {
                store: request.owner.store.clone(),
                instance: request.binding.plan.instance.clone(),
                number,
            },
            binding: request.binding.clone(),
        };
        recovery.next_attempt = number.checked_add(1).ok_or(Fault::Capacity)?;
        recovery.attempts.push(RetainedAttempt {
            attempt: attempt.clone(),
            phase: AttemptPhase::Reserved,
        });
        recovery.invocation_high_water = recovery.invocation_high_water.max(
            request
                .invocation
                .number
                .checked_add(1)
                .ok_or(Fault::Capacity)?,
        );
        recovery.invocation_count = state
            .storage_invocations
            .len()
            .checked_add(state.retired_storage_invocations.len())
            .ok_or(Fault::Capacity)?;
        let state = state.clone();
        runtime::save(
            session,
            state,
            recovery,
            prepare_kind(&request.binding.kind),
        )?;
        Ok(attempt)
    })();
    PrepareReply {
        request,
        result: match result {
            Ok(attempt) => PrepareResult::Prepared(Box::new(attempt)),
            Err(fault) => PrepareResult::Refused(fault),
        },
    }
}
fn prepare_kind(kind: &CommitKind) -> DiskOperation {
    match kind {
        CommitKind::AcceptedBatch => DiskOperation::PrepareAccepted,
        CommitKind::Candidate => DiskOperation::PrepareCandidate,
        CommitKind::SecurityEvidence => DiskOperation::PrepareSecurity,
        CommitKind::QualifiedFork => DiskOperation::PrepareFork,
    }
}
fn publish_kind(kind: &CommitKind) -> DiskOperation {
    match kind {
        CommitKind::AcceptedBatch => DiskOperation::PublishAccepted,
        CommitKind::Candidate => DiskOperation::PublishCandidate,
        CommitKind::SecurityEvidence => DiskOperation::PublishSecurity,
        CommitKind::QualifiedFork => DiskOperation::PublishFork,
    }
}
fn index<P: PolicyClock>(
    session: &DiskSession<P>,
    request: &ResolveRequest,
) -> Result<usize, Fault> {
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    pair.owned()?;
    if request.owner != session.request.owner
        || request.invocation.namespace != session.request.invocation_namespace
        || (pair
            .image
            .state
            .storage_invocations
            .get(&request.invocation)
            != Some(&StorageInvocation::Resolve(request.clone()))
            && pair
                .image
                .state
                .retired_storage_invocations
                .get(&request.invocation)
                != Some(&StorageInvocation::Resolve(request.clone())))
    {
        return Err(Fault::CallbackMismatch);
    }
    pair.image
        .recovery
        .attempts
        .iter()
        .position(|a| a.attempt == request.attempt)
        .ok_or(Fault::BindingMismatch)
}
pub(super) fn begin<P: PolicyClock>(
    session: &mut DiskSession<P>,
    request: BeginRequest,
) -> ResolveReply {
    let result = (|| {
        if request.request.operation != OperationKind::Begin {
            return Err(Fault::CallbackMismatch);
        }
        let at = index(session, &request.request)?;
        let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
        if matches!(
            pair.image.recovery.attempts[at].phase,
            AttemptPhase::Reserved
        ) {
            let required = request
                .request
                .attempt
                .binding
                .prestart
                .as_ref()
                .map(|p| p.cut.clone());
            let mut allow = true;
            if let Some(precondition) = &request.request.attempt.binding.prestart {
                // Provider-owned observation; caller Begin.current is never authority.
                authentication::begin(session)?;
                let observation = glade_crdt_evidence_api::EvidencePort::observe(
                    &mut session.evidence,
                    &session.request.instance,
                    &session.request.identity,
                )?;
                allow = observation.available
                    && observation.allowed
                    && observation.policy.digest() == precondition.cut.policy
                    && observation.time == precondition.cut.time
                    && !precondition.windows.is_empty()
                    && precondition
                        .windows
                        .iter()
                        .all(|window| super::super::auth::fits(&observation.time, window));
                let (retained, _) = authentication::persist_observed(session)?;
                // Persistence may have learned a later cut than observe().
                // Start requires the exact sealed policy and full interval at
                // this protected barrier; a changed cut resolves NonCommit.
                allow = allow
                    && super::super::namespace::record_bytes(
                        &retained.policy,
                        &session.evidence.config.limits,
                    )?
                    .digest()
                        == precondition.cut.policy
                    && retained.time.as_ref() == Some(&precondition.cut.time);
            }
            let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
            let mut recovery = pair.image.recovery.clone();
            recovery.attempts[at].phase = if allow {
                AttemptPhase::Started { cut: required }
            } else {
                AttemptPhase::Terminal(TerminalOutcome::NonCommit {
                    revision: request.request.attempt.binding.expected_revision,
                    storage: request.request.attempt.binding.storage.clone(),
                })
            };
            runtime::save(
                session,
                pair.image.state.clone(),
                recovery,
                if allow {
                    DiskOperation::Start
                } else {
                    DiskOperation::Fence
                },
            )?;
        }
        terminal(session, &request.request, at)
    })();
    ResolveReply {
        request: request.request,
        result: result.unwrap_or_else(ResolveResult::Refused),
    }
}
pub(super) fn resolve<P: PolicyClock>(
    session: &mut DiskSession<P>,
    request: ResolveRequest,
) -> ResolveReply {
    let result = (|| {
        let at = index(session, &request)?;
        if request.operation == OperationKind::Begin {
            return Err(Fault::CallbackMismatch);
        }
        let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
        if matches!(
            pair.image.recovery.attempts[at].phase,
            AttemptPhase::Reserved
        ) {
            if request.operation != OperationKind::Fence {
                return Ok(ResolveResult::Pending);
            }
            let mut recovery = pair.image.recovery.clone();
            recovery.attempts[at].phase = AttemptPhase::Terminal(TerminalOutcome::NonCommit {
                revision: request.attempt.binding.expected_revision,
                storage: request.attempt.binding.storage.clone(),
            });
            runtime::save(
                session,
                pair.image.state.clone(),
                recovery,
                DiskOperation::Fence,
            )?;
        }
        terminal(session, &request, at)
    })();
    ResolveReply {
        request,
        result: result.unwrap_or_else(ResolveResult::Refused),
    }
}
fn terminal<P: PolicyClock>(
    session: &mut DiskSession<P>,
    request: &ResolveRequest,
    at: usize,
) -> Result<ResolveResult, Fault> {
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    let retained = &pair.image.recovery.attempts[at];
    if let AttemptPhase::Terminal(outcome) = &retained.phase {
        return Ok(ResolveResult::Terminal(Box::new(Terminal {
            attempt: retained.attempt.clone(),
            outcome: outcome.clone(),
        })));
    }
    let AttemptPhase::Started { cut } = &retained.phase else {
        return Ok(ResolveResult::Pending);
    };
    let binding = &retained.attempt.binding;
    if pair.image.recovery.revisions.get(&binding.plan.instance) != Some(&binding.expected_revision)
    {
        return Err(Fault::Integrity);
    }
    bindings::check(
        &mut session.evidence,
        &session.request,
        binding,
        pair.image.state.next_effect_id,
    )?;
    let outcome = TerminalOutcome::Committed {
        revision: binding
            .expected_revision
            .checked_add(1)
            .ok_or(Fault::Capacity)?,
        storage: binding.storage.clone(),
        receipts: binding.receipts.clone(),
        start_cut: cut.clone(),
    };
    let mut recovery = pair.image.recovery.clone();
    recovery
        .revisions
        .insert(binding.plan.instance.clone(), binding.expected_revision + 1);
    recovery.attempts[at].phase = AttemptPhase::Terminal(outcome.clone());
    runtime::save(
        session,
        pair.image.state.clone(),
        recovery,
        publish_kind(&binding.kind),
    )?;
    Ok(ResolveResult::Terminal(Box::new(Terminal {
        attempt: request.attempt.clone(),
        outcome,
    })))
}
