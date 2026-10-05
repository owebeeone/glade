//! Actual Pure-core transitions are selected before their host effects run.
use super::*;
use glade_crdt_evidence_api::{BoundQuery, EvidencePort};
use std::collections::VecDeque;
pub(super) fn save<P: PolicyClock>(
    session: &mut DiskSession<P>,
    state: State,
    mut recovery: Recovery,
    kind: DiskOperation,
) -> Result<(), Fault> {
    let mut next = session.checkpoint_template()?.image;
    next.state = state;
    recovery.invocation_count = next
        .state
        .storage_invocations
        .len()
        .checked_add(next.state.retired_storage_invocations.len())
        .ok_or(Fault::Capacity)?;
    let invocation_floor = next
        .state
        .storage_invocations
        .keys()
        .chain(next.state.retired_storage_invocations.keys())
        .try_fold(recovery.invocation_high_water, |high, id| {
            Ok::<_, Fault>(high.max(id.number.checked_add(1).ok_or(Fault::Capacity)?))
        })?;
    recovery.invocation_high_water = invocation_floor;
    next.recovery = recovery;
    next.sessions = session.evidence.session_custody();
    let pair = session.pair.as_mut().ok_or(Fault::Ownership)?;
    let mut floor = pair.envelope.floor.clone();
    floor.issuance.next_effect_id = floor.issuance.next_effect_id.max(next.state.next_effect_id);
    floor.next_attempt = floor.next_attempt.max(next.recovery.next_attempt);
    floor.invocation_high_water = floor
        .invocation_high_water
        .max(next.recovery.invocation_high_water);
    let critical_change = matches!(
        kind,
        DiskOperation::PrepareAccepted
            | DiskOperation::PrepareCandidate
            | DiskOperation::PrepareSecurity
            | DiskOperation::PrepareFork
            | DiskOperation::Start
            | DiskOperation::PublishAccepted
            | DiskOperation::PublishCandidate
            | DiskOperation::PublishSecurity
            | DiskOperation::PublishFork
            | DiskOperation::Fence
    ) || next.state.attempts != pair.image.state.attempts
        || next.state.terminal_results != pair.image.state.terminal_results;
    if !critical_change {
        reserves::ordinary(&next)?;
    }
    reserves::critical(&next, &floor)?;
    pair.transact(next, floor, kind, |cut| (session.observe_cut)(kind, cut))
}
pub(super) fn event<P: PolicyClock>(
    session: &mut DiskSession<P>,
    event: Event,
) -> Result<Vec<Effect>, Fault> {
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    pair.owned()?;
    let transition = glade_crdt_admission_core::step(&pair.image.state, event);
    let recovery = pair.image.recovery.clone();
    save(
        session,
        transition.state,
        recovery,
        DiskOperation::Checkpoint,
    )?;
    Ok(transition.effects)
}
pub(super) fn submit<P: PolicyClock>(
    session: &mut DiskSession<P>,
    candidate: Candidate,
    channel: Digest,
    nonce: Digest,
) -> Result<Vec<Outcome>, Fault> {
    let digest = candidate.operation.canonical().digest();
    let authorization = session
        .evidence
        .authorized
        .get(&digest)
        .ok_or(Fault::Ownership)?;
    if authorization.channel != channel
        || authorization.session_nonce != nonce
        || candidate.instance != session.request.instance
    {
        return Err(Fault::Ownership);
    }
    authentication::persist(session)?;
    let effects = event(session, Event::SubmitLocal(Box::new(candidate)))?;
    let outcomes = drain(session, effects)?;
    // Exact retries also require fresh possession; they do not reach Verify.
    session.evidence.authorized.remove(&digest);
    session.evidence.sealable.remove(&digest);
    Ok(outcomes)
}
pub(super) fn drain<P: PolicyClock>(
    session: &mut DiskSession<P>,
    effects: Vec<Effect>,
) -> Result<Vec<Outcome>, Fault> {
    let mut pending = VecDeque::from(effects);
    let mut outcomes = vec![];
    let limit = session
        .request
        .limits
        .storage
        .invocation_history
        .checked_mul(8)
        .and_then(|n| n.checked_add(session.request.limits.decode.items))
        .ok_or(Fault::Capacity)?;
    let mut count = 0usize;
    while let Some(effect) = pending.pop_front() {
        count = count.checked_add(1).ok_or(Fault::Capacity)?;
        if count > limit {
            return Err(Fault::Capacity);
        }
        let callback = match effect {
            Effect::Verify(query) => {
                // Historical verification is pure: it reads only the query's own signed
                // bytes and the static registry, consults no current cut and moves no
                // custody (GladeEvidenceProviderClassification), so a crash inside it
                // hides no observation and it takes no barrier. Local verification
                // consults authority and moves custody, and keeps its barrier.
                let barrier = !matches!(query.mode, Mode::Historical);
                if barrier {
                    authentication::begin(session)?;
                }
                let reply = session.evidence.verify(BoundQuery {
                    query,
                    identity: session.request.identity.clone(),
                });
                if barrier {
                    retain_observed(session)?;
                }
                Some(Event::EvidenceResolved(Box::new(reply)))
            }
            Effect::Seal(query) => {
                authentication::begin(session)?;
                let reply = session.evidence.seal(BoundQuery {
                    query,
                    identity: session.request.identity.clone(),
                });
                retain_observed(session)?;
                Some(Event::EvidenceResolved(Box::new(reply)))
            }
            Effect::Prepare { request, .. } | Effect::RecoverPlan(request) => {
                Some(Event::PrepareResolved(storage::prepare(session, request)))
            }
            Effect::Begin(request) => Some(Event::CommitResolved(storage::begin(session, request))),
            Effect::Lookup(request) => {
                Some(Event::LookupResolved(storage::resolve(session, request)))
            }
            Effect::Fence(request) => {
                Some(Event::FenceResolved(storage::resolve(session, request)))
            }
            Effect::Report(outcome) => {
                outcomes.push(outcome);
                None
            }
            Effect::RequestMissing(slots) => {
                outcomes.push(Outcome::MissingHistory(slots));
                None
            }
            Effect::ProjectionChanged(_) | Effect::OfferMissing(_) => None,
        };
        if let Some(callback) = callback {
            pending.extend(event(session, callback)?);
        }
    }
    Ok(outcomes)
}
/// Reattach exact retained host requests before exposing a recovered kernel.
/// No peer data, AttemptId-only request, or newly invented authority is used.
pub(super) fn reattach<P: PolicyClock>(session: &mut DiskSession<P>) -> Result<(), Fault> {
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    genesis::validate(&mut session.evidence, &session.request, pair)?;
    let mut rebased = pair.image.state.clone();
    let mut recovery = pair.image.recovery.clone();
    rebased.next_effect_id = rebased
        .next_effect_id
        .max(pair.envelope.floor.issuance.next_effect_id);
    recovery.next_attempt = recovery.next_attempt.max(pair.envelope.floor.next_attempt);
    recovery.invocation_high_water = recovery
        .invocation_high_water
        .max(pair.envelope.floor.invocation_high_water);
    if !pair.envelope.unmaterialized.is_empty() {
        rebased
            .instances
            .get_mut(&session.request.instance)
            .ok_or(Fault::Integrity)?
            .recovery_incomplete = true;
    }
    if rebased != pair.image.state || recovery != pair.image.recovery {
        save(session, rebased, recovery, DiskOperation::Checkpoint)?;
    }
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    let cached = pair
        .image
        .state
        .instances
        .get(&session.request.instance)
        .ok_or(Fault::Integrity)?;
    if cached.time_floor_ms < pair.envelope.floor.issuance.time_floor_ms
        || cached.policy.digest() != pair.envelope.floor.issuance.policy_digest
    {
        // Only the injected trusted provider can supply the missing current
        // observation. A digest/empty peer inventory cannot reconstruct it.
        authentication::persist(session)?;
    }
    super::evidence_recovery::queries(session)?;
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    let outstanding: Vec<_> = pair
        .image
        .state
        .storage_invocations
        .values()
        .cloned()
        .collect();
    for invocation in outstanding {
        let callback = match invocation {
            StorageInvocation::Prepare(request) | StorageInvocation::RecoverPlan(request) => {
                Event::PrepareResolved(storage::prepare(session, request))
            }
            StorageInvocation::Resolve(request) => {
                if request.operation == OperationKind::Begin {
                    let current = request
                        .attempt
                        .binding
                        .prestart
                        .as_ref()
                        .map(|p| p.cut.clone())
                        .unwrap_or(LocalPrecondition {
                            policy: [0; 32],
                            time: TimeInterval {
                                earliest_ms: 0,
                                latest_ms: 0,
                            },
                        });
                    Event::CommitResolved(storage::begin(
                        session,
                        BeginRequest { request, current },
                    ))
                } else {
                    let reply = storage::resolve(session, request.clone());
                    if request.operation == OperationKind::Inspect {
                        Event::LookupResolved(reply)
                    } else {
                        Event::FenceResolved(reply)
                    }
                }
            }
        };
        let effects = event(session, callback)?;
        drain(session, effects)?;
    }
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    if !pair.image.state.preparing.is_empty()
        || pair
            .image
            .state
            .attempts
            .values()
            .any(|attempt| !matches!(attempt.phase, AttemptPhase::Terminal(_)))
    {
        let effects = event(
            session,
            Event::ResumeRecovery(session.request.instance.clone()),
        )?;
        drain(session, effects)?;
    }
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    genesis::validate(&mut session.evidence, &session.request, pair)
}
pub(super) fn retain_observed<P: PolicyClock>(session: &mut DiskSession<P>) -> Result<(), Fault> {
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    let policy = session.evidence.observed_policy.ok_or(Fault::Unavailable)?;
    if pair.envelope.observing
        || policy
            != (
                pair.envelope.floor.issuance.policy_generation,
                pair.envelope.floor.issuance.policy_digest,
            )
        || session.evidence.observed_time_floor > pair.envelope.floor.issuance.time_floor_ms
    {
        authentication::persist(session)?;
    }
    Ok(())
}
