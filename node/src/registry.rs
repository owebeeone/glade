//! The system-data seam (GDL-036, Lane R step 1) — two traits that hide two
//! deferred implementations, so the later swap is an impl detail:
//!
//! - [`RegistryApi`] hides **where answers come from**. Reads are
//!   queries-over-fold (`who_serves`/`replicas_of`/`grants_for`/`nodes_of`),
//!   never `get_config`; writes are record APPENDS carrying **origin
//!   attribution** even in blob-land (`append(rec, origin)`), never
//!   `set_config`. A real home-share-fold impl (WD P2) must slot in with no
//!   caller changing.
//! - [`StoreApi`] hides **how a node persists**. The interim engine keeps the
//!   whole system state as ONE taut [`SystemSnapshot`] blob, rewritten on
//!   change; a SQLite engine slots in later behind the SAME trait (SQLite is a
//!   store engine, never the replication mechanism).
//!
//! Records are wire [`Op`]s (the ONE op envelope) whose payload is a taut
//! record (`sysdata.rs`, WD §2). A snapshot is a cached fold + heads
//! (SubstrateV1 §2): loading it is verify-as-ingest from a carrier named "the
//! disk" — the SAME per-origin chain checks the wire store runs (`store.rs`),
//! so hardening s-sync hardens boot for free.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use glade_wire::cbor::{self, DecodeError};
use glade_wire::generated::{Head, Op, Shape, StreamHeads};

use crate::chain::op_hash;
use crate::checkpoint::{self, Against, Checkpoint, Floor, Placement};
use crate::envelope::{self, Refused};
use crate::grants::Policy;
use crate::peer::NodeIdentity;
use crate::records_file::{self, FileError, RecordsFile};
use crate::sysdata::{
    BindingDecl, BindingRetraction, CapabilityGrant, CapabilityRevocation, ChainCheckpoint,
    NodeRecord, NodeRecoveryKey, NodeTransportBinding, NodeTransportRevocation, PrincipalRecord,
    ServeClaim, ServiceDefinition, SystemSnapshot, WorkspaceEntry,
};
use crate::transport::{self, TransportFold};

/// The home share — the user-scale system declaration space (WD §2). All
/// directory records live here.
pub const HOME: &str = "home";

// The per-kind stream ids (glade ids) inside the home share. The record kind is
// the stream; the payload is the taut record. This is the `dir.workspaces`
// glade id the boot/discovery traces subscribe to.
pub const G_NODES: &str = "dir.nodes";
pub const G_WORKSPACES: &str = "dir.workspaces";
pub const G_CLAIMS: &str = "dir.claims";
pub const G_GRANTS: &str = "dir.grants";
pub const G_REVOCATIONS: &str = "dir.revocations";
// App declaration records (GDL-037): what an <app>.glade file registers.
pub const G_BINDINGS: &str = "dir.bindings";
pub const G_SERVICES: &str = "dir.services";
// Binding retractions (R9(a)): a binding line an app file no longer declares.
// Folded with dir.bindings — together, the binding family.
pub const G_BINDING_RETRACTIONS: &str = "dir.binding-retractions";
// Principals minimal (GLP-0006 P0.S7; the stream GDL-038 names): identity as
// data — session Hellos auto-append unknown principals; nothing enforced.
pub const G_PRINCIPALS: &str = "dir.principals";
// The transport-key binding (plan Step 4.2): a node's iroh endpoint keys,
// bound and revoked in its own chain (`transport.rs`).
pub const G_TRANSPORT_BINDINGS: &str = "dir.transport-bindings";
pub const G_TRANSPORT_REVOCATIONS: &str = "dir.transport-revocations";
// The recovery key (plan Step 4.1c): the public half a node commits to, in its
// own chain (`recovery.rs`).
pub const G_RECOVERY_KEYS: &str = "dir.recovery-keys";
// Signed checkpoints (plan Step 4.5c): a register of each node's own, one for
// each chain it compacts (`checkpoint.rs`).
pub const G_CHECKPOINTS: &str = "dir.checkpoints";

/// One home-share record (WD §2). Each variant folds by its own semantics; the
/// enum is the append surface so `append` stays typed and the glade-id/shape
/// wiring is not a caller concern.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Node(NodeRecord),
    Workspace(WorkspaceEntry),
    Serve(ServeClaim),
    Grant(CapabilityGrant),
    Revoke(CapabilityRevocation),
    Binding(BindingDecl),
    Retract(BindingRetraction),
    Service(ServiceDefinition),
    Principal(PrincipalRecord),
    Transport(NodeTransportBinding),
    TransportRevoke(NodeTransportRevocation),
    Recovery(NodeRecoveryKey),
    Checkpoint(ChainCheckpoint),
}

impl Record {
    /// The stream (glade id) this record kind lives on.
    pub fn glade_id(&self) -> &'static str {
        match self {
            Record::Node(_) => G_NODES,
            Record::Workspace(_) => G_WORKSPACES,
            Record::Serve(_) => G_CLAIMS,
            Record::Grant(_) => G_GRANTS,
            Record::Revoke(_) => G_REVOCATIONS,
            Record::Binding(_) => G_BINDINGS,
            Record::Retract(_) => G_BINDING_RETRACTIONS,
            Record::Service(_) => G_SERVICES,
            Record::Principal(_) => G_PRINCIPALS,
            Record::Transport(_) => G_TRANSPORT_BINDINGS,
            Record::TransportRevoke(_) => G_TRANSPORT_REVOCATIONS,
            Record::Recovery(_) => G_RECOVERY_KEYS,
            Record::Checkpoint(_) => G_CHECKPOINTS,
        }
    }

    /// Is this a POLICY record? Policy records fail CLOSED on load (AZ-11): an
    /// unparseable/broken policy op is dropped, never leniently kept, and the
    /// grant fold of a load that dropped one is unreadable (plan Step 4.3).
    pub fn is_policy(glade_id: &str) -> bool {
        matches!(glade_id, G_GRANTS | G_REVOCATIONS)
    }

    /// Is `glade_id` a stream of the binding family, folded as one
    /// (`dir.bindings` + `dir.binding-retractions`)?
    fn is_binding_family(glade_id: &str) -> bool {
        matches!(glade_id, G_BINDINGS | G_BINDING_RETRACTIONS)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let c = match self {
            Record::Node(r) => r.to_cbor(),
            Record::Workspace(r) => r.to_cbor(),
            Record::Serve(r) => r.to_cbor(),
            Record::Grant(r) => r.to_cbor(),
            Record::Revoke(r) => r.to_cbor(),
            Record::Binding(r) => r.to_cbor(),
            Record::Retract(r) => r.to_cbor(),
            Record::Service(r) => r.to_cbor(),
            Record::Principal(r) => r.to_cbor(),
            Record::Transport(r) => r.to_cbor(),
            Record::TransportRevoke(r) => r.to_cbor(),
            Record::Recovery(r) => r.to_cbor(),
            Record::Checkpoint(r) => r.to_cbor(),
        };
        cbor::encode(&c)
    }
}

/// Ingest rejection — the verify-as-ingest failures (s-sync Y3), reused for both
/// live appends and disk load. A rejected op and its suffix are excluded from
/// the fold; the fold stays a pure function of the valid op-set.
/// `Equivocation` is a different op at a position the chain already holds, a
/// fork; the same op again is no error but [`Ingested::Duplicate`].
/// `Unverified` is an op a sealed registry was handed that does not verify
/// (plan Step 4.1b), and `NotOurs` an append a sealed registry was asked to
/// make under another node's origin. `Malformed` is an op an unsealed
/// registry was handed whose record `cbor::try_decode` refuses (F15b).
/// `Rewrite` is a checkpoint whose base moves back from the one held, or
/// names another hash there (plan Step 4.5c).
#[derive(Debug, PartialEq)]
pub enum RegistryError {
    Gap { expected: i64, got: i64 },
    ChainBreak { origin: String, seq: i64 },
    Equivocation { origin: String, seq: i64 },
    Unverified { origin: String, why: Refused },
    NotOurs { origin: String },
    Malformed { origin: String, why: DecodeError },
    Rewrite { origin: String, seq: i64 },
}

/// Where an ingested op landed: appended to its chain, or a checkpoint placed;
/// already held there byte for byte, a re-delivery that changes nothing (the
/// wire store's `Append`, `store.rs`); or covered, at or below its chain's
/// floor, or a checkpoint older than the one held, seen and not held (plan
/// Step 4.5c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingested {
    Appended,
    Duplicate,
    Covered,
}

// ============================================================================
// StoreApi — how a node persists (the swappable engine).
// ============================================================================

/// Persist the whole system state. The trait is deliberately the whole-blob
/// shape so a SQLite (or any) engine can re-implement it without any caller
/// changing — `load`/`save` a [`SystemSnapshot`], nothing else. Nothing above
/// this trait knows files exist. A snapshot here is a fold and its heads: its
/// `revision` is the engine's own, which `load` leaves unset and `save`
/// ignores.
pub trait StoreApi {
    fn load(&self) -> io::Result<SystemSnapshot>;
    fn save(&mut self, snap: &SystemSnapshot) -> io::Result<()>;
}

/// The interim engine: the whole snapshot as one taut message on disk
/// (`records.json`), rewritten tmp+rename (crash-atomic) through
/// [`RecordsFile`], which keeps its revision. This IS the degenerate-sync
/// artifact a connecting peer would ingest.
///
/// At-rest bytes are canonical CBOR of the [`SystemSnapshot`] (see the module
/// note): hashing == at-rest, so verify-as-ingest is uniform. The spec's
/// JSON-text rendering is a later cosmetic — the seam does not depend on it.
///
/// A load reads the file with checked heads, so a damaged one fails as
/// `InvalidData`, never a panic, and the handle keeps the revision it read. A
/// save is a compare-and-swap against the revision this handle last read or
/// wrote: it returns once the snapshot is synced and renamed into place with
/// the next revision, and the rename synced with its directory (plan Step
/// 4.4), and it fails, writing nothing, if another handle saved meanwhile. A
/// handle that has read nothing saves over whatever records.json holds.
pub struct BlobStore {
    file: RecordsFile,
    seen: Mutex<Seen>,
}

