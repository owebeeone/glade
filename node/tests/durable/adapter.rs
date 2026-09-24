//! The tests the contracts README requires of a real persistence adapter
//! (`glade/contracts/README.md`, "Test coverage and limits"), on the node's
//! real engine, records.json through `BlobStore`. Each says what it proves and
//! what it cannot. It is not `SnapshotStore`: records.json has no revision and
//! the node does not depend on the persistence port, so PS-001..008 do not run
//! here (`glade/dev-docs/GladeNodeAssembly.md`, "Durable store and restart").
//! Cancellation of a pending future has no test here: `StoreApi` is
//! synchronous, and `claims.rs` tests the one future that waits around a save.
//! Capacity has none either: neither store enforces one, and a full disk
//! cannot be produced here.

use std::fs;

use glade_node::cbor;
use glade_node::registry::{BlobStore, StoreApi};
use glade_node::sysdata::SystemSnapshot;

use crate::disk::DiskStore;

/// A snapshot the engine carries as bytes: records.json does not decode the
/// records inside it, so any bytes do.
fn snapshot(fill: u8, len: usize) -> SystemSnapshot {
    SystemSnapshot {
        records: vec![vec![fill; len]],
        heads: vec![],
    }
}

/// Recovery after a known failure: a save the file system refuses leaves
/// records.json with its bytes, and the next save, once the refusal is gone,
/// lands whole. The refusal is a directory where the temp file goes; ENOSPC
/// or EIO part-way through the write or at the sync fail at other points,
/// and are not produced here.
#[test]
fn a_refused_save_leaves_records_json_and_the_next_save_lands() {
    let disk = DiskStore::fresh("known-failure");
    let (first, second) = (snapshot(1, 8), snapshot(2, 64));
    let mut store = BlobStore::new(disk.dir());
    store.save(&first).unwrap();
    let records = disk.dir().join("records.json");
    let before = fs::read(&records).unwrap();

    disk.refuse_saves(true);
    assert!(store.save(&second).is_err());
    assert_eq!(
        fs::read(&records).unwrap(),
        before,
        "records.json keeps its bytes"
    );
    assert_eq!(store.load().unwrap(), first);

    disk.refuse_saves(false);
    store.save(&second).unwrap();
    assert_eq!(BlobStore::new(disk.dir()).load().unwrap(), second);
    assert!(!disk.dir().join("records.json.tmp").exists());
}

/// An interrupted write: a save killed before its rename leaves a partial
/// temp file beside the previous records.json. Load reads the previous
/// snapshot whole and never the temp file, and the next save replaces both.
/// The test writes the partial file itself: it cannot show that a real crash
/// leaves only this state (rename ordering, power loss).
#[test]
fn a_save_interrupted_before_its_rename_leaves_the_previous_snapshot_whole() {
    let disk = DiskStore::fresh("interrupted");
    let (first, second) = (snapshot(1, 8), snapshot(2, 64));
    let mut store = BlobStore::new(disk.dir());
    store.save(&first).unwrap();
    let bytes = cbor::encode(&second.to_cbor());
    let tmp = disk.dir().join("records.json.tmp");
    fs::write(&tmp, &bytes[..bytes.len() / 2]).unwrap();

    assert_eq!(BlobStore::new(disk.dir()).load().unwrap(), first);
    store.save(&second).unwrap();
    assert_eq!(BlobStore::new(disk.dir()).load().unwrap(), second);
    assert!(!tmp.exists(), "the leftover was replaced, then renamed");
}

/// Concurrent handles: two handles on one directory, in turn. The second's
/// save replaces the first's, which it never read, and nothing reports a
/// conflict: there is no revision to compare, so one writer per instance
/// rests on the instance lock (`sysdir`'s `instance_lock_is_single_writer`
/// proves the lock). Two saves in flight at once can also tear the file, since
/// the handles share one temp name; that depends on timing, and no test here
/// shows it.
#[test]
fn two_handles_on_one_directory_replace_each_other_without_a_conflict() {
    let disk = DiskStore::fresh("two-handles");
    let (mut one, mut two) = (BlobStore::new(disk.dir()), BlobStore::new(disk.dir()));
    one.save(&snapshot(1, 8)).unwrap();
    two.save(&snapshot(2, 8)).unwrap();
    assert_eq!(
        one.load().unwrap(),
        snapshot(2, 8),
        "the first save is gone"
    );
}
