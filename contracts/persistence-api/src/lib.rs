//! Draft local snapshot persistence, independent of node, codec and async runtime.
//!
//! Bytes are one caller-encoded, versioned snapshot (including any operation heads).
//! This port neither invents a wire format nor confers source-write authority.
//! Replication remains attributed operations, not snapshot/database replication.

use std::future::Future;

#[cfg(feature = "conformance")]
pub mod conformance;

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