/// What a [`BlobStore`] handle knows of records.json's revision.
#[derive(Clone, Copy)]
enum Seen {
    /// Nothing yet, or nothing since a save whose outcome is unknown: its next
    /// save goes over whatever records.json holds.
    Nothing,
    /// That there is no records.json.
    Absent,
    /// The revision it last read or wrote.
    At(u64),
}

impl BlobStore {
    /// A blob engine writing `records.json` under `dir`.
    pub fn new(dir: impl AsRef<Path>) -> BlobStore {
        let (file, seen) = (RecordsFile::new(dir), Mutex::new(Seen::Nothing));
        BlobStore { file, seen }
    }
}

impl StoreApi for BlobStore {
    fn load(&self) -> io::Result<SystemSnapshot> {
        let path = self.file.path();
        let (seen, snap) = match self.file.load().map_err(|e| failed(path, e))? {
            None => (Seen::Absent, SystemSnapshot::default()),
            Some((revision, bytes)) => {
                let snap = records_file::decode(&bytes);
                let snap = snap.map_err(|why| failed(path, FileError::Corrupt(why)))?;
                (Seen::At(revision), snap)
            }
        };
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = seen;
        Ok(snap)
    }

    fn save(&mut self, snap: &SystemSnapshot) -> io::Result<()> {
        let path = self.file.path();
        let seen = self.seen.get_mut().unwrap_or_else(PoisonError::into_inner);
        let expected = match *seen {
            Seen::At(revision) => Some(revision),
            Seen::Absent => None,
            Seen::Nothing => self.file.revision().map_err(|e| failed(path, e))?,
        };
        let snapshot = records_file::encode(snap);
        let answer = self.file.compare_exchange(expected, &snapshot);
        match &answer {
            Ok(revision) => *seen = Seen::At(*revision),
            Err(FileError::OutcomeUnknown(_)) => *seen = Seen::Nothing,
            Err(_) => {}
        }
        answer.map(|_| ()).map_err(|e| failed(path, e))
    }
}

/// `e`, a failed load or save of `path`, as the error the engine answers: a
/// damaged records.json names itself and the way out, and an I/O failure
/// keeps its kind.
fn failed(path: &Path, e: FileError) -> io::Error {
    let file = path.display();
    match e {
        FileError::Unavailable(e) | FileError::OutcomeUnknown(e) => e,
        FileError::Corrupt(why) => io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{file} cannot be read as a snapshot ({why}): it is damaged, or not a records.json; move it aside to start without it"
            ),
        ),
        FileError::Conflict { .. } => io::Error::other(format!("{file}: {e}: another handle saved it")),
        FileError::Exhausted | FileError::NotASnapshot => io::Error::other(format!("{file}: {e}")),
    }
}

// A rename is durable once its directory is synced. On Unix that is a sync of
// a handle opened on the directory; std opens no directory handle elsewhere,
// so there the rename is not synced. Each platform's branch is one braced
// module, so the condition encloses the whole section.
#[cfg(unix)]
pub(crate) mod entry_sync {
    use std::path::Path;

    pub(crate) fn sync(dir: &Path) -> std::io::Result<()> {
        std::fs::File::open(dir)?.sync_all()
    }
}

#[cfg(not(unix))]
pub(crate) mod entry_sync {
    use std::path::Path;

