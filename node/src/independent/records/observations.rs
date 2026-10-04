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
    if let Ok(bundle) = super::node_ingress::read_bundle(&observation.bytes, &limits) {
        if !matches!(guard.role.as_str(), "local-qualification" | "bundle") {
            return Err(Fault::Integrity);
        }
        let reference = super::node_ingress::bundle_ref(&bundle, &observation.bytes);
        if bundle.instance != guard.id.instance
            || bundle.candidate.instance != guard.id.instance
            || observation.expected != vec![reference.clone()]
            || bundle.candidate.operation.inner().share != evidence.config.descriptor.share
            || bundle.candidate.operation.inner().glade_id != evidence.config.descriptor.glade_id
            || bundle.candidate.operation.inner().key != evidence.config.descriptor.key
            || (guard.role == "local-qualification"
                && (guard.source != crate::signing::public_key(&evidence.config.node_seed)
                    || bundle.kind != RecordKind::Candidate
                    || bundle.receipt.is_some()
                    || bundle.fork.is_some()
                    || bundle.candidate.evidence.admission.is_some()))
        {
            return Err(Fault::Integrity);
        }
        return Ok(Inventory {
            instance: bundle.instance,
            source: guard.source,
            number: observation.id,
            entries: vec![reference],
            total_bytes: observation.bytes.value().len(),
        });
    }
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
        || (guard.role == "inventory"
            && (inventory.entries.is_empty() != (inventory.total_bytes == 0)))
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
/// The original local bytes stay distinct from the later sealed admission.
/// Retention is justified only by a selected terminal native acceptance whose
/// verified local query carries that exact original operation and evidence.
pub(super) fn classification<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    image: &ReplicaImage,
    guard: &ReceiveGuard,
    observation: &Observation,
    reference: &RecordRef,
) -> Result<Classification, Fault> {
    let Ok(bundle) = super::node_ingress::read_bundle(&observation.bytes, &evidence.config.limits)
    else {
        if guard.role == "inventory" {
            let (_, inventory): (SignedRecord, Inventory) =
                decode(observation.bytes.value(), &evidence.config.limits).map_err(codec_fault)?;
            if known_total(evidence, request, image, &inventory.entries)?
                .is_some_and(|total| total != inventory.total_bytes)
            {
                // Preserve the inconsistent immutable promise as unresolved.
                // Valid native app custody/receipts remain untouched.
                return Ok(Classification::Missing);
            }
        }
        for bundle in super::node_exchange::native_history(image)? {
            let raw = encode_profile(&bundle, &evidence.config.limits).map_err(codec_fault)?;
            if super::node_ingress::bundle_ref(&bundle, &raw) == *reference {
                return Ok(Classification::Retained(reference.bundle));
            }
        }
        for stored in &image.observations {
            if stored.expected != vec![reference.clone()]
                || stored.bytes.digest() != reference.bundle
            {
                continue;
            }
            let Ok(bundle) =
                super::node_ingress::read_bundle(&stored.bytes, &evidence.config.limits)
            else {
                continue;
            };
            if remote_custody(evidence, request, image, &bundle)? {
                return Ok(Classification::Retained(reference.bundle));
            }
        }
        return Ok(Classification::Missing);
    };
    if super::node_ingress::bundle_ref(&bundle, &observation.bytes) != *reference {
        return Ok(Classification::Missing);
    }
    if guard.role == "local-qualification" {
        let state = image
            .state
            .instances
            .get(&bundle.instance)
            .ok_or(Fault::Integrity)?;
        for attempt in &image.recovery.attempts {
            let AttemptPhase::Terminal(TerminalOutcome::Committed { receipts, .. }) =
                &attempt.phase
            else {
                continue;
            };
            let batch = decode_batch(
                attempt.attempt.binding.batch.value(),
                &evidence.config.limits,
            )
            .map_err(codec_fault)?;
            for accepted in &batch.plan.acceptances {
                let query = &accepted.verified_query;
                if matches!(query.mode, Mode::Local { .. })
                    && query.instance == bundle.instance
                    && query.descriptor == evidence.config.descriptor
                    && query.operation == bundle.candidate.operation
                    && query.evidence == bundle.candidate.evidence
                    && receipts.contains(&accepted.receipt)
                    && state.accepted.iter().any(|stored| {
                        stored.candidate == accepted.candidate
                            && stored.facts == accepted.facts
                            && stored.receipt == accepted.receipt
                    })
                {
                    return Ok(Classification::Retained(reference.bundle));
                }
            }
        }
    }
    if guard.role == "bundle" && remote_custody(evidence, request, image, &bundle)? {
        return Ok(Classification::Retained(reference.bundle));
    }
    Ok(Classification::Missing)
}
fn known_total<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    image: &ReplicaImage,
    entries: &[RecordRef],
) -> Result<Option<usize>, Fault> {
    let mut native = Vec::new();
    for bundle in super::node_exchange::native_history(image)? {
        let raw = encode_profile(&bundle, &evidence.config.limits).map_err(codec_fault)?;
        native.push((
            super::node_ingress::bundle_ref(&bundle, &raw),
            raw.value().len(),
        ));
    }
    let mut total = 0_usize;
    for reference in entries {
        let mut length = native
            .iter()
            .find_map(|(found, length)| (found == reference).then_some(*length));
        if length.is_none() {
            for stored in &image.observations {
                if stored.expected != vec![reference.clone()]
                    || stored.bytes.digest() != reference.bundle
                {
                    continue;
                }
                let Ok(bundle) =
                    super::node_ingress::read_bundle(&stored.bytes, &evidence.config.limits)
                else {
                    continue;
                };
                if remote_custody(evidence, request, image, &bundle)? {
                    length = Some(stored.bytes.value().len());
                    break;
                }
            }
        }
        let Some(length) = length else {
            return Ok(None);
        };
        total = total.checked_add(length).ok_or(Fault::Capacity)?;
    }
    Ok(Some(total))
}
fn remote_custody<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    image: &ReplicaImage,
    bundle: &TransferBundle,
) -> Result<bool, Fault> {
    let state = image
        .state
        .instances
        .get(&bundle.instance)
        .ok_or(Fault::Integrity)?;
    match bundle.kind {
        RecordKind::Accepted => {
            let facts = bindings::record(evidence, request, &bundle.candidate)?;
            let Some(receipt) = &bundle.receipt else {
                return Ok(false);
            };
            Ok(facts.historical_admitter_authorized
                && facts.historical_policy_allowed
                && facts.payload_valid
                && facts.signatures_valid
                && facts.issuance_unique
                && receipt.instance == bundle.instance
                && receipt.operation == bundle.candidate.operation.canonical().digest()
                && receipt.stable_id == receipt.operation
                && receipt.storage == StorageClass::LocalProcessRestart
                && bundle
                    .candidate
                    .evidence
                    .admission
                    .as_ref()
                    .is_some_and(|admission| admission.digest() == receipt.admission)
                && state
                    .accepted
                    .iter()
                    .any(|value| value.candidate.operation == bundle.candidate.operation))
        }
        RecordKind::Candidate => Ok(bundle.receipt.is_none()
            && bundle.fork.is_none()
            && (state.candidates.contains(&bundle.candidate)
                || state
                    .accepted
                    .iter()
                    .any(|stored| stored.candidate == bundle.candidate)
                || state.security_evidence.contains(&bundle.candidate))),
        RecordKind::Security => Ok(bundle.receipt.is_none()
            && bundle.fork.is_none()
            && state.security_evidence.contains(&bundle.candidate)),
        RecordKind::Fork => Ok(bundle.receipt.is_none()
            && bundle.fork.as_ref().is_some_and(|fork| {
                fork.first == bundle.candidate && state.qualified_forks.contains(fork)
            })),
    }
}
/// Only an earlier immutable *full* inventory on the same authenticated
/// source/channel may authorize a served bundle. A request page is not that cut.
pub(super) fn full_manifest(
    image: &ReplicaImage,
    floor: &ReplicaFloor,
    source: Digest,
    channel: Digest,
    reference: &RecordRef,
    before: u64,
) -> bool {
    image.observations.iter().any(|observation| {
        if observation.id >= before
            || observation.source != source
            || observation.channel != channel
        {
            return false;
        }
        let Some(GuardStatus::Settled { guard, .. }) = floor.guards.get(&observation.guard) else {
            return false;
        };
        if guard.role != "inventory" {
            return false;
        }
        decode::<(SignedRecord, Inventory)>(observation.bytes.value(), &image.limits.decode)
            .is_ok_and(|(_, inventory)| {
                observation.expected == inventory.entries && inventory.entries.contains(reference)
            })
    })
}
pub(super) fn retained<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
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
            if guard.role == "bundle"
                && !full_manifest(
                    image,
                    floor,
                    guard.source,
                    guard.channel,
                    reference,
                    observation.id,
                )
            {
                return Err(Fault::BindingMismatch);
            }
            expected.push(Obligation {
                observation: observation.id,
                expected: reference.clone(),
                classification: classification(
                    evidence,
                    request,
                    image,
                    guard,
                    observation,
                    reference,
                )?,
            });
        }
    }
    // Until actual transfer/application custody is present, no caller label may
    // manufacture retention. Later classification must prove its exact stored bundle.
    if image.obligations.len() != expected.len()
        || image
            .obligations
            .iter()
            .zip(&expected)
            .any(|(stored, proven)| {
                stored.observation != proven.observation
                    || stored.expected != proven.expected
                    || (stored.classification != Classification::Missing
                        && stored.classification != proven.classification)
            })
    {
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
