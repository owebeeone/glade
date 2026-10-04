//! Exact finite floor guard custody and combined bounded local cut.
use super::*;
pub(super) fn fresh(
    request: &ReplicaOpen,
    pair: &PairStore,
    guard: &ReceiveGuard,
) -> Result<(), Fault> {
    let floor = &pair.envelope.floor;
    if floor.guards.len() >= request.limits.guard_history {
        return Err(Fault::Capacity);
    }
    if floor
        .guards
        .values()
        .any(|status| matches!(status, GuardStatus::Active(_)))
    {
        return Err(Fault::Ownership);
    }
    if guard.id.store != request.owner.store
        || guard.id.instance != request.instance
        || guard.id.number != floor.guard_high_water
        || guard.owner != request.owner
        || guard.lease != floor.lease
        || guard.source == [0; 32]
        || guard.channel == [0; 32]
        || guard.base_generation != floor.selected.generation
        || guard.base_digest != floor.selected.digest
        || guard.max_bytes == 0
        || guard.max_bytes > request.limits.inbox_bytes
        || guard.max_items == 0
        || guard.max_items > request.limits.inventory_items
        || !matches!(
            guard.role.as_str(),
            "inventory" | "page" | "bundle" | "local-qualification"
        )
    {
        return Err(Fault::BindingMismatch);
    }
    Ok(())
}
pub(super) fn retained(
    request: &ReplicaOpen,
    image: &ReplicaImage,
    floor: &ReplicaFloor,
) -> Result<(), Fault> {
    if floor.guards.len() > request.limits.guard_history
        || floor
            .guards
            .values()
            .filter(|status| matches!(status, GuardStatus::Active(_)))
            .count()
            > 1
    {
        return Err(Fault::Integrity);
    }
    for (id, status) in &floor.guards {
        let guard = match status {
            GuardStatus::Active(guard)
            | GuardStatus::Settled { guard, .. }
            | GuardStatus::Abandoned { guard, .. }
            | GuardStatus::Loss { guard, .. } => guard,
        };
        if id != &guard.id
            || id.store != request.owner.store
            || id.instance != request.instance
            || id.number >= floor.guard_high_water
            || guard.owner != request.owner
            || guard.lease > floor.lease
            || guard.lease == 0
            || guard.base_generation > image.generation
            || guard.source == [0; 32]
            || guard.channel == [0; 32]
            || guard.max_bytes == 0
            || guard.max_bytes > request.limits.inbox_bytes
            || guard.max_items == 0
            || guard.max_items > request.limits.inventory_items
            || !matches!(
                guard.role.as_str(),
                "inventory" | "page" | "bundle" | "local-qualification"
            )
        {
            return Err(Fault::Integrity);
        }
    }
    Ok(())
}
pub(super) fn manifest(pair: &PairStore) -> Result<Digest, Fault> {
    let image = &pair.image;
    let value = (
        (image.instance.clone(), (image.generation, image.watermark)),
        (
            (image.observations.clone(), image.obligations.clone()),
            (
                (
                    pair.envelope.floor.guards.clone(),
                    image.permanent_loss.clone(),
                ),
                pair.envelope.unmaterialized.clone(),
            ),
        ),
    );
    Ok(encode(&value, &image.limits.decode)
        .map_err(super::super::namespace::codec_fault)?
        .digest())
}
pub(super) fn reconstruct(pair: &PairStore, request: ReconstructionRequest) -> ReconstructResult {
    let image = &pair.image;
    let Ok(manifest) = manifest(pair) else {
        return ReconstructResult::Unavailable;
    };
    if request.instance != image.instance
        || request.generation != image.generation
        || request.watermark != image.watermark
        || request.manifest != manifest
    {
        return ReconstructResult::Refused(Fault::BindingMismatch);
    }
    let transition =
        glade_crdt_admission_core::step(&image.state, Event::Read(image.instance.clone()));
    let Some(kernel) = transition
        .effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Report(Outcome::Read(cut)) => Some(cut),
            _ => None,
        })
    else {
        return ReconstructResult::Unavailable;
    };
    let guards: Vec<_> = pair
        .envelope
        .floor
        .guards
        .values()
        .filter_map(|status| match status {
            GuardStatus::Active(guard) => Some(guard.clone()),
            _ => None,
        })
        .collect();
    let permanent_loss = !image.permanent_loss.is_empty()
        || !pair.envelope.unmaterialized.is_empty()
        || pair
            .envelope
            .floor
            .guards
            .values()
            .any(|status| matches!(status, GuardStatus::Loss { .. }));
    let unresolved = image.obligations.iter().any(|obligation| {
        matches!(
            obligation.classification,
            Classification::Missing | Classification::Loss(_)
        )
    });
    let complete_local =
        kernel.complete_local && guards.is_empty() && !permanent_loss && !unresolved;
    ReconstructResult::Cut(Box::new(ReplicaCut {
        kernel,
        generation: image.generation,
        watermark: image.watermark,
        manifest,
        obligations: image.obligations.clone(),
        guards,
        permanent_loss,
        complete_local,
    }))
}
