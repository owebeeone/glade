//! records.json and its revision (the owner's ruling of 2026-09-24: the
//! persistence suite runs on records.json, with an optional revision field in
//! `SystemSnapshot`). The file is the canonical CBOR of the node's snapshot,
//! a map, with the store's revision at key 3: 1 at the first save, one more at
//! each. A file without key 3, as every build before wrote it, reads as
//! revision 1, and an older build reads keys 1 and 2 and drops the revision
//! when it saves. The store walks the file without decoding it, so a damaged
//! one answers [`FileError::Corrupt`], never a panic, and a save compares the
//! revision it expects with the one held, under a lock, before it writes. The
//! design is `glade/dev-docs/GladeNodeAssembly.md`, "The persistence suite on
//! records.json". The store is also the persistence port, `SnapshotStore`,
//! for snapshots only (F2, the owner's answer (a) of 2026-09-27), and the
//! node's own saves go through [`RecordsFile::compare_exchange`] as before.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use glade_persistence_api::{Snapshot, SnapshotStore, StoreError};
use glade_wire::cbor::{self, Cbor};

use crate::envelope::{self, head};
use crate::registry::entry_sync;
use crate::sysdata::SystemSnapshot;

/// The key records.json keeps its revision at (`SystemSnapshot.revision`).
const REVISION: u64 = 3;

// The CBOR major types the store reads, and null.
const UINT: u8 = 0;
const MAP: u8 = 5;
const NULL: u8 = 0xf6;

/// What `Corrupt` says of an item cut short, or whose head glade-wire never
/// writes (a reserved one, an indefinite length).
const TORN: &str = "a torn or unreadable item";

/// What records.json's store answers instead of a snapshot or a commit: the
/// persistence contract's outcomes, each with what went wrong.
#[derive(Debug)]
pub enum FileError {
    /// A read, the lock, or a write before the rename failed: nothing changed.
    Unavailable(io::Error),
    /// records.json is not a CBOR map this build can read, or its key 3 is
    /// not a revision. A compare-and-swap that finds it writes nothing.
    Corrupt(&'static str),
    /// records.json holds another revision than the one expected (`None`:
    /// no records.json).
    Conflict {
        held: Option<u64>,
        expected: Option<u64>,
    },
    /// The revision is `u64::MAX`, so there is no next one. Nothing was
    /// written.
    Exhausted,
    /// The rename happened and the directory's sync failed: the new snapshot
    /// is in place, and may not survive a crash.
    OutcomeUnknown(io::Error),
    /// The snapshot handed to the store is not a CBOR map, its keys unsigned
    /// integers in order and none of them 3. Nothing was written.
    NotASnapshot,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileError::Unavailable(e) => write!(f, "unavailable: {e}"),
            FileError::Corrupt(why) => write!(f, "corrupt: {why}"),
            FileError::Conflict { held, expected } => {
                let (held, expected) = (named(*held), named(*expected));
                write!(f, "it holds {held}, where {expected} was expected")
            }
            FileError::Exhausted => f.write_str("its revision is at its maximum"),
            FileError::OutcomeUnknown(e) => write!(f, "written, and may not survive a crash: {e}"),
            FileError::NotASnapshot => {
                f.write_str("the snapshot handed to it is not a map without key 3")
            }
        }
    }
}

/// A revision as a message names it.
fn named(revision: Option<u64>) -> String {
    revision.map_or_else(
        || "no revision".into(),
        |revision| format!("revision {revision}"),
    )
}

/// records.json in one directory, with its revision. A handle holds nothing
/// but the path: every call reads the file, so any number of handles, in one
/// process or several, see one store.
pub struct RecordsFile {
    path: PathBuf,
}

impl RecordsFile {
    /// The store of `records.json` under `dir`.
    pub fn new(dir: impl AsRef<Path>) -> RecordsFile {
        let path = dir.as_ref().join("records.json");
        RecordsFile { path }
    }

