//! On-disk instance layout + the load-validation ladder (GDL-036).
//!
//! A node lives under `<root>/sys/<name>/`. The instance root is
//! `$HOME/.glade`, or `GLADE_HOME` when set (tests set it); `glade-node`
//! reads both once, at its entry point, and passes the root in
//! ([`instance_root`]). This module reads no environment. The launch
//! **profile** picks a default instance name; profiles are DEPLOYMENT labels,
//! not protocol types — no trace ever sees a profile name.
//!
//! | file            | trust class                              | ships |
//! |-----------------|------------------------------------------|-------|
//! | `node.key`      | 1 — node secret (mode 0600)              | never |
//! | `endpoint.key`  | 1 — iroh endpoint secret (mode 0600)     | never |
//! | `records.json`  | 2 — signed replicated records (snapshot) | yes   |
//! | `local.json`    | 3 — node-private assertions              | never |
//! | `cache/`        | 4 — derived, rebuildable                 | never |
//! | `instance.lock` | — single-writer lock                     | never |
//! | `records.json.lock` | — records.json's compare-and-swap lock | never |
//! | `records.legacy-<date>.json` | — set aside, never read (4.1a, 4.1b) | never |
//!
//! Boot = sync from a carrier named "the disk", in class order: node.key perms
//! → NodeId; records.json verify-as-ingest (the same chain checks as the wire
//! store); local.json self-signature with fail-closed defaults; cache/ hash or
//! discard-and-refold. Nothing above [`StoreApi`] knows files exist.
//!
//! Plan Step 4.1a: `node.key` is an Ed25519 seed and the NodeId is its public
//! key (`GladeNodeSigning.md` D2). Plan Step 4.1b: every class-2 record is
//! signed by its origin (`envelope.rs`), and the registry is sealed as this
//! node, so it signs what it appends and loads only records that verify. A
//! boot that finds unsigned records, written before the step, sets them
//! aside once (D8); among them are any under the id the key had before
//! 4.1a, `sha256(node.key)`.
//!
//! Plan Step 4.1c: the class-3 self-signature is checked under D7's
//! `local-overlay` tag (`overlay.rs`), and a first boot given `--recovery-out`
//! commits the node's recovery key in the save that writes its presence
//! (`recovery.rs`).
//!
//! Plan Step 4.2: `endpoint.key` is the iroh endpoint's seed, kept apart from
//! `node.key` so the identity survives the transport key's replacement. The
//! boot binds it to the node by a signed record, and revokes the node's
//! bindings of any key it replaced (`transport.rs`).
//!
//! records.json carries its store's revision (the owner's ruling of
//! 2026-09-24, `records_file.rs`), and is read with checked heads: a damaged
//! one refuses the boot with a message, where the wire codec panicked.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use glade_wire::cbor;

use crate::claims::LEASE_TTL_MS;
use crate::envelope::{self, Format};
use crate::overlay::{self, Checked};
use crate::peer::NodeIdentity;
use crate::recovery::{self, Committed};
use crate::registry::{BlobStore, Record, Registry, RegistryApi, StoreApi, HOME};
use crate::signing;
use crate::store::unused_path;
use crate::sysdata::{NodeRecord, ServeClaim, SystemSnapshot};
use crate::transport::{self, EndpointKey, Rebound};

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