    pub(crate) fn sync(_dir: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

/// An in-memory engine — stands in for the "SQLite / op-granular fold later"
/// engine in the conformance gate: a DIFFERENT StoreApi impl that must be
/// behaviorally indistinguishable through the trait. Also the browser twin's
/// shape (GC-4, one seam two runtimes).
#[derive(Default)]
pub struct MemStore {
    snap: SystemSnapshot,
}

impl StoreApi for MemStore {
    fn load(&self) -> io::Result<SystemSnapshot> {
        Ok(self.snap.clone())
    }
    fn save(&mut self, snap: &SystemSnapshot) -> io::Result<()> {
        self.snap = snap.clone();
        Ok(())
    }
}

// ============================================================================
// RegistryApi — where answers come from (queries over a fold).
// ============================================================================

/// Reads are queries-over-fold; writes are attributed appends. A fold-backed
/// impl (op-granular home-share sync, WD P2) slots in behind this unchanged.
pub trait RegistryApi {
    /// Append a record as THIS node's attributed op. `origin` rides every
    /// record from day one so migration to per-origin logs is mechanical.
    fn append(&mut self, rec: Record, origin: &str) -> Result<(), RegistryError>;

    /// Which node currently serves `workspace`, at the reader's clock `now_ms`.
    /// Lease expiry is evaluated at read time, never inside the fold; the
    /// highest live epoch wins, and at an equal epoch the lower node id
    /// ([`rank_claims`]).
    fn who_serves(&self, workspace: &str, now_ms: i64) -> Option<String>;

    /// Eligible replica nodes for `share` (WorkspaceEntry.eligible_hosts, LWW).
    fn replicas_of(&self, share: &str) -> Vec<String>;

    /// Verbs granted to `principal` on `share` — set-union, revocation wins.
    fn grants_for(&self, principal: &str, share: &str) -> Vec<String>;

    /// The live binding declarations (R9(a)) — the [`BindingFold`] of
    /// `dir.bindings` and `dir.binding-retractions`: per glade id the newest
    /// live declaration, in glade-id order.
    fn bindings_of(&self) -> Vec<BindingDecl>;

    /// Nodes operated by `operator` (NodeRecord set-union).
    fn nodes_of(&self, operator: &str) -> Vec<String>;

    /// The cached fold + heads — for `StoreApi::save` / degenerate sync.
    fn snapshot(&self) -> SystemSnapshot;
}

/// The interim RegistryApi: an in-memory op-set materialised from a snapshot,
/// appends applied in memory. Same per-origin chain discipline as the wire
/// store, so the disk gets no more trust than any peer. A clone is the staged
/// copy [`Registry::accept`] changes before anything is saved.
///
/// A booted node's registry is sealed (plan Step 4.1b): it appends as the
/// node alone, each record in a signed envelope (`envelope.rs`), and takes in
/// only ops that verify. An unsealed one keeps bare records, as tests and the
/// journeys' in-memory record host use it.
#[derive(Clone, Default)]
pub struct Registry {
    /// The valid op-set, in ingest order. The fold is a pure function of this.
    ops: Vec<Op>,
    /// Per (glade_id, origin) chain tip: (last_seq, last_hash) — for assigning
    /// the next append's seq/prev and for chain-continuity checks on ingest.
    /// A chain with a floor and no op above it has the floor as its tip.
    tips: BTreeMap<(String, String), (i64, [u8; 32])>,
    /// The register (plan Step 4.5c): per (stream, origin), the newest
    /// checkpoint held, and the floor it gives that chain. Its ops are held
    /// here, not in `ops`, and a snapshot lists them first.
    checkpoints: BTreeMap<(String, String), (Op, Floor)>,
    /// How many ops and checkpoints this registry has taken, which
    /// [`Registry::accept`] saves on: a checkpoint can leave the fold no
    /// longer, or shorter.
    changes: u64,
    /// Whether the load that built this registry quarantined a grant or a
    /// revocation ([`Registry::from_snapshot`]), so its grant fold cannot be
    /// read ([`Registry::policy`]).
    policy_quarantined: bool,
    /// The node this registry appends as and seals for, when it is sealed.
    seal: Option<NodeIdentity>,
}

impl Registry {
    /// A fresh, empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// A fresh, empty registry sealed as `identity` (plan Step 4.1b).
    pub fn sealed(identity: NodeIdentity) -> Registry {
        let seal = Some(identity);
        Registry {
            seal,
            ..Registry::default()
        }
    }

    /// Materialise a registry from a snapshot — verify-as-ingest per class-2
    /// (s-sync Y2): chain continuity + seq monotonicity per origin. A rejected
    /// op and its chain suffix are excluded (Y3); policy records fail CLOSED.
    /// A record held twice is taken once: the repeat is a duplicate, which
    /// quarantines nothing and leaves the rest of its chain loading.
    /// Returns the count of quarantined (rejected) records as load evidence.
    /// A quarantined grant or revocation leaves the grant fold unreadable
    /// ([`Registry::policy`]).
    pub fn from_snapshot(snap: &SystemSnapshot) -> (Registry, usize) {
        Registry::new().load(snap)
    }

    /// [`Registry::from_snapshot`], sealed as `identity` (plan Step 4.1b), as
    /// a booted node loads records.json: a record that does not verify is
    /// quarantined, as a chain break is.
    pub fn from_snapshot_as(snap: &SystemSnapshot, identity: NodeIdentity) -> (Registry, usize) {
        Registry::sealed(identity).load(snap)
    }

    fn load(self, snap: &SystemSnapshot) -> (Registry, usize) {
        let mut reg = self;
        let mut rejected = 0usize;
        // Track chains whose tail is poisoned so the suffix is dropped too.
        let mut poisoned: BTreeMap<(String, String), bool> = BTreeMap::new();
        let mut ops = Vec::new();
        for (at, bytes) in snap.records.iter().enumerate() {
            // An op that cannot be read may be on any stream, a grant's or a
            // revocation's, so the grant fold fails closed (F15b).
            match envelope::decode_op(bytes) {
                Ok(op) => ops.push(op),
                Err(why) => {
                    eprintln!("{}", envelope::unreadable_op("quarantined", at, why));
                    rejected += 1;
                    reg.policy_quarantined = true;
                }
            }
        }
        // Two passes (plan Step 4.5c): checkpoints first, whatever the order,
        // so that each chain they fold starts at its floor.
        let (checkpoints, rest): (Vec<Op>, Vec<Op>) =
            ops.into_iter().partition(|op| op.glade_id == G_CHECKPOINTS);
        for op in checkpoints.into_iter().chain(rest) {
            let chain = (op.glade_id.clone(), op.origin.clone());
            let policy = Record::is_policy(&op.glade_id);
            if *poisoned.get(&chain).unwrap_or(&false) {
                rejected += 1; // suffix of an already-rejected op
                reg.policy_quarantined |= policy;
                continue;
            }
            match reg.ingest(op) {
                Ok(_) => {}
                Err(_) => {
                    rejected += 1;
                    reg.policy_quarantined |= policy;
                    poisoned.insert(chain, true);
                }
            }
        }
        (reg, rejected)
    }

    /// The grant fold the serve paths check (plan Step 4.3): every grant and
    /// every revocation this registry holds, folded as `grants_for` folds
    /// them. `None` when its load quarantined a grant or a revocation: a
    /// revocation set aside would let its grant stand, so the fold fails
    /// closed rather than answer from what is left (AZ-11).
    pub fn policy(&self) -> Option<Policy> {
        if self.policy_quarantined {
            return None;
        }
        // A grant or a revocation that cannot be read fails it closed too.
        let mut policy = Policy::default();
        for o in self.fold_iter(G_GRANTS) {
            let grant = envelope::record(o, CapabilityGrant::from_cbor).ok()?;
            policy.grant(&grant.principal, &grant.share, grant.verbs);
        }
        for o in self.fold_iter(G_REVOCATIONS) {
            let revocation = envelope::record(o, CapabilityRevocation::from_cbor).ok()?;
            policy.revoke(&revocation.principal, &revocation.share);
        }
        Some(policy)
    }

    /// Whether the load that built this registry quarantined a grant or a
    /// revocation, which leaves [`Registry::policy`] unreadable.
    pub fn policy_quarantined(&self) -> bool {
        self.policy_quarantined
    }

    /// Ingest a fully-formed op with per-origin chain checks (the shared
    /// verify path for both live appends and disk load, and for the ops the
    /// assembly's record host is handed, `assembly::Records::ingest`). An op
    /// the chain already holds, byte for byte, is a duplicate: taken as held,
    /// and nothing changes (plan Step 4.4; owner, 2026-09-24). A sealed
    /// registry takes an op only if it verifies (plan Step 4.1b).
    pub(crate) fn ingest(&mut self, op: Op) -> Result<Ingested, RegistryError> {
        if self.seal.is_some() {
            if let Err(why) = envelope::verify(&op) {
                let origin = op.origin;
                return Err(RegistryError::Unverified { origin, why });
            }
        } else if let Err(why) = cbor::try_decode(&envelope::record_bytes(&op.payload)) {
            // An unsealed registry does not verify, so no kind check reads
            // the record: it refuses one its folds could not read (F15b).
            let origin = op.origin;
            return Err(RegistryError::Malformed { origin, why });
        }
        self.link(op)
    }

    /// The chain checks of [`Registry::ingest`], for an op verified there or
    /// built here. A chain with a floor (plan Step 4.5c) takes an op at or
    /// below it as covered, and one at its base with another hash as a fork.
    fn link(&mut self, op: Op) -> Result<Ingested, RegistryError> {
        if op.glade_id == G_CHECKPOINTS {
            return self.place(op);
        }
        let chain = (op.glade_id.clone(), op.origin.clone());
        if let Some(&(_, floor)) = self.checkpoints.get(&chain) {
            match checkpoint::against(floor, &op) {
                Against::Above => {}
                Against::Covered => return Ok(Ingested::Covered),
                Against::Fork => {
                    let (origin, seq) = (op.origin, op.seq);
                    return Err(RegistryError::Equivocation { origin, seq });
                }
            }
        }
        if let Some(&(last_seq, last_hash)) = self.tips.get(&chain) {
            if op.seq <= last_seq {
                // At or below the tip: the same op again is a duplicate, as
                // the wire store takes it; a different op there is a fork.
                if self.holds(&op) {
                    return Ok(Ingested::Duplicate);
                }
                return Err(RegistryError::Equivocation { origin: op.origin, seq: op.seq });
            }
            if op.seq != last_seq + 1 {
                return Err(RegistryError::Gap { expected: last_seq + 1, got: op.seq });
            }
            match &op.prev {
                Some(prev) if prev.as_slice() != last_hash => {
                    return Err(RegistryError::ChainBreak { origin: op.origin, seq: op.seq });
                }
                _ => {}
            }
        } else if op.seq != 0 {
            return Err(RegistryError::Gap { expected: 0, got: op.seq });
        }
        let hash = op_hash(&op);
        self.tips.insert(chain, (op.seq, hash));
        self.ops.push(op);
        self.changes += 1;
        Ok(Ingested::Appended)
    }

    /// Place `op`, a checkpoint, in the register (plan Step 4.5c): its own
    /// check, then [`checkpoint::place`]. The op this registry holds at its
    /// base must hash to it, or it is a fork. Placed, it replaces the one
    /// held, and the chain it names keeps no op at or below its base.
    fn place(&mut self, op: Op) -> Result<Ingested, RegistryError> {
        let (origin, seq) = (op.origin.clone(), op.seq);
        let unverified = |why| {
            let origin = origin.clone();
            RegistryError::Unverified { origin, why }
        };
        let Checkpoint { stream, floor } = checkpoint::check(&op).map_err(unverified)?;
        let chain = (stream, origin.clone());
        let held = self.checkpoints.get(&chain).map(|(held, at)| (held, *at));
        match checkpoint::place((&op, floor), held) {
            Placement::Placed => {}
            Placement::Duplicate => return Ok(Ingested::Duplicate),
            Placement::Seen => return Ok(Ingested::Covered),
            Placement::Fork => return Err(RegistryError::Equivocation { origin, seq }),
            Placement::ChainBreak => return Err(RegistryError::ChainBreak { origin, seq }),
            Placement::Rewrite => return Err(RegistryError::Rewrite { origin, seq }),
        }
        let base = floor.0;
        let named = |held: &Op| held.glade_id == chain.0 && held.origin == chain.1;
        let at_base = self.ops.iter().find(|held| named(held) && held.seq == base);
        if at_base.is_some_and(|held| checkpoint::against(floor, held) == Against::Fork) {
            return Err(RegistryError::Equivocation { origin, seq: base });
        }
        self.ops.retain(|held| !named(held) || held.seq > base);
        // The chain's tip is its floor while nothing above the floor is held.
        let tip = self.tips.entry(chain.clone()).or_insert(floor);
        if tip.0 <= base {
            *tip = floor;
        }
        let own = (G_CHECKPOINTS.to_string(), origin);
        self.tips.insert(own, (seq, op_hash(&op)));
        self.checkpoints.insert(chain, (op, floor));
        self.changes += 1;
        Ok(Ingested::Appended)
    }

    /// Does the fold hold `op` byte for byte (the same op hash) at its chain
    /// position? A scan of the op-set: a re-delivery is rare, and the fold
    /// keeps no index by seq.
    fn holds(&self, op: &Op) -> bool {
        let at = |held: &&Op| {
            held.glade_id == op.glade_id && held.origin == op.origin && held.seq == op.seq
        };
        let held = self.ops.iter().find(at);
        held.is_some_and(|held| op_hash(held) == op_hash(op))
    }

    /// Is there a NodeRecord for `node_id` in the fold? The boot ladder uses
    /// this for the class-1↔class-2 identity match: our derived NodeId MUST
    /// correspond to our own NodeRecord (else it is a first boot, or — if the
    /// key was replaced — identity loss, and the mesh rejects us).
    pub fn has_node(&self, node_id: &str) -> bool {
        self.fold_iter(G_NODES)
            .into_iter()
            .filter_map(|o| envelope::folded(o, NodeRecord::from_cbor))
            .any(|record| record.node_id == node_id)
    }

    /// Decoded records of one kind, in deterministic (origin, seq) order —
    /// the fold's input. Time never enters here.
    fn fold_iter(&self, glade_id: &str) -> Vec<&Op> {
        let mut v: Vec<&Op> = self.ops.iter().filter(|o| o.glade_id == glade_id).collect();
        v.sort_by(|a, b| (a.origin.as_str(), a.seq).cmp(&(b.origin.as_str(), b.seq)));
        v
    }

    /// The ops `origin`'s chain on `glade_id` holds, in seq order: from its
    /// floor, once a checkpoint gave it one (plan Step 4.5c).
    pub(crate) fn chain(&self, glade_id: &str, origin: &str) -> Vec<&Op> {
        let ops = self.fold_iter(glade_id).into_iter();
        ops.filter(|op| op.origin == origin).collect()
    }

    /// Append `rec` under `origin`'s chain and hand back the built op — the
    /// runtime directory-write path (`claims.rs`) seeds/fans/pushes the SAME
    /// bytes it persisted; `RegistryApi::append` delegates here. A sealed
    /// registry appends under its own node's origin alone, and seals the
    /// record once the op is built (plan Step 4.1b).
    pub fn append_returning(&mut self, rec: Record, origin: &str) -> Result<Op, RegistryError> {
        let ours = self.seal.map(|me| transport::hex(&me.node_id));
        if ours.is_some_and(|ours| ours != origin) {
            let origin = origin.into();
            return Err(RegistryError::NotOurs { origin });
        }
        let glade_id = rec.glade_id();
        let chain = (glade_id.to_string(), origin.to_string());
        let (seq, prev) = match self.tips.get(&chain) {
            Some(&(last_seq, last_hash)) => (last_seq + 1, Some(last_hash.to_vec())),
            None => (0, None),
        };
        // The binding family folds newest-wins ACROSS its two streams, so its
        // lamport is one clock over both (see `next_binding_lamport`); every
        // other kind keeps its chain seq.
        let lamport = if Record::is_binding_family(glade_id) { self.next_binding_lamport() } else { seq };
        let mut op = Op {
            share: HOME.into(),
            glade_id: glade_id.into(),
            key: vec![],
            origin: origin.into(),
            seq,
            prev,
            lamport,
            refs: vec![],
            shape: Shape::Log,
            payload: rec.encode(),
        };
        if let Some(me) = &self.seal {
            op.payload = envelope::seal(me, &op);
        }
        self.link(op.clone())?;
        Ok(op)
    }

    /// The next lamport on the binding family: one past the highest either
    /// of its streams holds, from any origin, so an append is newer than
    /// every binding record already here — whichever stream it is on. With
    /// one origin and no retraction this is the record's own seq, which is
    /// what every binding record written before R9 carries, so no envelope
    /// that existed before moves.
    fn next_binding_lamport(&self) -> i64 {
        self.ops
            .iter()
            .filter(|o| Record::is_binding_family(&o.glade_id))
            .map(|o| o.lamport + 1)
            .max()
            .unwrap_or(0)
    }

    /// The recovery key `node` has committed to (plan Step 4.1c;
    /// `GladeNodeSigning.md` D10 (a)), in hex: the first `NodeRecoveryKey` in
    /// its own chain that names it. `None` until it has committed one.
    pub fn recovery_key(&self, node: &str) -> Option<String> {
        let own = self.fold_iter(G_RECOVERY_KEYS);
        let own = own.into_iter().filter(|o| o.origin == node);
        let keys = own.filter_map(|o| envelope::folded(o, NodeRecoveryKey::from_cbor));
        let ours = keys.filter(|key| key.node == node);
        ours.map(|key| key.recovery_key).next()
    }

    /// The transport-binding fold of this registry's records (plan Step 4.2):
    /// which endpoint keys its nodes have bound and revoked.
    pub fn transport(&self) -> TransportFold {
        TransportFold::over(&self.ops)
    }

    /// Is a byte-identical record already in the fold? The diff basis for
    /// idempotent minting — the same rule `appdecl::register` applies. It
    /// compares the record an op carries, never its envelope, which differs
    /// at every append (plan Step 4.1b).
    pub fn contains(&self, glade_id: &str, record: &[u8]) -> bool {
        let held = |o: &Op| o.glade_id == glade_id && envelope::record_bytes(&o.payload) == record;
        self.ops.iter().any(held)
    }

    /// Durable acceptance (slice profile SP-L1, plan Step 4.4): `change` runs
    /// on a staged copy of this fold, the copy's snapshot is saved through
    /// `store`, and only then does the copy become the fold. A change the
    /// registry refuses, or a save that fails, leaves the fold and the stored
    /// snapshot as they were: nothing unsaved is read or sent, and a retry is
    /// judged against what was accepted. A change that takes nothing, a
    /// duplicate or a covered op among them, saves nothing. It counts what
    /// was taken, not ops, since a checkpoint can leave the fold no longer
    /// (plan Step 4.5c). The copy costs what the save does, one pass over
    /// every op.
    pub fn accept<T, E: From<io::Error>>(
        &mut self,
        store: &mut dyn StoreApi,
        change: impl FnOnce(&mut Registry) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut staged = self.clone();
        let out = change(&mut staged)?;
        if staged.changes == self.changes {
            return Ok(out);
        }
        store.save(&staged.snapshot())?;
        *self = staged;
        Ok(out)
    }
}

/// How two live claims on one share rank: `Greater` when `a` holds the share
/// over `b`. The higher epoch holds, and at an equal epoch the lower node id.
/// Two nodes that each claim a share before either has the other's claim mint
/// the same epoch, so every fold that names a share's holder ranks by this,
/// never by the order it reads the claims in: [`RegistryApi::who_serves`] and
/// the served store's `mesh::who_serves`.
pub(crate) fn rank_claims(a: &ServeClaim, b: &ServeClaim) -> std::cmp::Ordering {
    a.epoch.cmp(&b.epoch).then_with(|| b.node.cmp(&a.node))
}

impl RegistryApi for Registry {
    fn append(&mut self, rec: Record, origin: &str) -> Result<(), RegistryError> {
        self.append_returning(rec, origin).map(|_| ())
    }

    fn who_serves(&self, workspace: &str, now_ms: i64) -> Option<String> {
        self.fold_iter(G_CLAIMS)
            .into_iter()
            .filter_map(|o| envelope::folded(o, ServeClaim::from_cbor))
            .filter(|c| c.share == workspace && c.lease_expiry_ms > now_ms) // read-time expiry
            .max_by(rank_claims) // highest live epoch, then the lower node id
            .map(|c| c.node)
    }

    fn replicas_of(&self, share: &str) -> Vec<String> {
        // LWW-per-workspace: the latest (origin, seq) WorkspaceEntry wins.
        let mut latest: Option<WorkspaceEntry> = None;
        for o in self.fold_iter(G_WORKSPACES) {
            let Some(e) = envelope::folded(o, WorkspaceEntry::from_cbor) else {
                continue;
            };
            if e.workspace == share {
                latest = Some(e);
            }
        }
        let mut hosts = latest.map(|e| e.eligible_hosts).unwrap_or_default();
        hosts.sort();
        hosts.dedup();
        hosts
    }

    fn grants_for(&self, principal: &str, share: &str) -> Vec<String> {
        // Revocation wins: a matching (principal, share) revocation clears the
        // grant (policy fails closed — an ambiguous grant yields nothing).
        let revoked = self
            .fold_iter(G_REVOCATIONS)
            .into_iter()
            .map(|o| envelope::record(o, CapabilityRevocation::from_cbor))
            .any(|r| match r {
                Ok(r) => r.principal == principal && r.share == share,
                // one that cannot be read may be this pair's: fail closed
                Err(_) => true,
            });
        if revoked {
            return vec![];
        }
        let mut verbs: Vec<String> = self
            .fold_iter(G_GRANTS)
            .into_iter()
            .filter_map(|o| envelope::folded(o, CapabilityGrant::from_cbor))
            .filter(|g| g.principal == principal && g.share == share)
            .flat_map(|g| g.verbs)
            .collect();
        verbs.sort();
        verbs.dedup();
        verbs
    }

    fn bindings_of(&self) -> Vec<BindingDecl> {
        BindingFold::over(&self.ops).live()
    }

    fn nodes_of(&self, operator: &str) -> Vec<String> {
        let mut nodes: Vec<String> = self
            .fold_iter(G_NODES)
            .into_iter()
            .filter_map(|o| envelope::folded(o, NodeRecord::from_cbor))
            .filter(|n| n.operator == operator)
            .map(|n| n.node_id)
            .collect();
        nodes.sort();
        nodes.dedup();
        nodes
    }

    fn snapshot(&self) -> SystemSnapshot {
        // Checkpoints first (plan Step 4.5c), so that a load or a served store
        // seeded from the snapshot starts each chain they fold at its floor.
        let held = self.checkpoints.values().map(|(op, _)| op);
        let encode = |o: &Op| cbor::encode(&o.to_cbor());
        let records = held.chain(&self.ops).map(encode).collect();
        // heads: one StreamHeads per (share, glade_id, key) with per-origin
        // chain heads — the resume vector a peer needs (degenerate sync).
        let mut by_stream: BTreeMap<String, Vec<Head>> = BTreeMap::new();
        for ((glade_id, origin), (seq, hash)) in &self.tips {
            by_stream.entry(glade_id.clone()).or_default().push(Head {
                origin: origin.clone(),
                seq: *seq,
                hash: Some(hash.to_vec()),
            });
        }
        let heads = by_stream
            .into_iter()
            .map(|(glade_id, mut hs)| {
                hs.sort_by(|a, b| a.origin.cmp(&b.origin));
                cbor::encode(
                    &StreamHeads { share: HOME.into(), glade_id, key: vec![], heads: hs }.to_cbor(),
                )
            })
            .collect();
        // A fold has no revision: the engine that saves it keeps its own.
        SystemSnapshot {
            records,
            heads,
            revision: None,
        }
    }
}

// ============================================================================
// The binding fold (R9(a)) — dir.bindings and its retractions, as one.
// ============================================================================

/// Where a binding-family record stands in the fold's order: the documented
/// `value` rule, highest `(lamport, origin)` wins (glade-gyld README), made
/// total by the stream — a retraction outranks a declaration it ties — and
/// then the chain seq. Within one origin this order is "newest", because one
/// clock numbers the family on each node (`next_binding_lamport`). Records
/// from several nodes carry several clocks, so across origins it is an order
/// and not a newest, and it only chooses between live declarations: a
/// retraction is scoped to its own origin (STA-P3-1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Stamp {
    lamport: i64,
    origin: String,
    retraction: bool,
    seq: i64,
}

/// One origin's newest record for one `(app, glade_id)`: a declaration, with
/// its record's bytes as stored, or `None` for a retraction.
#[derive(Clone, Debug)]
struct Newest {
    stamp: Stamp,
    decl: Option<(BindingDecl, Vec<u8>)>,
}

/// One `(app, glade_id)`'s newest record from each origin that wrote one.
type Origins = BTreeMap<String, Newest>;

/// The `dir.bindings` fold (R9(a)), a pure function of an op-set, never of
/// arrival order. Per `(app, glade_id, origin)`, the origin being the op's,
/// which its seal proves, the newest binding-family record wins, and a
/// retraction that is newest takes down that origin's declaration and no
/// other (STA-P3-1). Per `(app, glade_id)`, the highest declaration still
/// live across origins stands; per glade id, the highest standing across
/// apps is the surface. A retraction therefore retracts only its own app's
/// declaration, as its own node made it. The same fold serves the registry,
/// `register`'s diff, and the served store (`exchange::declared_exchange`).
/// A registry holds one node's records, so each pair there has one origin;
/// the served store also holds peers' records, each numbered on its own
/// node's clock, and no node's retraction takes down another's declaration.
#[derive(Clone, Debug, Default)]
pub struct BindingFold {
    newest: BTreeMap<(String, String), Origins>,
}

impl BindingFold {
    /// Fold the binding family out of `ops`; ops on any other stream are
    /// ignored.
    pub fn over<'a>(ops: impl IntoIterator<Item = &'a Op>) -> BindingFold {
        let mut newest: BTreeMap<(String, String), Origins> = BTreeMap::new();
        for op in ops {
            let (app, glade_id, decl) = match op.glade_id.as_str() {
                G_BINDINGS => {
                    let Some(b) = envelope::folded(op, BindingDecl::from_cbor) else {
                        continue;
                    };
                    let record = envelope::record_bytes(&op.payload);
                    (b.app.clone(), b.glade_id.clone(), Some((b, record)))
                }
                G_BINDING_RETRACTIONS => {
                    let Some(r) = envelope::folded(op, BindingRetraction::from_cbor) else {
                        continue;
                    };
                    (r.app, r.glade_id, None)
                }
                _ => continue,
            };
            let stamp =
                Stamp { lamport: op.lamport, origin: op.origin.clone(), retraction: decl.is_none(), seq: op.seq };
            let candidate = Newest { stamp, decl };
            let origins = newest.entry((app, glade_id)).or_default();
            match origins.entry(op.origin.clone()) {
                Entry::Vacant(slot) => {
                    slot.insert(candidate);
                }
                Entry::Occupied(mut slot) => {
                    if candidate.stamp > slot.get().stamp {
                        slot.insert(candidate);
                    }
                }
            }
        }
        BindingFold { newest }
    }

