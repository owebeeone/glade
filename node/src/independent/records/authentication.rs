//! Authentication custody and trusted policy/time observations use the same selected image.
use super::super::namespace::{codec_fault, record_bytes};
use super::*;
pub(super) fn begin<P: PolicyClock>(session: &mut DiskSession<P>) -> Result<(), Fault> {
    let pair = session.pair.as_mut().ok_or(Fault::Ownership)?;
    pair.observation_marker(true, |cut| {
        (session.observe_cut)(DiskOperation::Observation, cut)
    })
}
pub(super) fn persist<P: PolicyClock>(session: &mut DiskSession<P>) -> Result<(), Fault> {
    begin(session)?;
    let (cut, policy) = session.evidence.current()?;
    let policy_bytes = record_bytes(&cut.policy, &session.evidence.config.limits)?;
    let time = cut.time.ok_or(Fault::Unavailable)?;
    let pair = session.pair.as_mut().ok_or(Fault::Ownership)?;
    let cached = pair
        .image
        .state
        .instances
        .get(&session.request.instance)
        .ok_or(Fault::Integrity)?;
    if cached.policy == policy_bytes
        && cached.policy_time == time
        && pair.image.sessions == session.evidence.session_custody()
        && pair.envelope.floor.issuance.time_floor_ms == session.evidence.observed_time_floor
        && pair.envelope.floor.issuance.policy_generation == policy.generation
        && pair.envelope.floor.issuance.policy_digest == policy_bytes.digest()
    {
        return pair.observation_marker(false, |cut| {
            (session.observe_cut)(DiskOperation::Observation, cut)
        });
    }
    let mut request = session.checkpoint_template()?;
    request.image.state = glade_crdt_admission_core::step(
        &request.image.state,
        Event::ObservePolicy {
            instance: session.request.instance.clone(),
            policy: policy_bytes.clone(),
            time,
        },
    )
    .state;
    request.image.sessions = session.evidence.session_custody();
    session
        .evidence
        .validate_sessions(&request.image.sessions)?;
    let pair = session.pair.as_mut().ok_or(Fault::Ownership)?;
    let mut projected = pair.envelope.floor.clone();
    projected.issuance.time_floor_ms = session.evidence.observed_time_floor;
    projected.issuance.policy_generation = policy.generation;
    projected.issuance.policy_digest = policy_bytes.digest();
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
    )
}