/// The instance root, from the values of `GLADE_HOME` and `HOME` that a
/// composition root read once, at its entry point: `GLADE_HOME`, else
/// `$HOME/.glade`, else `./.glade`. Tests pass a temp dir — the real
/// `~/.glade` is never touched.
pub fn instance_root(glade_home: Option<String>, home: Option<String>) -> PathBuf {
    if let Some(h) = glade_home {
        return PathBuf::from(h);
    }
    let home = home.unwrap_or_else(|| ".".into());
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
    /// Take the lock at `path`, or say the instance is locked already
    /// (`AddrInUse`). Boot takes it, and so does `glade-node endpoint-id`
    /// while it mints a key (`endpoint_id.rs`).
    pub(crate) fn acquire(path: PathBuf) -> io::Result<InstanceLock> {
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
    /// The unsigned records this boot set aside (plan Step 4.1b).
    pub set_aside: Option<SetAside>,
    /// What this boot did to the node's transport bindings (plan Step 4.2).
    pub rebound: Rebound,
    /// What the class-3 check made of `local.json` (plan Step 4.1c).
    pub overlay: Checked,
    /// The recovery key a first boot given `--recovery-out` committed (plan
    /// Step 4.1c).
    pub recovery: Option<Committed>,
    /// The class-1 node key, an Ed25519 seed — kept in memory ONLY to sign as
    /// this node ([`Boot::identity`]); never shipped, never in any snapshot.
    seed: [u8; 32],
    /// The class-1 endpoint key (plan Step 4.2), which the iroh endpoint
    /// binds with ([`Boot::endpoint_key`]).
    endpoint: EndpointKey,
    _lock: InstanceLock,
}

/// What a boot set aside (plan Step 4.1b; `GladeNodeSigning.md` D8 (a)): the
/// unsigned records in records.json, which every node wrote before the step,
/// written to a new `records.legacy-<date>.json` in the instance and never
/// folded. The first boot after the step does it once. It takes in plan Step
/// 4.1a's set-aside of the records under the key's old id: they are unsigned.
#[derive(Debug)]
pub struct SetAside {
    pub records: usize,
    pub file: PathBuf,
}

/// The line both composition roots print after `node`.
impl fmt::Display for SetAside {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.file.file_name().unwrap_or_default().to_string_lossy();
        let records = self.records;
        write!(f, "set aside {records} unsigned record(s) in {name}")
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

    /// The key this node's iroh endpoint binds with, the same at every boot
    /// (plan Step 4.2), and bound to this node in its records.
    pub fn endpoint_key(&self) -> EndpointKey {
        self.endpoint
    }
}

/// Boot a node for `profile` under the instance root `root`, optionally
/// overriding the instance name, which `named_instance` checks, and the
/// operator, and taking `--recovery-out` and the node's lease. See
/// [`boot_at_with`].
pub fn boot(
    root: &Path,
    profile: Profile,
    name: Option<&str>,
    operator: Option<&str>,
    recovery_out: Option<&Path>,
    lease_ms: i64,
) -> io::Result<Boot> {
    let dir = instance_dir(root, profile, name)?;
    boot_at_with(dir, operator.unwrap_or("local"), recovery_out, lease_ms)
}

/// Where [`boot`] puts the instance for `profile`, or for `name` when given:
/// `<root>/sys/<name>`, once `named_instance` has checked the name.
pub fn instance_dir(root: &Path, profile: Profile, name: Option<&str>) -> io::Result<PathBuf> {
    named_instance(root, name.unwrap_or_else(|| profile.default_name()))
}

/// The instance `<root>/sys/<name>` (F9, the owner's ruling of 2026-09-27):
/// `name` must match `[A-Za-z0-9._-]{1,63}` and not end in `.`, so the path
/// names a directory in `<root>/sys` and nowhere else (`.` and `..` end in
/// one), and one instance on every platform: Windows trims a trailing dot,
/// so there `n.` would be `n` (F9 (b)). Nor may it be a name Windows keeps
/// for a device ([`windows_device`]), which there names the device and no
/// directory. Any other name is refused
/// (`InvalidInput`), quoted as given, before anything is written: both
/// roots' boots, `glade-node recovery` and `glade-node endpoint-id` take
/// their instance here.
pub(crate) fn named_instance(root: &Path, name: &str) -> io::Result<PathBuf> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
    let fits = (1..=63).contains(&name.len()) && name.chars().all(allowed);
    if !fits || name.ends_with('.') {
        let why = format!(
            "--name {name:?}: an instance name must match [A-Za-z0-9._-]{{1,63}} and not end in a dot"
        );
        return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
    }
    if windows_device(name) {
        let why = format!(
            "--name {name:?}: an instance name must not be a Windows device name (con, prn, aux, nul, com1-com9, lpt1-lpt9) in any case, alone or before a dot"
        );
        return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
    }
    Ok(root.join("sys").join(name))
}

/// Whether Windows reads `name` as a device (the owner's ruling of
/// 2026-09-27): `con`, `prn`, `aux`, `nul`, `com1`-`com9` or `lpt1`-`lpt9`,
/// in any case, alone or before a dot, as `con.txt` is `con` there.
fn windows_device(name: &str) -> bool {
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    match stem.to_ascii_lowercase().as_bytes() {
        b"con" | b"prn" | b"aux" | b"nul" => true,
        [b'c', b'o', b'm', n] | [b'l', b'p', b't', n] => matches!(n, b'1'..=b'9'),
        _ => false,
    }
}

/// Run the load-validation ladder at an explicit instance dir (tests pass a
/// temp dir — no `GLADE_HOME` env race), with the default lease. Class
/// order: 1 → 2 → 3 → 4.
pub fn boot_at(dir: PathBuf, operator: &str) -> io::Result<Boot> {
    boot_at_with(dir, operator, None, LEASE_TTL_MS)
}

