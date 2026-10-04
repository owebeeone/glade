//! Typed INTERNAL semantics; JSON traces are harness output, not a wire schema.
use std::collections::BTreeMap;

pub use glade_crdt_storage_attempt_api::*;
use glade_wire::{cbor, generated::Op};

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
#[derive(Clone, Debug, PartialEq)]
pub struct Accepted {
    pub candidate: Candidate,
    pub facts: Facts,
    pub receipt: Receipt,
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
    /// Explicit retained conviction; each rival must independently qualify.
    pub qualified_forks: Vec<ForkEvidence>,
    pub charges: Charges,
}
pub type CommitReply = ResolveReply;
pub type LookupRequest = ResolveRequest;
pub type LookupReply = ResolveReply;
pub type CommitResult = ResolveResult;
pub type LookupResult = ResolveResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelAttempt {
    pub attempt: AttemptBinding,
    pub phase: AttemptPhase,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageInvocation {
    Prepare(PrepareRequest),
    RecoverPlan(PrepareRequest),
    Resolve(ResolveRequest),
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
    /// Batch custody only; this outcome is never application admission.
    BatchRetained {
        instance: Instance,
        kind: CommitKind,
        batch_digest: Digest,
        revision: u64,
        storage: StorageClass,
    },
    /// Immutable batch identity; no application receipt is implied.
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
    PrepareResolved(PrepareReply),
    CommitResolved(CommitReply),
    FenceResolved(ResolveReply),
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
    Prepare {
        plan: CommitPlan,
        request: PrepareRequest,
    },
    RecoverPlan(PrepareRequest),
    Begin(BeginRequest),
    Fence(ResolveRequest),
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
    /// Sticky observed recovery loss. No clearing event exists in this contract.
    pub recovery_incomplete: bool,
    pub storage_integrity_failed: bool,
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
    /// Keyed by invocation scalar inside invocation_namespace; Pending retires
    /// only that invocation, never the reserved plan.
    pub lookups: BTreeMap<u64, LookupRequest>,
    /// Bound to caller-owned validated store/session and never inferred from peers.
    pub storage_owner: Option<Owner>,
    /// Finite session declaration caps ledger, callbacks and critical reservations.
    pub storage_limits: Option<Limits>,
    pub plan_namespace: Digest,
    pub invocation_namespace: Digest,
    pub attempts: BTreeMap<PlanKey, KernelAttempt>,
    pub preparing: BTreeMap<PlanKey, PrepareRequest>,
    /// Outstanding exact issued requests only.
    pub storage_invocations: BTreeMap<InvocationId, StorageInvocation>,
    /// Consumed requests retained within invocation_history, for duplicate versus
    /// contradictory-result authentication. No implicit retirement/GC on reopen.
    pub retired_storage_invocations: BTreeMap<InvocationId, StorageInvocation>,
    /// Retained for the incarnation, even after reservation release.
    pub terminal_results: BTreeMap<AttemptId, Terminal>,
    /// First contradictory bound assertion per attempt, capped by retained attempts.
    pub terminal_conflicts: BTreeMap<AttemptId, Terminal>,
    pub next_effect_id: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub state: State,
    pub effects: Vec<Effect>,
}

mod production;
pub use production::*;

mod proofs;
pub use proofs::*;
