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