/// [`boot_at`], taking `--recovery-out` (plan Step 4.1c): at a first boot the
/// node commits a recovery key, and writes its secret to `recovery_out`, in
/// the save that writes its presence; a later boot given one is refused
/// before records.json is written. The path is checked before anything is
/// written, against the instance root `dir` lives under
/// (`recovery::check_out`). A first boot leases its claim on `home` for
/// `lease_ms`, the node's lease (`claims::Leases`), which adoption renews.
pub fn boot_at_with(
    dir: PathBuf,
    operator: &str,
    recovery_out: Option<&Path>,
    lease_ms: i64,
) -> io::Result<Boot> {
    let root = recovery::root_of(&dir);
    let recovery_out = recovery_out.map(|out| recovery::check_out(root, out));
    let recovery_out = recovery_out.transpose()?;
    fs::create_dir_all(dir.join("cache"))?; // class 4: cache/ present, never load-bearing
    let lock = InstanceLock::acquire(dir.join("instance.lock"))?;

    // ---- class 1: node.key -> NodeId, endpoint.key (ssh-discipline perms) -
    let seed = load_or_create_secret(&dir, "node.key")?;
    let endpoint = EndpointKey::from_seed(load_or_create_secret(&dir, "endpoint.key")?);
    let node_id = node_id_of(&seed);

    // ---- class 2: records.json -> verify-as-ingest -> the fold -------------
    // Unsigned records leave the snapshot first, once (plan Step 4.1b); the
    // registry, sealed as this node, then takes only records that verify.
    let mut store = BlobStore::new(&dir);
    let mut snap = store.load()?;
    let set_aside = set_aside(&dir, &mut snap)?;
    let identity = NodeIdentity::from_key(seed);
    let (mut registry, rejected) = Registry::from_snapshot_as(&snap, identity);

    // ---- class 3: local.json (node-self-signature, fail-closed) ------------
    // A file that fails its check is discarded to the fail-closed defaults,
    // and the roots say so (plan Step 4.1c).
    let overlay = overlay::load(&dir, &identity);

    // ---- class 1 <-> class 2 identity match / first-boot presence ----------
    // Our derived NodeId must correspond to our own NodeRecord. Absent it, this
    // is a first boot: write presence (K1) — an ATTRIBUTED append, not setConfig.
    let mut changed = set_aside.is_some();
    let mut recovery = None;
    if !registry.has_node(&node_id) {
        registry
            .append(Record::Node(NodeRecord { node_id: node_id.clone(), operator: operator.into() }), &node_id)
            .map_err(reg_io)?;
        registry
            .append(
                // Lease expiry is an ABSOLUTE wall-clock ms, stamped at write
                // time (the clock is used to WRITE; it never enters the fold).
                Record::Serve(ServeClaim {
                    node: node_id.clone(),
                    share: HOME.into(),
                    lease_expiry_ms: now_ms() + lease_ms,
                    epoch: 1,
                }),
                &node_id,
            )
            .map_err(reg_io)?;
        // The recovery key (plan Step 4.1c): its secret written first, then
        // its commitment saved with the presence.
        if let Some(out) = &recovery_out {
            let (record, committed) = recovery::mint(&node_id, out)?;
            let recovered = registry.append(Record::Recovery(record), &node_id);
            recovered.map_err(reg_io)?;
            recovery = Some(committed);
        }
        changed = true;
    } else if let Some(out) = &recovery_out {
        return Err(recovery::not_first_boot(&dir, out));
    }
    // The endpoint key bound to this node, and any key it replaced revoked
    // (plan Step 4.2), in the same save.
    let rebound = transport::bind_at_boot(&mut registry, &seed, &endpoint, now_ms())?;
    changed |= rebound.minted || rebound.revoked > 0;
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
        rebound,
        overlay,
        recovery,
        seed,
        endpoint,
        _lock: lock,
    })
}

/// Load the class-1 secret `name`, `node.key` or `endpoint.key` (refusing
/// group/world-readable, the ssh discipline, and any length but the 32 bytes
/// of an Ed25519 seed), or create it 0600 on first boot from the OS's
/// randomness. Never shipped, never in any snapshot. `glade-node
/// endpoint-id` reads and mints `endpoint.key` with it too.
pub(crate) fn load_or_create_secret(dir: &Path, name: &str) -> io::Result<[u8; 32]> {
    let path = dir.join(name);
    if path.exists() {
        platform::check_key_perms(&path)?;
        let mut held = Vec::new();
        fs::File::open(&path)?.read_to_end(&mut held)?;
        return held.as_slice().try_into().map_err(|_| {
            let why = format!("{name} is {} bytes, not an Ed25519 seed's 32", held.len());
            io::Error::new(io::ErrorKind::InvalidData, why)
        });
    }
    let seed = signing::random_seed()?;
    platform::write_secret(&path, &seed)?;
    Ok(seed)
}

