//! Physical structural invariants; complete host/cryptographic validation follows.
use super::*;
pub(super) fn codec_fault(error: Error) -> Fault {
    match error {
        Error::Capacity => Fault::Capacity,
        Error::Invalid => Fault::Integrity,
        Error::Unavailable => Fault::Unavailable,
    }
}
pub(super) fn pending(envelope: &FloorEnvelope) -> Result<(), Fault> {
    match (
        &envelope.floor.intent,
        &envelope.pending,
        &envelope.completion,
    ) {
        (None, None, None) => Ok(()),
        (Some(intent), Some(pending), Some(completion)) => {
            if completion.selected != pending.next
                || completion.intent.is_some()
                || completion.sequence
                    != envelope
                        .floor
                        .sequence
                        .checked_add(1)
                        .ok_or(Fault::Capacity)?
                || completion.owner != envelope.floor.owner
                || completion.root_registration != envelope.floor.root_registration
                || completion.path_binding != envelope.floor.path_binding
                || completion.lease != envelope.floor.lease
                || completion.issuance != envelope.floor.issuance
                || completion.next_attempt != envelope.floor.next_attempt
                || completion.invocation_high_water != envelope.floor.invocation_high_water
                || completion.guard_high_water != envelope.floor.guard_high_water
                || pending.prior != envelope.floor.selected
                || pending.prior != intent.prior
                || pending.next != intent.next
                || intent.next.generation
                    != intent
                        .prior
                        .generation
                        .checked_add(1)
                        .ok_or(Fault::Capacity)?
                || intent.next.slot > 1
                || intent.next.slot == intent.prior.slot
                || intent.issuance != envelope.floor.issuance
                || intent.next_attempt != envelope.floor.next_attempt
                || intent.invocation_high_water != envelope.floor.invocation_high_water
                || intent.guard_high_water != envelope.floor.guard_high_water
                || intent.guards != envelope.floor.guards
            {
                return Err(Fault::Integrity);
            }
            Ok(())
        }
        _ => Err(Fault::Integrity),
    }
}
pub(super) fn envelope(
    envelope: &FloorEnvelope,
    image: &ReplicaImage,
    bounds: &DecodeLimits,
) -> Result<(), Fault> {
    let floor = &envelope.floor;
    pending(envelope)?;
    if floor.version != 1
        || image.version != 1
        || floor.selected.slot > 1
        || floor.selected.generation != image.generation
        || floor.owner != image.recovery.owner
        || floor.root_registration != image.prior_floor.root_registration
        || floor.path_binding != image.prior_floor.path_binding
        || floor.lease < image.prior_floor.lease
        || floor.issuance.plan_namespace != image.state.plan_namespace
        || floor.issuance.invocation_namespace != image.state.invocation_namespace
        || floor.issuance.next_effect_id < image.state.next_effect_id
        || floor.next_attempt < image.recovery.next_attempt
        || floor.invocation_high_water < image.recovery.invocation_high_water
        || floor.guards.len() > image.limits.guard_history
        || envelope.unmaterialized.len() > image.limits.guard_history
    {
        return Err(Fault::Integrity);
    }
    if let (Some(intent), Some(meta)) = (&floor.intent, &envelope.pending) {
        if encode(intent, bounds).map_err(codec_fault)?.digest() != meta.intent {
            return Err(Fault::Integrity);
        }
    }
    Ok(())
}
pub(super) fn advance(old: &ReplicaFloor, next: &ReplicaFloor) -> Result<(), Fault> {
    if old.version != next.version
        || old.owner != next.owner
        || old.root_registration != next.root_registration
        || old.path_binding != next.path_binding
        || next.lease < old.lease
        || old.sequence != next.sequence
        || old.selected != next.selected
        || next.intent.is_some()
        || next.next_attempt < old.next_attempt
        || next.invocation_high_water < old.invocation_high_water
        || next.guard_high_water < old.guard_high_water
        || old.issuance.plan_namespace != next.issuance.plan_namespace
        || old.issuance.invocation_namespace != next.issuance.invocation_namespace
        || next.issuance.next_effect_id < old.issuance.next_effect_id
        || next.issuance.time_floor_ms < old.issuance.time_floor_ms
        || next.issuance.policy_generation < old.issuance.policy_generation
        || (next.issuance.policy_generation == old.issuance.policy_generation
            && next.issuance.policy_digest != old.issuance.policy_digest)
    {
        return Err(Fault::Integrity);
    }
    for (origin, prior) in &old.issuance.origins {
        let later = next.issuance.origins.get(origin).ok_or(Fault::Integrity)?;
        if later.certificate != prior.certificate
            || later.next_seq < prior.next_seq
            || later.lamport < prior.lamport
            || (later.next_seq == prior.next_seq && later.prev != prior.prev)
            || prior
                .reserved_intents
                .iter()
                .any(|intent| !later.reserved_intents.contains(intent))
        {
            return Err(Fault::Integrity);
        }
    }
    Ok(())
}
pub(super) fn capacity(path: &Path, bounds: &DecodeLimits) -> Result<usize, Fault> {
    // Header is fixed; inspecting a bounded reservation does not parse user lengths.
    const HEADER: usize = b"glade.ic3.image\0\x01".len() + 8 + 32;
    regular(path)?;
    let len = usize::try_from(fs::metadata(path).map_err(io_fault)?.len())
        .map_err(|_| Fault::Capacity)?;
    let capacity = len.checked_sub(HEADER).ok_or(Fault::Integrity)?;
    if capacity == 0 || capacity > bounds.bytes {
        return Err(Fault::Capacity);
    }
    Ok(capacity)
}
