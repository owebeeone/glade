//! Narrow trusted Records storage-attempt boundary, not a peer DTO or physical host.
//! All ownership, policy/time, restoration and finite limits are explicit inputs.
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instance {
    pub descriptor: Digest,
    pub incarnation: String,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalPrecondition {
    pub policy: Digest,
    pub time: TimeInterval,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoreIncarnation(pub Digest);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlanKey {
    pub namespace: Digest,
    pub instance: Instance,
    pub number: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AttemptId {
    pub store: StoreIncarnation,
    pub instance: Instance,
    pub number: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct InvocationId {
    pub namespace: Digest,
    pub number: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub store: StoreIncarnation,
    pub generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartPrecondition {
    pub cut: LocalPrecondition,
    /// All original finite permit windows; empty is invalid for local intent.
    pub windows: Vec<Window>,
}
/// Immutable owned opaque batch and complete storage binding. Semantic continuations
/// are encoded by the kernel in batch bytes, retained by Records without interpretation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub plan: PlanKey,
    pub batch: Bytes,
    pub expected_revision: u64,
    pub storage: StorageClass,
    pub kind: CommitKind,
    pub charges: Charges,
    pub receipts: Vec<Receipt>,
    pub prestart: Option<StartPrecondition>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptBinding {
    pub id: AttemptId,
    pub binding: Binding,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareRequest {
    pub owner: Owner,
    pub invocation: InvocationId,
    pub binding: Binding,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareResult {
    /// Includes unknown registration and absent plan index; no new attempt implied.
    Pending,
    Prepared(Box<AttemptBinding>),
    Refused(Fault),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareReply {
    pub request: PrepareRequest,
    pub result: PrepareResult,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationKind {
    Begin,
    Inspect,
    Fence,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveRequest {
    pub owner: Owner,
    pub invocation: InvocationId,
    pub operation: OperationKind,
    pub attempt: AttemptBinding,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeginRequest {
    pub request: ResolveRequest,
    pub current: LocalPrecondition,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalOutcome {
    Committed {
        revision: u64,
        storage: StorageClass,
        receipts: Vec<Receipt>,
        start_cut: Option<LocalPrecondition>,
    },
    /// Retained terminal tombstone proves this exact attempt can never publish.
    NonCommit {
        revision: u64,
        storage: StorageClass,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terminal {
    pub attempt: AttemptBinding,
    pub outcome: TerminalOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveResult {
    Pending,
    Started { cut: Option<LocalPrecondition> },
    Terminal(Box<Terminal>),
    Refused(Fault),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveReply {
    pub request: ResolveRequest,
    pub result: ResolveResult,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttemptPhase {
    Reserved,
    Started { cut: Option<LocalPrecondition> },
    Terminal(TerminalOutcome),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedAttempt {
    pub attempt: AttemptBinding,
    pub phase: AttemptPhase,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub instances: usize,
    pub attempts: usize,
    pub invocation_history: usize,
    pub max_batch_bytes: usize,
    pub max_receipts: usize,
    pub max_windows: usize,
    pub max_name_bytes: usize,
    pub critical_bytes: usize,
    pub critical_per_attempt: usize,
}
/// Supplied after trusted exclusive ownership/floor validation; no UUID issuance here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub owner: Owner,
    pub invocation_namespace: Digest,
    pub limits: Limits,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub owner: Owner,
    pub invocation_namespace: Digest,
    pub limits: Limits,
    pub preparing: Vec<Binding>,
    pub revisions: std::collections::BTreeMap<Instance, u64>,
    pub attempts: Vec<RetainedAttempt>,
    pub next_attempt: u64,
    pub invocation_high_water: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryResult {
    Validated(Box<Recovery>),
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseResult {
    Closed,
    Pending,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    BindingMismatch,
    CallbackMismatch,
    Ownership,
    Capacity,
    Integrity,
    Unavailable,
}
/// Records implements this contract using a qualified StorageAdapter. Opening MUST
/// validate exclusive ownership and retained floors before returning a usable session.
pub trait StorageAttemptHost {
    type Session: StorageAttemptSession;
    fn open(&mut self, request: OpenRequest) -> Result<Self::Session, Fault>;
}
/// Calls can remain Pending indefinitely. Dropping a waiter/session does not cancel
/// owned work. close MUST drain/fence every worker before reporting Closed.
pub trait StorageAttemptSession {
    fn recover(&mut self) -> RecoveryResult;
    fn prepare(&mut self, request: PrepareRequest) -> PrepareReply;
    fn recover_plan(&mut self, request: PrepareRequest) -> PrepareReply;
    fn begin(&mut self, request: BeginRequest) -> ResolveReply;
    fn inspect(&mut self, request: ResolveRequest) -> ResolveReply;
    fn request_fence(&mut self, request: ResolveRequest) -> ResolveReply;
    fn close(&mut self) -> CloseResult;
}