    /// The file this store keeps.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The revision records.json holds and its snapshot, the file's map
    /// without key 3, byte for byte; `None` when there is no records.json. A
    /// file without key 3 is revision 1, and its snapshot the whole file.
    pub fn load(&self) -> Result<Option<(u64, Vec<u8>)>, FileError> {
        let Some(file) = self.read()? else {
            return Ok(None);
        };
        let map = Map::walk(&file).map_err(FileError::Corrupt)?;
        let (revision, at) = map.revision(&file).map_err(FileError::Corrupt)?;
        let Some(at) = at else {
            return Ok(Some((revision, file)));
        };
        let mut snapshot = Vec::with_capacity(file.len());
        put_head(&mut snapshot, MAP, map.entries.len() as u64 - 1);
        snapshot.extend_from_slice(&file[map.body..at.start]);
        snapshot.extend_from_slice(&file[at.end..]);
        Ok(Some((revision, snapshot)))
    }

    /// The revision records.json holds, `None` when there is none, read as
    /// [`RecordsFile::load`] reads it.
    pub fn revision(&self) -> Result<Option<u64>, FileError> {
        let Some(file) = self.read()? else {
            return Ok(None);
        };
        let map = Map::walk(&file).map_err(FileError::Corrupt)?;
        let (revision, _) = map.revision(&file).map_err(FileError::Corrupt)?;
        Ok(Some(revision))
    }

    /// Replace records.json with `snapshot` at the next revision, if the
    /// revision it holds is `expected` (`None`: there is none); the new
    /// revision is the answer. Under the store's lock, so two handles take
    /// turns: the one that comes second reads the first one's revision. The
    /// write is 4.4's: a temp file, synced, renamed over records.json, and
    /// the directory synced. Every answer but `OutcomeUnknown` leaves
    /// records.json as it was, or wholly replaced.
    pub fn compare_exchange(
        &self,
        expected: Option<u64>,
        snapshot: &[u8],
    ) -> Result<u64, FileError> {
        let dir = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        fs::create_dir_all(&dir).map_err(FileError::Unavailable)?;
        let _turn = self.lock()?;
        let held = self.revision()?;
        if held != expected {
            return Err(FileError::Conflict { held, expected });
        }
        let revision = match held {
            None => 1,
            Some(held) => held.checked_add(1).ok_or(FileError::Exhausted)?,
        };
        let file = with_revision(snapshot, revision).ok_or(FileError::NotASnapshot)?;
        let tmp = self.path.with_extension("json.tmp");
        // Durable before visible: the bytes reach the device before the
        // rename makes them records.json, and the rename is then synced.
        let written = fs::File::create(&tmp).and_then(|mut out| {
            out.write_all(&file)?;
            out.sync_all()
        });
        written.map_err(FileError::Unavailable)?;
        fs::rename(&tmp, &self.path).map_err(FileError::Unavailable)?;
        entry_sync::sync(&dir).map_err(FileError::OutcomeUnknown)?;
        Ok(revision)
    }

    /// records.json's bytes, `None` when it is absent.
    fn read(&self) -> Result<Option<Vec<u8>>, FileError> {
        match fs::read(&self.path) {
            Ok(file) => Ok(Some(file)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(FileError::Unavailable(e)),
        }
    }

    /// The store's lock, `records.json.lock` beside records.json, held until
    /// the answer is dropped: an exclusive OS lock, which the kernel releases
    /// with its process. The file stays, empty.
    fn lock(&self) -> Result<fs::File, FileError> {
        let path = self.path.with_extension("json.lock");
        let mut options = fs::OpenOptions::new();
        let file = options.write(true).create(true).truncate(false).open(path);
        let file = file.map_err(FileError::Unavailable)?;
        file.lock().map_err(FileError::Unavailable)?;
        Ok(file)
    }
}

/// records.json's store as the persistence port (F2, the owner's answer (a)
/// of 2026-09-27): records.json holds snapshots only. The bytes committed must
/// be a snapshot as the node writes one, a canonical CBOR map without key 3;
/// any other bytes are refused before anything is written, as `Capacity`, the
/// nearest outcome the contract has. The revision is the file's. Each future
/// does its work when first polled and finishes in that poll: an unpolled one
/// writes nothing, and a polled one leaves nothing pending to cancel.
/// PS-001..008 run on it in `tests/durable`, through a fixture that carries
/// each probe's bytes as the one record of a snapshot.
impl SnapshotStore for RecordsFile {
    async fn load(&self) -> Result<Option<Snapshot>, StoreError> {
        let loaded = RecordsFile::load(self)?;
        Ok(loaded.map(|(revision, bytes)| Snapshot { revision, bytes }))
    }

