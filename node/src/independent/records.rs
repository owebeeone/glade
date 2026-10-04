use super::disk::{DiskLayout, DiskOperation, PairStore};
use super::{GenuineEvidence, PolicyClock};
use glade_crdt_recovery_codec::*;
mod authentication;
mod bindings;
mod evidence_recovery;
mod genesis;
mod guards;
mod host;
mod loss;
mod node_exchange;
mod node_ingress;
mod observations;
mod persist;
mod reserves;
mod runtime;
mod storage;
mod validation;

use glade_crdt_recovery_api::*;
use std::path::PathBuf;
pub struct DiskRootConfig {
    pub data: PathBuf,
    pub trusted_floor: PathBuf,
    pub registration: Digest,
    pub provision: bool,
}
pub struct DiskHost<P: PolicyClock> {
    evidence: Option<GenuineEvidence<P>>,
    roots: DiskRootConfig,
}
pub struct DiskSession<P: PolicyClock> {
    evidence: GenuineEvidence<P>,
    request: ReplicaOpen,
    pair: Option<PairStore>,
    authority: IngressAuthority,
    closing: bool,
    pending_ingress: std::collections::BTreeMap<GuardId, persist::PendingIngress>,
    // Owned private physical-cut observation; never a process-global hook.
    observe_cut: Box<dyn FnMut(DiskOperation, super::disk::PairCut) -> Result<(), Fault> + Send>,
}
impl<P: PolicyClock> DiskHost<P> {
    pub fn new(evidence: GenuineEvidence<P>, roots: DiskRootConfig) -> Self {
        Self {
            evidence: Some(evidence),
            roots,
        }
    }
}
impl<P: PolicyClock> DiskSession<P> {
    fn close_owned(&mut self) -> CloseResult {
        self.closing = true;
        if !self.authority.can_close()
            || !self.pending_ingress.is_empty()
            || self.pair.as_ref().is_some_and(|pair| {
                let state = &pair.image.state;
                pair.uncertain()
                    || pair.envelope.observing
                    || pair.envelope.pending.is_some()
                    || pair
                        .envelope
                        .floor
                        .guards
                        .values()
                        .any(|status| matches!(status, GuardStatus::Active(_)))
                    || !state.queries.is_empty()
                    || !state.preparing.is_empty()
                    || !state.commits.is_empty()
                    || !state.unknown_commits.is_empty()
                    || !state.storage_invocations.is_empty()
                    || !state.lookups.is_empty()
                    || pair
                        .image
                        .recovery
                        .attempts
                        .iter()
                        .any(|attempt| !matches!(attempt.phase, AttemptPhase::Terminal(_)))
            })
        {
            return CloseResult::Pending;
        }
        self.pair.take();
        CloseResult::Closed
    }
    /// Retries only the exact owned ingress continuation retained by this session.
    /// Unknown storage stays unavailable; reopening preserves the durable guard.
    /// Retains permanent loss only after this session's receive has drained.
    pub fn retain_ingress_loss(&mut self, permit: IngressPermit, observed: Bytes) -> PersistResult {
        loss::retain(self, permit, observed)
    }
    pub fn retry_ingress(&mut self, guard: &GuardId) -> PersistResult {
        persist::retry(self, guard)
    }
    /// Converts only this host's already-drained retained continuation to loss.
    pub fn retain_pending_ingress_loss(&mut self, guard: &GuardId) -> PersistResult {
        loss::pending(self, guard)
    }
    pub fn submit_local(
        &mut self,
        candidate: Candidate,
        channel: Digest,
        session: Digest,
    ) -> Result<Vec<Outcome>, Fault> {
        if self.closing {
            return Err(Fault::Ownership);
        }
        runtime::submit(self, candidate, channel, session)
    }

