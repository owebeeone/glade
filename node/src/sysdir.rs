//! On-disk instance layout + the load-validation ladder (GDL-036).
//!
//! A node lives under `$HOME/.glade/sys/<name>/` (`GLADE_HOME` overrides
//! `$HOME/.glade` for tests). The launch **profile** picks a default instance
//! name; profiles are DEPLOYMENT labels, not protocol types — no trace ever
//! sees a profile name.
//!
//! | file            | trust class                              | ships |
//! |-----------------|------------------------------------------|-------|
//! | `node.key`      | 1 — node secret (mode 0600)              | never |
//! | `records.json`  | 2 — signed replicated records (snapshot) | yes   |
//! | `local.json`    | 3 — node-private assertions              | never |
//! | `cache/`        | 4 — derived, rebuildable                 | never |
//! | `instance.lock` | — single-writer lock                     | never |
//! | `records.legacy-<date>.json` | — set aside, never read (4.1a)  | never |
//!
//! Boot = sync from a carrier named "the disk", in class order: node.key perms
//! → NodeId; records.json verify-as-ingest (the same chain checks as the wire
//! store); local.json self-signature with fail-closed defaults; cache/ hash or
//! discard-and-refold. Nothing above [`StoreApi`] knows files exist.
//!
//! Plan Step 4.1a: `node.key` is an Ed25519 seed and the NodeId is its public
//! key (`GladeNodeSigning.md` D2). A boot that finds this node's records under
//! the id the key had before, `sha256(node.key)`, sets them aside once. Still
//! structural: class-2 records carry no signatures until 4.1b, and the class-3
//! self-signature is not checked until 4.1c. The permission check, the class-2
//! chain verification, and the fail-closed load STRUCTURE are real.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use glade_wire::cbor;
use glade_wire::generated::Op;
use sha2::{Digest, Sha256};

use crate::peer::NodeIdentity;
use crate::registry::{BlobStore, Record, Registry, RegistryApi, StoreApi, HOME};
use crate::signing;
use crate::store::unused_path;
use crate::sysdata::{NodeRecord, ServeClaim, SystemSnapshot};

/// A launch profile — a default instance name + typical roles. A deployment
/// label only; the protocol knows roles + operators, never a profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Profile {
    /// localhost session host — the entry node for the local UI.
    Local,
    /// workspace host — claim-holder, grazel/gwz embedded.
    Peer,
    /// DC / fleet — entry + durable home-node roles.
    Server,
}

impl Profile {
    pub fn default_name(self) -> &'static str {
        match self {
            Profile::Local => "glade-local",
            Profile::Peer => "glade-peer",
            Profile::Server => "glade-server",
        }
    }

    pub fn parse(s: &str) -> Option<Profile> {
        match s {
            "local" | "glade-local" => Some(Profile::Local),
            "peer" | "glade-peer" => Some(Profile::Peer),
            "server" | "glade-server" => Some(Profile::Server),
            _ => None,
        }
    }
}

/// `$GLADE_HOME`, else `$HOME/.glade`. Tests must set `GLADE_HOME` to a temp
/// dir — the real `~/.glade` is never touched.
pub fn glade_home() -> PathBuf {
    if let Ok(h) = std::env::var("GLADE_HOME") {
        return PathBuf::from(h);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".glade")
}

/// The single-writer lock (plan Step 4.4's question 3, owner 2026-09-24): an
/// exclusive OS lock (`File::try_lock`: `flock` on Unix, `LockFileEx` on
/// Windows) on an open handle to `instance.lock`, held for the instance's
/// life. The kernel releases it with its process, so a crash leaves no lock
/// that refuses the next boot, while a live holder, in this process or
/// another, still refuses it. The file records the holder's pid for diagnosis
/// (gryth-ui's `gyld-ui.py` reads it), and a clean release removes it, as the
/// O_EXCL lock before it did: removed first, then the handle closed.
pub struct InstanceLock {
    path: PathBuf,
    _held: fs::File,
}

impl InstanceLock {
    fn acquire(path: PathBuf) -> io::Result<InstanceLock> {
        let locked = || {
            let what = format!("instance already locked: {}", path.display());
            io::Error::new(io::ErrorKind::AddrInUse, what)
        };
        // A holder removes the file before it lets the lock go, so a boot
        // that opened the file just then may lock one its path no longer
        // names. It starts again: a third boot could lock a new file there.
        for _ in 0..3 {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(fs::TryLockError::WouldBlock) => return Err(locked()),
                Err(fs::TryLockError::Error(e)) => return Err(e),
            }
            if platform::names(&path, &file)? {
                let pid = std::process::id();
                let _ = file.set_len(0).and_then(|()| write!(file, "{pid}"));
                return Ok(InstanceLock { path, _held: file });
            }
        }
        Err(locked())
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Removed while still locked; the handle, and with it the lock, goes
        // after this.
        let _ = fs::remove_file(&self.path);
    }
}