    async fn compare_exchange(
        &self,
        expected: Option<u64>,
        bytes: Vec<u8>,
    ) -> Result<Snapshot, StoreError> {
        let revision = RecordsFile::compare_exchange(self, expected, &bytes)?;
        Ok(Snapshot { revision, bytes })
    }
}

/// The port's outcome for each of the store's (F2): bytes that are not a
/// snapshot are `Capacity`.
impl From<FileError> for StoreError {
    fn from(e: FileError) -> StoreError {
        match e {
            FileError::Unavailable(_) => StoreError::Unavailable,
            FileError::Corrupt(_) => StoreError::Corrupt,
            FileError::Conflict { .. } => StoreError::Conflict,
            FileError::Exhausted => StoreError::Exhausted,
            FileError::OutcomeUnknown(_) => StoreError::OutcomeUnknown,
            FileError::NotASnapshot => StoreError::Capacity,
        }
    }
}

/// The node's snapshot as records.json's store takes it: keys 1 and 2, the
/// records and the heads, canonically encoded, and no key 3, which the store
/// adds.
pub(crate) fn encode(snap: &SystemSnapshot) -> Vec<u8> {
    let mut map = snap.to_cbor();
    if let Cbor::Map(entries) = &mut map {
        entries.retain(|(key, _)| *key != REVISION as i64);
    }
    cbor::encode(&map)
}

/// The node's snapshot in the bytes records.json's store loads, which hold no
/// revision: keys 1 and 2, each one list of byte strings, read without
/// panicking, so a damaged file says what is wrong where the wire codec did.
/// Other keys are left, as the wire codec left them: a newer build's, which
/// the next save drops, as an older build drops the revision.
pub(crate) fn decode(snapshot: &[u8]) -> Result<SystemSnapshot, &'static str> {
    let map = Map::walk(snapshot)?;
    let list = |key: u64| {
        let mut held = map.entries.iter().filter(|entry| entry.key == key);
        match (held.next(), held.next()) {
            (Some(entry), None) => byte_strings(&snapshot[entry.value.clone()]),
            _ => None,
        }
    };
    let records = list(1).ok_or("key 1, the records, is not one list of byte strings")?;
    let heads = list(2).ok_or("key 2, the heads, is not one list of byte strings")?;
    Ok(SystemSnapshot {
        records,
        heads,
        revision: None,
    })
}

/// The byte strings `value` holds, one CBOR array of byte strings exactly,
/// read by 4.1b's checked decoder.
fn byte_strings(value: &[u8]) -> Option<Vec<Vec<u8>>> {
    let Some(Cbor::Array(items)) = envelope::parse(value) else {
        return None;
    };
    let bytes = |item| match item {
        Cbor::Bytes(bytes) => Some(bytes),
        _ => None,
    };
    items.into_iter().map(bytes).collect()
}

/// A CBOR map walked, not decoded: where its entries begin, and each entry's
/// key and extent. Each key is an unsigned integer; each value is one
/// well-formed item, skipped by its length.
pub(crate) struct Map {
    /// Where the first entry begins, just past the map's head.
    body: usize,
    pub(crate) entries: Vec<Entry>,
}

/// One entry of a walked map: its key, and where it and its value lie.
pub(crate) struct Entry {
    pub(crate) key: u64,
    entry: Range<usize>,
    pub(crate) value: Range<usize>,
}

