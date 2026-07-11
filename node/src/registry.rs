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

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use glade_wire::cbor;
use glade_wire::generated::{Head, Op, Shape, StreamHeads};

use crate::chain::op_hash;
use crate::sysdata::{
    BindingDecl, CapabilityGrant, CapabilityRevocation, NodeRecord, PrincipalRecord, ServeClaim,
    ServiceDefinition, SystemSnapshot, WorkspaceEntry,
};

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
// Principals minimal (GLP-0006 P0.S7; the stream GDL-038 names): identity as
// data — session Hellos auto-append unknown principals; nothing enforced.
pub const G_PRINCIPALS: &str = "dir.principals";

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
    Service(ServiceDefinition),
    Principal(PrincipalRecord),
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
            Record::Service(_) => G_SERVICES,
            Record::Principal(_) => G_PRINCIPALS,
        }
    }

    /// Is this a POLICY record? Policy records fail CLOSED on load (AZ-11): an
    /// unparseable/broken policy op is dropped, never leniently kept.
    pub fn is_policy(glade_id: &str) -> bool {
        matches!(glade_id, G_GRANTS | G_REVOCATIONS)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let c = match self {
            Record::Node(r) => r.to_cbor(),
            Record::Workspace(r) => r.to_cbor(),
            Record::Serve(r) => r.to_cbor(),
            Record::Grant(r) => r.to_cbor(),
            Record::Revoke(r) => r.to_cbor(),
            Record::Binding(r) => r.to_cbor(),
            Record::Service(r) => r.to_cbor(),
            Record::Principal(r) => r.to_cbor(),
        };
        cbor::encode(&c)
    }
}

/// Ingest rejection — the verify-as-ingest failures (s-sync Y3), reused for both
/// live appends and disk load. A rejected op and its suffix are excluded from
/// the fold; the fold stays a pure function of the valid op-set.
#[derive(Debug, PartialEq)]
pub enum RegistryError {
    Gap { expected: i64, got: i64 },
    ChainBreak { origin: String, seq: i64 },
    Equivocation { origin: String, seq: i64 },
}

// ============================================================================
// StoreApi — how a node persists (the swappable engine).
// ============================================================================

/// Persist the whole system state. The trait is deliberately the whole-blob
/// shape so a SQLite (or any) engine can re-implement it without any caller
/// changing — `load`/`save` a [`SystemSnapshot`], nothing else. Nothing above
/// this trait knows files exist.
pub trait StoreApi {
    fn load(&self) -> io::Result<SystemSnapshot>;
    fn save(&mut self, snap: &SystemSnapshot) -> io::Result<()>;
}

/// The interim engine: the whole snapshot as one taut message on disk
/// (`records.json`), rewritten tmp+rename (crash-atomic). This IS the
/// degenerate-sync artifact a connecting peer would ingest.
///
/// At-rest bytes are canonical CBOR of the [`SystemSnapshot`] (see the module
/// note): hashing == at-rest, so verify-as-ingest is uniform. The spec's
/// JSON-text rendering is a later cosmetic — the seam does not depend on it.
pub struct BlobStore {
    path: PathBuf,
}

impl BlobStore {
    /// A blob engine writing `records.json` under `dir`.
    pub fn new(dir: impl AsRef<Path>) -> BlobStore {
        BlobStore { path: dir.as_ref().join("records.json") }
    }
}

impl StoreApi for BlobStore {
    fn load(&self) -> io::Result<SystemSnapshot> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(SystemSnapshot::from_cbor(&cbor::decode(&bytes))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SystemSnapshot::default()),
            Err(e) => Err(e),
        }
    }

    fn save(&mut self, snap: &SystemSnapshot) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = cbor::encode(&snap.to_cbor());
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &self.path) // crash-atomic swap
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
    /// Lease expiry is evaluated at read time, never inside the fold; highest
    /// live epoch wins.
    fn who_serves(&self, workspace: &str, now_ms: i64) -> Option<String>;

    /// Eligible replica nodes for `share` (WorkspaceEntry.eligible_hosts, LWW).
    fn replicas_of(&self, share: &str) -> Vec<String>;

    /// Verbs granted to `principal` on `share` — set-union, revocation wins.
    fn grants_for(&self, principal: &str, share: &str) -> Vec<String>;

    /// Nodes operated by `operator` (NodeRecord set-union).
    fn nodes_of(&self, operator: &str) -> Vec<String>;

    /// The cached fold + heads — for `StoreApi::save` / degenerate sync.
    fn snapshot(&self) -> SystemSnapshot;
}