/// A booted node: the acquired instance, its identity, the materialised
/// registry (queries-over-fold, ready) and the store engine behind it.
pub struct Boot {
    pub dir: PathBuf,
    pub node_id: String,
    pub operator: String,
    pub registry: Registry,
    pub store: BlobStore,
    /// records quarantined by verify-as-ingest (load evidence).
    pub rejected: usize,
    /// What this boot set aside under the node's old id (plan Step 4.1a).
    pub set_aside: Option<SetAside>,
    /// The class-1 node key, an Ed25519 seed — kept in memory ONLY to sign as
    /// this node ([`Boot::identity`]); never shipped, never in any snapshot.
    seed: [u8; 32],
    _lock: InstanceLock,
}

/// What a boot set aside (plan Step 4.1a; `GladeNodeSigning.md` D8 (a)): this
/// node's records under the id its key had before the step,
/// `hex(sha256(node.key))`, written to a new `records.legacy-<date>.json` in
/// the instance and never folded. The first boot after the step does it once.
#[derive(Debug)]
pub struct SetAside {
    pub records: usize,
    pub old_id: String,
    pub file: PathBuf,
}

/// The line both composition roots print after `node`.
impl fmt::Display for SetAside {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.file.file_name().unwrap_or_default().to_string_lossy();
        let (records, old_id) = (self.records, &self.old_id);
        write!(
            f,
            "set aside {records} record(s) of old node id {old_id} in {name}"
        )
    }
}

impl Boot {
    /// The peer-link identity for this node: `NodeIdentity::from_key(node.key)`,
    /// so the node_id spoken on the node<->node HELLO seam is the raw-bytes twin
    /// of the hex NodeId in directory records — ONE identity, two renderings.
    /// Claim routing depends on this: a folded `ServeClaim.node` (hex) must
    /// match the id a peer link vouches.
    pub fn identity(&self) -> io::Result<NodeIdentity> {
        Ok(NodeIdentity::from_key(self.seed))
    }

    /// The id this node's key had before plan Step 4.1a, `hex(sha256(key))`,
    /// which its older records name.
    pub(crate) fn legacy_id(&self) -> String {
        legacy_id_of(&self.seed)
    }
}

/// Boot a node for `profile`, optionally overriding the instance name and the
/// operator. Resolves the instance dir under [`glade_home`]. See [`boot_at`].
pub fn boot(profile: Profile, name: Option<&str>, operator: Option<&str>) -> io::Result<Boot> {
    boot_at(instance_dir(profile, name), operator.unwrap_or("local"))
}

/// Where [`boot`] puts the instance for `profile`, or for `name` when given:
/// `<glade_home>/sys/<name>`.
pub fn instance_dir(profile: Profile, name: Option<&str>) -> PathBuf {
    let name = name.unwrap_or_else(|| profile.default_name());
    glade_home().join("sys").join(name)
}

/// Run the load-validation ladder at an explicit instance dir (tests pass a
/// temp dir — no `GLADE_HOME` env race). Class order: 1 → 2 → 3 → 4.
pub fn boot_at(dir: PathBuf, operator: &str) -> io::Result<Boot> {
    fs::create_dir_all(dir.join("cache"))?; // class 4: cache/ present, never load-bearing
    let lock = InstanceLock::acquire(dir.join("instance.lock"))?;

    // ---- class 1: node.key -> NodeId (ssh-discipline perms) ----------------
    let seed = load_or_create_node_key(&dir)?;
    let node_id = node_id_of(&seed);

    // ---- class 2: records.json -> verify-as-ingest -> the fold -------------
    // The records under the key's old id leave the snapshot first, once.
    let mut store = BlobStore::new(&dir);
    let mut snap = store.load()?;
    let set_aside = set_aside(&dir, &mut snap, &legacy_id_of(&seed))?;
    let (mut registry, rejected) = Registry::from_snapshot(&snap);

    // ---- class 3: local.json (node-self-signature, fail-closed) ------------
    load_local_json(&dir); // structural in M-LIMP; failures discard to defaults

    // ---- class 1 <-> class 2 identity match / first-boot presence ----------
    // Our derived NodeId must correspond to our own NodeRecord. Absent it, this
    // is a first boot: write presence (K1) — an ATTRIBUTED append, not setConfig.
    let mut changed = set_aside.is_some();
    if !registry.has_node(&node_id) {
        registry
            .append(Record::Node(NodeRecord { node_id: node_id.clone(), operator: operator.into() }), &node_id)
            .map_err(reg_io)?;
        registry
            .append(
                // Lease expiry is an ABSOLUTE wall-clock ms, stamped at write
                // time (the clock is used to WRITE; it never enters the fold).
                Record::Serve(ServeClaim { node: node_id.clone(), share: HOME.into(), lease_expiry_ms: now_ms() + 30_000, epoch: 1 }),
                &node_id,
            )
            .map_err(reg_io)?;
        changed = true;
    }
    if changed {
        store.save(&registry.snapshot())?; // rewritten tmp+rename
    }

    Ok(Boot {
        dir,
        node_id,
        operator: operator.into(),
        registry,
        store,
        rejected,
        set_aside,
        seed,
        _lock: lock,
    })
}