impl Map {
    /// `bytes` walked as one CBOR map, with nothing left over.
    pub(crate) fn walk(bytes: &[u8]) -> Result<Map, &'static str> {
        let mut at = 0;
        let (major, count) = head(bytes, &mut at).ok_or(TORN)?;
        if major != MAP {
            return Err("not a map");
        }
        let body = at;
        // An entry takes two bytes at least, so a count past the bytes left
        // is refused before anything is allocated for it.
        let count = usize::try_from(count)
            .ok()
            .filter(|count| *count <= (bytes.len() - at) / 2);
        let count = count.ok_or(TORN)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let start = at;
            let (major, key) = head(bytes, &mut at).ok_or(TORN)?;
            if major != UINT {
                return Err("a key that is not an unsigned integer");
            }
            let value = at;
            skip(bytes, &mut at)?;
            let (entry, value) = (start..at, value..at);
            entries.push(Entry { key, entry, value });
        }
        if at != bytes.len() {
            return Err("bytes left over");
        }
        Ok(Map { body, entries })
    }

    /// The revision `file`, walked as this map, holds, and where its key 3
    /// lies: none there, or a null there, is revision 1.
    fn revision(&self, file: &[u8]) -> Result<(u64, Option<Range<usize>>), &'static str> {
        let mut held = self.entries.iter().filter(|entry| entry.key == REVISION);
        let Some(entry) = held.next() else {
            return Ok((1, None));
        };
        if held.next().is_some() {
            return Err("key 3 twice");
        }
        let value = &file[entry.value.clone()];
        if value == [NULL] {
            return Ok((1, Some(entry.entry.clone())));
        }
        match head(value, &mut 0) {
            Some((UINT, revision)) if revision > 0 => Ok((revision, Some(entry.entry.clone()))),
            _ => Err("key 3 is not a revision"),
        }
    }
}

/// Past one well-formed CBOR item at `*at` without decoding it: integers,
/// byte and text strings, arrays, maps and simple values, as glade-wire
/// writes them, nested to any depth. A torn item, a reserved head or an
/// indefinite length (which `head` refuses), and a tag, are refused.
fn skip(bytes: &[u8], at: &mut usize) -> Result<(), &'static str> {
    // The items still to pass: an array adds its items, a map two per entry.
    let mut items: u64 = 1;
    while items > 0 {
        items -= 1;
        let (major, n) = head(bytes, at).ok_or(TORN)?;
        match major {
            0 | 1 | 7 => {}
            2 | 3 => {
                let end = usize::try_from(n).ok().and_then(|n| at.checked_add(n));
                *at = end.filter(|end| *end <= bytes.len()).ok_or(TORN)?;
            }
            4 => items = items.checked_add(n).ok_or(TORN)?,
            5 => {
                items = n
                    .checked_mul(2)
                    .and_then(|n| items.checked_add(n))
                    .ok_or(TORN)?
            }
            _ => return Err("a tag"),
        }
    }
    Ok(())
}

/// `snapshot`, a canonical CBOR map without key 3, with `revision` added at
/// key 3, where canonical order puts it: `None` when `snapshot` is not such a
/// map, its head in shortest form and its keys in increasing order.
fn with_revision(snapshot: &[u8], revision: u64) -> Option<Vec<u8>> {
    let map = Map::walk(snapshot).ok()?;
    let count = map.entries.len() as u64;
    let mut shortest = Vec::new();
    put_head(&mut shortest, MAP, count);
    let ordered = map.entries.windows(2).all(|pair| pair[0].key < pair[1].key);
    let free = map.entries.iter().all(|entry| entry.key != REVISION);
    if shortest != snapshot[..map.body] || !ordered || !free {
        return None;
    }
    let mut file = Vec::with_capacity(snapshot.len() + 10);
    put_head(&mut file, MAP, count + 1);
    let after = map.entries.iter().find(|entry| entry.key > REVISION);
    let at = after.map_or(snapshot.len(), |entry| entry.entry.start);
    file.extend_from_slice(&snapshot[map.body..at]);
    put_head(&mut file, UINT, REVISION);
    put_head(&mut file, UINT, revision);
    file.extend_from_slice(&snapshot[at..]);
    Some(file)
}