/// The interim RegistryApi: an in-memory op-set materialised from a snapshot,
/// appends applied in memory. Same per-origin chain discipline as the wire
/// store, so the disk gets no more trust than any peer.
#[derive(Default)]
pub struct Registry {
    /// The valid op-set, in ingest order. The fold is a pure function of this.
    ops: Vec<Op>,
    /// Per (glade_id, origin) chain tip: (last_seq, last_hash) — for assigning
    /// the next append's seq/prev and for chain-continuity checks on ingest.
    tips: BTreeMap<(String, String), (i64, [u8; 32])>,
}

impl Registry {
    /// A fresh, empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Materialise a registry from a snapshot — verify-as-ingest per class-2
    /// (s-sync Y2): chain continuity + seq monotonicity per origin. A rejected
    /// op and its chain suffix are excluded (Y3); policy records fail CLOSED.
    /// Returns the count of quarantined (rejected) records as load evidence.
    pub fn from_snapshot(snap: &SystemSnapshot) -> (Registry, usize) {
        let mut reg = Registry::new();
        let mut rejected = 0usize;
        // Track chains whose tail is poisoned so the suffix is dropped too.
        let mut poisoned: BTreeMap<(String, String), bool> = BTreeMap::new();
        for bytes in &snap.records {
            let op = Op::from_cbor(&cbor::decode(bytes));
            let chain = (op.glade_id.clone(), op.origin.clone());
            if *poisoned.get(&chain).unwrap_or(&false) {
                rejected += 1; // suffix of an already-rejected op
                continue;
            }
            match reg.ingest(op) {
                Ok(()) => {}
                Err(_) => {
                    rejected += 1;
                    poisoned.insert(chain, true);
                }
            }
        }
        (reg, rejected)
    }

