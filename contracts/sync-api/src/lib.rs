//! Authenticated replica operation transfer; no source command authority.
use std::{future::Future, num::NonZeroUsize};
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

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Synthetic [sequence,payload] records; real adapters use their own valid signed corpus.
    use crate::*;
    /// Adapter-reusable assertion. Supply valid signed bytes and native scoped cursors.
    /// Requires an exclusively controlled namespace and a page covering the supplied batch.
    pub async fn assert_ingest_and_replay<R: ReplicaSync>(
        replica: &R,
        operations: Vec<Vec<u8>>,
        receipt: IngestReceipt<R::Cursor>,
        request: ReadRequest<R::Cursor>,
        page: Page<R::Cursor>,
    ) where
        R::Cursor: std::fmt::Debug,
    {
        assert_eq!(
            replica.ingest(operations.clone()).await,
            Ok(receipt.clone())
        );
        assert_eq!(replica.ingest(operations).await, Ok(receipt));
        assert!(page.operations.len() <= request.max_operations.get());
        assert!(page.operations.iter().map(Vec::len).sum::<usize>() <= request.max_bytes.get());
        assert_eq!(replica.read(request).await, Ok(page));
    }
    /// Known-error atomicity with caller-supplied invalid corpus and inspection cursor.
    /// Inspection MUST cover every location the invalid batch could affect. No other writer.
    pub async fn assert_rejection<R: ReplicaSync>(
        replica: &R,
        operations: Vec<Vec<u8>>,
        error: SyncError,
        inspection: ReadRequest<R::Cursor>,
    ) where
        R::Cursor: std::fmt::Debug,
    {
        assert_ne!(error, SyncError::OutcomeUnknown);
        let before = replica.read(inspection.clone()).await.unwrap();
        assert_eq!(replica.ingest(operations).await, Err(error));
        assert_eq!(replica.read(inspection).await, Ok(before));
    }
    pub fn request() -> ReadRequest<u64> {
        ReadRequest {
            after: None,
            max_operations: NonZeroUsize::new(1).unwrap(),
            max_bytes: NonZeroUsize::new(2).unwrap(),
        }
    }
    /// SY-001. Empty fixture, sequential two-byte records; duplicate is no-op.
    pub async fn roundtrip<R: ReplicaSync<Cursor = u64>>(replica: &R) {
        let ops = vec![vec![1, 10], vec![2, 20]];
        assert_eq!(
            replica.ingest(ops.clone()).await,
            Ok(IngestReceipt { cursor: 2 })
        );
        assert_eq!(replica.ingest(ops).await, Ok(IngestReceipt { cursor: 2 }));
        assert_eq!(
            replica.read(request()).await,
            Ok(Page {
                operations: vec![vec![1, 10]],
                cursor: 1,
                more: true
            })
        );
        let mut next = request();
        next.after = Some(1);
        assert_eq!(
            replica.read(next).await,
            Ok(Page {
                operations: vec![vec![2, 20]],
                cursor: 2,
                more: false
            })
        );
        assert_eq!(
            replica.ingest(vec![vec![1, 99]]).await,
            Err(SyncError::Conflict)
        );
        assert_eq!(
            replica.ingest(vec![]).await,
            Ok(IngestReceipt { cursor: 2 })
        );
    }
    /// SY-002. Invalid final record MUST NOT retain the valid first record.
    pub async fn invalid<R: ReplicaSync<Cursor = u64>>(replica: &R) {
        assert_eq!(
            replica.ingest(vec![vec![1, 10], vec![]]).await,
            Err(SyncError::InvalidOperation)
        );
        assert_eq!(
            replica.read(request()).await,
            Ok(Page {
                operations: vec![],
                cursor: 0,
                more: false
            })
        );
    }
    /// SY-003. Impossible cursor and too-small byte budget are explicit failures.
    pub async fn bounds<R: ReplicaSync<Cursor = u64>>(replica: &R) {
        replica.ingest(vec![vec![1, 10]]).await.unwrap();
        let mut r = request();
        r.after = Some(9);
        assert_eq!(replica.read(r).await, Err(SyncError::InvalidCursor));
        let mut r = request();
        r.max_bytes = NonZeroUsize::new(1).unwrap();
        assert_eq!(replica.read(r).await, Err(SyncError::Capacity));
    }
}
