//! Authenticated replica operation transfer; no source command authority.
use std::{future::Future, num::NonZeroUsize};
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadRequest<C> {
    pub after: Option<C>,
    pub max_operations: NonZeroUsize,
    pub max_bytes: NonZeroUsize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<C> {
    pub operations: Vec<Vec<u8>>,
    pub cursor: C,
    pub more: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestReceipt<C> {
    pub cursor: C,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncError {
    Denied,
    Unavailable,
    InvalidCursor,
    Gap,
    InvalidOperation,
    Conflict,
    Capacity,
    OutcomeUnknown,
}

/// One authenticated replica namespace. Factory/composition MUST bind immutable
/// stream scope, generation and caller; each read/ingest MUST check current access.
/// Cursor is an opaque, namespace-scoped LOCAL resume token, not an authority term,
/// global total order or source revision. A profile may carry a per-origin frontier.
/// Expired/foreign cursors MUST return Gap/InvalidCursor, never silently start over.
///
/// Read MUST preserve canonical signed operation bytes and attribution, respect both
/// item and byte limits, and advance its cursor only over returned operations.
/// If the next indivisible op cannot fit, return Capacity, not an empty more=true
/// loop. more=false denotes the observed local boundary, not global completeness.
/// Retention/checkpoint profiles MUST retain enough authority/fencing history;
/// this interface does not authorize discarding it or snapshot-as-replication.
///
/// Ingest MUST validate schema, signatures, source authority, scope and origin
/// chains BEFORE atomically persisting the whole bounded batch. Duplicate canonical
/// operations are idempotent; conflicting identities fail. Success is durable local
/// retention, not source-write authority, remote replication or applied projection.
/// All known errors MUST leave the batch unapplied; OutcomeUnknown may be committed.
/// Invalid bytes MUST NOT panic or partially poison state. Empty ingest is a no-op.
/// Profile-specific batch limits MUST be enforced before mutation.
///
/// Futures MUST be lazy; cancelling polled ingest may leave an unknown outcome.
/// Reconciliation uses canonical identity/current state, not blind command retry.
///
/// ```compile_fail
/// use glade_sync_api::ReplicaSync;
/// struct Missing;
/// impl ReplicaSync for Missing { type Cursor=u64; }
/// ```
pub trait ReplicaSync: Send + Sync {
    type Cursor: Clone + Eq + Send + Sync;
    fn read(
        &self,
        request: ReadRequest<Self::Cursor>,
    ) -> impl Future<Output = Result<Page<Self::Cursor>, SyncError>> + Send;
    fn ingest(
        &self,
        operations: Vec<Vec<u8>>,
    ) -> impl Future<Output = Result<IngestReceipt<Self::Cursor>, SyncError>> + Send;
}
