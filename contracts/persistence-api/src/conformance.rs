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
