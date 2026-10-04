//! Exact fresh signed namespace and empty-state recovery; later ledgers have stricter validators.
use super::*;
use glade_crdt_evidence_api::EvidencePort;
use std::collections::BTreeMap;
pub(super) fn request<P: PolicyClock>(
    evidence: &GenuineEvidence<P>,
    request: &ReplicaOpen,
) -> Result<(), Fault> {
    if request.instance != evidence.config.descriptor.instance
        || request.descriptor != evidence.config.descriptor
        || request.identity != evidence.config.identity
        || request.limits != evidence.intent.limits
        || request.owner.store.0 == [0; 32]
        || request.owner.generation == 0
        || request.plan_namespace == [0; 32]
        || request.invocation_namespace == [0; 32]
        || request.plan_namespace == request.invocation_namespace
        || evidence.intent.storage != StorageClass::LocalProcessRestart
        || request.limits.image_bytes == 0
        || request.limits.image_bytes > request.limits.decode.bytes
        || request.limits.guard_history == 0
        || request.limits.storage.instances == 0
    {
        return Err(Fault::Integrity);
    }
    Ok(())
}
pub(super) fn state(
    request: &ReplicaOpen,
    policy: Bytes,
    time: TimeInterval,
    time_floor: u64,
) -> State {
    let instance = InstanceState {
        descriptor: request.descriptor.clone(),
        revision: 0,
        storage: StorageClass::LocalProcessRestart,
        accepted: vec![],
        candidates: vec![],
        security_evidence: vec![],
        qualified_forks: vec![],
        canonical_origins: BTreeMap::new(),
        heads: BTreeMap::new(),
        pending: vec![],
        quarantine: BTreeMap::new(),
        policy,
        recovery_incomplete: false,
        storage_integrity_failed: false,
        time_floor_ms: time_floor,
        policy_time: time,
        used: Charges {
            edit: 0,
            recovery: 0,
            evidence: 0,
            pending: 0,
        },
    };
    State {
        instances: BTreeMap::from([(request.instance.clone(), instance)]),
        storage_owner: Some(request.owner.clone()),
        storage_limits: Some(request.limits.storage.clone()),
        plan_namespace: request.plan_namespace,
        invocation_namespace: request.invocation_namespace,
        ..State::default()
    }
}
pub(super) fn create<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    registration: Digest,
    path: Digest,
) -> Result<(ReplicaImage, ReplicaFloor), Fault> {
    let observed = evidence.observe(&request.instance, &request.identity)?;
    if !observed.allowed || !observed.available {
        return Err(Fault::Ownership);
    }
    let policy: PolicyBody = super::super::namespace::signed_bytes(
        &observed.policy,
        &evidence.config.trusted_root,
        &evidence.config.limits,
    )?;
    let issuance = IssuanceFloor {
        time_floor_ms: evidence.observed_time_floor,
        policy_generation: policy.generation,
        policy_digest: observed.policy.digest(),
        plan_namespace: request.plan_namespace,
        invocation_namespace: request.invocation_namespace,
        next_effect_id: 0,
        origins: BTreeMap::new(),
    };
    let floor = ReplicaFloor {
        issuance,
        version: 1,
        owner: request.owner.clone(),
        root_registration: registration,
        path_binding: path,
        lease: 1,
        sequence: 0,
        selected: Selection {
            slot: 0,
            generation: 0,
            digest: [0; 32],
        },
        next_attempt: 1,
        invocation_high_water: 0,
        guard_high_water: 0,
        guards: BTreeMap::new(),
        intent: None,
    };
    let recovery = Recovery {
        owner: request.owner.clone(),
        invocation_namespace: request.invocation_namespace,
        limits: request.limits.storage.clone(),
        preparing: vec![],
        revisions: BTreeMap::from([(request.instance.clone(), 0)]),
        attempts: vec![],
        next_attempt: 1,
        invocation_high_water: 0,
        invocation_count: 0,
    };
    let image = ReplicaImage {
        version: 1,
        instance: request.instance.clone(),
        identity: request.identity.clone(),
        generation: 0,
        predecessor: [0; 32],
        state: state(
            request,
            observed.policy,
            observed.time,
            evidence.observed_time_floor,
        ),
        recovery,
        prior_floor: floor.clone(),
        origins: vec![],
        observations: vec![],
        obligations: vec![],
        sessions: vec![],
        watermark: 0,
        permanent_loss: vec![],
        limits: request.limits.clone(),
    };
    Ok((image, floor))
}
pub(super) fn validate<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    pair: &PairStore,
) -> Result<(), Fault> {
    snapshot(evidence, request, &pair.image, &pair.envelope)
}
pub(super) fn snapshot<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    image: &ReplicaImage,
    envelope: &super::super::disk::FloorEnvelope,
) -> Result<(), Fault> {
    self::request(evidence, request)?;
    let floor = &envelope.floor;
    if image.instance != request.instance
        || image.identity != request.identity
        || image.limits != request.limits
        || floor.owner != request.owner
        || floor.issuance.plan_namespace != request.plan_namespace
        || floor.issuance.invocation_namespace != request.invocation_namespace
        || image.recovery.owner != request.owner
        || image.recovery.invocation_namespace != request.invocation_namespace
        || image.recovery.limits != request.limits.storage
        || !image.origins.is_empty()
    {
        return Err(Fault::Integrity);
    }
    evidence.validate_sessions(&image.sessions)?;
    super::loss::retained(image, floor)?;
    super::guards::retained(request, image, floor)?;
    super::observations::retained(evidence, request, image, floor)?;
    let value = image
        .state
        .instances
        .get(&request.instance)
        .ok_or(Fault::Integrity)?;
    let policy: PolicyBody = super::super::namespace::signed_bytes(
        &value.policy,
        &evidence.config.trusted_root,
        &evidence.config.limits,
    )?;
    let lost_observation = envelope
        .unmaterialized
        .iter()
        .any(|pending| pending.kind == DiskOperation::Observation);
    if policy.instance != request.instance
        || policy.identity != request.identity
        || policy.creation_root != request.descriptor.creation_root
        || policy.generation > floor.issuance.policy_generation
        || (policy.generation == floor.issuance.policy_generation
            && value.policy.digest() != floor.issuance.policy_digest)
        || (policy.generation < floor.issuance.policy_generation && !lost_observation)
        || value.time_floor_ms > floor.issuance.time_floor_ms
        || (value.time_floor_ms < floor.issuance.time_floor_ms && !lost_observation)
        || value.policy_time.earliest_ms < value.time_floor_ms
        || value.policy_time.earliest_ms > value.policy_time.latest_ms
    {
        return Err(Fault::Integrity);
    }
    super::validation::state(evidence, request, image, floor)?;
    super::reserves::critical(image, floor)?;
    Ok(())
}
