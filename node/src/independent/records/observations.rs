//! Signed bounded inventory observations and exact retained obligation bindings.
use super::super::namespace::{codec_fault, signed_body};
use super::*;
use std::collections::BTreeSet;
pub(super) fn checked<P: PolicyClock>(
    evidence: &GenuineEvidence<P>,
    guard: &ReceiveGuard,
    observation: &Observation,
) -> Result<Inventory, Fault> {
    if observation.instance != guard.id.instance
        || observation.guard != guard.id
        || observation.source != guard.source
        || observation.channel != guard.channel
        || observation.id != guard.id.number.checked_add(1).ok_or(Fault::Capacity)?
        || observation.bytes.value().len() > guard.max_bytes
    {
        return Err(Fault::BindingMismatch);
    }
    let mut limits = evidence.config.limits.clone();
    limits.bytes = limits.bytes.min(guard.max_bytes);
    let (record, inventory): (SignedRecord, Inventory) =
        decode(observation.bytes.value(), &limits).map_err(codec_fault)?;
    let header: InventoryHeaderBody = signed_body(&record, &guard.source, &limits)?;
    if header.instance != guard.id.instance
        || header.identity != evidence.config.identity
        || header.source != guard.source
        || header.target != crate::signing::public_key(&evidence.config.node_seed)
        || header.channel != guard.channel
        || header.nonce == [0; 32]
        || inventory.instance != guard.id.instance
        || inventory.source != guard.source
        || header.number != inventory.number
        || inventory.entries.len() > guard.max_items
        || header.entries != inventory.entries.len()
        || header.pages != usize::from(!inventory.entries.is_empty())
        || header.total_bytes != inventory.total_bytes
        || inventory.total_bytes > evidence.intent.limits.inventory_bytes
        || observation.expected != inventory.entries
        || encode_profile(&inventory, &limits)
            .map_err(codec_fault)?
            .digest()
            != header.manifest
    {
        return Err(Fault::Integrity);
    }
    let mut seen = BTreeSet::new();
    for record in &inventory.entries {
        if record.origin.is_empty()
            || record.seq < 0
            || record.seq >= 9_007_199_254_740_991
            || !seen.insert(
                encode_profile(record, &limits)
                    .map_err(codec_fault)?
                    .value()
                    .to_vec(),
            )
        {
            return Err(Fault::Integrity);
        }
    }
    Ok(inventory)
}
pub(super) fn retained<P: PolicyClock>(
    evidence: &GenuineEvidence<P>,
    request: &ReplicaOpen,
    image: &ReplicaImage,
    floor: &ReplicaFloor,
) -> Result<(), Fault> {
    if image.observations.len() > request.limits.inbox_items
        || image.obligations.len() > request.limits.inventory_items
        || !image
            .observations
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id)
        || image.watermark
            != image
                .observations
                .last()
                .map_or(0, |observation| observation.id)
    {
        return Err(Fault::Integrity);
    }
    let mut total = 0usize;
    let mut expected = vec![];
    for observation in &image.observations {
        total = total
            .checked_add(observation.bytes.value().len())
            .ok_or(Fault::Capacity)?;
        if total > request.limits.inbox_bytes {
            return Err(Fault::Capacity);
        }
        let Some(GuardStatus::Settled {
            guard,
            observation: hash,
            image_generation,
        }) = floor.guards.get(&observation.guard)
        else {
            return Err(Fault::Integrity);
        };
        if *hash != observation.bytes.digest() || *image_generation > image.generation {
            return Err(Fault::Integrity);
        }
        checked(evidence, guard, observation)?;
        for reference in &observation.expected {
            expected.push(Obligation {
                observation: observation.id,
                expected: reference.clone(),
                classification: Classification::Missing,
            });
        }
    }
    // Until actual transfer/application custody is present, no caller label may
    // manufacture retention. Later classification must prove its exact stored bundle.
    if image.obligations != expected {
        return Err(Fault::Integrity);
    }
    for status in floor.guards.values() {
        match status {
            GuardStatus::Settled {
                guard, observation, ..
            } => {
                if !image
                    .observations
                    .iter()
                    .any(|item| item.guard == guard.id && item.bytes.digest() == *observation)
                {
                    return Err(Fault::Integrity);
                }
            }
            GuardStatus::Abandoned { floor_sequence, .. } => {
                if *floor_sequence > floor.sequence {
                    return Err(Fault::Integrity);
                }
            }
            GuardStatus::Loss { .. } => {}
            GuardStatus::Active(_) => {}
        }
    }
    Ok(())
}
