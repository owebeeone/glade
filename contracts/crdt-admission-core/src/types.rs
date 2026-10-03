//! Typed INTERNAL semantics; JSON traces are harness output, not a wire schema.
use std::collections::BTreeMap;

use glade_wire::{cbor, generated::Op};
use sha2::{Digest as _, Sha256};

pub type Digest = [u8; 32];

/// Owned immutable bytes. Every binding uses the digest computed from these bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bytes {
    value: Vec<u8>,
    digest: Digest,
}
impl Bytes {
    pub fn new(value: Vec<u8>) -> Self {
        let digest = Sha256::digest(&value).into();
        Self { value, digest }
    }
    pub fn value(&self) -> &[u8] {
        &self.value
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
}

/// No independently supplied decoded fields can disagree with canonical inner bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Operation {
    inner: Op,
    canonical: Bytes,
}
impl Operation {
    pub fn new(inner: Op) -> Self {
        let canonical = Bytes::new(cbor::encode(&inner.to_cbor()));
        Self { inner, canonical }
    }
    pub fn inner(&self) -> &Op {
        &self.inner
    }
    pub fn canonical(&self) -> &Bytes {
        &self.canonical
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instance {
    pub descriptor: Digest,
    pub incarnation: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionMode {
    Independent,
    Legacy,
    Strong,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub max_operation_bytes: usize,
    pub max_origins: usize,
    pub max_refs: usize,
    pub edit_bytes: usize,
    pub recovery_bytes: usize,
    pub evidence_bytes: usize,
    pub pending_bytes: usize,
    pub pending_items: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub instance: Instance,
    pub canonical: Bytes,
    pub creation_root: Digest,
    pub resource: String,
    pub share: String,
    pub glade_id: String,
    pub key: Vec<u8>,
    pub engine: String,
    pub payload_profile: String,
    pub admission_profile: String,
    pub authorization_profile: String,
    pub mode: AdmissionMode,
    pub bounds: Bounds,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub instance: Instance,
    pub origin: String,
    pub seq: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeInterval {
    pub earliest_ms: u64,
    pub latest_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub not_before_ms: u64,
    pub not_after_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginCertificate {
    pub instance: Instance,
    pub canonical_origin: String,
    pub epoch: String,
    pub display_name: String,
    pub writer_key: Digest,
    pub requester: String,
    pub canonical: Bytes,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceSet {
    pub certificate: Bytes,
    pub permit: Bytes,
    pub policy: Bytes,
    pub admission: Option<Bytes>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Local {
        current_policy: Digest,
        time: TimeInterval,
    },
    Historical,
    AdmissionSeal,
}
/// Reply MUST match every field, including evidence bytes, scope and validation mode.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub id: u64,
    pub instance: Instance,
    pub descriptor: Descriptor,
    pub operation: Operation,
    pub evidence: EvidenceSet,
    pub mode: Mode,
    pub verified_facts: Option<Box<Facts>>,
    pub validated_under: Option<Mode>,
}
/// Trusted provider results, NOT cryptographic proof produced by this pure crate.
/// Historical authorization does not consume the kernel's derived quarantine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub certificate: OriginCertificate,
    pub descriptor_valid: bool,
    pub payload_valid: bool,
    pub signatures_valid: bool,
    pub issuance_unique: bool,
    pub permit_window: Window,
    pub permit_first_seq: i64,
    pub permit_last_seq: i64,
    pub current_allowed: bool,
    pub historical_admitter_authorized: bool,
    pub historical_policy_allowed: bool,
    pub historical_time: Option<TimeInterval>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Verified(Box<Facts>),
    Invalid,
    Unavailable,
    Sealed(Bytes),
}
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceReply {
    pub query: Query,
    pub verdict: Verdict,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub instance: Instance,
    pub operation: Operation,
    pub evidence: EvidenceSet,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageClass {
    VolatileTest,
    LocalProcessRestart,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub instance: Instance,
    pub operation: Digest,
    pub admission: Digest,
    pub stable_id: Digest,
    pub storage: StorageClass,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Accepted {
    pub candidate: Candidate,
    pub facts: Facts,
    pub receipt: Receipt,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Charges {
    pub edit: usize,
    pub recovery: usize,
    pub evidence: usize,
    pub pending: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitKind {
    AcceptedBatch,
    Candidate,
    SecurityEvidence,
    QualifiedFork,
}
#[derive(Clone, Debug, PartialEq)]
pub struct StagedAcceptance {
    pub candidate: Candidate,
    pub facts: Facts,
    pub verified_query: Query,
    pub seal_query: Option<Query>,
    pub sealed_admission: Bytes,
    pub receipt: Receipt,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PendingVerification {
    pub query: Query,
    pub candidate: Candidate,
    pub verified: Option<Facts>,
    pub verified_query: Option<Query>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalPrecondition {
    pub policy: Digest,
    pub time: TimeInterval,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CommitPlan {
    pub id: u64,
    pub instance: Instance,
    pub expected_revision: u64,
    pub local_precondition: Option<LocalPrecondition>,
    pub batch: Bytes,
    pub kind: CommitKind,
    pub records: Vec<Candidate>,
    pub acceptances: Vec<StagedAcceptance>,
    pub charges: Charges,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitResult {
    Committed {
        revision: u64,
        storage: StorageClass,
    },
    KnownNotCommitted,
    OutcomeUnknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitReply {
    pub plan_id: u64,
    pub instance: Instance,
    pub batch_digest: Digest,
    pub result: CommitResult,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupResult {
    Committed { receipt: Receipt, revision: u64 },
    KnownAbsent,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupRequest {
    pub plan_id: u64,
    pub expected_revision: u64,
    pub instance: Instance,
    pub operation: Digest,
    pub batch_digest: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupReply {
    pub plan_id: u64,
    pub expected_revision: u64,
    pub instance: Instance,
    pub operation: Digest,
    pub batch_digest: Digest,
    pub result: LookupResult,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ForkEvidence {
    pub first: Candidate,
    pub second: Candidate,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadCut {
    pub instance: Instance,
    pub contiguous_common_heads: BTreeMap<String, (i64, Digest)>,
    pub historically_qualified: Vec<Digest>,
    pub fork_slots: Vec<Slot>,
    pub eligible: Vec<Digest>,
    pub pending: Vec<Slot>,
    pub quarantine: BTreeMap<String, i64>,
    pub complete_local: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    AcceptedLocal(Receipt),
    ExactRetry(Receipt),
    OutcomeUnknown(Digest),
    MissingHistory(Vec<Slot>),
    PolicyPending,
    Denied,
    Invalid,
    Unsupported,
    Capacity,
    IntegrityConflict,
    CallbackMismatch,
    Unavailable,
    Read(ReadCut),
}
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    SubmitLocal(Box<Candidate>),
    OfferReplica(Box<Candidate>),
    EvidenceResolved(Box<EvidenceReply>),
    CommitResolved(CommitReply),
    LookupResolved(LookupReply),
    ObservePolicy {
        instance: Instance,
        policy: Bytes,
        time: TimeInterval,
    },
    ObserveFrontier {
        instance: Instance,
        heads: BTreeMap<String, (i64, Digest)>,
    },
    OfferFork(Box<ForkEvidence>),
    ResumeRecovery(Instance),
    Read(Instance),
}
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Verify(Query),
    Seal(Query),
    Commit(CommitPlan),
    Lookup(LookupRequest),
    RequestMissing(Vec<Slot>),
    OfferMissing(Vec<Candidate>),
    ProjectionChanged(ReadCut),
    Report(Outcome),
}
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceState {
    pub descriptor: Descriptor,
    pub revision: u64,
    pub storage: StorageClass,
    pub accepted: Vec<Accepted>,
    pub candidates: Vec<Candidate>,
    pub security_evidence: Vec<Candidate>,
    pub qualified_forks: Vec<ForkEvidence>,
    pub canonical_origins: BTreeMap<String, OriginCertificate>,
    pub heads: BTreeMap<String, (i64, Digest)>,
    pub pending: Vec<Slot>,
    pub quarantine: BTreeMap<String, i64>,
    pub policy: Bytes,
    pub time_floor_ms: u64,
    pub policy_time: TimeInterval,
    pub used: Charges,
}
#[derive(Clone, Debug, PartialEq, Default)]
pub struct State {
    pub instances: BTreeMap<Instance, InstanceState>,
    pub queries: BTreeMap<u64, PendingVerification>,
    pub commits: BTreeMap<u64, CommitPlan>,
    pub unknown_commits: BTreeMap<u64, CommitPlan>,
    pub lookups: BTreeMap<u64, LookupRequest>,
    pub next_effect_id: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub state: State,
    pub effects: Vec<Effect>,
}