    /// Ingest a fully-formed op with per-origin chain checks (the shared
    /// verify path for both live appends and disk load).
    fn ingest(&mut self, op: Op) -> Result<(), RegistryError> {
        let chain = (op.glade_id.clone(), op.origin.clone());
        if let Some(&(last_seq, last_hash)) = self.tips.get(&chain) {
            if op.seq <= last_seq {
                // A record at or below the tip with a different hash is a fork.
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
        Ok(())
    }

    /// Is there a NodeRecord for `node_id` in the fold? The boot ladder uses
    /// this for the class-1↔class-2 identity match: our derived NodeId MUST
    /// correspond to our own NodeRecord (else it is a first boot, or — if the
    /// key was replaced — identity loss, and the mesh rejects us).
    pub fn has_node(&self, node_id: &str) -> bool {
        self.fold_iter(G_NODES)
            .into_iter()
            .any(|o| NodeRecord::from_cbor(&cbor::decode(&o.payload)).node_id == node_id)
    }

    /// Decoded records of one kind, in deterministic (origin, seq) order —
    /// the fold's input. Time never enters here.
    fn fold_iter(&self, glade_id: &str) -> Vec<&Op> {
        let mut v: Vec<&Op> = self.ops.iter().filter(|o| o.glade_id == glade_id).collect();
        v.sort_by(|a, b| (a.origin.as_str(), a.seq).cmp(&(b.origin.as_str(), b.seq)));
        v
    }

    /// Append `rec` under `origin`'s chain and hand back the built op — the
    /// runtime directory-write path (`claims.rs`) seeds/fans/pushes the SAME
    /// bytes it persisted; `RegistryApi::append` delegates here.
    pub fn append_returning(&mut self, rec: Record, origin: &str) -> Result<Op, RegistryError> {
        let glade_id = rec.glade_id();
        let chain = (glade_id.to_string(), origin.to_string());
        let (seq, prev) = match self.tips.get(&chain) {
            Some(&(last_seq, last_hash)) => (last_seq + 1, Some(last_hash.to_vec())),
            None => (0, None),
        };
        let op = Op {
            share: HOME.into(),
            glade_id: glade_id.into(),
            key: vec![],
            origin: origin.into(),
            seq,
            prev,
            lamport: seq,
            refs: vec![],
            shape: Shape::Log,
            payload: rec.encode(),
        };
        self.ingest(op.clone())?;
        Ok(op)
    }

    /// Is a byte-identical record already in the fold? The diff basis for
    /// idempotent minting — the same rule `appdecl::register` applies.
    pub fn contains(&self, glade_id: &str, payload: &[u8]) -> bool {
        self.ops.iter().any(|o| o.glade_id == glade_id && o.payload == payload)
    }
}

impl RegistryApi for Registry {
    fn append(&mut self, rec: Record, origin: &str) -> Result<(), RegistryError> {
        self.append_returning(rec, origin).map(|_| ())
    }

    fn who_serves(&self, workspace: &str, now_ms: i64) -> Option<String> {
        self.fold_iter(G_CLAIMS)
            .into_iter()
            .map(|o| ServeClaim::from_cbor(&cbor::decode(&o.payload)))
            .filter(|c| c.share == workspace && c.lease_expiry_ms > now_ms) // read-time expiry
            .max_by_key(|c| c.epoch) // highest live epoch wins
            .map(|c| c.node)
    }

    fn replicas_of(&self, share: &str) -> Vec<String> {
        // LWW-per-workspace: the latest (origin, seq) WorkspaceEntry wins.
        let mut latest: Option<WorkspaceEntry> = None;
        for o in self.fold_iter(G_WORKSPACES) {
            let e = WorkspaceEntry::from_cbor(&cbor::decode(&o.payload));
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
            .map(|o| CapabilityRevocation::from_cbor(&cbor::decode(&o.payload)))
            .any(|r| r.principal == principal && r.share == share);
        if revoked {
            return vec![];
        }
        let mut verbs: Vec<String> = self
            .fold_iter(G_GRANTS)
            .into_iter()
            .map(|o| CapabilityGrant::from_cbor(&cbor::decode(&o.payload)))
            .filter(|g| g.principal == principal && g.share == share)
            .flat_map(|g| g.verbs)
            .collect();
        verbs.sort();
        verbs.dedup();
        verbs
    }

    fn nodes_of(&self, operator: &str) -> Vec<String> {
        let mut nodes: Vec<String> = self
            .fold_iter(G_NODES)
            .into_iter()
            .map(|o| NodeRecord::from_cbor(&cbor::decode(&o.payload)))
            .filter(|n| n.operator == operator)
            .map(|n| n.node_id)
            .collect();
        nodes.sort();
        nodes.dedup();
        nodes
    }

    fn snapshot(&self) -> SystemSnapshot {
        let records = self.ops.iter().map(|o| cbor::encode(&o.to_cbor())).collect();
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
        SystemSnapshot { records, heads }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let op = Op::from_cbor(&cbor::decode(bytes));
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
            let op = Op::from_cbor(&cbor::decode(b));
            op.origin == "peerX" && op.seq == 0
        }).unwrap();
        let mut op = Op::from_cbor(&cbor::decode(&snap.records[target]));
        op.payload.push(0xff); // malicious edit — indistinguishable from a bad sync chunk
        snap.records[target] = cbor::encode(&op.to_cbor());
        let (reg, rejected) = Registry::from_snapshot(&snap);
        assert!(rejected >= 1, "the tampered op (and its suffix) is quarantined");
        // the honest records still fold.
        assert_eq!(reg.nodes_of("gianni"), vec!["glade-local", "peer1"]);
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
}