    pub fn issue_challenge(
        &mut self,
        request: super::SessionRequest,
    ) -> Result<SignedRecord, Fault> {
        if self.closing {
            return Err(Fault::Ownership);
        }
        self.pair.as_ref().ok_or(Fault::Ownership)?.owned()?;
        authentication::begin(self)?;
        let challenge = self.evidence.issue_challenge(request);
        authentication::persist(self)?;
        challenge
    }
    pub fn authenticate(
        &mut self,
        response: SignedRecord,
        channel: Digest,
        session: Digest,
    ) -> Result<(), Fault> {
        if self.closing {
            return Err(Fault::Ownership);
        }
        self.pair.as_ref().ok_or(Fault::Ownership)?.owned()?;
        authentication::begin(self)?;
        let result = self.evidence.authenticate(response, channel, session);
        authentication::persist(self)?;
        result
    }

    pub fn reconstruction_request(&self) -> Result<ReconstructionRequest, Fault> {
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        Ok(ReconstructionRequest {
            instance: pair.image.instance.clone(),
            generation: pair.image.generation,
            watermark: pair.image.watermark,
            manifest: guards::manifest(pair)?,
        })
    }
    pub fn checkpoint_template(&self) -> Result<Checkpoint, Fault> {
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        let mut image = pair.image.clone();
        image.generation = image.generation.checked_add(1).ok_or(Fault::Capacity)?;
        image.predecessor = pair.envelope.floor.selected.digest;
        image.prior_floor = pair.envelope.floor.clone();
        Ok(Checkpoint {
            expected_floor: pair.floor_digest()?,
            expected_generation: pair.image.generation,
            expected_digest: pair.envelope.floor.selected.digest,
            image,
        })
    }
}
impl<P: PolicyClock> DiskHost<P> {
    /// Node-local locked restoration: trusted floor, never an image or caller
    /// projection, supplies the retained namespace and exact open authority.
    pub(crate) fn open_node(&mut self, mut request: ReplicaOpen) -> Result<DiskSession<P>, Fault> {
        if self.roots.provision {
            return self.open_owned(request, None);
        }
        let layout = DiskLayout::open(
            &self.roots.data,
            &self.roots.trusted_floor,
            self.roots.registration,
        )?;
        request.expected_floor = layout.floor_digest(request.limits.decode.bytes)?;
        self.open_owned(request, Some(layout))
    }
    fn open_owned(
        &mut self,
        request: ReplicaOpen,
        supplied_layout: Option<DiskLayout>,
    ) -> Result<DiskSession<P>, Fault> {
        let mut evidence = self.evidence.take().ok_or(Fault::Ownership)?;
        genesis::request(&evidence, &request)?;
        let pair = if self.roots.provision {
            if request.expected_floor != [0; 32] {
                return Err(Fault::Integrity);
            }
            let layout = DiskLayout::provision(
                &self.roots.data,
                &self.roots.trusted_floor,
                self.roots.registration,
            )?;
            let (image, floor) = genesis::create(
                &mut evidence,
                &request,
                self.roots.registration,
                layout.path_binding()?,
            )?;
            PairStore::create(layout, image, floor, request.limits.decode.clone())?
        } else {
            let layout = match supplied_layout {
                Some(layout) => layout,
                None => DiskLayout::open(
                    &self.roots.data,
                    &self.roots.trusted_floor,
                    self.roots.registration,
                )?,
            };
            if layout.floor_digest(request.limits.decode.bytes)? != request.expected_floor {
                return Err(Fault::Integrity);
            }
            let retained = layout.read_floor(&request.limits.decode)?;
            if retained.floor.owner != request.owner
                || retained.floor.issuance.plan_namespace != request.plan_namespace
                || retained.floor.issuance.invocation_namespace != request.invocation_namespace
            {
                return Err(Fault::BindingMismatch);
            }
            let mut pair = PairStore::open_checked(
                layout,
                request.limits.decode.clone(),
                |_| Ok(()),
                |image, envelope| {
                    evidence.restore_sessions(&image.sessions)?;
                    genesis::snapshot(&mut evidence, &request, image, envelope)
                },
            )?;
            evidence.restore_sessions(&pair.image.sessions)?;
            genesis::validate(&mut evidence, &request, &pair)?;
            let mut next = pair.image.clone();
            next.generation = next.generation.checked_add(1).ok_or(Fault::Capacity)?;
            next.predecessor = pair.envelope.floor.selected.digest;
            next.prior_floor = pair.envelope.floor.clone();
            let mut projected = pair.envelope.floor.clone();
            projected.lease = projected.lease.checked_add(1).ok_or(Fault::Capacity)?;
            pair.transact(next, projected, DiskOperation::Lease, |_| Ok(()))?;
            pair
        };
        genesis::validate(&mut evidence, &request, &pair)?;
        evidence.restore_sessions(&pair.image.sessions)?;
        evidence.observed_policy = Some((
            pair.envelope.floor.issuance.policy_generation,
            pair.envelope.floor.issuance.policy_digest,
        ));
        evidence.observed_time_floor = pair.envelope.floor.issuance.time_floor_ms;
        let mut session = DiskSession {
            evidence,
            request: request.clone(),
            pair: Some(pair),
            authority: IngressAuthority::new(request.limits.guard_history),
            observe_cut: Box::new(|_, _| Ok(())),
            pending_ingress: std::collections::BTreeMap::new(),
            closing: false,
        };
        runtime::reattach(&mut session)?;
        Ok(session)
    }
}
impl<P: PolicyClock> ReplicaRecoverySession for DiskSession<P> {
    fn load(&mut self) -> LoadResult {
        let Some(pair) = &self.pair else {
            return LoadResult::Unavailable;
        };
        if pair.owned().is_err()
            || pair.envelope.observing
            || genesis::validate(&mut self.evidence, &self.request, pair).is_err()
        {
            return LoadResult::Unavailable;
        }
        LoadResult::Validated(Box::new(ReplicaRecovery {
            image: pair.image.clone(),
            authoritative_floor: pair.envelope.floor.clone(),
        }))
    }
    fn checkpoint(&mut self, request: Checkpoint) -> PersistResult {
        persist::plain(self, request)
    }
    fn begin_ingress(&mut self, guard: ReceiveGuard) -> Result<IngressPermit, Fault> {
        if self.closing {
            return Err(Fault::Ownership);
        }
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        guards::fresh(&self.request, pair, &guard)?;
        let mut reserved = pair.envelope.floor.clone();
        reserved.guard_high_water = reserved
            .guard_high_water
            .checked_add(1)
            .ok_or(Fault::Capacity)?;
        reserved
            .guards
            .insert(guard.id.clone(), GuardStatus::Active(guard.clone()));
        loss::reserve(&pair.image, &reserved, &guard)?;
        authentication::begin(self)?;
        self.evidence.current()?;
        let (cut, policy) = authentication::persist_observed(self)?;
        let pair = self.pair.as_mut().ok_or(Fault::Ownership)?;
        guards::fresh(&self.request, pair, &guard)?;
        let policy_digest =
            super::namespace::record_bytes(&cut.policy, &self.evidence.config.limits)?.digest();
        if policy_digest != pair.envelope.floor.issuance.policy_digest
            || self.evidence.observed_time_floor != pair.envelope.floor.issuance.time_floor_ms
        {
            return Err(Fault::Unavailable);
        }
        if !policy.replica_holders.contains(&guard.source)
            || policy.revoked.contains(&guard.source)
            || !super::auth::fits(cut.time.as_ref().ok_or(Fault::Unavailable)?, &policy.window)
            || cut
                .time
                .as_ref()
                .is_none_or(|time| time.latest_ms > guard.deadline_ms)
        {
            return Err(Fault::Ownership);
        }
        let mut projected = pair.envelope.floor.clone();
        projected.guard_high_water = projected
            .guard_high_water
            .checked_add(1)
            .ok_or(Fault::Capacity)?;
        projected
            .guards
            .insert(guard.id.clone(), GuardStatus::Active(guard.clone()));
        loss::reserve(&pair.image, &projected, &guard)?;
        pair.floor_only(projected, |cut| {
            (self.observe_cut)(
                DiskOperation::BeginIngress,
                super::disk::PairCut::Intent(cut),
            )
        })?;
        self.authority.issue(guard)
    }
    fn receive_ingress<F: std::future::Future<Output = Result<Bytes, Fault>>>(
        &mut self,
        permit: IngressPermit,
        make: impl FnOnce() -> F,
    ) -> Result<GuardedReceive<F>, Fault> {
        if self.closing {
            return Err(Fault::Ownership);
        }
        let pair = self.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        if !self.authority.owns(&permit)
            || pair.envelope.floor.guards.get(&permit.guard().id)
                != Some(&GuardStatus::Active(permit.guard().clone()))
        {
            return Err(Fault::Ownership);
        }
        self.authority.receive(permit, make)
    }
    fn settle_ingress(
        &mut self,
        permit: IngressPermit,
        observation: Observation,
        checkpoint: Checkpoint,
    ) -> PersistResult {
        persist::settle(self, permit, observation, checkpoint)
    }
    fn abandon_ingress(&mut self, permit: IngressPermit) -> PersistResult {
        persist::abandon(self, permit)
    }
    fn record_observation(
        &mut self,
        observation: Observation,
        checkpoint: Checkpoint,
    ) -> PersistResult {
        persist::observation(self, observation, checkpoint)
    }
    fn reconstruct_cut(&mut self, request: ReconstructionRequest) -> ReconstructResult {
        let Some(pair) = &self.pair else {
            return ReconstructResult::Unavailable;
        };
        if pair.owned().is_err()
            || pair.envelope.observing
            || genesis::validate(&mut self.evidence, &self.request, pair).is_err()
        {
            return ReconstructResult::Unavailable;
        }
        guards::reconstruct(pair, request)
    }
    fn close(&mut self) -> CloseResult {
        self.close_owned()
    }
}

