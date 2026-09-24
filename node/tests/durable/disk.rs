//! The engine this binary gives the journeys: records.json through
//! `BlobStore`, the node's real `StoreApi` engine, in a fresh temp directory
//! per test node (under `TMPDIR`), removed once the last handle on it drops.
//! A read-back opens a second `BlobStore` on the directory, so it reads the
//! file, not the record host's memory: but the file this process just wrote,
//! from the page cache, so it is no evidence of crash safety or fsync.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use glade_node::cbor;
use glade_node::registry::{BlobStore, StoreApi};
use glade_node::sysdata::SystemSnapshot;
use glade_wire::generated::Op;

/// A directory of its own under the temp dir, removed on drop.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// A fresh, empty directory whose name says which test node owns it.
    pub fn new(tag: &str) -> ScratchDir {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let name = format!("glade-durable-{}-{n}-{tag}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        ScratchDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A test node's engine on disk: records.json in its own scratch directory.
#[derive(Clone)]
pub struct DiskStore(Arc<ScratchDir>);

impl DiskStore {
    /// An engine over a fresh directory, holding nothing yet.
    pub fn fresh(tag: &str) -> DiskStore {
        DiskStore(Arc::new(ScratchDir::new(tag)))
    }

    pub fn dir(&self) -> &Path {
        self.0.path()
    }

    /// What records.json holds, read through a handle of its own.
    pub fn snapshot(&self) -> SystemSnapshot {
        let store = BlobStore::new(self.dir());
        store.load().expect("records.json reads back")
    }

    /// The ops records.json holds, in the order the fold holds them.
    pub fn ops(&self) -> Vec<Op> {
        let records = self.snapshot().records;
        records
            .iter()
            .map(|bytes| Op::from_cbor(&cbor::decode(bytes)))
            .collect()
    }

    /// A `BlobStore` on this directory, for a record host to persist through.
    pub fn boxed(&self) -> Box<dyn StoreApi + Send> {
        Box::new(BlobStore::new(self.dir()))
    }

    /// Whether every save fails from now on: a directory stands where the
    /// save's temp file goes, so its first write fails and records.json is
    /// left as it is. A refusal the file system makes, not a full disk or a
    /// failing device.
    pub fn refuse_saves(&self, refuse: bool) {
        let blocker = self.dir().join("records.json.tmp");
        if refuse {
            fs::create_dir(&blocker).expect("the blocker is made");
        } else {
            fs::remove_dir(&blocker).expect("the blocker is removed");
        }
    }
}