/// Load `node.key` (refusing group/world-readable, the ssh discipline, and any
/// length but the 32 bytes of an Ed25519 seed) or create it 0600 on first boot
/// from the OS's randomness. Class-1 secret: never shipped, never in any
/// snapshot.
fn load_or_create_node_key(dir: &Path) -> io::Result<[u8; 32]> {
    let path = dir.join("node.key");
    if path.exists() {
        platform::check_key_perms(&path)?;
        let mut held = Vec::new();
        fs::File::open(&path)?.read_to_end(&mut held)?;
        return held.as_slice().try_into().map_err(|_| {
            let why = format!("node.key is {} bytes, not an Ed25519 seed's 32", held.len());
            io::Error::new(io::ErrorKind::InvalidData, why)
        });
    }
    let seed = signing::random_seed()?;
    platform::write_secret(&path, &seed)?;
    Ok(seed)
}

/// Plan Step 4.1a (`GladeNodeSigning.md` D8 (a), for the id change): take this
/// node's records under `old_id` out of `snap`, and write them, byte for byte,
/// to a new `records.legacy-<date>.json` in `dir`, synced with its directory
/// entry before records.json is saved without them. A crash between the two
/// repeats this at the next boot, into a second file: nothing is lost.
fn set_aside(dir: &Path, snap: &mut SystemSnapshot, old_id: &str) -> io::Result<Option<SetAside>> {
    let ours = |bytes: &Vec<u8>| Op::from_cbor(&cbor::decode(bytes)).origin == old_id;
    let (old, kept): (Vec<_>, Vec<_>) = snap.records.drain(..).partition(ours);
    snap.records = kept;
    if old.is_empty() {
        return Ok(None);
    }
    let records = old.len();
    let legacy = SystemSnapshot {
        records: old,
        heads: vec![],
    };
    let file = unused_path(dir, &format!("records.legacy-{}", today()), ".json");
    let mut out = fs::File::create_new(&file)?;
    out.write_all(&cbor::encode(&legacy.to_cbor()))?;
    out.sync_all()?;
    crate::registry::entry_sync::sync(dir)?;
    let old_id = old_id.into();
    Ok(Some(SetAside {
        records,
        old_id,
        file,
    }))
}

/// Today's UTC date, `YYYY-MM-DD`, which names what plan Step 4.1a sets aside.
pub(crate) fn today() -> String {
    date_of(now_ms())
}