    /// The live bindings: per glade id, the highest declaration standing
    /// across apps, in glade-id order.
    pub fn live(&self) -> Vec<BindingDecl> {
        let mut by_id: BTreeMap<&str, (&Stamp, &BindingDecl)> = BTreeMap::new();
        for ((_, glade_id), origins) in &self.newest {
            let Some((stamp, (decl, _))) = standing(origins) else {
                continue;
            };
            let best = by_id.get(glade_id.as_str());
            if best.is_none_or(|(best, _)| stamp > *best) {
                by_id.insert(glade_id.as_str(), (stamp, decl));
            }
        }
        by_id.into_values().map(|(_, decl)| decl.clone()).collect()
    }

    /// The declarations standing for `app`, by glade id, with the record's
    /// bytes stored for each: what `register` diffs that app's file against.
    pub fn declared_by(&self, app: &str) -> BTreeMap<String, Vec<u8>> {
        self.newest
            .iter()
            .filter(|((a, _), _)| a == app)
            .filter_map(|((_, glade_id), origins)| {
                standing(origins).map(|(_, (_, bytes))| (glade_id.clone(), bytes.clone()))
            })
            .collect()
    }

    /// The `(app, glade_id)` pairs every origin has retracted: each origin's
    /// newest record for the pair is a retraction.
    pub fn retracted(&self) -> Vec<(String, String)> {
        let pairs = self.newest.iter();
        let retracted = pairs.filter(|(_, origins)| standing(origins).is_none());
        retracted.map(|(key, _)| key.clone()).collect()
    }
}

