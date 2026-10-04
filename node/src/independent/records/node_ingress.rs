//! Private actual-node adaptation of the frozen owned admission/recovery ports.
use super::super::namespace::{codec_fault, signed_body};
use super::*;
use crate::independent::TrustedCut;
use crate::signing::public_key;
impl<P: PolicyClock> DiskSession<P> {
    pub(crate) fn local_request(
        &mut self,
        record: SignedRecord,
        channel: Digest,
        nonce: Digest,
        challenge: Digest,
    ) -> Result<SignedRecord, Fault> {
        let writer = record.signer;
        let request: LocalChallengeBody =
            signed_body(&record, &writer, &self.request.limits.decode)?;
        if request.instance != self.request.instance
            || request.identity != self.request.identity
            || request.resource != self.request.descriptor.resource
            || request.node != public_key(&self.evidence.config.node_seed)
            || request.writer_key != writer
            || request.role != "local-append-request"
            || request.channel == [0; 32]
            || request.session_nonce == [0; 32]
            || request.challenge_nonce == [0; 32]
            || !self.evidence.certificates.values().any(|(_, certificate)| {
                certificate.writer_key == writer && certificate.requester == request.requester
            })
        {
            return Err(Fault::Integrity);
        }
        self.issue_challenge(super::super::SessionRequest {
            requester: request.requester,
            writer_key: writer,
            operation: request.operation,
            channel,
            session_nonce: nonce,
            challenge_nonce: challenge,
            window: request.window,
        })
    }
    /// A guard bounded from the caller's current trusted observation (its
    /// round's, or the local receive's own `node_observe`).
    pub(crate) fn node_guard(
        &self,
        cut: &TrustedCut,
        source: Digest,
        channel: Digest,
        role: &str,
        deadline: u64,
    ) -> Result<ReceiveGuard, Fault> {
        let time = cut.time.as_ref().ok_or(Fault::Unavailable)?;
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        let floor = &pair.envelope.floor;
        Ok(ReceiveGuard {
            id: GuardId {
                store: self.request.owner.store.clone(),
                instance: self.request.instance.clone(),
                number: floor.guard_high_water,
            },
            owner: self.request.owner.clone(),
            lease: floor.lease,
            source,
            channel,
            role: role.into(),
            base_generation: floor.selected.generation,
            base_digest: floor.selected.digest,
            max_bytes: self
                .request
                .limits
                .inbox_bytes
                .min(self.request.limits.decode.bytes),
            max_items: self.request.limits.inventory_items,
            deadline_ms: time
                .latest_ms
                .checked_add(deadline)
                .ok_or(Fault::Capacity)?,
        })
    }
    pub(crate) fn finish_node_input(
        &mut self,
        permit: IngressPermit,
        raw: Bytes,
        expected: Vec<RecordRef>,
    ) -> Result<(), Fault> {
        let guard = permit.guard().clone();
        let observation = Observation {
            id: guard.id.number.checked_add(1).ok_or(Fault::Capacity)?,
            instance: guard.id.instance.clone(),
            source: guard.source,
            channel: guard.channel,
            guard: guard.id,
            bytes: raw,
            expected,
        };
        let mut checkpoint = match self.checkpoint_template() {
            Ok(checkpoint) => checkpoint,
            Err(fault) => {
                self.retain_ingress_loss(permit, observation.bytes);
                return Err(fault);
            }
        };
        checkpoint.image.observations.push(observation.clone());
        for reference in &observation.expected {
            checkpoint.image.obligations.push(Obligation {
                observation: observation.id,
                expected: reference.clone(),
                classification: Classification::Missing,
            });
        }
        checkpoint.image.watermark = observation.id;
        match self.settle_ingress(permit, observation, checkpoint) {
            PersistResult::Committed { .. } => Ok(()),
            PersistResult::Unknown => Err(Fault::Unavailable),
            PersistResult::Refused(fault) => Err(fault),
        }
    }
    pub(crate) fn classify_node_inputs(&mut self) -> Result<(), Fault> {
        let mut checkpoint = self.checkpoint_template()?;
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        for obligation in &mut checkpoint.image.obligations {
            let observation = pair
                .image
                .observations
                .iter()
                .find(|item| item.id == obligation.observation)
                .ok_or(Fault::Integrity)?;
            let Some(GuardStatus::Settled { guard, .. }) =
                pair.envelope.floor.guards.get(&observation.guard)
            else {
                return Err(Fault::Integrity);
            };
            obligation.classification = observations::classification(
                &mut self.evidence,
                &self.request,
                &pair.image,
                guard,
                observation,
                &obligation.expected,
            )?;
        }
        observations::retained(
            &mut self.evidence,
            &self.request,
            &checkpoint.image,
            &pair.envelope.floor,
        )?;
        if checkpoint.image.obligations == pair.image.obligations {
            return Ok(());
        }
        let projected = pair.envelope.floor.clone();
        match persist::select(self, checkpoint, projected, DiskOperation::Checkpoint) {
            PersistResult::Committed { .. } => Ok(()),
            PersistResult::Unknown => Err(Fault::Unavailable),
            PersistResult::Refused(fault) => Err(fault),
        }
    }
    pub(crate) fn local_result(
        &mut self,
        bundle: TransferBundle,
        channel: Digest,
        nonce: Digest,
    ) -> Result<(Vec<Outcome>, Option<TransferBundle>), Fault> {
        if bundle.instance != self.request.instance
            || bundle.candidate.instance != self.request.instance
            || bundle.kind != RecordKind::Candidate
            || bundle.receipt.is_some()
            || bundle.fork.is_some()
            || bundle.candidate.evidence.admission.is_some()
        {
            return Err(Fault::Integrity);
        }
        let outcomes = self.submit_local(bundle.candidate.clone(), channel, nonce)?;
        self.classify_node_inputs()?;
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        let accepted = pair
            .image
            .state
            .instances
            .get(&self.request.instance)
            .ok_or(Fault::Integrity)?
            .accepted
            .iter()
            .find(|accepted| accepted.candidate.operation == bundle.candidate.operation);
        Ok((
            outcomes,
            accepted.map(|accepted| TransferBundle {
                instance: self.request.instance.clone(),
                candidate: accepted.candidate.clone(),
                kind: RecordKind::Accepted,
                receipt: Some(accepted.receipt.clone()),
                fork: None,
            }),
        ))
    }
}
pub(super) fn bundle_ref(bundle: &TransferBundle, raw: &Bytes) -> RecordRef {
    let op = bundle.candidate.operation.inner();
    RecordRef {
        kind: bundle.kind.clone(),
        origin: op.origin.clone(),
        seq: op.seq,
        operation: bundle.candidate.operation.canonical().digest(),
        bundle: raw.digest(),
        receipt: bundle.receipt.clone(),
    }
}
pub(super) fn read_bundle(raw: &Bytes, limits: &DecodeLimits) -> Result<TransferBundle, Fault> {
    decode_profile(raw.value(), limits).map_err(codec_fault)
}