/// The UTC calendar date of an epoch-ms instant, `YYYY-MM-DD`: Howard
/// Hinnant's civil-from-days, on days since 1970-01-01.
fn date_of(ms: i64) -> String {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Wall-clock now in epoch ms — used only to STAMP write-time values (lease
/// expiry); never consulted inside the fold.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// NodeId = hex of the node key's Ed25519 public key (plan Step 4.1a, D2): the
/// id is the key, so a verifier needs no lookup. Key replacement changes it ⇒
/// identity loss, never forgery.
fn node_id_of(seed: &[u8; 32]) -> String {
    hex(&signing::public_key(seed))
}

/// The id the same key had before plan Step 4.1a, `hex(sha256(key))`, which
/// the node's older records name.
fn legacy_id_of(seed: &[u8; 32]) -> String {
    hex(&Sha256::digest(seed))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// What std offers on Unix alone: file modes, and a file's identity. Each
// platform's branch is one braced module, so the condition encloses the whole
// section.
#[cfg(unix)]
mod platform {
    use std::fs;
    use std::io::{self, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub(super) fn check_key_perms(path: &Path) -> io::Result<()> {
        let mode = fs::metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "node.key is group/world-accessible (mode {:o}) — refusing",
                    mode & 0o777
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn write_secret(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)
    }

    /// Whether `path` names `file`: the same device and inode. A file removed
    /// since it was opened is named by nothing, or by a new file.
    pub(super) fn names(path: &Path, file: &fs::File) -> io::Result<bool> {
        let held = file.metadata()?;
        match fs::metadata(path) {
            Ok(named) => Ok(named.dev() == held.dev() && named.ino() == held.ino()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use std::fs;
    use std::io;
    use std::path::Path;

    pub(super) fn check_key_perms(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    pub(super) fn write_secret(path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(path, bytes)
    }

    /// std has no stable file identity here, so the path is taken to name the
    /// file: a named gap (`GladeNodeAssembly.md`, "Hardening").
    pub(super) fn names(_path: &Path, _file: &fs::File) -> io::Result<bool> {
        Ok(true)
    }
}

/// Class 3 — node-private assertions (authority overlay, suspect marks, resume
/// vectors). Node-self-signed; a failed check discards each item to its
/// declared MOST-restrictive default, never to "off". Structural in M-LIMP
/// (there are no overlay items yet); the fail-closed rule is the wall.
fn load_local_json(dir: &Path) -> LocalOverlay {
    let path = dir.join("local.json");
    match fs::read(&path) {
        Ok(_bytes) => LocalOverlay::fail_closed(), // self-sig verify TODO -> defaults for now
        Err(_) => LocalOverlay::fail_closed(),
    }
}

/// The node-private authority overlay. It only ever NARROWS granted rights, so
/// tamper cannot exceed a grant; every field has a fail-closed default.
#[derive(Debug, PartialEq)]
pub struct LocalOverlay;
impl LocalOverlay {
    fn fail_closed() -> LocalOverlay {
        LocalOverlay
    }
}

fn reg_io(e: crate::registry::RegistryError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("registry append rejected: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-sysdir-{name}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn boot_creates_layout_and_writes_presence() {
        let dir = fresh("layout");
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        // the per-instance directory + class-4 cache exist.
        assert!(dir.join("node.key").exists());
        assert!(dir.join("records.json").exists()); // class-2 snapshot written
        assert!(dir.join("cache").is_dir());
        assert!(dir.join("instance.lock").exists()); // held while `boot` lives
        // presence: our node is in the fold, attributed to our own id.
        assert!(boot.registry.has_node(&boot.node_id));
        assert_eq!(boot.registry.nodes_of("gianni"), vec![boot.node_id.clone()]);
        // the node served itself its own home before any client: whoServes(home).
        assert_eq!(boot.registry.who_serves(HOME, 0), Some(boot.node_id.clone()));
    }

    #[test]
    fn reboot_is_idempotent_presence_not_duplicated() {
        let dir = fresh("reboot");
        let first = boot_at(dir.clone(), "gianni").unwrap();
        let id = first.node_id.clone();
        drop(first);
        let second = boot_at(dir, "gianni").unwrap();
        // still exactly one node for the operator — presence written once.
        assert_eq!(second.node_id, id);
        assert_eq!(second.nodes_of_ops(), 1);
    }

    /// A records.json that holds one record twice (plan Step 4.4; owner,
    /// 2026-09-24): boot takes the repeat as a duplicate, quarantines nothing,
    /// and folds the rest of that origin's chain; the next save writes the
    /// record once. Until the ruling the repeat was a fork, and boot set it
    /// aside with every later record of the chain.
    #[test]
    fn boot_takes_a_record_held_twice_as_one_and_keeps_the_rest_of_its_chain() {
        let dir = fresh("duplicate");
        let mut peer = Registry::new();
        for lease_expiry_ms in [1_000, 2_000, 5_000] {
            let share = "ws-x".into();
            let claim = ServeClaim {
                node: "peer".into(),
                share,
                lease_expiry_ms,
                epoch: 1,
            };
            peer.append(Record::Serve(claim), "peer").unwrap();
        }
        let mut snap = peer.snapshot();
        let repeat = snap.records[1].clone();
        snap.records.insert(2, repeat.clone());
        BlobStore::new(&dir).save(&snap).unwrap();

        let boot = boot_at(dir.clone(), "gianni").unwrap();
        assert_eq!(boot.rejected, 0, "nothing quarantined");
        let serves = boot.registry.who_serves("ws-x", 3_000);
        assert_eq!(serves, Some("peer".into()), "the third claim folds");
        let saved = BlobStore::new(&dir).load().unwrap();
        let held = saved
            .records
            .iter()
            .filter(|record| **record == repeat)
            .count();
        assert_eq!(held, 1, "and is saved once");
    }

    /// Plan Step 4.1a (D8 (a), for the id change): an instance written before
    /// the step names its node `hex(sha256(node.key))`, as the records.json
    /// written here does. The first boot on the new id writes those records,
    /// byte for byte, to a new `records.legacy-<date>.json`, beside an older
    /// file of that name, which it does not overwrite. records.json keeps
    /// another origin's record and gains the new presence, and the operator
    /// has one node, not two. A second boot sets nothing aside. It does not
    /// reach the served store (`claims.rs` does), or a crash between the writes.
    #[test]
    fn a_first_boot_on_the_new_id_sets_the_old_records_aside_once() {
        let dir = fresh("legacy");
        let seed = boot_at(dir.clone(), "gianni").unwrap().seed;
        let old = legacy_id_of(&seed);
        let mut before = Registry::new();
        let presence = NodeRecord {
            node_id: old.clone(),
            operator: "gianni".into(),
        };
        before.append(Record::Node(presence), &old).unwrap();
        let claim = |node: &str, share: &str| {
            Record::Serve(ServeClaim {
                node: node.into(),
                share: share.into(),
                lease_expiry_ms: 1,
                epoch: 1,
            })
        };
        before.append(claim(&old, HOME), &old).unwrap();
        before.append(claim("peer", "ws-p"), "peer").unwrap();
        let written = before.snapshot();
        BlobStore::new(&dir).save(&written).unwrap();
        let taken = dir.join(format!("records.legacy-{}.json", date_of(now_ms())));
        fs::write(&taken, "an older file").unwrap();

        let boot = boot_at(dir.clone(), "gianni").unwrap();
        let aside = boot.set_aside.as_ref().expect("the old records set aside");
        assert_eq!((aside.records, &aside.old_id), (2, &old));
        assert_ne!(aside.file, taken);
        let older = fs::read(&taken).unwrap();
        assert_eq!(older, b"an older file", "not overwritten");
        let held = SystemSnapshot::from_cbor(&cbor::decode(&fs::read(&aside.file).unwrap()));
        assert_eq!(held.records, written.records[..2], "byte for byte");
        assert_eq!(boot.registry.nodes_of("gianni"), vec![boot.node_id.clone()]);
        let saved = BlobStore::new(&dir).load().unwrap();
        let origins: Vec<String> = saved
            .records
            .iter()
            .map(|bytes| Op::from_cbor(&cbor::decode(bytes)).origin)
            .collect();
        assert!(!origins.contains(&old), "records.json holds none of them");
        let peer = "peer".to_string();
        assert!(origins.contains(&peer), "and keeps the peer's");
        drop(boot);
        assert!(boot_at(dir, "gianni").unwrap().set_aside.is_none(), "once");
    }

    /// The legacy file's date: the UTC calendar date of an epoch-ms instant,
    /// across a leap day, the last and first milliseconds of a day, and
    /// before 1970.
    #[test]
    fn dates_are_utc_calendar_dates() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(951_782_400_000), "2000-02-29");
        assert_eq!(date_of(1_790_208_000_000 - 1), "2026-09-23");
        assert_eq!(date_of(1_790_208_000_000), "2026-09-24");
        assert_eq!(date_of(-1), "1969-12-31");
    }

    /// Plan Step 4.1a: `node.key` is an Ed25519 seed, so a key that is not 32
    /// bytes refuses the boot (`InvalidData`) before anything is written.
    /// Before, boot hashed whatever the file held and wrote presence under it.
    #[test]
    fn a_node_key_that_is_not_32_bytes_refuses_the_boot() {
        let dir = fresh("short-key");
        drop(boot_at(dir.clone(), "gianni").unwrap());
        fs::remove_file(dir.join("records.json")).unwrap();
        let path = dir.join("node.key");
        let key = fs::OpenOptions::new().write(true).open(path);
        key.and_then(|key| key.set_len(31)).unwrap();
        let err = boot_at(dir.clone(), "gianni").map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(!dir.join("records.json").exists(), "nothing written");
    }

    #[test]
    fn instance_lock_is_single_writer() {
        let dir = fresh("lock");
        let held = boot_at(dir.clone(), "gianni").unwrap();
        // a second boot on the SAME dir while the first is live is refused.
        let err = boot_at(dir.clone(), "gianni").map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        drop(held); // releasing the lock lets a new writer in.
        assert!(boot_at(dir, "gianni").is_ok());
    }

    /// Plan Step 4.4's question 3 (owner, 2026-09-24): a crash leaves
    /// `instance.lock` behind, holding its dead process's pid, with no one
    /// holding its lock; the test writes that file itself. The next boot
    /// takes the instance and records its own pid, a second boot while it
    /// lives is still refused, and a clean release removes the file, as
    /// before. It crashes no process: `tests/stop_signal.rs` kills a node.
    #[test]
    fn a_lock_file_left_by_a_crash_blocks_no_boot() {
        let dir = fresh("crash");
        fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("instance.lock");
        fs::write(&lock, "4242").unwrap();
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        // A Unix lock is advisory, so anyone can read the pid while it is
        // held (gryth-ui's gyld-ui.py does). A Windows lock (LockFileEx over
        // the whole file) refuses other handles' reads until it is released,
        // so there the pid is readable only by the holder.
        if cfg!(unix) {
            let pid = std::process::id().to_string();
            assert_eq!(fs::read_to_string(&lock).unwrap(), pid, "the holder's pid");
        }
        let err = boot_at(dir.clone(), "gianni").map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        drop(boot);
        assert!(!lock.exists(), "a clean release removes the file");
    }

    /// One identity, two renderings: the peer-link NodeIdentity derived from
    /// node.key is the raw-bytes twin of the hex NodeId in directory records —
    /// the identity match claim routing (WD §4) stands on.
    #[test]
    fn boot_identity_matches_directory_node_id() {
        let dir = fresh("identity");
        let boot = boot_at(dir, "gianni").unwrap();
        let id = boot.identity().unwrap();
        let hexed: String = id.node_id.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hexed, boot.node_id);
    }

    #[test]
    fn profiles_name_the_instance() {
        assert_eq!(Profile::Local.default_name(), "glade-local");
        assert_eq!(Profile::Peer.default_name(), "glade-peer");
        assert_eq!(Profile::Server.default_name(), "glade-server");
        assert_eq!(Profile::parse("peer"), Some(Profile::Peer));
        assert_eq!(Profile::parse("nope"), None);
    }

    // small helper: how many nodes the operator has (presence-count assertion).
    impl Boot {
        fn nodes_of_ops(&self) -> usize {
            self.registry.nodes_of(&self.operator).len()
        }
    }

    // File modes and file identity are Unix notions. A braced module, so the
    // condition encloses the whole section.
    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn node_key_is_0600_and_group_readable_is_refused() {
            let dir = fresh("perms");
            let boot = boot_at(dir.clone(), "gianni").unwrap();
            let node_id = boot.node_id.clone();
            // release the lock so we can reboot
            drop(boot);
            // the key was created 0600.
            let mode = fs::metadata(dir.join("node.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
            // widen to group-readable -> the ssh discipline refuses the next boot.
            fs::set_permissions(dir.join("node.key"), fs::Permissions::from_mode(0o640)).unwrap();
            let err = boot_at(dir.clone(), "gianni").map(|_| ()).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
            // restore + reboot: SAME NodeId (derived from the same key) — identity
            // is stable across reboots; verify-as-ingest re-materialises the fold.
            fs::set_permissions(dir.join("node.key"), fs::Permissions::from_mode(0o600)).unwrap();
            let again = boot_at(dir, "gianni").unwrap();
            assert_eq!(again.node_id, node_id);
            assert_eq!(again.rejected, 0);
        }

        /// The instance lock's identity check (plan Step 4.4's question 3): a
        /// file removed after it was opened is not what its path names, nor
        /// is a new file made at the path since. A boot that locked such a
        /// file starts again; that race lies between two system calls, and
        /// this test does not force it.
        #[test]
        fn the_lock_path_names_only_the_file_it_opened() {
            let dir = fresh("names");
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("instance.lock");
            let opened = fs::File::create(&path).unwrap();
            assert!(platform::names(&path, &opened).unwrap(), "opened");
            fs::remove_file(&path).unwrap();
            assert!(!platform::names(&path, &opened).unwrap(), "removed");
            fs::write(&path, "").unwrap();
            assert!(!platform::names(&path, &opened).unwrap(), "a new file");
        }
    }
}