/// The declaration standing for one `(app, glade_id)`: the highest of those
/// its origins' newest records make, or `None` once each has retracted it.
fn standing(origins: &Origins) -> Option<(&Stamp, &(BindingDecl, Vec<u8>))> {
    let newest = origins.values();
    let live = newest.filter_map(|n| Some((&n.stamp, n.decl.as_ref()?)));
    live.max_by_key(|(stamp, _)| *stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ws(id: &str, hosts: &[&str]) -> Record {
        Record::Workspace(WorkspaceEntry {
            workspace: id.into(),
            name: id.into(),
            eligible_hosts: hosts.iter().map(|s| s.to_string()).collect(),
        })
    }
    fn claim(node: &str, share: &str, expiry: i64, epoch: i64) -> Record {
        Record::Serve(ServeClaim { node: node.into(), share: share.into(), lease_expiry_ms: expiry, epoch })
    }

    /// A scripted directory used by several tests + the conformance gate.
    fn scripted() -> Registry {
        let mut r = Registry::new();
        r.append(Record::Node(NodeRecord { node_id: "glade-local".into(), operator: "gianni".into() }), "glade-local").unwrap();
        r.append(Record::Node(NodeRecord { node_id: "peer1".into(), operator: "gianni".into() }), "glade-local").unwrap();
        r.append(ws("ws-razel", &["peer1", "peer2"]), "glade-local").unwrap();
        r.append(ws("ws-attic", &["attic-mini"]), "glade-local").unwrap();
        r.append(ws("home", &["glade-local"]), "glade-local").unwrap();
        r.append(claim("peer1", "ws-razel", 30_000, 1), "peer1").unwrap();
        r.append(claim("peer2", "ws-razel", 30_000, 2), "peer2").unwrap(); // higher epoch
        r.append(Record::Grant(CapabilityGrant { principal: "gianni".into(), share: "ws-razel".into(), verbs: vec!["read".into(), "write".into()] }), "glade-local").unwrap();
        r.append(Record::Grant(CapabilityGrant { principal: "eve".into(), share: "ws-razel".into(), verbs: vec!["read".into()] }), "glade-local").unwrap();
        r.append(Record::Revoke(CapabilityRevocation { principal: "eve".into(), share: "ws-razel".into() }), "glade-local").unwrap();
        r
    }

    #[test]
    fn appends_are_origin_attributed() {
        let r = scripted();
        let snap = r.snapshot();
        // every persisted record carries its appending origin (blob-land, still
        // attributed) — the migration-to-per-origin-logs invariant.
        for bytes in &snap.records {
            let op = Op::decode(bytes).unwrap();
            assert!(!op.origin.is_empty(), "record missing origin attribution");
        }
        assert!(!snap.heads.is_empty(), "snapshot carries heads (cached fold + heads)");
    }

    #[test]
    fn queries_over_the_fold() {
        let r = scripted();
        assert_eq!(r.nodes_of("gianni"), vec!["glade-local", "peer1"]);
        assert_eq!(r.replicas_of("ws-razel"), vec!["peer1", "peer2"]);
        // highest LIVE epoch wins at read time.
        assert_eq!(r.who_serves("ws-razel", 0), Some("peer2".into()));
        // grants: set-union, revocation wins.
        assert_eq!(r.grants_for("gianni", "ws-razel"), vec!["read", "write"]);
        assert_eq!(r.grants_for("eve", "ws-razel"), Vec::<String>::new()); // revoked
    }

    #[test]
    fn lease_expiry_is_read_time_not_folded() {
        let r = scripted();
        // same op-set, different reader clock -> different answer, fold unchanged.
        assert_eq!(r.who_serves("ws-razel", 0), Some("peer2".into()));
        assert_eq!(r.who_serves("ws-razel", 40_000), None); // all leases expired
    }

    /// Two nodes that each claim a share before either has the other's claim
    /// mint the same epoch. Every fold that names its holder then names the
    /// lower node id, in whichever order the claims arrive: the registry's
    /// and the served store's. A higher epoch still holds over a lower id.
    #[test]
    fn an_equal_epoch_is_held_by_the_lower_node_id_in_every_fold() {
        use crate::mesh::who_serves;
        use crate::store::Store;

        let node = |key: u8| NodeIdentity::from_key([key; 32]);
        let ids = [1, 2].map(|key| transport::hex(&node(key).node_id));
        let (lower, higher) = if ids[0] < ids[1] { (0, 1) } else { (1, 0) };
        let mut writers = [Registry::sealed(node(1)), Registry::sealed(node(2))];
        let mut claim_at = |at: usize, epoch: i64| {
            let rec = claim(&ids[at], "ws-tie", 30_000, epoch);
            writers[at].append_returning(rec, &ids[at]).unwrap()
        };
        let tied = [claim_at(0, 1), claim_at(1, 1)];
        let over = claim_at(higher, 2);
        // The holder each fold names: the registry's, then the served store's.
        let folds = |registry: &Registry, store: &Store| {
            let ours = registry.who_serves("ws-tie", 0);
            (ours, who_serves(store, "ws-tie", 0))
        };
        for (run, order) in [[0, 1], [1, 0]].into_iter().enumerate() {
            let mut registry = Registry::sealed(node(3));
            let root = std::env::temp_dir().join(format!("glade-reg-tie-{run}"));
            let _ = fs::remove_dir_all(&root);
            let mut store = Store::open(&root).unwrap();
            for at in order {
                registry.ingest(tied[at].clone()).unwrap();
                store.append(tied[at].clone()).unwrap();
            }
            let held = Some(ids[lower].clone());
            let said = format!("claims in order {order:?}");
            assert_eq!(folds(&registry, &store), (held.clone(), held), "{said}");
            registry.ingest(over.clone()).unwrap();
            store.append(over.clone()).unwrap();
            let held = Some(ids[higher].clone());
            let said = format!("{said}, then epoch 2");
            assert_eq!(folds(&registry, &store), (held.clone(), held), "{said}");
            fs::remove_dir_all(&root).unwrap();
        }
    }

    /// The seam's central requirement (#6): the blob engine and a DIFFERENT
    /// engine (mem — the SQLite/fold-later stand-in) are behaviorally
    /// indistinguishable through the trait. Every query matches, byte-for-byte.
    #[test]
    fn blob_impl_equiv_future_fold_impl() {
        let live = scripted();
        let snap = live.snapshot();

        let dir = std::env::temp_dir().join("glade-reg-conformance");
        let _ = fs::remove_dir_all(&dir);
        let mut blob = BlobStore::new(&dir);
        blob.save(&snap).unwrap();
        let (from_blob, rej_b) = Registry::from_snapshot(&blob.load().unwrap());

        let mut mem = MemStore::default();
        mem.save(&snap).unwrap();
        let (from_mem, rej_m) = Registry::from_snapshot(&mem.load().unwrap());

        assert_eq!((rej_b, rej_m), (0, 0), "clean snapshot ingests with no rejects");
        for (p, s, now) in [("ws-razel", "ws-razel", 0i64), ("ws-attic", "ws-attic", 100_000)] {
            assert_eq!(live.who_serves(p, now), from_blob.who_serves(p, now));
            assert_eq!(from_blob.who_serves(p, now), from_mem.who_serves(p, now));
            assert_eq!(live.replicas_of(s), from_blob.replicas_of(s));
            assert_eq!(from_blob.replicas_of(s), from_mem.replicas_of(s));
        }
        assert_eq!(live.nodes_of("gianni"), from_mem.nodes_of("gianni"));
        assert_eq!(live.grants_for("gianni", "ws-razel"), from_blob.grants_for("gianni", "ws-razel"));
        assert_eq!(live.grants_for("eve", "ws-razel"), from_mem.grants_for("eve", "ws-razel"));
        // a snapshot is a cached fold + heads: round-trip is byte-identical.
        assert_eq!(live.snapshot(), from_blob.snapshot());
        assert_eq!(from_blob.snapshot(), from_mem.snapshot());
    }

    #[test]
    fn verify_as_ingest_rejects_a_tampered_chain() {
        let mut r = scripted();
        // a valid appended chain of two claims on origin "peerX"
        r.append(claim("peerX", "ws-x", 30_000, 1), "peerX").unwrap();
        r.append(claim("peerX", "ws-x", 30_000, 2), "peerX").unwrap();
        let mut snap = r.snapshot();
        // tamper: corrupt one claim record's payload -> its op-hash changes, so
        // the NEXT op's prev no longer matches -> chain break -> suffix dropped.
        let target = snap.records.iter().position(|b| {
            let op = Op::decode(b).unwrap();
            op.origin == "peerX" && op.seq == 0
        }).unwrap();
        let mut op = Op::decode(&snap.records[target]).unwrap();
        op.payload.push(0xff); // malicious edit — indistinguishable from a bad sync chunk
        snap.records[target] = cbor::encode(&op.to_cbor());
        let (reg, rejected) = Registry::from_snapshot(&snap);
        assert!(rejected >= 1, "the tampered op (and its suffix) is quarantined");
        // the honest records still fold.
        assert_eq!(reg.nodes_of("gianni"), vec!["glade-local", "peer1"]);
    }

    /// Plan Step 4.3's fail direction at load: a grant or a revocation
    /// quarantined by verify-as-ingest leaves the grant fold unreadable, since
    /// what is left could let a revoked grant stand; a quarantined record of
    /// another kind leaves it readable. Here the second of two revocations
    /// loses its predecessor, so it arrives as a gap.
    #[test]
    fn a_quarantined_grant_or_revocation_leaves_the_fold_unreadable() {
        use glade_grant_api::{Denial, GrantPort, Holder};

        use crate::grants::PolicyView;

        let revoke = |principal: &str| {
            let (principal, share) = (principal.into(), "ws-a".into());
            Record::Revoke(CapabilityRevocation { principal, share })
        };
        let mut r = Registry::new();
        let grant = CapabilityGrant {
            principal: "alice".into(),
            share: "ws-a".into(),
            verbs: vec!["read".into()],
        };
        r.append(Record::Grant(grant), "n1").unwrap();
        r.append(revoke("eve"), "n1").unwrap();
        r.append(revoke("mallory"), "n1").unwrap();
        r.append(claim("n1", "ws-a", 1_000, 1), "n1").unwrap();
        r.append(claim("n1", "ws-a", 2_000, 1), "n1").unwrap();
        let alice = Holder::Principal("alice".into());
        assert_eq!(
            PolicyView::of(r.policy()).check(&alice, "read", "ws-a"),
            Ok(())
        );

        // The snapshot without the first record of `glade_id`'s chain.
        let without = |glade_id: &str| {
            let mut snap = r.snapshot();
            snap.records.retain(|bytes| {
                let op = Op::decode(bytes).unwrap();
                !(op.glade_id == glade_id && op.seq == 0)
            });
            Registry::from_snapshot(&snap)
        };
        let (claims_cut, rejected) = without(G_CLAIMS);
        assert_eq!((rejected, claims_cut.policy_quarantined()), (1, false));
        assert_eq!(
            PolicyView::of(claims_cut.policy()).check(&alice, "read", "ws-a"),
            Ok(())
        );

        let (policy_cut, rejected) = without(G_REVOCATIONS);
        assert_eq!((rejected, policy_cut.policy_quarantined()), (1, true));
        assert_eq!(policy_cut.policy(), None);
        let answer = PolicyView::of(policy_cut.policy()).check(&alice, "read", "ws-a");
        assert_eq!(answer, Err(Denial::Unavailable));
    }

    /// Plan Step 4.5c: a sealed registry's own checkpoint at base B leaves
    /// no claim at or below B in its fold, and its next claim goes above its
    /// tip; its snapshot lists the checkpoint first. A sealed reload of that
    /// snapshot quarantines nothing, the chain starting at its floor, and has
    /// the same tips.
    #[test]
    fn a_sealed_registry_keeps_its_claims_from_its_checkpoints_floor() {
        let identity = NodeIdentity::from_key([14; 32]);
        let me = transport::hex(&identity.node_id);
        let mut r = Registry::sealed(identity);
        let mut claims = Vec::new();
        for expiry in 1..=6 {
            let op = r.append_returning(claim(&me, "ws", expiry * 1_000, 1), &me);
            claims.push(op.unwrap());
        }
        let hash = op_hash(&claims[3]).to_vec();
        let record = ChainCheckpoint {
            node: me.clone(),
            stream: G_CLAIMS.into(),
            seq: 3,
            hash,
        };
        let placed = r.append_returning(Record::Checkpoint(record), &me).unwrap();
        let held: Vec<i64> = r.fold_iter(G_CLAIMS).iter().map(|o| o.seq).collect();
        assert_eq!(held, [4, 5], "no claim at or below the base");
        let next = r.append_returning(claim(&me, "ws", 9_000, 1), &me).unwrap();
        let above = (6, Some(op_hash(&claims[5]).to_vec()));
        assert_eq!((next.seq, next.prev), above, "the next claim above the tip");
        let snap = r.snapshot();
        let first = envelope::decode_op(&snap.records[0]);
        assert_eq!(first, Ok(placed), "the checkpoint first");
        let (again, rejected) = Registry::from_snapshot_as(&snap, identity);
        assert_eq!(rejected, 0, "the reload quarantined {rejected} record(s)");
        assert_eq!(again.tips, r.tips, "the same tips");
        assert_eq!(again.snapshot(), snap);
    }

    /// Plan Step 4.5c, section 6's rules at a node holding another's chain:
    /// an op at or below the floor is covered, not a fork, where one at the
    /// base with another hash is one; a checkpoint two seqs ahead of the one
    /// held is placed, across the gap, and an older one is seen; another at
    /// the held seq is a fork; one whose base moves back is refused. None of
    /// those refused changes the fold.
    #[test]
    fn a_covered_op_is_seen_and_a_newer_checkpoint_crosses_a_gap() {
        use crate::envelope::testing::checkpoint;
        let seed = [15; 32];
        let identity = NodeIdentity::from_key(seed);
        let peer = transport::hex(&identity.node_id);
        let mut origin = Registry::sealed(identity);
        let mut claims = Vec::new();
        for expiry in 1..=6 {
            let op = origin.append_returning(claim(&peer, "ws", expiry * 1_000, 1), &peer);
            claims.push(op.unwrap());
        }
        let mut node = Registry::sealed(NodeIdentity::from_key([16; 32]));
        for op in &claims {
            assert_eq!(node.ingest(op.clone()), Ok(Ingested::Appended));
        }
        let at = |base: usize| ChainCheckpoint {
            node: peer.clone(),
            stream: G_CLAIMS.into(),
            seq: base as i64,
            hash: op_hash(&claims[base]).to_vec(),
        };
        let mut sent = Vec::new();
        for base in 1..=3 {
            let appended = origin.append_returning(Record::Checkpoint(at(base)), &peer);
            sent.push(appended.unwrap());
        }
        let unsealed = Op {
            payload: claim(&peer, "ws", 99, 1).encode(),
            ..claims[1].clone()
        };
        let another = Op {
            payload: envelope::seal(&identity, &unsealed),
            ..unsealed
        };
        let (origin, seq) = (peer.clone(), 1);
        let fork = Err(RegistryError::Equivocation { origin, seq });
        let steps = [
            (&sent[0], Ok(Ingested::Appended), "placed"),
            (&claims[0], Ok(Ingested::Covered), "below the floor"),
            (&claims[1], Ok(Ingested::Covered), "at it"),
            (&another, fork, "at the base, another hash"),
            (&sent[2], Ok(Ingested::Appended), "two seqs ahead"),
            (&sent[1], Ok(Ingested::Covered), "an older one"),
        ];
        for (op, taken, what) in steps {
            assert_eq!(node.ingest(op.clone()), taken, "{what}");
        }
        let held: Vec<i64> = node.fold_iter(G_CLAIMS).iter().map(|o| o.seq).collect();
        assert_eq!(held, [4, 5]);
        let held = node.snapshot();
        let prev = |op: &Op| Some(op_hash(op).to_vec());
        let forked = checkpoint(seed, at(4), 2, prev(&sent[1]));
        let (origin, seq) = (peer.clone(), 2);
        let fork = RegistryError::Equivocation { origin, seq };
        assert_eq!(node.ingest(forked), Err(fork), "another at the held seq");
        let back = checkpoint(seed, at(2), 3, prev(&sent[2]));
        let (origin, seq) = (peer, 3);
        let rewrite = RegistryError::Rewrite { origin, seq };
        assert_eq!(node.ingest(back), Err(rewrite), "a base that moves back");
        assert_eq!(node.snapshot(), held, "nothing refused changes the fold");
    }

    /// Plan Step 4.5c: `accept` saves when the change took something, not
    /// when the fold grew. A tick's renewal and a checkpoint that drops one
    /// claim leave the fold no longer, and are saved all the same.
    #[test]
    fn a_change_that_drops_as_many_as_it_appends_is_saved() {
        let identity = NodeIdentity::from_key([17; 32]);
        let me = transport::hex(&identity.node_id);
        let (mut r, mut store) = (Registry::sealed(identity), MemStore::default());
        let append = |r: &mut Registry, record: Record| {
            let appended = r.append_returning(record, &me);
            appended.map_err(|e| io::Error::other(format!("{e:?}")))
        };
        let base = r.accept(&mut store, |r| {
            let base = append(r, claim(&me, "ws", 1_000, 1))?;
            append(r, claim(&me, "ws", 2_000, 1)).map(|_| base)
        });
        let hash = op_hash(&base.unwrap()).to_vec();
        let record = ChainCheckpoint {
            node: me.clone(),
            stream: G_CLAIMS.into(),
            seq: 0,
            hash,
        };
        let before = store.load().unwrap();
        let ticked = r.accept(&mut store, |r| {
            append(r, claim(&me, "ws", 3_000, 1))?;
            append(r, Record::Checkpoint(record))
        });
        assert!(ticked.is_ok());
        assert_eq!(r.fold_iter(G_CLAIMS).len(), 2, "the fold no longer");
        let after = store.load().unwrap();
        let saved = after != before;
        assert!(saved, "the engine's snapshot unchanged");
        assert!(after == r.snapshot(), "the fold saved");
    }

    /// A byte-identical re-delivery is a duplicate (plan Step 4.4; owner,
    /// 2026-09-24): the registry takes it as held and appends nothing, as the
    /// wire store takes it. A different op at a position already held is
    /// still a fork, `Equivocation`, and changes nothing either.
    #[test]
    fn a_repeat_is_a_duplicate_and_a_different_op_at_a_held_seq_a_fork() {
        let mut r = Registry::new();
        let first = r
            .append_returning(claim("n1", "ws", 1_000, 1), "n1")
            .unwrap();
        r.append(claim("n1", "ws", 2_000, 1), "n1").unwrap();
        let held = r.snapshot();
        assert_eq!(r.ingest(first.clone()), Ok(Ingested::Duplicate));
        assert_eq!(r.snapshot(), held, "a duplicate appends nothing");
        let fork = Op {
            payload: claim("n1", "ws", 9_000, 1).encode(),
            ..first
        };
        let fork = r.ingest(fork);
        let equivocation = RegistryError::Equivocation {
            origin: "n1".into(),
            seq: 0,
        };
        assert_eq!(fork, Err(equivocation));
        assert_eq!(r.snapshot(), held);
    }

    /// Plan Step 4.1b: a sealed registry appends each record in an envelope
    /// its node signed, which verifies, and diffs the record, not the
    /// envelope; it refuses to append under another origin; and it takes in
    /// only ops that verify, where an unsealed one takes a bare record. A
    /// reload of its snapshot, sealed, quarantines nothing.
    #[test]
    fn a_sealed_registry_signs_its_appends_and_takes_only_what_verifies() {
        let identity = NodeIdentity::from_key([12; 32]);
        let me = transport::hex(&identity.node_id);
        let mut r = Registry::sealed(identity);
        let op = r.append_returning(claim(&me, "ws", 1_000, 1), &me).unwrap();
        assert_eq!(envelope::verify(&op), Ok(()), "the appended op verifies");
        assert!(r.contains(G_CLAIMS, &claim(&me, "ws", 1_000, 1).encode()));
        let theirs = r.append(claim("peer", "ws", 1_000, 2), "peer");
        assert_eq!(
            theirs,
            Err(RegistryError::NotOurs {
                origin: "peer".into()
            })
        );

        let bare = Op {
            seq: 1,
            prev: Some(op_hash(&op).to_vec()),
            payload: claim(&me, "ws", 2_000, 1).encode(),
            ..op.clone()
        };
        let unsigned = RegistryError::Unverified {
            origin: me.clone(),
            why: Refused::Unsigned,
        };
        assert_eq!(r.ingest(bare.clone()), Err(unsigned));
        let other = NodeIdentity::from_key([13; 32]);
        let forged = Op {
            payload: envelope::seal(&other, &bare),
            ..bare.clone()
        };
        let bad = RegistryError::Unverified {
            origin: me.clone(),
            why: Refused::Signature,
        };
        assert_eq!(r.ingest(forged), Err(bad));
        let first = Op {
            payload: claim(&me, "ws", 1_000, 1).encode(),
            ..op
        };
        assert_eq!(
            Registry::new().ingest(first),
            Ok(Ingested::Appended),
            "unsealed"
        );

        let (again, rejected) = Registry::from_snapshot_as(&r.snapshot(), identity);
        assert_eq!(rejected, 0);
        assert_eq!(again.snapshot(), r.snapshot());
    }

    fn decl(app: &str, glade_id: &str, shape: &str) -> Record {
        Record::Binding(BindingDecl {
            app: app.into(),
            glade_id: glade_id.into(),
            shape: shape.into(),
            authority: "share".into(),
            zone: "commons".into(),
            retention: "latest".into(),
        })
    }
    fn retract(app: &str, glade_id: &str) -> Record {
        Record::Retract(BindingRetraction { app: app.into(), glade_id: glade_id.into() })
    }
    /// The live bindings as (app, glade_id, shape).
    fn live(r: &Registry) -> Vec<(String, String, String)> {
        r.bindings_of().into_iter().map(|b| (b.app, b.glade_id, b.shape)).collect()
    }
    fn row(app: &str, glade_id: &str, shape: &str) -> (String, String, String) {
        (app.into(), glade_id.into(), shape.into())
    }
    fn ops_of(r: &Registry) -> Vec<Op> {
        r.snapshot().records.iter().map(|b| Op::decode(b).unwrap()).collect()
    }

    /// R9(a): `dir.bindings` folds by glade id and the newest declaration is
    /// the live one — a changed line replaces the surface's declaration.
    #[test]
    fn the_binding_fold_takes_the_newest_declaration_per_glade_id() {
        let mut r = Registry::new();
        assert_eq!(live(&r), vec![]);
        r.append(decl("a", "g", "value"), "n1").unwrap();
        r.append(decl("a", "h", "log"), "n1").unwrap();
        r.append(decl("a", "g", "log"), "n1").unwrap();
        assert_eq!(live(&r), vec![row("a", "g", "log"), row("a", "h", "log")]);
    }

    /// A retraction that is newest takes the surface down; a declaration
    /// newer than the retraction brings it back. The two ride different
    /// streams, so "newer" needs one clock across both (the lamport rule).
    #[test]
    fn a_newest_retraction_takes_a_surface_down_and_a_newer_declaration_revives_it() {
        let mut r = Registry::new();
        for id in ["g", "h", "k"] {
            r.append(decl("a", id, "value"), "n1").unwrap();
        }
        r.append(retract("a", "g"), "n1").unwrap();
        assert_eq!(live(&r), vec![row("a", "h", "value"), row("a", "k", "value")]);
        assert_eq!(BindingFold::over(&ops_of(&r)).retracted(), vec![("a".to_string(), "g".to_string())]);
        r.append(decl("a", "g", "log"), "n1").unwrap();
        assert_eq!(live(&r), vec![row("a", "g", "log"), row("a", "h", "value"), row("a", "k", "value")]);
        assert_eq!(BindingFold::over(&ops_of(&r)).retracted(), Vec::<(String, String)>::new());
    }

    /// R9(a)'s scope: a retraction retracts its own app's declaration and no
    /// other. Two apps declaring one glade id: the newest live declaration is
    /// the surface, and retracting one app's leaves the other's live.
    #[test]
    fn a_retraction_retracts_only_its_own_apps_declaration() {
        let mut r = Registry::new();
        r.append(decl("a", "g", "value"), "n1").unwrap();
        r.append(decl("b", "g", "log"), "n1").unwrap();
        assert_eq!(live(&r), vec![row("b", "g", "log")]);
        r.append(retract("b", "g"), "n1").unwrap();
        assert_eq!(live(&r), vec![row("a", "g", "value")], "a's declaration was never in scope");
        r.append(retract("a", "g"), "n1").unwrap();
        assert_eq!(live(&r), vec![]);
    }

    /// The binding family's lamport is one clock across its two streams: one
    /// past the highest either holds, from any origin. With one origin and no
    /// retraction that is the record's own seq, which is what every record
    /// written before R9 carries; every other record kind keeps lamport = seq.
    #[test]
    fn the_binding_family_lamport_is_one_clock_across_both_streams() {
        let mut r = Registry::new();
        let mut at = |rec: Record, origin: &str| {
            let op = r.append_returning(rec, origin).unwrap();
            (op.glade_id, op.seq, op.lamport)
        };
        assert_eq!(at(decl("a", "g", "value"), "n1"), (G_BINDINGS.into(), 0, 0));
        assert_eq!(at(decl("a", "h", "value"), "n1"), (G_BINDINGS.into(), 1, 1));
        assert_eq!(at(claim("n1", "ws", 1, 1), "n1"), (G_CLAIMS.into(), 0, 0));
        assert_eq!(at(retract("a", "g"), "n1"), (G_BINDING_RETRACTIONS.into(), 0, 2));
        assert_eq!(at(decl("a", "g", "log"), "n1"), (G_BINDINGS.into(), 2, 3));
        assert_eq!(at(retract("a", "h"), "n1"), (G_BINDING_RETRACTIONS.into(), 1, 4));
        // a second origin's first record is still the newest
        assert_eq!(at(decl("a", "k", "value"), "n2"), (G_BINDINGS.into(), 0, 5));
        assert_eq!(at(claim("n1", "ws", 1, 2), "n1"), (G_CLAIMS.into(), 1, 1));
    }

    /// The glade ids `app` has live in one fold of every record `regs` hold.
    fn live_together(regs: &[&Registry], app: &str) -> Vec<String> {
        let ops: Vec<Op> = regs.iter().copied().flat_map(ops_of).collect();
        let live = BindingFold::over(&ops).declared_by(app);
        live.into_keys().collect()
    }

    /// `app`'s live bindings as rows, in one fold of every record `regs` hold.
    fn rows_together(regs: &[&Registry], app: &str) -> Vec<(String, String, String)> {
        let ops: Vec<Op> = regs.iter().copied().flat_map(ops_of).collect();
        let live = BindingFold::over(&ops).live();
        let ours = live.into_iter().filter(|b| b.app == app);
        ours.map(|b| (b.app, b.glade_id, b.shape)).collect()
    }

    /// STA-P3-1: a retraction is scoped to its op's origin. Each registry
    /// numbers the binding family on its own clock and never ingests another
    /// node's records, so where two nodes' records are folded together, as
    /// the served store does, one node's retraction can outrank the other's
    /// declaration. It still takes down only its own node's: each origin's
    /// declaration stands or falls by that origin's records, and across
    /// origins the stamp only chooses between live declarations.
    #[test]
    fn across_two_origins_a_retraction_takes_down_only_its_own_origins_declaration() {
        let (mut a, mut b) = (Registry::new(), Registry::new());
        // A's clock runs ahead of B's: six binding records of another app.
        for i in 0..6 {
            let other = decl("other", &format!("o{i}"), "value");
            a.append(other, "A").unwrap();
        }
        for (reg, origin) in [(&mut a, "A"), (&mut b, "B")] {
            reg.append(decl("grazel", "g", "value"), origin).unwrap();
            reg.append(decl("grazel", "h", "value"), origin).unwrap();
        }
        let retraction = a.append_returning(retract("grazel", "g"), "A").unwrap();
        assert_eq!(retraction.lamport, 8);
        // A's retraction takes A's `g` down; folded together, B's stands.
        assert_eq!(live_together(&[&a], "grazel"), ["h"]);
        assert_eq!(live_together(&[&a, &b], "grazel"), ["g", "h"]);
        // B changes `g` after A's retraction. On B's clock that declaration
        // is lamport 2, below the retraction's 8 and A's own `g` at 6, and it
        // is the live `g` together, as it is in B's registry, the basis of
        // B's next `register` diff.
        let later = b.append_returning(decl("grazel", "g", "log"), "B").unwrap();
        assert_eq!(later.lamport, 2);
        let want = vec![row("grazel", "g", "log"), row("grazel", "h", "value")];
        assert_eq!(rows_together(&[&a, &b], "grazel"), want);
        assert_eq!(live(&b), want);
    }

    /// A retraction from an origin that never declared its `(app, glade_id)`
    /// retracts nothing, however far that origin's clock runs ahead.
    #[test]
    fn a_retraction_from_an_origin_that_never_declared_it_retracts_nothing() {
        let (mut a, mut b) = (Registry::new(), Registry::new());
        b.append(decl("grazel", "g", "value"), "B").unwrap();
        a.append(decl("other", "o", "value"), "A").unwrap();
        let retraction = a.append_returning(retract("grazel", "g"), "A").unwrap();
        assert_eq!(retraction.lamport, 1, "above B's declaration, at 0");
        assert_eq!(live_together(&[&a, &b], "grazel"), ["g"]);
        let want = [row("grazel", "g", "value")];
        assert_eq!(rows_together(&[&a, &b], "grazel"), want);
        let ops: Vec<Op> = [&a, &b].into_iter().flat_map(ops_of).collect();
        let retracted = BindingFold::over(&ops).retracted();
        assert!(retracted.is_empty(), "nothing is retracted: {retracted:?}");
    }

    /// The fold is a pure function of the op-set: any arrival order, and a
    /// reload through verify-as-ingest, give the same live set.
    #[test]
    fn the_binding_fold_is_a_pure_function_of_the_op_set() {
        let mut r = Registry::new();
        r.append(decl("a", "g", "value"), "n1").unwrap();
        r.append(decl("a", "h", "value"), "n1").unwrap();
        r.append(decl("b", "g", "log"), "n2").unwrap();
        r.append(retract("a", "h"), "n1").unwrap();
        r.append(decl("a", "h", "log"), "n1").unwrap();
        r.append(retract("b", "g"), "n2").unwrap();
        let ops = ops_of(&r);
        let forward = BindingFold::over(&ops).live();
        assert_eq!(live(&r), vec![row("a", "g", "value"), row("a", "h", "log")]);
        let mut reversed = ops.clone();
        reversed.reverse();
        assert_eq!(BindingFold::over(&reversed).live(), forward);
        let (again, rejected) = Registry::from_snapshot(&r.snapshot());
        assert_eq!(rejected, 0);
        assert_eq!(again.bindings_of(), forward);
    }

    /// No existing record kind's bytes move (the amendment ADDS a kind). A
    /// `BindingDecl` payload as the pre-amendment codec wrote it — pinned here
    /// by hand from the canonical encoding, not produced by the code under
    /// test — decodes and re-encodes to the same bytes, alone and inside its
    /// stored op through a snapshot reload; a first binding record's envelope
    /// still says seq 0, lamport 0.
    #[test]
    fn a_stored_binding_decl_round_trips_byte_identically() {
        let pinned = "a601666772617a656c02687465726d2e6c6f6703636c6f67046573686172650567636f6d6d6f6e73066b66726f6d5f637572736f72";
        let payload: Vec<u8> =
            (0..pinned.len()).step_by(2).map(|i| u8::from_str_radix(&pinned[i..i + 2], 16).unwrap()).collect();
        let b = BindingDecl {
            app: "grazel".into(),
            glade_id: "term.log".into(),
            shape: "log".into(),
            authority: "share".into(),
            zone: "commons".into(),
            retention: "from_cursor".into(),
        };
        assert_eq!(BindingDecl::decode(&payload).unwrap(), b);
        assert_eq!(Record::Binding(b.clone()).encode(), payload);
        let mut r = Registry::new();
        r.append(Record::Binding(b), "n1").unwrap();
        let snap = r.snapshot();
        let op = Op::decode(&snap.records[0]).unwrap();
        assert_eq!((op.payload.as_slice(), op.seq, op.lamport), (payload.as_slice(), 0, 0));
        let (back, rejected) = Registry::from_snapshot(&snap);
        assert_eq!(rejected, 0);
        assert_eq!(back.snapshot(), snap);
    }

    #[test]
    fn store_save_is_crash_atomic_and_reloads() {
        let dir = std::env::temp_dir().join("glade-reg-atomic");
        let _ = fs::remove_dir_all(&dir);
        let snap = scripted().snapshot();
        {
            let mut s = BlobStore::new(&dir);
            s.save(&snap).unwrap();
        }
        // reopen: same bytes back (records.json survives the drop).
        let reloaded = BlobStore::new(&dir).load().unwrap();
        assert_eq!(reloaded, snap);
        // no tmp file left behind after the rename.
        assert!(!dir.join("records.json.tmp").exists());
    }

    /// F15b: a record nested 100,000 deep, which the wire codec's decode
    /// recursed on until the stack overflowed, is never taken into a
    /// directory stream. An unsealed registry, the record host of the test
    /// compositions and the journeys, refuses it at ingest, as a sealed one
    /// refuses it unverified. A snapshot holding it, or holding an op itself
    /// nested so, loads with both quarantined and the grant fold closed, and
    /// every fold answers.
    #[test]
    fn a_nested_record_is_refused_and_a_nested_op_quarantined() {
        let mut nested = vec![0x81; 100_000];
        nested.push(0);
        let op = Op {
            share: HOME.into(),
            glade_id: G_GRANTS.into(),
            origin: "n1".into(),
            shape: Shape::Log,
            payload: nested.clone(),
            ..Op::default()
        };
        let mut unsealed = Registry::new();
        let refused = unsealed.ingest(op.clone());
        assert!(refused.is_err(), "{refused:?}");
        let mut sealed = Registry::sealed(NodeIdentity::from_key([7; 32]));
        assert!(sealed.ingest(op.clone()).is_err());

        let records = vec![cbor::encode(&op.to_cbor()), nested];
        let snap = SystemSnapshot {
            records,
            heads: vec![],
            revision: None,
        };
        let (loaded, rejected) = Registry::from_snapshot(&snap);
        assert_eq!(rejected, 2);
        assert!(loaded.policy().is_none(), "the grant fold fails closed");
        assert_eq!(loaded.grants_for("p", "ws"), Vec::<String>::new());
        assert_eq!(unsealed.who_serves("ws", 0), None);
    }

    /// F15b: a grant or a revocation a fold cannot read closes the grant
    /// fold: the policy is unreadable, and `grants_for` grants nothing, since
    /// a revocation it cannot read may be the pair's. Ingest and load never
    /// let one in; this one is put in by hand.
    #[test]
    fn a_policy_record_that_cannot_be_read_closes_the_grant_fold() {
        let mut r = Registry::new();
        let grant = CapabilityGrant {
            principal: "p".into(),
            share: "ws".into(),
            verbs: vec!["read.*".into()],
        };
        r.append(Record::Grant(grant), "n1").unwrap();
        assert!(r.policy().is_some());
        assert_eq!(r.grants_for("p", "ws"), ["read.*"]);
        let mut nested = vec![0x81; 100_000];
        nested.push(0);
        r.ops.push(Op {
            share: HOME.into(),
            glade_id: G_REVOCATIONS.into(),
            origin: "n1".into(),
            shape: Shape::Log,
            payload: nested,
            ..Op::default()
        });
        assert!(r.policy().is_none());
        assert_eq!(r.grants_for("p", "ws"), Vec::<String>::new());
    }
}