/// A CBOR head in shortest form: `major`, then `n` in the fewest bytes that
/// hold it, as glade-wire writes one.
fn put_head(out: &mut Vec<u8>, major: u8, n: u64) {
    let major = major << 5;
    if n < 24 {
        out.push(major | n as u8);
    } else if n <= 0xff {
        out.extend([major | 24, n as u8]);
    } else if n <= 0xffff {
        out.push(major | 25);
        out.extend((n as u16).to_be_bytes());
    } else if n <= 0xffff_ffff {
        out.push(major | 26);
        out.extend((n as u32).to_be_bytes());
    } else {
        out.push(major | 27);
        out.extend(n.to_be_bytes());
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{BlobStore, StoreApi};
    use std::sync::mpsc;
    use std::time::Duration;

    /// An empty directory of this test's own under the temp dir.
    fn fresh(name: &str) -> PathBuf {
        let name = format!("glade-records-file-{}-{name}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A snapshot the store carries as bytes: it reads neither list inside.
    fn snapshot(fill: u8) -> SystemSnapshot {
        let (records, heads) = (vec![vec![fill; 3], vec![fill; 70]], vec![vec![fill]]);
        SystemSnapshot {
            records,
            heads,
            revision: None,
        }
    }

    /// `snap` as every build before this step wrote records.json: keys 1 and
    /// 2 alone, as the generated codec for those two keys encoded them.
    fn before_the_step(snap: &SystemSnapshot) -> Vec<u8> {
        let list =
            |items: &[Vec<u8>]| Cbor::Array(items.iter().map(|x| Cbor::Bytes(x.clone())).collect());
        cbor::encode(&Cbor::Map(vec![
            (1, list(&snap.records)),
            (2, list(&snap.heads)),
        ]))
    }

    /// The ruling's acceptance: a records.json from before this step, and one
    /// whose key 3 is null, load byte for byte as revision 1, and the node's
    /// engine reads the same snapshot from it; its next save writes revision
    /// 2, with keys 1 and 2 as they were.
    #[test]
    fn a_file_from_before_the_revision_loads_byte_for_byte_as_revision_1() {
        let dir = fresh("before");
        let (snap, file) = (snapshot(1), RecordsFile::new(&dir));
        let old = before_the_step(&snap);
        fs::write(file.path(), &old).unwrap();
        assert_eq!(file.load().unwrap(), Some((1, old.clone())));
        let null = cbor::encode(&snap.to_cbor());
        assert_eq!(
            null.last(),
            Some(&NULL),
            "the generated codec's no revision"
        );
        fs::write(file.path(), &null).unwrap();
        assert_eq!(file.load().unwrap(), Some((1, old.clone())));

        fs::write(file.path(), &old).unwrap();
        let mut store = BlobStore::new(&dir);
        assert_eq!(store.load().unwrap(), snap, "the node's engine reads it");
        assert_eq!(fs::read(file.path()).unwrap(), old, "read, not written");
        store.save(&snap).unwrap();
        assert_eq!(file.load().unwrap(), Some((2, old)));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Each compare-and-swap commits the next revision with the bytes, in
    /// one file: the snapshot's map with `3: revision` where canonical order
    /// puts it. A second handle reads what the first wrote.
    #[test]
    fn each_swap_commits_the_next_revision_with_its_bytes() {
        let dir = fresh("revisions");
        let file = RecordsFile::new(&dir);
        assert_eq!(file.load().unwrap(), None, "established absence");
        let first = encode(&snapshot(1));
        assert_eq!(file.compare_exchange(None, &first).unwrap(), 1);
        let mut on_disk = first.clone();
        on_disk[0] += 1; // a map of three entries, not two
        on_disk.extend([REVISION as u8, 1]);
        assert_eq!(fs::read(file.path()).unwrap(), on_disk);
        assert_eq!(file.load().unwrap(), Some((1, first)));
        let second = encode(&snapshot(2));
        assert_eq!(file.compare_exchange(Some(1), &second).unwrap(), 2);
        let again = RecordsFile::new(&dir).load().unwrap();
        assert_eq!(again, Some((2, second)), "a second handle reads it");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A swap that expects another revision than the one held, or none when
    /// one is held, conflicts, names both, and writes nothing.
    #[test]
    fn a_stale_revision_conflicts_and_writes_nothing() {
        let dir = fresh("stale");
        let file = RecordsFile::new(&dir);
        file.compare_exchange(None, &encode(&snapshot(1))).unwrap();
        let held = fs::read(file.path()).unwrap();
        for expected in [None, Some(0), Some(2)] {
            let answer = file.compare_exchange(expected, &encode(&snapshot(2)));
            let conflict = matches!(answer, Err(FileError::Conflict { held: Some(1), expected: e }) if e == expected);
            assert!(conflict, "{expected:?}: {answer:?}");
        }
        assert_eq!(fs::read(file.path()).unwrap(), held, "written nothing");
        let answer = file.compare_exchange(None, &[]).unwrap_err().to_string();
        assert_eq!(
            answer,
            "it holds revision 1, where no revision was expected"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A records.json that is damaged, or not a snapshot's map, is `Corrupt`,
    /// saying what is wrong, for a load and for a swap, which writes nothing:
    /// never a panic, and never absence. The node's engine answers it as
    /// `InvalidData`, naming the file, as it does a snapshot whose key 1 is
    /// not a list of byte strings.
    #[test]
    fn a_damaged_records_json_is_corrupt_and_never_a_panic() {
        let dir = fresh("damaged");
        let file = RecordsFile::new(&dir);
        let good = with_revision(&encode(&snapshot(1)), 7).unwrap();
        let cases: [(&str, Vec<u8>, &str); 12] = [
            ("empty", vec![], TORN),
            ("torn", good[..good.len() / 2].to_vec(), TORN),
            (
                "left over",
                [good.clone(), vec![0]].concat(),
                "bytes left over",
            ),
            ("an array", vec![0x80], "not a map"),
            (
                "a text key",
                vec![0xa1, 0x61, b'k', 0],
                "a key that is not an unsigned integer",
            ),
            ("indefinite", vec![0xa1, 1, 0x9f, 0xff], TORN),
            ("a tag", vec![0xa1, 1, 0xc0, 0], "a tag"),
            (
                "a count past the bytes",
                vec![0xba, 0xff, 0xff, 0xff, 0xff],
                TORN,
            ),
            ("revision 0", vec![0xa1, 3, 0], "key 3 is not a revision"),
            (
                "a text revision",
                vec![0xa1, 3, 0x61, b'1'],
                "key 3 is not a revision",
            ),
            (
                "a negative revision",
                vec![0xa1, 3, 0x20],
                "key 3 is not a revision",
            ),
            ("key 3 twice", vec![0xa2, 3, 1, 3, 2], "key 3 twice"),
        ];
        for (case, bytes, why) in cases {
            fs::write(file.path(), &bytes).unwrap();
            let load = file.load();
            assert!(
                matches!(load, Err(FileError::Corrupt(w)) if w == why),
                "{case}: {load:?}"
            );
            let swap = file.compare_exchange(Some(7), &encode(&snapshot(2)));
            assert!(
                matches!(swap, Err(FileError::Corrupt(_))),
                "{case}: {swap:?}"
            );
            assert_eq!(
                fs::read(file.path()).unwrap(),
                bytes,
                "{case}: written nothing"
            );
        }
        fs::write(file.path(), &good[..good.len() / 2]).unwrap();
        let err = BlobStore::new(&dir).load().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let named = format!(
            "{} cannot be read as a snapshot ({TORN})",
            file.path().display()
        );
        assert!(err.to_string().starts_with(&named), "{err}");
        let not_a_list = Cbor::Map(vec![(1, Cbor::Int(1)), (2, Cbor::Array(vec![]))]);
        fs::write(file.path(), cbor::encode(&not_a_list)).unwrap();
        let err = BlobStore::new(&dir).load().unwrap_err().to_string();
        assert!(
            err.contains("(key 1, the records, is not one list of byte strings)"),
            "{err}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Two handles take turns: while another holds records.json's lock, here
    /// the test itself, a swap waits for it, and lands once it is released.
    #[test]
    fn a_swap_waits_for_the_lock_another_handle_holds() {
        let dir = fresh("turns");
        let lock = dir.join("records.json.lock");
        let held = fs::File::create(&lock).unwrap();
        held.lock().unwrap();
        let (done, finished) = mpsc::channel();
        let swap = std::thread::spawn({
            let dir = dir.clone();
            move || {
                let answer = RecordsFile::new(&dir).compare_exchange(None, &encode(&snapshot(1)));
                done.send(()).unwrap();
                answer.unwrap()
            }
        });
        let waited = finished.recv_timeout(Duration::from_millis(200)).is_err();
        assert!(waited, "the swap waits for the lock");
        assert!(
            !dir.join("records.json").exists(),
            "and has written nothing"
        );
        held.unlock().unwrap();
        assert_eq!(swap.join().unwrap(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The contract's range: a records.json at revision `u64::MAX`, the CBOR
    /// unsigned integer it is, loads; a swap from it is `Exhausted` and writes
    /// nothing.
    #[test]
    fn the_last_revision_is_exhausted_and_writes_nothing() {
        let dir = fresh("exhausted");
        let file = RecordsFile::new(&dir);
        let snap = encode(&snapshot(9));
        let last = with_revision(&snap, u64::MAX).unwrap();
        assert!(last.ends_with(&[0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]));
        fs::write(file.path(), &last).unwrap();
        assert_eq!(file.load().unwrap(), Some((u64::MAX, snap)));
        let answer = file.compare_exchange(Some(u64::MAX), &encode(&snapshot(0)));
        assert!(matches!(answer, Err(FileError::Exhausted)), "{answer:?}");
        assert_eq!(fs::read(file.path()).unwrap(), last, "written nothing");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Option (ii)'s compatibility: the build before this step read
    /// records.json with the codec generated for keys 1 and 2, exactly as
    /// below, so it reads a file this build wrote; it saved those two keys
    /// alone, dropping the revision, and this build reads that as revision 1.
    #[test]
    fn an_older_build_reads_the_file_and_drops_the_revision_when_it_saves() {
        let dir = fresh("older");
        let (snap, file) = (snapshot(3), RecordsFile::new(&dir));
        file.compare_exchange(None, &encode(&snap)).unwrap();
        file.compare_exchange(Some(1), &encode(&snap)).unwrap();
        let older = |bytes: &[u8]| {
            let c = cbor::try_decode(bytes).unwrap();
            let list = |key| {
                let items = c.try_get(key).unwrap().try_array().unwrap();
                items
                    .iter()
                    .map(|x| x.try_bytes().unwrap())
                    .collect::<Vec<_>>()
            };
            (list(1), list(2))
        };
        let read = older(&fs::read(file.path()).unwrap());
        assert_eq!(read, (snap.records.clone(), snap.heads.clone()));
        fs::write(file.path(), before_the_step(&snap)).unwrap();
        assert_eq!(
            file.load().unwrap(),
            Some((1, encode(&snap))),
            "revision 1 again"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// F2 (the owner's answer (a) of 2026-09-27): as the persistence port,
    /// records.json holds snapshots only. Bytes that are not a snapshot as
    /// the node writes one, here a probe's `[1, 2, 3]`, empty bytes, the
    /// snapshot with its key 3 as the generated codec writes it, and a map
    /// whose keys are out of order, are refused as `Capacity`, and nothing is
    /// written. The node's snapshot commits at revision 1 and loads back byte
    /// for byte. Each of the store's outcomes is the port's of the same name,
    /// but for bytes that are not a snapshot.
    #[tokio::test]
    async fn as_the_port_it_takes_snapshots_only() {
        let dir = fresh("port");
        let file = RecordsFile::new(&dir);
        // `{2: [], 1: []}`, by hand: glade-wire's encoder sorts a map's keys.
        let unordered = vec![0xa2, 0x02, 0x80, 0x01, 0x80];
        let with_key_3 = cbor::encode(&snapshot(1).to_cbor());
        for bytes in [vec![1, 2, 3], vec![], with_key_3, unordered] {
            let refused = SnapshotStore::compare_exchange(&file, None, bytes).await;
            assert_eq!(refused, Err(StoreError::Capacity));
            assert!(!file.path().exists(), "nothing written");
        }
        let bytes = encode(&snapshot(1));
        let committed = Snapshot { revision: 1, bytes };
        let answer = SnapshotStore::compare_exchange(&file, None, committed.bytes.clone());
        assert_eq!(answer.await, Ok(committed.clone()));
        assert_eq!(SnapshotStore::load(&file).await, Ok(Some(committed)));

        let io = || io::Error::other("the test's");
        let (held, expected) = (Some(2), Some(1));
        let conflict = FileError::Conflict { held, expected };
        let outcomes = [
            (FileError::Unavailable(io()), StoreError::Unavailable),
            (FileError::Corrupt("the test's"), StoreError::Corrupt),
            (conflict, StoreError::Conflict),
            (FileError::Exhausted, StoreError::Exhausted),
            (FileError::OutcomeUnknown(io()), StoreError::OutcomeUnknown),
            (FileError::NotASnapshot, StoreError::Capacity),
        ];
        for (outcome, port) in outcomes {
            assert_eq!(StoreError::from(outcome), port);
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