impl<P: PolicyClock> StorageAttemptSession for DiskSession<P> {
    fn recover(&mut self) -> RecoveryResult {
        let Some(pair) = &self.pair else {
            return RecoveryResult::Unavailable;
        };
        if pair.owned().is_err()
            || pair.envelope.observing
            || genesis::validate(&mut self.evidence, &self.request, pair).is_err()
        {
            return RecoveryResult::Unavailable;
        }
        RecoveryResult::Validated(Box::new(pair.image.recovery.clone()))
    }
    fn prepare(&mut self, request: PrepareRequest) -> PrepareReply {
        storage::prepare(self, request)
    }
    fn recover_plan(&mut self, request: PrepareRequest) -> PrepareReply {
        storage::prepare(self, request)
    }
    fn begin(&mut self, request: BeginRequest) -> ResolveReply {
        storage::begin(self, request)
    }
    fn inspect(&mut self, request: ResolveRequest) -> ResolveReply {
        if request.operation != OperationKind::Inspect {
            return ResolveReply {
                request,
                result: ResolveResult::Refused(Fault::CallbackMismatch),
            };
        }
        storage::resolve(self, request)
    }
    fn request_fence(&mut self, request: ResolveRequest) -> ResolveReply {
        if request.operation != OperationKind::Fence {
            return ResolveReply {
                request,
                result: ResolveResult::Refused(Fault::CallbackMismatch),
            };
        }
        storage::resolve(self, request)
    }
    fn close(&mut self) -> CloseResult {
        self.close_owned()
    }
}
impl<P: PolicyClock> StorageAttemptHost for DiskHost<P> {
    type Session = DiskSession<P>;
    fn open(&mut self, request: OpenRequest) -> Result<Self::Session, Fault> {
        host::narrow(self, request)
    }
}

impl<P: PolicyClock> ReplicaRecoveryHost for DiskHost<P> {
    type Session = DiskSession<P>;
    fn open_replica(&mut self, request: ReplicaOpen) -> Result<Self::Session, Fault> {
        self.open_owned(request, None)
    }
}

pub(crate) fn node_reference(bundle: &TransferBundle, raw: &Bytes) -> RecordRef {
    node_ingress::bundle_ref(bundle, raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[path = "../tests.rs"]
    mod content;
}
