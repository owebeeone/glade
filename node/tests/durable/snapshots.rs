//! The persistence port's conformance suite, PS-001..008, on records.json's
//! store (F2; the owner's answer (a) of 2026-09-27). records.json holds
//! snapshots only, so its store, as `SnapshotStore`, refuses bytes that are
//! not one (`records_file.rs`). The probes' bytes, `[1, 2, 3]` and the rest,
//! reach it through [`OneRecord`], a fixture that carries each as the one
//! record of a snapshot and takes it back out. So each probe runs end to end
//! on the real file, in the one form the node writes: its revision, its
//! compare-and-swap under the lock, and its checked reader. Each test's setup
//! is the precondition its probe names. What they cannot show: a crash or a
//! lost fsync (PS-006's reply is lost in the fixture, after a real commit),
//! or two processes on the lock.

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use glade_node::cbor::{self, Cbor};
use glade_node::records_file::RecordsFile;
use glade_persistence_api::conformance;
use glade_persistence_api::{Snapshot, SnapshotStore, StoreError};

use crate::disk::ScratchDir;

/// The fixture: `store`, with each probe's bytes carried as the one record of
/// a snapshot, `{1: [bytes], 2: []}`, as the node encodes its snapshot. A
/// reply it is told to lose is dropped after the commit (PS-006).
struct OneRecord<S> {
    store: S,
    lose_reply: AtomicBool,
}

impl<S> OneRecord<S> {
    fn new(store: S) -> OneRecord<S> {
        let lose_reply = AtomicBool::new(false);
        OneRecord { store, lose_reply }
    }
}

/// `bytes` as the one record of a snapshot.
fn wrap(bytes: Vec<u8>) -> Vec<u8> {
    let records = Cbor::Array(vec![Cbor::Bytes(bytes)]);
    cbor::encode(&Cbor::Map(vec![(1, records), (2, Cbor::Array(vec![]))]))
}

/// The one record of `snapshot`, at its revision.
fn unwrap(snapshot: Snapshot) -> Snapshot {
    let map = cbor::try_decode(&snapshot.bytes).unwrap();
    let [record] = map.try_get(1).unwrap().try_array().unwrap() else {
        panic!("not one record: {map:?}");
    };
    let bytes = record.try_bytes().unwrap();
    let revision = snapshot.revision;
    Snapshot { revision, bytes }
}

impl<S: SnapshotStore> SnapshotStore for OneRecord<S> {
    async fn load(&self) -> Result<Option<Snapshot>, StoreError> {
        Ok(self.store.load().await?.map(unwrap))
    }

    async fn compare_exchange(
        &self,
        expected: Option<u64>,
        bytes: Vec<u8>,
    ) -> Result<Snapshot, StoreError> {
        let committed = self.store.compare_exchange(expected, wrap(bytes)).await?;
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            return Err(StoreError::OutcomeUnknown);
        }
        Ok(unwrap(committed))
    }
}

/// records.json's store in a fresh directory of its own, through the fixture.
fn fresh(tag: &str) -> (ScratchDir, OneRecord<RecordsFile>) {
    let dir = ScratchDir::new(tag);
    let store = OneRecord::new(RecordsFile::new(dir.path()));
    (dir, store)
}

/// PS-001..003: bytes round-trip; a create conflicts once a snapshot is
/// there, and a replace names the revision it replaces; an empty record is a
/// present snapshot; a swap that is never polled writes nothing. records.json
/// then holds the probe's last snapshot, at revision 2.
#[tokio::test]
async fn ps_001_to_003_round_trip_on_records_json() {
    let (dir, store) = fresh("ps-001");
    conformance::roundtrip(&store).await;
    let held = RecordsFile::new(dir.path()).load().unwrap();
    assert_eq!(held, Some((2, wrap(vec![]))));
}

/// PS-004: storage configured unavailable for both operations, a directory
/// where records.json goes, answers `Unavailable` to a load and to a swap,
/// which writes nothing. With the directory gone and the snapshot that was
/// there put back, it loads as it was.
#[tokio::test]
async fn ps_004_unavailable_storage_on_records_json() {
    let (dir, store) = fresh("ps-004");
    let kept = store.compare_exchange(None, vec![6]).await.unwrap();
    let (records, aside) = (dir.path().join("records.json"), dir.path().join("aside"));
    fs::rename(&records, &aside).unwrap();
    fs::create_dir(&records).unwrap();
    conformance::known_failure(&store).await;
    let tmp = dir.path().join("records.json.tmp");
    assert!(!tmp.exists(), "written nothing");
    fs::remove_dir(&records).unwrap();
    fs::rename(&aside, &records).unwrap();
    assert_eq!(store.load().await, Ok(Some(kept)));
}

/// PS-005: a records.json cut in half is `Corrupt` to a load and to a swap,
/// never absence or an empty snapshot, and the swap writes nothing.
#[tokio::test]
async fn ps_005_corrupt_storage_on_records_json() {
    let (dir, store) = fresh("ps-005");
    store.compare_exchange(None, vec![6; 40]).await.unwrap();
    let records = dir.path().join("records.json");
    let whole = fs::read(&records).unwrap();
    fs::write(&records, &whole[..whole.len() / 2]).unwrap();
    let torn = fs::read(&records).unwrap();
    conformance::corrupt(&store).await;
    assert_eq!(fs::read(&records).unwrap(), torn, "written nothing");
}

/// PS-006: a reply lost after a durable commit. The swap commits on the real
/// store and the fixture drops its answer; the probe then reads the commit,
/// and a retry of the same create conflicts. The store answers
/// `OutcomeUnknown` itself only when the directory's sync fails after the
/// rename, which no test here produces.
#[tokio::test]
async fn ps_006_lost_reply_on_records_json() {
    let (_dir, store) = fresh("ps-006");
    store.lose_reply.store(true, Ordering::SeqCst);
    conformance::lost_reply(&store).await;
}

/// PS-007: a commit reads back through a new handle on the same directory. It
/// reads the file this process just wrote, from the page cache: no evidence
/// of a crash surviving.
#[tokio::test]
async fn ps_007_reopen_on_records_json() {
    let (dir, store) = fresh("ps-007");
    let reopen = |_: &OneRecord<RecordsFile>| OneRecord::new(RecordsFile::new(dir.path()));
    conformance::reopen(&store, reopen).await;
}

/// PS-008: a records.json at revision `u64::MAX`, written by hand with the
/// probe's `[9]` as its snapshot's one record, loads; a swap from it is
/// `Exhausted`, and writes nothing.
#[tokio::test]
async fn ps_008_exhausted_on_records_json() {
    let (dir, store) = fresh("ps-008");
    // `{1: [h'09'], 2: [], 3: u64::MAX}`: the snapshot, then key 3 as the
    // 9-byte unsigned integer, in a map of three.
    let mut file = wrap(vec![9]);
    file[0] = 0xa3;
    file.extend([0x03, 0x1b]);
    file.extend(u64::MAX.to_be_bytes());
    let records = dir.path().join("records.json");
    fs::write(&records, &file).unwrap();
    conformance::exhausted(&store).await;
    assert_eq!(fs::read(&records).unwrap(), file, "written nothing");
}