/// Plan Step 4.1b (`GladeNodeSigning.md` D8 (a)): take the unsigned records,
/// written before the step, out of `snap`, and write them, byte for byte, to a
/// new `records.legacy-<date>.json` in `dir`, synced with its directory entry
/// before records.json is saved without them. A crash between the two
/// repeats this at the next boot, into a second file: nothing is lost. A
/// record in a format this build does not know refuses the boot before
/// anything is written (part 2's hardening): it is not this build's to set
/// aside.
fn set_aside(dir: &Path, snap: &mut SystemSnapshot) -> io::Result<Option<SetAside>> {
    let (mut old, mut kept) = (Vec::new(), Vec::new());
    for bytes in snap.records.drain(..) {
        // An op that cannot be read is left to the registry's load, which
        // quarantines it and says so (F15b).
        let Ok(op) = envelope::decode_op(&bytes) else {
            kept.push(bytes);
            continue;
        };
        match envelope::format(&op) {
            Format::Sealed => kept.push(bytes),
            Format::Unsigned => old.push(bytes),
            Format::Unknown => {
                return Err(envelope::unreadable(&dir.join("records.json"), &op));
            }
        }
    }
    snap.records = kept;
    if old.is_empty() {
        return Ok(None);
    }
    let records = old.len();
    let legacy = SystemSnapshot {
        records: old,
        heads: vec![],
        revision: None,
    };
    let file = unused_path(dir, &format!("records.legacy-{}", today()), ".json");
    let mut out = fs::File::create_new(&file)?;
    out.write_all(&cbor::encode(&legacy.to_cbor()))?;
    out.sync_all()?;
    crate::registry::entry_sync::sync(dir)?;
    Ok(Some(SetAside { records, file }))
}

/// Today's UTC date, `YYYY-MM-DD`, which names what plan Steps 4.1a and 4.1b
/// set aside.
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
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{name} is group/world-accessible (mode {:o}) — refusing",
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

