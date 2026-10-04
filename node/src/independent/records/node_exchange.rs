//! Private finite carrier rounds use committed native history, never maximum heads.
use super::super::namespace::{codec_fault, sign_record, signed_body};
use super::*;
use crate::signing::public_key;
use std::collections::BTreeMap;

pub(super) fn native_history(image: &ReplicaImage) -> Result<Vec<TransferBundle>, Fault> {
    let state = image
        .state
        .instances
        .get(&image.instance)
        .ok_or(Fault::Integrity)?;
    let mut records = Vec::new();
    for accepted in &state.accepted {
        records.push(TransferBundle {
            instance: image.instance.clone(),
            candidate: accepted.candidate.clone(),
            kind: RecordKind::Accepted,
            receipt: Some(accepted.receipt.clone()),
            fork: None,
        });
    }
    for candidate in &state.candidates {
        records.push(TransferBundle {
            instance: image.instance.clone(),
            candidate: candidate.clone(),
            kind: RecordKind::Candidate,
            receipt: None,
            fork: None,
        });
    }
    for candidate in &state.security_evidence {
        records.push(TransferBundle {
            instance: image.instance.clone(),
            candidate: candidate.clone(),
            kind: RecordKind::Security,
            receipt: None,
            fork: None,
        });
    }
    for fork in &state.qualified_forks {
        records.push(TransferBundle {
            instance: image.instance.clone(),
            candidate: fork.first.clone(),
            kind: RecordKind::Fork,
            receipt: None,
            fork: Some(fork.clone()),
        });
    }
    let mut sorted = BTreeMap::new();
    let mut total = 0_usize;
    for bundle in records {
        let raw = encode_profile(&bundle, &image.limits.decode).map_err(codec_fault)?;
        total = total
            .checked_add(raw.value().len())
            .ok_or(Fault::Capacity)?;
        if total > image.limits.inventory_bytes || sorted.len() >= image.limits.inventory_items {
            return Err(Fault::Capacity);
        }
        sorted.insert(raw.value().to_vec(), bundle);
    }
    Ok(sorted.into_values().collect())
}
/// Native projection and retained peer originals are distinct custody. A
/// duplicate operation may have multiple genuine original node admissions.
/// Only exact proven, previously guarded peer bundles join the advertised cut.
pub(super) fn history(image: &ReplicaImage) -> Result<Vec<TransferBundle>, Fault> {
    let mut records = native_history(image)?;
    for observation in &image.observations {
        let Some(GuardStatus::Settled { guard, .. }) =
            image.prior_floor.guards.get(&observation.guard)
        else {
            continue;
        };
        if guard.role != "bundle" {
            continue;
        }
        let Ok(bundle) = super::node_ingress::read_bundle(&observation.bytes, &image.limits.decode)
        else {
            continue;
        };
        let reference = super::node_ingress::bundle_ref(&bundle, &observation.bytes);
        if image.obligations.iter().any(|obligation| {
            obligation.observation == observation.id
                && obligation.expected == reference
                && obligation.classification == Classification::Retained(reference.bundle)
        }) {
            records.push(bundle);
        }
    }
    let mut sorted = BTreeMap::new();
    let mut total = 0_usize;
    for bundle in records {
        let raw = encode_profile(&bundle, &image.limits.decode).map_err(codec_fault)?;
        if sorted.contains_key(raw.value()) {
            continue;
        }
        total = total
            .checked_add(raw.value().len())
            .ok_or(Fault::Capacity)?;
        if total > image.limits.inventory_bytes || sorted.len() >= image.limits.inventory_items {
            return Err(Fault::Capacity);
        }
        sorted.insert(raw.value().to_vec(), bundle);
    }
    Ok(sorted.into_values().collect())
}
impl<P: PolicyClock> DiskSession<P> {
    /// Replay exact fully settled remote inbox custody before exposing ingress.
    /// Local writer intents still require a fresh local possession exchange;
    /// orphan Active guards confer no reconstructed receive capability.
    pub(crate) fn resume_node_inputs(&mut self) -> Result<(), Fault> {
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        let observations = pair.image.observations.clone();
        let floor = pair.envelope.floor.clone();
        for observation in &observations {
            let Some(GuardStatus::Settled { guard, .. }) = floor.guards.get(&observation.guard)
            else {
                return Err(Fault::Integrity);
            };
            if guard.role != "bundle" {
                continue;
            }
            let bundle =
                super::node_ingress::read_bundle(&observation.bytes, &self.request.limits.decode)?;
            let reference = super::node_ingress::bundle_ref(&bundle, &observation.bytes);
            let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
            if matches!(
                observations::classification(
                    &mut self.evidence,
                    &self.request,
                    &pair.image,
                    guard,
                    observation,
                    &reference
                )?,
                Classification::Retained(_)
            ) {
                continue;
            }
            let mut original = None;
            for prior in &observations {
                if prior.id >= observation.id
                    || prior.source != guard.source
                    || prior.channel != guard.channel
                {
                    continue;
                }
                let Some(GuardStatus::Settled { guard: earlier, .. }) =
                    floor.guards.get(&prior.guard)
                else {
                    continue;
                };
                if earlier.role != "inventory" {
                    continue;
                }
                let (_, inventory): (SignedRecord, Inventory) =
                    decode(prior.bytes.value(), &self.request.limits.decode)
                        .map_err(codec_fault)?;
                if inventory.entries.contains(&reference) {
                    original = Some(inventory);
                    break;
                }
            }
            let inventory = original.ok_or(Fault::BindingMismatch)?;
            self.node_offer(bundle, &reference, &inventory, guard.source, guard.channel)?;
        }
        if !observations.is_empty() {
            self.classify_node_inputs()?;
        }
        Ok(())
    }
    pub(crate) fn node_history(&self) -> Result<Vec<TransferBundle>, Fault> {
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        history(&pair.image)
    }
    pub(crate) fn node_hello(
        &mut self,
        target: Digest,
        dialer: Digest,
        acceptor: Digest,
        exporter: Digest,
        nonce: Digest,
        role: &str,
    ) -> Result<SignedRecord, Fault> {
        self.node_holder(target)?;
        sign_record(
            &self.evidence.config.node_seed,
            &ExchangeHello {
                instance: self.request.instance.clone(),
                identity: self.request.identity.clone(),
                source: public_key(&self.evidence.config.node_seed),
                target,
                dialer_endpoint: dialer,
                acceptor_endpoint: acceptor,
                exporter,
                nonce,
                role: role.into(),
            },
            &self.request.limits.decode,
        )
    }
    pub(crate) fn node_holder(&mut self, remote: Digest) -> Result<(), Fault> {
        let (cut, policy) = authentication::persist_observed(self)?;
        let local = public_key(&self.evidence.config.node_seed);
        if !policy.replica_holders.contains(&local)
            || !policy.replica_holders.contains(&remote)
            || policy.revoked.contains(&local)
            || policy.revoked.contains(&remote)
            || !super::super::auth::fits(
                cut.time.as_ref().ok_or(Fault::Unavailable)?,
                &policy.window,
            )
        {
            return Err(Fault::Ownership);
        }
        Ok(())
    }
    pub(crate) fn node_verify_hello(
        &mut self,
        record: &SignedRecord,
        source: Digest,
        dialer: Digest,
        acceptor: Digest,
        exporter: Digest,
        role: &str,
    ) -> Result<(), Fault> {
        let hello: ExchangeHello = signed_body(record, &source, &self.request.limits.decode)?;
        if hello.instance != self.request.instance
            || hello.identity != self.request.identity
            || hello.source != source
            || hello.target != public_key(&self.evidence.config.node_seed)
            || hello.dialer_endpoint != dialer
            || hello.acceptor_endpoint != acceptor
            || hello.exporter != exporter
            || hello.nonce == [0; 32]
            || hello.role != role
        {
            return Err(Fault::Integrity);
        }
        self.node_holder(source)
    }
    pub(crate) fn node_inventory(&self, records: &[TransferBundle]) -> Result<Inventory, Fault> {
        let mut entries = Vec::new();
        let mut total = 0_usize;
        for bundle in records {
            let raw = encode_profile(bundle, &self.request.limits.decode).map_err(codec_fault)?;
            total = total
                .checked_add(raw.value().len())
                .ok_or(Fault::Capacity)?;
            if entries.len() >= self.request.limits.inventory_items
                || total > self.request.limits.inventory_bytes
            {
                return Err(Fault::Capacity);
            }
            entries.push(super::node_ingress::bundle_ref(bundle, &raw));
        }
        Ok(Inventory {
            instance: self.request.instance.clone(),
            source: public_key(&self.evidence.config.node_seed),
            number: self
                .pair
                .as_ref()
                .ok_or(Fault::Ownership)?
                .envelope
                .floor
                .guard_high_water,
            entries,
            total_bytes: total,
        })
    }
    pub(crate) fn node_inventory_header(
        &mut self,
        inventory: &Inventory,
        target: Digest,
        channel: Digest,
    ) -> Result<SignedRecord, Fault> {
        self.node_holder(target)?;
        sign_record(
            &self.evidence.config.node_seed,
            &InventoryHeaderBody {
                instance: self.request.instance.clone(),
                identity: self.request.identity.clone(),
                source: public_key(&self.evidence.config.node_seed),
                target,
                channel,
                nonce: crate::signing::random_seed().map_err(|_| Fault::Unavailable)?,
                number: inventory.number,
                pages: usize::from(!inventory.entries.is_empty()),
                entries: inventory.entries.len(),
                total_bytes: inventory.total_bytes,
                manifest: encode_profile(inventory, &self.request.limits.decode)
                    .map_err(codec_fault)?
                    .digest(),
            },
            &self.request.limits.decode,
        )
    }
    pub(crate) fn node_missing(&self, inventory: &Inventory) -> Result<Inventory, Fault> {
        let own = self.node_inventory(&self.node_history()?)?;
        let mut missing = inventory.clone();
        missing.source = public_key(&self.evidence.config.node_seed);
        missing.number = self
            .pair
            .as_ref()
            .ok_or(Fault::Ownership)?
            .envelope
            .floor
            .guard_high_water;
        missing
            .entries
            .retain(|reference| !own.entries.contains(reference));
        // The request carries references only. Payload sizes remain bound by
        // the previously retained full source inventory, not this request.
        missing.total_bytes = 0;
        Ok(missing)
    }
    pub(crate) fn node_offer(
        &mut self,
        bundle: TransferBundle,
        reference: &RecordRef,
        inventory: &Inventory,
        source: Digest,
        channel: Digest,
    ) -> Result<(), Fault> {
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        if !pair.image.observations.iter().any(|observation| {
            observation.source == source
                && observation.channel == channel
                && observation.expected == inventory.entries
                && decode::<(SignedRecord, Inventory)>(
                    observation.bytes.value(),
                    &self.request.limits.decode,
                )
                .is_ok_and(|(_, stored)| stored == *inventory)
        }) {
            return Err(Fault::BindingMismatch);
        }
        if !observations::full_manifest(
            &pair.image,
            &pair.envelope.floor,
            source,
            channel,
            reference,
            u64::MAX,
        ) {
            return Err(Fault::BindingMismatch);
        }
        let raw = encode_profile(&bundle, &self.request.limits.decode).map_err(codec_fault)?;
        if !inventory.entries.contains(reference)
            || super::node_ingress::bundle_ref(&bundle, &raw) != *reference
        {
            return Err(Fault::BindingMismatch);
        }
        let facts = bindings::record(&mut self.evidence, &self.request, &bundle.candidate)?;
        if let Some(receipt) = &bundle.receipt {
            if bundle.kind != RecordKind::Accepted
                || receipt.instance != bundle.instance
                || receipt.operation != bundle.candidate.operation.canonical().digest()
                || receipt.stable_id != receipt.operation
                || receipt.storage != StorageClass::LocalProcessRestart
                || bundle
                    .candidate
                    .evidence
                    .admission
                    .as_ref()
                    .is_none_or(|admission| admission.digest() != receipt.admission)
                || !facts.historical_admitter_authorized
                || !facts.historical_policy_allowed
            {
                return Err(Fault::Integrity);
            }
        } else if bundle.kind == RecordKind::Accepted {
            return Err(Fault::Integrity);
        }
        let effects = match bundle.kind {
            RecordKind::Fork => {
                let fork = bundle.fork.ok_or(Fault::Integrity)?;
                if fork.first != bundle.candidate {
                    return Err(Fault::Integrity);
                }
                runtime::event(self, Event::OfferFork(Box::new(fork)))?
            }
            _ => {
                if bundle.fork.is_some() {
                    return Err(Fault::Integrity);
                }
                runtime::event(self, Event::OfferReplica(Box::new(bundle.candidate)))?
            }
        };
        runtime::drain(self, effects)?;
        self.classify_node_inputs()
    }
}
