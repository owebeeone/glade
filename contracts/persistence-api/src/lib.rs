//! Draft local snapshot persistence, independent of node, codec and async runtime.
//!
//! Bytes are one caller-encoded, versioned snapshot (including any operation heads).
//! This port neither invents a wire format nor confers source-write authority.
//! Replication remains attributed operations, not snapshot/database replication.

use std::future::Future;

/// One complete local snapshot; revision is storage-local, not a source epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub revision: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    Conflict,
    Unavailable,
    Corrupt,
    Capacity,
    Exhausted,
    OutcomeUnknown,
}

/// A handle bound to one immutable local snapshot namespace.
///
/// Implementations MUST linearize load/compare_exchange within that namespace,
/// including competing handles. None means positively established absence, never
/// unreadable/corrupt storage. Empty bytes are a present snapshot, not deletion.
///
/// Successful writes MUST atomically and durably replace all bytes and revision.
/// An acknowledged successful write MUST survive restart unless superseded by a
/// subsequent committed write. Interrupted or unknown writes MUST recover the
/// entire old or entire new snapshot, never a torn combination.
/// Initial revision is 1; subsequent commits increment without wrapping. Expected
/// None means create-if-absent, Some(r) means replace exactly revision r. All
/// errors except OutcomeUnknown MUST leave the stored state unchanged by this call.
/// Capacity MUST be enforced before mutation. Readers may still observe other writers.
///
/// Futures MUST be lazy. Dropping an unpolled future causes no write; dropping a
/// polled write does not imply rollback. OutcomeUnknown MAY have committed.
/// This is NOT request deduplication: callers requiring durable retry identity MUST
/// store it inside their transaction or use a journal. Reading after an unknown
/// outcome cannot attribute a commit if other writers have intervened.
///
/// Schema/authenticity validation remains with the caller; Corrupt denotes storage
/// integrity failure. No durability across machine loss or peer replication is implied.
///
/// ```compile_fail
/// use glade_persistence_api::SnapshotStore;
/// struct Missing;
/// impl SnapshotStore for Missing {}
/// ```
pub trait SnapshotStore: Send + Sync {
    fn load(&self) -> impl Future<Output = Result<Option<Snapshot>, StoreError>> + Send;
    fn compare_exchange(
        &self,
        expected: Option<u64>,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<Snapshot, StoreError>> + Send;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Reusable probes; fixture preconditions are explicit. Not a physical crash harness.
    use crate::{Snapshot, SnapshotStore, StoreError};

    /// PS-001–003. Requires an empty, exclusively controlled namespace.
    pub async fn roundtrip<S: SnapshotStore>(store: &S) {
        assert_eq!(store.load().await, Ok(None));
        let unused = store.compare_exchange(None, vec![99]);
        assert_eq!(store.load().await, Ok(None), "write started before polling");
        drop(unused);
        assert_eq!(store.load().await, Ok(None));
        let first = Snapshot {
            revision: 1,
            bytes: vec![1, 2, 3],
        };
        assert_eq!(
            store.compare_exchange(None, first.bytes.clone()).await,
            Ok(first.clone())
        );
        assert_eq!(store.load().await, Ok(Some(first.clone())));
        assert_eq!(
            store.compare_exchange(None, vec![8]).await,
            Err(StoreError::Conflict)
        );
        assert_eq!(
            store.compare_exchange(Some(0), vec![8]).await,
            Err(StoreError::Conflict)
        );
        assert_eq!(store.load().await, Ok(Some(first)));
        let second = Snapshot {
            revision: 2,
            bytes: vec![],
        };
        assert_eq!(
            store.compare_exchange(Some(1), vec![]).await,
            Ok(second.clone())
        );
        assert_eq!(store.load().await, Ok(Some(second.clone())));
        assert_eq!(
            store.compare_exchange(Some(1), vec![8]).await,
            Err(StoreError::Conflict)
        );
        assert_eq!(store.load().await, Ok(Some(second)));
    }

    /// PS-004. Requires storage configured unavailable for both operations.
    /// State preservation after restoring access must additionally be tested by adapters.
    pub async fn known_failure<S: SnapshotStore>(store: &S) {
        assert_eq!(store.load().await, Err(StoreError::Unavailable));
        assert_eq!(
            store.compare_exchange(None, vec![1]).await,
            Err(StoreError::Unavailable)
        );
    }

    /// PS-005. Requires corrupt storage; corruption MUST NOT become empty/success.
    pub async fn corrupt<S: SnapshotStore>(store: &S) {
        assert_eq!(store.load().await, Err(StoreError::Corrupt));
        assert_eq!(
            store.compare_exchange(None, vec![1]).await,
            Err(StoreError::Corrupt)
        );
    }

    /// PS-006. Requires empty storage and injected lost reply AFTER durable commit.
    pub async fn lost_reply<S: SnapshotStore>(store: &S) {
        assert_eq!(
            store.compare_exchange(None, vec![4, 5]).await,
            Err(StoreError::OutcomeUnknown)
        );
        assert_eq!(
            store.load().await,
            Ok(Some(Snapshot {
                revision: 1,
                bytes: vec![4, 5]
            }))
        );
        assert_eq!(
            store.compare_exchange(None, vec![4, 5]).await,
            Err(StoreError::Conflict)
        );
    }

    /// PS-007. Requires empty storage. Callback MUST reopen the same backing namespace.
    pub async fn reopen<S: SnapshotStore, T: SnapshotStore>(
        store: &S,
        reopen: impl FnOnce(&S) -> T + Send,
    ) {
        let expected = Snapshot {
            revision: 1,
            bytes: vec![7, 8],
        };
        assert_eq!(
            store.compare_exchange(None, expected.bytes.clone()).await,
            Ok(expected.clone())
        );
        let restored = reopen(store);
        assert_eq!(restored.load().await, Ok(Some(expected)));
        assert_eq!(
            restored.compare_exchange(None, vec![0]).await,
            Err(StoreError::Conflict)
        );
    }

    /// PS-008. Requires a preloaded maximum revision with bytes [9].
    pub async fn exhausted<S: SnapshotStore>(store: &S) {
        let before = Some(Snapshot {
            revision: u64::MAX,
            bytes: vec![9],
        });
        assert_eq!(store.load().await, Ok(before.clone()));
        assert_eq!(
            store.compare_exchange(Some(u64::MAX), vec![0]).await,
            Err(StoreError::Exhausted)
        );
        assert_eq!(store.load().await, Ok(before));
    }
}
