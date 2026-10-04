//! Authentication custody and trusted policy/time observations use the same selected image.
use super::super::namespace::{codec_fault, record_bytes};
use super::*;
use crate::independent::TrustedCut;
pub(super) fn begin<P: PolicyClock>(session: &mut DiskSession<P>) -> Result<(), Fault> {
    let pair = session.pair.as_mut().ok_or(Fault::Ownership)?;
    pair.observation_marker(true, |cut| {
        (session.observe_cut)(DiskOperation::Observation, cut)
    })
}
pub(super) fn persist<P: PolicyClock>(session: &mut DiskSession<P>) -> Result<(), Fault> {
    persist_observed(session).map(|_| ())
}
/// Return the exact validated observation only after its barrier is selected.
/// Callers making a current authorization decision must use this complete cut,
/// not an earlier consultation or the scalar issuance lower bound.
/// The provider is read first, once. A cut already equal to the owned, settled
/// durable floor's needs no barrier and is returned with no marker write. Any
/// other reading, a failed one included, then takes the barrier as before: the
/// marker, then that reading's outcome, persistence and the marker's clear.
pub(super) fn persist_observed<P: PolicyClock>(
    session: &mut DiskSession<P>,
) -> Result<(TrustedCut, PolicyBody), Fault> {
    let observed = session.evidence.current();
    if let Ok((cut, policy)) = &observed {
        let settled = session.pair.as_ref().is_some_and(|pair| {
            pair.owned().is_ok() && pair.envelope.pending.is_none() && !pair.envelope.observing
        });
        if settled && matches!(durable(session, cut, policy), Ok(true)) {
            return observed;
        }
    }
    begin(session)?;
    let (cut, policy) = observed?;
    if durable(session, &cut, &policy)? {
        let pair = session.pair.as_mut().ok_or(Fault::Ownership)?;
        pair.observation_marker(false, |cut| {
            (session.observe_cut)(DiskOperation::Observation, cut)
        })?;
        return Ok((cut, policy));
    }
    let policy_bytes = record_bytes(&cut.policy, &session.evidence.config.limits)?;
    let time = cut.time.clone().ok_or(Fault::Unavailable)?;
    let mut request = session.checkpoint_template()?;
    let transition = glade_crdt_admission_core::step(
        &request.image.state,
        Event::ObservePolicy {
            instance: session.request.instance.clone(),
            policy: policy_bytes.clone(),
            time,
        },
    );
    request.image.state = transition.state;
    // ObservePolicy may issue a Fence. Its selected image must already retain
    // the full original request accounting, even before its callback runs.
    request.image.recovery.invocation_count = request
        .image
        .state
        .storage_invocations
        .len()
        .checked_add(request.image.state.retired_storage_invocations.len())
        .ok_or(Fault::Capacity)?;
    request.image.recovery.invocation_high_water = request
        .image
        .state
        .storage_invocations
        .keys()
        .chain(request.image.state.retired_storage_invocations.keys())
        .try_fold(request.image.recovery.invocation_high_water, |high, id| {
            Ok::<_, Fault>(high.max(id.number.checked_add(1).ok_or(Fault::Capacity)?))
        })?;
    request.image.sessions = session.evidence.session_custody();
    session
        .evidence
        .validate_sessions(&request.image.sessions)?;
    let pair = session.pair.as_mut().ok_or(Fault::Ownership)?;
    let mut projected = pair.envelope.floor.clone();
    projected.issuance.time_floor_ms = session.evidence.observed_time_floor;
    projected.issuance.policy_generation = policy.generation;
    projected.issuance.policy_digest = policy_bytes.digest();
    projected.invocation_high_water = projected
        .invocation_high_water
        .max(request.image.recovery.invocation_high_water);
    projected.issuance.next_effect_id = projected
        .issuance
        .next_effect_id
        .max(request.image.state.next_effect_id);
    // Bound the complete serialized request before physical intent. This is
    // custody only: no operation or application receipt is admitted here.
    encode(&request.image, &request.image.limits.decode).map_err(codec_fault)?;
    super::reserves::ordinary(&request.image)?;
    super::reserves::critical(&request.image, &projected)?;
    pair.transact(
        request.image,
        projected,
        DiskOperation::Observation,
        |cut| (session.observe_cut)(DiskOperation::Observation, cut),
    )?;
    // ObservePolicy can issue an original Fence for a still-Reserved attempt.
    // Its request is selected with the observation before any host callback;
    // retire it through the ordinary core callback path, not by dropping it.
    super::runtime::drain(session, transition.effects)?;
    Ok((cut, policy))
}
/// The provider's cut equals the durable floor's: policy bytes and time, session
/// custody, time floor, policy generation and digest.
fn durable<P: PolicyClock>(
    session: &DiskSession<P>,
    cut: &TrustedCut,
    policy: &PolicyBody,
) -> Result<bool, Fault> {
    let policy_bytes = record_bytes(&cut.policy, &session.evidence.config.limits)?;
    let time = cut.time.clone().ok_or(Fault::Unavailable)?;
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    let cached = pair
        .image
        .state
        .instances
        .get(&session.request.instance)
        .ok_or(Fault::Integrity)?;
    Ok(cached.policy == policy_bytes
        && cached.policy_time == time
        && pair.image.sessions == session.evidence.session_custody()
        && pair.envelope.floor.issuance.time_floor_ms == session.evidence.observed_time_floor
        && pair.envelope.floor.issuance.policy_generation == policy.generation
        && pair.envelope.floor.issuance.policy_digest == policy_bytes.digest())
}
