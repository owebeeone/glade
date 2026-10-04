//! IC3 owned representation, not verified proof or physical recovery authority.
use crate::*;
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityBinding {
    pub root: Digest,
    pub intent: Bytes,
    pub zone: String,
    pub parameters: Bytes,
    pub declaration: Bytes,
    pub declaration_version: String,
    pub schema: Bytes,
    pub schema_identity: String,
    pub schema_version: String,
    pub corpus_identity: String,
    pub corpus_version: String,
    pub corpus_digest: Digest,
    pub proof_profile: String,
    pub recovery_profile: String,
    pub retention_policy: String,
    pub rejoin_policy: String,
    pub initial_chain_base: i64,
    pub migration_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    pub bytes: usize,
    pub items: usize,
    pub depth: usize,
    pub name_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaLimits {
    pub storage: Limits,
    pub decode: DecodeLimits,
    pub image_bytes: usize,
    pub guard_history: usize,
    pub inbox_items: usize,
    pub inbox_bytes: usize,
    pub inventory_items: usize,
    pub inventory_bytes: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ReplicaOpen {
    pub instance: Instance,
    pub descriptor: Descriptor,
    pub identity: IdentityBinding,
    pub owner: Owner,
    pub plan_namespace: Digest,
    pub invocation_namespace: Digest,
    pub limits: ReplicaLimits,
    pub expected_floor: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GuardId {
    pub store: StoreIncarnation,
    pub instance: Instance,
    pub number: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiveGuard {
    pub id: GuardId,
    pub owner: Owner,
    pub lease: u64,
    pub source: Digest,
    pub channel: Digest,
    pub role: String,
    pub base_generation: u64,
    pub base_digest: Digest,
    pub max_bytes: usize,
    pub max_items: usize,
    pub deadline_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardStatus {
    Active(ReceiveGuard),
    Settled {
        guard: ReceiveGuard,
        observation: Digest,
        image_generation: u64,
    },
    Abandoned {
        guard: ReceiveGuard,
        floor_sequence: u64,
    },
    Loss {
        guard: ReceiveGuard,
        loss: Digest,
        image_generation: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub slot: u8,
    pub generation: u64,
    pub digest: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloorIntent {
    pub issuance: IssuanceFloor,
    pub prior: Selection,
    pub next: Selection,
    pub next_attempt: u64,
    pub invocation_high_water: u64,
    pub guard_high_water: u64,
    pub guards: BTreeMap<GuardId, GuardStatus>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaFloor {
    pub issuance: IssuanceFloor,
    pub version: u64,
    pub owner: Owner,
    pub root_registration: Digest,
    pub path_binding: Digest,
    pub lease: u64,
    pub sequence: u64,
    pub selected: Selection,
    pub next_attempt: u64,
    pub invocation_high_water: u64,
    pub guard_high_water: u64,
    pub guards: BTreeMap<GuardId, GuardStatus>,
    pub intent: Option<FloorIntent>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordKind {
    Accepted,
    Candidate,
    Security,
    Fork,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordRef {
    pub kind: RecordKind,
    pub origin: String,
    pub seq: i64,
    pub operation: Digest,
    pub bundle: Digest,
    pub receipt: Option<Receipt>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    pub instance: Instance,
    pub source: Digest,
    pub number: u64,
    pub entries: Vec<RecordRef>,
    pub total_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryPage {
    pub instance: Instance,
    pub inventory: Digest,
    pub index: usize,
    pub count: usize,
    pub entries: Vec<RecordRef>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TransferBundle {
    pub instance: Instance,
    pub candidate: Candidate,
    pub kind: RecordKind,
    pub receipt: Option<Receipt>,
    pub fork: Option<ForkEvidence>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedRecord {
    pub purpose: String,
    pub version: u64,
    pub signer: Digest,
    pub body: Bytes,
    pub signature: Bytes,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExchangeHello {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub source: Digest,
    pub target: Digest,
    pub dialer_endpoint: Digest,
    pub acceptor_endpoint: Digest,
    pub exporter: Digest,
    pub nonce: Digest,
    pub role: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub id: u64,
    pub instance: Instance,
    pub source: Digest,
    pub channel: Digest,
    pub guard: GuardId,
    pub bytes: Bytes,
    pub expected: Vec<RecordRef>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Classification {
    Retained(Digest),
    Missing,
    Rejected(Bytes),
    Loss(Bytes),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obligation {
    pub observation: u64,
    pub expected: RecordRef,
    pub classification: Classification,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginCustody {
    pub certificate: OriginCertificate,
    pub next_seq: i64,
    pub prev: Option<Digest>,
    pub lamport: i64,
    pub frontier: Vec<RecordRef>,
    pub intents: Vec<Bytes>,
    pub receipts: Vec<Receipt>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ReplicaImage {
    pub version: u64,
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub generation: u64,
    pub predecessor: Digest,
    pub state: State,
    pub recovery: Recovery,
    pub prior_floor: ReplicaFloor,
    pub origins: Vec<OriginCustody>,
    pub observations: Vec<Observation>,
    pub obligations: Vec<Obligation>,
    pub sessions: Vec<SessionCustody>,
    pub watermark: u64,
    pub permanent_loss: Vec<Bytes>,
    pub limits: ReplicaLimits,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    pub expected_floor: Digest,
    pub expected_generation: u64,
    pub expected_digest: Digest,
    pub image: ReplicaImage,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconstructionRequest {
    pub instance: Instance,
    pub generation: u64,
    pub watermark: u64,
    pub manifest: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaCut {
    pub kernel: ReadCut,
    pub generation: u64,
    pub watermark: u64,
    pub manifest: Digest,
    pub obligations: Vec<Obligation>,
    pub guards: Vec<ReceiveGuard>,
    pub permanent_loss: bool,
    pub complete_local: bool,
}

/// A nonrecursive image plus the independently authoritative current floor.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplicaRecovery {
    pub image: ReplicaImage,
    pub authoritative_floor: ReplicaFloor,
}
