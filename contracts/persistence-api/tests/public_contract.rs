use glade_persistence_api::{Snapshot, SnapshotStore, StoreError, conformance};
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

// Volatile deterministic model. Reopening copies memory; this is not crash proof.
#[derive(Default)]
struct Model {
    value: Mutex<Option<Snapshot>>,
    mode: u8,
}

impl SnapshotStore for Model {
    async fn load(&self) -> Result<Option<Snapshot>, StoreError> {
        match self.mode {
            1 => Err(StoreError::Unavailable),
            2 => Err(StoreError::Corrupt),
            _ => Ok(self.value.lock().unwrap().clone()),
        }
    }
    async fn compare_exchange(
        &self,
        expected: Option<u64>,
        bytes: Vec<u8>,
    ) -> Result<Snapshot, StoreError> {
        match self.mode {
            1 => return Err(StoreError::Unavailable),
            2 => return Err(StoreError::Corrupt),
            _ => {}
        }
        let mut state = self.value.lock().unwrap();
        if self.mode != 3 && state.as_ref().map(|s| s.revision) != expected {
            return Err(StoreError::Conflict);
        }
        let revision = match state.as_ref() {
            None => 1,
            Some(s) => s.revision.checked_add(1).ok_or(StoreError::Exhausted)?,
        };
        let snapshot = Snapshot { revision, bytes };
        *state = Some(snapshot.clone());
        if self.mode == 4 {
            Err(StoreError::OutcomeUnknown)
        } else {
            Ok(snapshot)
        }
    }
}

fn run(future: impl Future<Output = ()> + Send) {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        future.as_mut().poll(&mut context),
        Poll::Ready(())
    ));
}

#[test]
fn ps_001_003_roundtrip_cas_and_lazy_construction() {
    run(conformance::roundtrip(&Model::default()));
}

#[test]
fn ps_004_unavailable_is_not_absence_or_success() {
    run(conformance::known_failure(&Model {
        mode: 1,
        ..Model::default()
    }));
}

#[test]
fn ps_005_corruption_is_not_empty() {
    run(conformance::corrupt(&Model {
        mode: 2,
        ..Model::default()
    }));
}

#[test]
fn ps_006_lost_reply_reconciles_whole_snapshot() {
    run(conformance::lost_reply(&Model {
        mode: 4,
        ..Model::default()
    }));
}

#[test]
fn ps_007_reopen_retains_revision_and_bytes() {
    run(conformance::reopen(&Model::default(), |s| Model {
        value: Mutex::new(s.value.lock().unwrap().clone()),
        mode: 0,
    }));
}

#[test]
fn ps_008_revision_exhaustion_does_not_mutate() {
    run(conformance::exhausted(&Model {
        value: Mutex::new(Some(Snapshot {
            revision: u64::MAX,
            bytes: vec![9],
        })),
        mode: 0,
    }));
}

#[test]
#[should_panic]
fn rejects_ignored_cas() {
    run(conformance::roundtrip(&Model {
        mode: 3,
        ..Model::default()
    }));
}

#[test]
#[should_panic]
fn rejects_forgotten_snapshot() {
    run(conformance::reopen(&Model::default(), |_| Model::default()));
}