fn reg_io(e: crate::registry::RegistryError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("registry append rejected: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::generated::Op;

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
    /// aside with every later record of the chain. The records are another
    /// node's, signed by it (plan Step 4.1b), so boot keeps them.
    #[test]
    fn boot_takes_a_record_held_twice_as_one_and_keeps_the_rest_of_its_chain() {
        let dir = fresh("duplicate");
        let peer_key = NodeIdentity::from_key([5; 32]);
        let peer_id = hex(&peer_key.node_id);
        let mut peer = Registry::sealed(peer_key);
        for lease_expiry_ms in [1_000, 2_000, 5_000] {
            let share = "ws-x".into();
            let claim = ServeClaim {
                node: peer_id.clone(),
                share,
                lease_expiry_ms,
                epoch: 1,
            };
            peer.append(Record::Serve(claim), &peer_id).unwrap();
        }
        let mut snap = peer.snapshot();
        let repeat = snap.records[1].clone();
        snap.records.insert(2, repeat.clone());
        BlobStore::new(&dir).save(&snap).unwrap();

        let boot = boot_at(dir.clone(), "gianni").unwrap();
        assert_eq!(boot.rejected, 0, "nothing quarantined");
        assert!(boot.set_aside.is_none(), "nothing unsigned");
        let serves = boot.registry.who_serves("ws-x", 3_000);
        assert_eq!(serves, Some(peer_id), "the third claim folds");
        let saved = BlobStore::new(&dir).load().unwrap();
        let held = saved
            .records
            .iter()
            .filter(|record| **record == repeat)
            .count();
        assert_eq!(held, 1, "and is saved once");
    }

    /// Plan Step 4.1b (`GladeNodeSigning.md` D8 (a)): an instance written
    /// before the step holds its records unsigned: here its presence, its
    /// home claim, its endpoint key's binding and a grant, under its id, and
    /// a record under the id its key had before plan Step 4.1a, which 4.1a's
    /// own set-aside would have taken. The first boot on this build writes
    /// every one, byte for byte, to a new `records.legacy-<date>.json`
    /// beside an older file of that name, which it does not overwrite. It
    /// mints its presence and the binding of the same endpoint key again,
    /// revoking nothing; each record in records.json then verifies, and names
    /// only the node. A second boot sets nothing aside and writes nothing.
    /// It does not reach the served store (`store.rs` and `claims.rs` do), or
    /// a crash between the writes.
    #[test]
    fn a_first_boot_sets_the_unsigned_records_aside_once_and_mints_them_signed() {
        use sha2::{Digest, Sha256};
        let dir = fresh("unsigned");
        let first = boot_at(dir.clone(), "gianni").unwrap();
        let (seed, node, endpoint) = (first.seed, first.node_id.clone(), first.endpoint_key());
        drop(first);
        let old = hex(&Sha256::digest(seed));
        let mut before = Registry::new();
        let presence = |node_id: &str| {
            let (node_id, operator) = (node_id.into(), "gianni".into());
            Record::Node(NodeRecord { node_id, operator })
        };
        before.append(presence(&old), &old).unwrap();
        before.append(presence(&node), &node).unwrap();
        let claim = ServeClaim {
            node: node.clone(),
            share: HOME.into(),
            lease_expiry_ms: 1,
            epoch: 1,
        };
        before.append(Record::Serve(claim), &node).unwrap();
        let binding = transport::sign_binding(&seed, &endpoint.endpoint_id, 1);
        before.append(Record::Transport(binding), &node).unwrap();
        let grant = crate::sysdata::CapabilityGrant {
            principal: "owner".into(),
            share: "ws-x".into(),
            verbs: vec!["read.*".into()],
        };
        before.append(Record::Grant(grant), &node).unwrap();
        let written = before.snapshot();
        BlobStore::new(&dir).save(&written).unwrap();
        let taken = dir.join(format!("records.legacy-{}.json", date_of(now_ms())));
        fs::write(&taken, "an older file").unwrap();

        let boot = boot_at(dir.clone(), "gianni").unwrap();
        let aside = boot
            .set_aside
            .as_ref()
            .expect("the unsigned records set aside");
        assert_eq!(aside.records, 5);
        assert_ne!(aside.file, taken);
        assert_eq!(
            fs::read(&taken).unwrap(),
            b"an older file",
            "not overwritten"
        );
        let held = SystemSnapshot::from_cbor(&cbor::decode(&fs::read(&aside.file).unwrap()));
        assert_eq!(held.records, written.records, "byte for byte");
        assert_eq!((boot.rejected, boot.rebound), (0, rebound(true, 0)));
        assert_eq!(boot.registry.nodes_of("gianni"), vec![node.clone()]);
        let fold = boot.registry.transport();
        let bound = fold.binds(&signing::public_key(&seed), &endpoint.endpoint_id, now_ms());
        assert_eq!(bound, transport::Bound::Live, "the same key, bound again");
        let saved: Vec<Op> = BlobStore::new(&dir)
            .load()
            .unwrap()
            .records
            .iter()
            .map(|bytes| Op::from_cbor(&cbor::decode(bytes)))
            .collect();
        assert!(saved.iter().all(|op| op.origin == node), "only the node");
        assert!(
            saved.iter().all(|op| envelope::verify(op).is_ok()),
            "all signed"
        );
        drop(boot);
        let records = fs::read(dir.join("records.json")).unwrap();
        let again = boot_at(dir.clone(), "gianni").unwrap();
        assert!(again.set_aside.is_none(), "once");
        drop(again);
        assert_eq!(
            fs::read(dir.join("records.json")).unwrap(),
            records,
            "nothing written"
        );
    }

    /// Plan Step 4.1b: a signed record whose signature fails at load, here a
    /// revocation with a byte of its record changed, is quarantined with its
    /// chain's suffix, as a chain break is, not set aside as unsigned; and a
    /// grant or revocation quarantined so leaves the grant fold unreadable
    /// (AZ-11), where the untouched instance's fold reads.
    #[test]
    fn a_record_whose_signature_fails_at_load_is_quarantined_and_closes_the_grant_fold() {
        let dir = fresh("forged");
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        let node = boot.node_id.clone();
        let mut registry = boot.registry.clone();
        drop(boot);
        let revocation = crate::sysdata::CapabilityRevocation {
            principal: "eve".into(),
            share: "ws-x".into(),
        };
        registry.append(Record::Revoke(revocation), &node).unwrap();
        let mut snap = registry.snapshot();
        BlobStore::new(&dir).save(&snap).unwrap();
        let clean = boot_at(dir.clone(), "gianni").unwrap();
        assert!(clean.registry.policy().is_some());
        drop(clean);
        let at = snap.records.len() - 1;
        let mut op = Op::from_cbor(&cbor::decode(&snap.records[at]));
        let e = op.payload.iter().position(|b| *b == b'e').unwrap();
        op.payload[e] = b'm';
        snap.records[at] = cbor::encode(&op.to_cbor());
        BlobStore::new(&dir).save(&snap).unwrap();

        let boot = boot_at(dir, "gianni").unwrap();
        assert!(
            boot.set_aside.is_none(),
            "signed, so not set aside as unsigned"
        );
        assert_eq!(boot.rejected, 1);
        assert!(boot.registry.policy_quarantined());
        assert_eq!(boot.registry.policy(), None);
    }

    /// Plan Step 4.1b's part 2's hardening: a record in a format this build
    /// does not know, here what a newer build might write (this build's
    /// envelope on a stream it does not know), refuses the boot with a
    /// message naming it, before anything is written: records.json is as it
    /// was, and no legacy file appears.
    #[test]
    fn a_boot_refuses_a_record_in_a_format_it_does_not_know_and_writes_nothing() {
        let dir = fresh("newer");
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        let (seed, node) = (boot.seed, boot.node_id.clone());
        let mut snap = boot.registry.snapshot();
        drop(boot);
        let newer = envelope::testing::newer(seed);
        snap.records.push(cbor::encode(&newer.to_cbor()));
        BlobStore::new(&dir).save(&snap).unwrap();
        let written = fs::read(dir.join("records.json")).unwrap();

        let err = boot_at(dir.clone(), "gianni").map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let named = format!(
            "holds a home record this build cannot read (dir.key-rotations of node {node} at seq 0)"
        );
        assert!(err.to_string().contains(&named), "{err}");
        assert_eq!(fs::read(dir.join("records.json")).unwrap(), written);
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let legacy = names.iter().any(|name| name.starts_with("records.legacy"));
        assert!(!legacy, "nothing set aside: {names:?}");
    }

    /// The persistence suite on records.json (owner, 2026-09-24): a
    /// records.json that is damaged, here cut in half, refuses the boot with
    /// a message naming it and what is wrong, before anything is written:
    /// records.json is as it was, and no legacy file appears. The wire codec
    /// panicked on it.
    #[test]
    fn a_boot_refuses_a_damaged_records_json_and_writes_nothing() {
        let dir = fresh("damaged");
        drop(boot_at(dir.clone(), "gianni").unwrap());
        let records = dir.join("records.json");
        let whole = fs::read(&records).unwrap();
        let torn = &whole[..whole.len() / 2];
        fs::write(&records, torn).unwrap();

        let err = boot_at(dir.clone(), "gianni").map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let named = format!(
            "{} cannot be read as a snapshot (a torn or unreadable item): it is damaged, or not a records.json",
            records.display()
        );
        assert!(err.to_string().starts_with(&named), "{err}");
        assert_eq!(fs::read(&records).unwrap(), torn, "written nothing");
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let legacy = names.iter().any(|name| name.starts_with("records.legacy"));
        assert!(!legacy, "nothing set aside: {names:?}");
    }

    /// The ruling's acceptance at boot: an instance whose records.json the
    /// build before this step wrote, keys 1 and 2 alone, boots with the same
    /// fold, nothing quarantined or set aside, and records.json read, not
    /// written; the boot's store then saves it as revision 2.
    #[test]
    fn an_instance_from_before_the_revision_boots_unchanged_as_revision_1() {
        use crate::records_file::RecordsFile;
        use glade_wire::cbor::Cbor;

        let dir = fresh("before-revision");
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        let fold = boot.registry.snapshot();
        drop(boot);
        let list = |items: &[Vec<u8>]| {
            let items = items.iter().map(|x| Cbor::Bytes(x.clone()));
            Cbor::Array(items.collect())
        };
        let (records, heads) = (list(&fold.records), list(&fold.heads));
        let old = cbor::encode(&Cbor::Map(vec![(1, records), (2, heads)]));
        let path = dir.join("records.json");
        fs::write(&path, &old).unwrap();

        let mut boot = boot_at(dir.clone(), "gianni").unwrap();
        assert_eq!((boot.rejected, boot.set_aside.is_none()), (0, true));
        assert_eq!(boot.registry.snapshot(), fold, "the same fold");
        assert_eq!(fs::read(&path).unwrap(), old, "read, not written");
        boot.store.save(&boot.registry.snapshot()).unwrap();
        let saved = RecordsFile::new(&dir).load().unwrap();
        assert_eq!(saved, Some((2, old)), "revision 2, keys 1 and 2 kept");
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

    fn rebound(minted: bool, revoked: usize) -> Rebound {
        Rebound { minted, revoked }
    }

    /// Plan Step 4.2 (F1): `endpoint.key` is a second 32-byte secret, whose
    /// key is not the node's, loaded at every boot, so the endpoint id is the
    /// same at each; a length other than 32 refuses the boot, naming the
    /// file.
    #[test]
    fn the_endpoint_key_is_a_second_secret_kept_across_boots() {
        let dir = fresh("endpoint-key");
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        let key = boot.endpoint_key();
        assert_ne!(transport::hex(&key.endpoint_id), boot.node_id);
        let held: [u8; 32] = fs::read(dir.join("endpoint.key"))
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(EndpointKey::from_seed(held).endpoint_id, key.endpoint_id);
        drop(boot);
        let again = boot_at(dir.clone(), "gianni").unwrap();
        assert_eq!(again.endpoint_key().endpoint_id, key.endpoint_id);
        drop(again);
        let file = fs::OpenOptions::new()
            .write(true)
            .open(dir.join("endpoint.key"));
        file.and_then(|file| file.set_len(31)).unwrap();
        let err = boot_at(dir, "gianni").map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().starts_with("endpoint.key is 31 bytes"),
            "{err}"
        );
    }

    /// Plan Step 4.2, section 5: the first boot binds the endpoint key in
    /// records.json; the next writes nothing; a boot after `endpoint.key` was
    /// moved aside binds the new key and revokes the old one; the old key
    /// restored refuses the boot, and records.json is not written.
    #[test]
    fn a_boot_binds_its_endpoint_key_and_revokes_a_replaced_one() {
        let dir = fresh("rebind");
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        let node = boot.identity().unwrap().node_id;
        let old = boot.endpoint_key().endpoint_id;
        assert_eq!(boot.rebound, rebound(true, 0));
        let fold = boot.registry.transport();
        assert_eq!(fold.binds(&node, &old, now_ms()), transport::Bound::Live);
        drop(boot);
        let saved = fs::read(dir.join("records.json")).unwrap();
        let again = boot_at(dir.clone(), "gianni").unwrap();
        assert_eq!(again.rebound, rebound(false, 0));
        drop(again);
        assert_eq!(
            fs::read(dir.join("records.json")).unwrap(),
            saved,
            "nothing written"
        );

        fs::rename(dir.join("endpoint.key"), dir.join("endpoint.key.old")).unwrap();
        let replaced = boot_at(dir.clone(), "gianni").unwrap();
        let new = replaced.endpoint_key().endpoint_id;
        assert_ne!(new, old);
        assert_eq!(replaced.rebound, rebound(true, 1));
        let fold = replaced.registry.transport();
        assert_eq!(fold.binds(&node, &old, now_ms()), transport::Bound::Revoked);
        assert_eq!(fold.binds(&node, &new, now_ms()), transport::Bound::Live);
        drop(replaced);

        let saved = fs::read(dir.join("records.json")).unwrap();
        fs::rename(dir.join("endpoint.key.old"), dir.join("endpoint.key")).unwrap();
        let err = boot_at(dir.clone(), "gianni").map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(dir.join("records.json")).unwrap(), saved);
    }

    /// Plan Step 4.2, section 6: an instance written before the step has a
    /// `node.key` and records.json but no `endpoint.key` and no binding. Its
    /// next boot mints the key and exactly one binding, revokes nothing, and
    /// sets nothing aside or re-mints presence.
    #[test]
    fn an_instance_from_before_the_step_binds_a_new_key_at_its_next_boot() {
        let dir = fresh("before-4-2");
        drop(boot_at(dir.clone(), "gianni").unwrap());
        let store = BlobStore::new(&dir);
        let mut snap = store.load().unwrap();
        let transport_record = |bytes: &Vec<u8>| {
            let glade_id = Op::from_cbor(&cbor::decode(bytes)).glade_id;
            glade_id.starts_with("dir.transport-")
        };
        snap.records.retain(|bytes| !transport_record(bytes));
        let mut store = store;
        store
            .save(&Registry::from_snapshot(&snap).0.snapshot())
            .unwrap();
        fs::remove_file(dir.join("endpoint.key")).unwrap();

        let boot = boot_at(dir.clone(), "gianni").unwrap();
        assert!(dir.join("endpoint.key").exists());
        assert_eq!(boot.rebound, rebound(true, 0));
        assert!(boot.set_aside.is_none());
        assert_eq!(boot.nodes_of_ops(), 1);
        let saved = BlobStore::new(&dir).load().unwrap();
        let bindings = saved.records.iter().filter(|bytes| transport_record(bytes));
        assert_eq!(bindings.count(), 1);
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

    /// The instance root is `GLADE_HOME` when given, else `$HOME/.glade`, else
    /// `./.glade`, from the values handed in; an instance lives under it.
    #[test]
    fn the_instance_root_is_glade_home_else_home_dot_glade() {
        let (glade_home, home) = (Some("/g".to_owned()), Some("/h".to_owned()));
        assert_eq!(instance_root(glade_home, home.clone()), Path::new("/g"));
        assert_eq!(instance_root(None, home), Path::new("/h").join(".glade"));
        assert_eq!(instance_root(None, None), Path::new(".").join(".glade"));
        let sys = Path::new("/r").join("sys");
        let named = |name| instance_dir(Path::new("/r"), Profile::Peer, name).unwrap();
        assert_eq!(named(None), sys.join("glade-peer"));
        assert_eq!(named(Some("n")), sys.join("n"));
    }

    /// F9 (the owner's ruling of 2026-09-27): an instance name matches
    /// `[A-Za-z0-9._-]{1,63}` and does not end in `.`, so `<root>/sys/<name>`
    /// is a directory in `<root>/sys`, one on every platform; the profiles'
    /// default names are such names, and a dot may begin a name or sit
    /// inside one. Any other is refused (`InvalidInput`), the message
    /// quoting what was given: one that climbs out, the empty name, a
    /// separator of either platform, a drive's colon, a space, a letter
    /// outside ASCII, a NUL, 64 characters, and (F9 (b)) a name ending in a
    /// dot, which Windows trims, even at 63 characters.
    #[test]
    fn an_instance_name_is_checked() {
        let root = Path::new("/r");
        let longest = "n".repeat(63);
        let names = ["grazel", "gwzit", "A.b_c-9", ".x", "..x", "x.y"];
        for name in names.into_iter().chain([longest.as_str()]) {
            let dir = named_instance(root, name);
            assert_eq!(dir.unwrap(), root.join("sys").join(name), "{name:?}");
        }
        for profile in [Profile::Local, Profile::Peer, Profile::Server] {
            let dir = instance_dir(root, profile, None).unwrap();
            assert_eq!(dir, root.join("sys").join(profile.default_name()));
        }
        let too_long = "n".repeat(64);
        let dotted = format!("{}.", "n".repeat(62));
        let trimmed = ["n.", "x.", "a..", "...", "x.y.", dotted.as_str()];
        let climbing = ["..", ".", "../x", "/r", "a/b", "a\\b", "c:x"];
        let others = ["", "a b", "é", "a\0", too_long.as_str()];
        for name in trimmed.into_iter().chain(climbing).chain(others) {
            let err = named_instance(root, name).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{name:?}");
            let said = format!(
                "--name {name:?}: an instance name must match [A-Za-z0-9._-]{{1,63}} and not end in a dot"
            );
            assert_eq!(err.to_string(), said);
            let unnamed = instance_dir(root, Profile::Local, Some(name));
            assert_eq!(unnamed.unwrap_err().to_string(), said);
        }
    }

    /// The owner's ruling of 2026-09-27: a name Windows reserves for a
    /// device, `con`, `prn`, `aux`, `nul`, `com1`-`com9` or `lpt1`-`lpt9`, is
    /// refused (`InvalidInput`) in any case, alone or before a dot, since
    /// there `con.txt` is `con`; the message quotes what was given. A name
    /// that only begins or ends as one does, or holds one after a dot, is a
    /// name.
    #[test]
    fn a_windows_device_name_is_refused() {
        let root = Path::new("/r");
        let alone = ["con", "PRN", "Aux", "nUl", "com1", "COM9", "lpt1", "LPT9"];
        let dotted = ["con.txt", "NUL.x", "aux.tar.gz", "Com5.a-b"];
        for name in alone.into_iter().chain(dotted) {
            let err = named_instance(root, name).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{name:?}");
            let said = format!(
                "--name {name:?}: an instance name must not be a Windows device name (con, prn, aux, nul, com1-com9, lpt1-lpt9) in any case, alone or before a dot"
            );
            assert_eq!(err.to_string(), said);
            let unnamed = instance_dir(root, Profile::Local, Some(name));
            assert_eq!(unnamed.unwrap_err().to_string(), said);
        }
        let names = [
            "cons", "icon", "com", "com10", "lpt1x", "con-x", "nul_1", "auxx.txt", "x.con", ".nul",
        ];
        for name in names {
            let dir = named_instance(root, name);
            assert_eq!(dir.unwrap(), root.join("sys").join(name), "{name:?}");
        }
    }

    /// F9: a boot named outside `<root>/sys`, here `../../outside`, is
    /// refused before anything is written: nothing appears under the root or
    /// beside it. Before the check, the boot made its instance beside the
    /// root, at `<root>/../outside`, and a `sys` in the root on its way.
    #[test]
    fn a_boot_named_outside_sys_is_refused_and_writes_nothing() {
        let base = fresh("named-outside");
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        let named = Some("../../outside");
        let booted = boot(&root, Profile::Local, named, None, None, LEASE_TTL_MS);
        let err = booted.map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
        let names = |dir: &Path| -> Vec<String> {
            let entries = fs::read_dir(dir).unwrap();
            let name = |entry: io::Result<fs::DirEntry>| entry.unwrap().file_name();
            entries
                .map(|entry| name(entry).to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(&base), ["root"], "nothing beside the root");
        assert_eq!(names(&root), Vec::<String>::new(), "nothing under it");
        fs::remove_dir_all(&base).unwrap();
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

        /// Plan Step 4.2: `endpoint.key` is created 0600, and a group-readable
        /// one refuses the boot, naming the file, as `node.key` does.
        #[test]
        fn endpoint_key_is_0600_and_group_readable_is_refused() {
            let dir = fresh("endpoint-perms");
            drop(boot_at(dir.clone(), "gianni").unwrap());
            let path = dir.join("endpoint.key");
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            let err = boot_at(dir, "gianni").map(|_| ()).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
            assert!(
                err.to_string().starts_with("endpoint.key is group"),
                "{err}"
            );
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
