//! Per-chain append-log store (P1.S1; zones, GladeZones.md).
//!
//! The authoritative unit is the **chain** `(share, glade_id, key, origin)`: one
//! monotonic `seq` sequence per origin within a `(share, glade_id, key)` zone.
//! The zone `key` is part of the chain axis — a private zone must be filterable
//! from what a peer receives, and a hash chain can't be filtered and still
//! verify (the `prev` links break), so each zone is its own chain (this refines
//! Decisions D8; `glade_id` rides the axis as before). The on-disk journal stays
//! per-`(share, origin)` — it is just an op log, regrouped into chains on `open`
//! by replaying each op's own `(share, glade_id, key, origin)`. Chain-hash /
//! equivocation verification (P1.S4) is per-chain.
//!
//! The `home` share is the directory's, and its ops are signed (plan Step
//! 4.1b): one lands only if it verifies (`envelope.rs`), and its chain starts
//! at seq 0. `open` checks each `home` journal the same way, and sets aside
//! one that does not verify.
//!
//! A checkpoint (plan Step 4.5c, `checkpoint.rs`) gives the `home` chain it
//! folds a floor: the chain keeps no op at or below its base, and goes on
//! above it, and its origin's journal is rewritten without them. The register
//! keeps the newest checkpoint of each chain, in its origin's chain on
//! `dir.checkpoints`, which a serve sends first.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use glade_wire::cbor;
use glade_wire::generated::{Head, Op, Shape, StreamHeads};
use glade_wire::swmr::{decode_swmr, SwmrPayloadError};

use crate::chain::op_hash;
use crate::checkpoint::{self, Against, Checkpoint, Floor, Placement};
use crate::envelope::{self, Format, Refused};
use crate::registry::{entry_sync, G_CHECKPOINTS, HOME};
use crate::sysdir::today;

/// Outcome of an append.
#[derive(Debug, PartialEq)]
pub enum Append {
    /// New op, persisted and indexed.
    Appended,
    /// `seq` already present with the *same* hash (idempotent re-delivery).
    Duplicate,
    /// `seq` below the first op its chain holds: taken as seen, and not held,
    /// so nothing here can judge it. The client path answers `Retention`
    /// (GladeSubstrateV1 §6, R2).
    BelowRetained,
}

/// A self-contained equivocation proof (GQ-9, SY4): two validly-shaped ops
/// signed into the SAME `(origin, zone, seq)` slot with different hashes. The
/// origin forked its own history; the proof convicts the origin, not the
/// carrier. `a` is the op the store already held; `b` is the conflicting
/// arrival. The chain id is derivable from either (both share it).
#[derive(Debug, Clone, PartialEq)]
pub struct EquivProof {
    pub a: Op,
    pub b: Op,
}

impl EquivProof {
    /// The forked `(share, glade_id, key, origin, seq)` slot.
    pub fn slot(&self) -> (String, String, Vec<u8>, String, i64) {
        (self.a.share.clone(), self.a.glade_id.clone(), self.a.key.clone(), self.a.origin.clone(), self.a.seq)
    }
}

#[derive(Debug)]
pub enum StoreError {
    /// Non-contiguous: an origin's log must advance by exactly one.
    Gap { expected: i64, got: i64 },
    /// A second op at an existing `(origin, seq)` with a *different* hash — a
    /// forked per-origin chain (GQ-9). Rejected, never folded.
    Equivocation { origin: String, seq: i64 },
    /// A new op's `prev` does not match its predecessor's hash.
    ChainBreak { origin: String, seq: i64 },
    /// The SWMR action envelope is not `glade.swmr.adapter/v1`.
    InvalidSwmrPayload { error: SwmrPayloadError },
    /// A SWMR surface already has a different authenticated writer origin.
    SwmrWriterConflict { expected: String, got: String },
    /// SWMR and a multi-writer fold MUST NOT share one zone-surface.
    ShapeConflict { expected: Shape, got: Shape },
    /// A `home` op that does not verify (plan Step 4.1b): unsigned, forged,
    /// or not the directory's form or kind.
    Unverified { origin: String, seq: i64, why: Refused },
    /// A checkpoint whose base moves back from the one held, or that names
    /// another hash there (plan Step 4.5c): refused, the pair kept as its
    /// proof.
    Rewrite {
        origin: String,
        seq: i64,
    },
    Io(std::io::Error),
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// Why an op was refused, as a line of text (plan Step 4.1b's part 2
/// reports a peer's refused records so).
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Gap { expected, got } => {
                write!(f, "a gap: expected seq {expected}, got {got}")
            }
            StoreError::Equivocation { origin, seq } => write!(f, "a fork at ({origin},{seq})"),
            StoreError::ChainBreak { origin, seq } => {
                write!(f, "a chain break at ({origin},{seq})")
            }
            StoreError::InvalidSwmrPayload { error } => {
                write!(f, "an invalid SWMR envelope: {error:?}")
            }
            StoreError::SwmrWriterConflict { expected, got } => {
                write!(f, "a second SWMR writer: expected {expected}, got {got}")
            }
            StoreError::ShapeConflict { expected, got } => {
                write!(f, "a shape conflict: expected {expected:?}, got {got:?}")
            }
            StoreError::Unverified { origin, seq, why } => {
                write!(f, "({origin},{seq}) does not verify: {why}")
            }
            StoreError::Rewrite { origin, seq } => write!(f, "a rewrite at ({origin},{seq})"),
            StoreError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

/// A chain identity: `(share, glade_id, key, origin)`. The zone `key` joins the
/// axis so each zone is an independently contiguous, independently shippable
/// chain (GladeZones.md).
type ChainId = (String, String, Vec<u8>, String);

fn chain_of(op: &Op) -> ChainId {
    (op.share.clone(), op.glade_id.clone(), op.key.clone(), op.origin.clone())
}

/// Each chain's ops, in order.
type Logs = BTreeMap<ChainId, Vec<Op>>;

/// The register (plan Step 4.5c): for each chain a checkpoint folds, the
/// newest checkpoint held for it, and the floor it gives that chain.
type Register = BTreeMap<ChainId, (Op, Floor)>;

/// Append-only per-chain op store, persisted under `root`.
pub struct Store {
    root: PathBuf,
    logs: BTreeMap<ChainId, Vec<Op>>,
    /// The register (plan Step 4.5c). Each checkpoint in it is also in the
    /// log of its own chain, on `dir.checkpoints`.
    register: Register,
    /// Recorded equivocation proofs (persisted under `<root>/proofs/`), in
    /// detection order. A fork is data with a signature on it — kept, not lost.
    proofs: Vec<EquivProof>,
    /// What `open` set aside: the `home` journals that did not verify.
    aside: Option<SetAside>,
}

/// The `home` journals `open` set aside (plan Step 4.1b; `GladeNodeSigning.md`
/// D8): each renamed, whole, to `<name>.legacy-<date>`, which `open` never
/// replays, because an op in it did not verify.
#[derive(Debug, PartialEq)]
pub struct SetAside {
    pub journals: usize,
    pub records: usize,
    pub date: String,
}

/// The line both composition roots print once the served store is open.
impl std::fmt::Display for SetAside {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (journals, records, date) = (self.journals, self.records, &self.date);
        write!(
            f,
            "set aside {journals} journal(s) of the served store's home share ({records} record(s)) that do not verify, renamed *.legacy-{date}"
        )
    }
}

impl Store {
    /// Open (and replay) a store rooted at `root`, creating it if absent. The
    /// journal is per-`(share, origin)` file; each op is regrouped into its
    /// chain `(share, glade_id, key, origin)` from its own fields, so one file
    /// can feed several zone-chains. File order preserves per-chain seq order.
    ///
    /// A `home` journal is replayed only if it verifies as its ops would if
    /// appended one by one (plan Step 4.1b). One that does not, such as any
    /// journal written before the step, is renamed aside whole, and
    /// [`Store::set_aside`] reports it. One holding a record in a format this
    /// build does not know refuses the open, and is left as it is. Its
    /// checkpoints are replayed first (plan Step 4.5c), and one that still
    /// holds ops they cover, as a crash before the rewrite would leave it, is
    /// taken without them and rewritten.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let root = root.into();
        let mut logs: BTreeMap<ChainId, Vec<Op>> = BTreeMap::new();
        let (mut register, mut covered) = (Register::new(), Vec::new());
        let mut aside = SetAside {
            journals: 0,
            records: 0,
            date: today(),
        };
        if root.exists() {
            for share_ent in fs::read_dir(&root)? {
                let share_ent = share_ent?;
                if !share_ent.file_type()?.is_dir() {
                    continue;
                }
                // The equivocation-proof journal lives at `<root>/proofs/`; it is
                // NOT a share (share dirs are hex, never "proofs") — skip it here,
                // it is replayed separately below.
                if share_ent.file_name() == "proofs" {
                    continue;
                }
                let home = share_ent.file_name().to_string_lossy() == hex(HOME);
                for log_ent in fs::read_dir(share_ent.path())? {
                    let log_ent = log_ent?;
                    let fname = log_ent.file_name().to_string_lossy().to_string();
                    if fname.ends_with(".log") {
                        let ops = read_log(&log_ent.path())?;
                        let unknown = |op: &&Op| home && envelope::format(op) == Format::Unknown;
                        if let Some(op) = ops.iter().find(unknown) {
                            return Err(envelope::unreadable(&log_ent.path(), op).into());
                        }
                        if !home {
                            for op in ops {
                                logs.entry(chain_of(&op)).or_default().push(op);
                            }
                            continue;
                        }
                        let Some((held, floors, covers)) = replayed(&ops) else {
                            let legacy = format!("{fname}.legacy-{}", aside.date);
                            fs::rename(
                                log_ent.path(),
                                unused_path(&share_ent.path(), &legacy, ""),
                            )?;
                            aside.journals += 1;
                            aside.records += ops.len();
                            continue;
                        };
                        if covers {
                            covered.extend(ops.first().map(|op| op.origin.clone()));
                        }
                        logs.extend(held);
                        register.extend(floors);
                    }
                }
            }
        }
        let proofs = read_proofs(&proofs_path(&root))?;
        let aside = (aside.journals > 0).then_some(aside);
        let store = Store {
            root,
            logs,
            register,
            proofs,
            aside,
        };
        // Rewritten once the directory is read, so no entry it lists moves.
        for origin in covered {
            store.rewrite(&origin)?;
        }
        Ok(store)
    }

    /// Q4-A: permanently retire this legacy store for a later verified migration cut.
    /// Draft consumer signature only; no successful seal is implemented yet.
    pub fn seal_legacy(&mut self) -> Result<(), StoreError> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Q4-A legacy store seal is not implemented",
        ).into())
    }

    /// The `home` journals `open` set aside, if any.
    pub fn set_aside(&self) -> Option<&SetAside> {
        self.aside.as_ref()
    }

    /// Append `op` to its `(share, glade_id, key, origin)` chain, with per-chain
    /// checks (P1.S4, GQ-9):
    /// - `seq <= last.seq`: idempotent if the stored op has the same hash;
    ///   **equivocation** (rejected) if a different hash — a forked chain;
    ///   below the chain's first held op, taken as seen but not held.
    /// - `seq == last.seq + 1`: if `prev` is present it must equal the
    ///   predecessor's hash (else **chain break**); absent `prev` is accepted
    ///   unverified (M-LIMP lenient — honest clients always set it).
    /// - otherwise a forward **gap**.
    ///
    /// A `home` op must verify before anything new of it is kept, and its
    /// chain starts at seq 0 (plan Step 4.1b), or above its floor (plan Step
    /// 4.5c): an op at or below it is taken as seen, `BelowRetained`. A
    /// checkpoint placed moves its chain's floor and rewrites its origin's
    /// journal. A byte-identical repeat was checked when it first landed.
    pub fn append(&mut self, op: Op) -> Result<Append, StoreError> {
        self.validate_surface_contract(&op)?;
        let chain = chain_of(&op);
        // Classify against the current tail without holding a borrow of `logs`
        // across the proof write / push (equivocation records into `proofs`).
        let home = op.share == HOME;
        let verdict = match home {
            true => classify_home(&self.logs, &self.register, &op),
            false => classify(self.logs.get(&chain), &op),
        };
        if home {
            checked(&verdict, &op)?;
        }
        match verdict {
            Verdict::Duplicate => Ok(Append::Duplicate),
            Verdict::BelowRetained => Ok(Append::BelowRetained),
            Verdict::Gap { expected, got } => Err(StoreError::Gap { expected, got }),
            Verdict::ChainBreak => Err(StoreError::ChainBreak { origin: op.origin, seq: op.seq }),
            Verdict::Equivocation(stored) => {
                // Two signed ops, one slot — persist the fork proof, then reject.
                self.record_equivocation(EquivProof { a: stored, b: op.clone() })?;
                Err(StoreError::Equivocation { origin: op.origin, seq: op.seq })
            }
            Verdict::Rewrite(stored) => {
                let proof = EquivProof {
                    a: stored,
                    b: op.clone(),
                };
                self.record_equivocation(proof)?;
                let (origin, seq) = (op.origin, op.seq);
                Err(StoreError::Rewrite { origin, seq })
            }
            Verdict::Appended => {
                append_to_log(&self.root, &op)?;
                self.logs.entry(chain).or_default().push(op);
                Ok(Append::Appended)
            }
            Verdict::Placed(folded, floor) => {
                let origin = op.origin.clone();
                place(&mut self.logs, &mut self.register, op, folded, floor);
                self.rewrite(&origin)?;
                Ok(Append::Appended)
            }
        }
    }

    /// Rewrite `origin`'s journal of the `home` share to the ops held of it,
    /// checkpoints first (plan Step 4.5c): into a temporary file, synced,
    /// renamed over the journal, and the directory synced, 4.4's order. The
    /// temporary name does not end in `.log`, so `open` never replays one
    /// that a crash left.
    fn rewrite(&self, origin: &str) -> Result<(), StoreError> {
        let ours = |((share, _, _, of), _): &(&ChainId, &Vec<Op>)| share == HOME && of == origin;
        let first = |((_, stream, _, _), _): &(&ChainId, &Vec<Op>)| stream == G_CHECKPOINTS;
        let (checkpoints, rest): (Vec<_>, Vec<_>) = self.logs.iter().filter(ours).partition(first);
        let ops = checkpoints.into_iter().chain(rest).flat_map(|(_, log)| log);
        let records: Vec<u8> = ops.flat_map(framed).collect();
        let dir = self.root.join(hex(HOME));
        let temp = dir.join(format!("{}.rewrite", hex(origin)));
        fs::create_dir_all(&dir)?;
        let mut file = fs::File::create(&temp)?;
        file.write_all(&records)?;
        file.sync_all()?;
        fs::rename(&temp, log_path(&self.root, HOME, origin))?;
        entry_sync::sync(&dir)?;
        Ok(())
    }

    /// Validate exact shape capability before any journal or index mutation.
    fn validate_surface_contract(&self, op: &Op) -> Result<(), StoreError> {
        if op.shape == Shape::Swmr {
            decode_swmr(&op.payload).map_err(|error| StoreError::InvalidSwmrPayload { error })?;
        }

        for ((share, glade_id, key, origin), log) in &self.logs {
            if share != &op.share || glade_id != &op.glade_id || key != &op.key || log.is_empty() {
                continue;
            }
            let existing_shape = log[0].shape;
            if (op.shape == Shape::Swmr
                || existing_shape == Shape::Swmr
                || op.shape == Shape::Crdt
                || existing_shape == Shape::Crdt)
                && existing_shape != op.shape
            {
                return Err(StoreError::ShapeConflict { expected: existing_shape, got: op.shape });
            }
            if op.shape == Shape::Swmr && origin != &op.origin {
                return Err(StoreError::SwmrWriterConflict {
                    expected: origin.clone(),
                    got: op.origin.clone(),
                });
            }
        }
        Ok(())
    }

    /// Persist an equivocation proof (both ops) under `<root>/proofs/` and keep
    /// it in memory. Idempotent-ish: the same fork re-detected appends again,
    /// which is harmless (proofs are evidence, not state).
    fn record_equivocation(&mut self, proof: EquivProof) -> Result<(), StoreError> {
        append_proof(&proofs_path(&self.root), &proof)?;
        self.proofs.push(proof);
        Ok(())
    }

    /// Recorded equivocation proofs, in detection order.
    pub fn equivocation_proofs(&self) -> &[EquivProof] {
        &self.proofs
    }

    /// Every zone-surface `(share, glade_id, key)` this store holds, deduped.
    pub fn zones(&self) -> Vec<(String, String, Vec<u8>)> {
        let mut zs: Vec<_> = self
            .logs
            .keys()
            .map(|(s, g, k, _)| (s.clone(), g.clone(), k.clone()))
            .collect();
        zs.dedup();
        zs
    }

    /// Every zone's version vector as `StreamHeads` — the per-(origin, zone)
    /// HEADS exchange unit. Each `Head` carries the origin's chain-head hash, so
    /// a peer can spot a same-seq/different-head fork straight off the vectors.
    pub fn all_heads(&self) -> Vec<StreamHeads> {
        self.zones()
            .into_iter()
            .map(|(share, glade_id, key)| self.zone_heads(&share, &glade_id, &key))
            .collect()
    }

    /// One zone's version vector: each origin's last seq, with the hash of its
    /// op there, or its floor while it holds none above it (plan Step 4.5c).
    /// The subscribe ack names it (GladeSubstrateV1 §6, R5).
    pub fn zone_heads(&self, share: &str, glade_id: &str, key: &[u8]) -> StreamHeads {
        let heads = self
            .logs
            .iter()
            .filter(|((s, g, k, _), _)| s == share && g == glade_id && k.as_slice() == key)
            .filter_map(|(chain, log)| {
                self.tip(chain, log).map(|(seq, hash)| Head {
                    origin: chain.3.clone(),
                    seq,
                    hash: Some(hash.to_vec()),
                })
            })
            .collect();
        StreamHeads {
            share: share.into(),
            glade_id: glade_id.into(),
            key: key.to_vec(),
            heads,
        }
    }

    /// Ops for a chain `(share, glade_id, key, origin)` with `seq > from_seq`, in
    /// order (the resume tail for one origin within a zone).
    pub fn scan(&self, share: &str, glade_id: &str, key: &[u8], origin: &str, from_seq: i64) -> Vec<Op> {
        self.logs
            .get(&(share.to_string(), glade_id.to_string(), key.to_vec(), origin.to_string()))
            .map(|log| log.iter().filter(|o| o.seq > from_seq).cloned().collect())
            .unwrap_or_default()
    }

    /// Per-origin head seq for a zone `(share, glade_id, key)` — its resume
    /// vector (origin -> max seq), a floor where nothing is held above it
    /// (plan Step 4.5c). Different zones (keys) never mix.
    pub fn heads(&self, share: &str, glade_id: &str, key: &[u8]) -> Vec<(String, i64)> {
        self.logs
            .iter()
            .filter(|((s, g, k, _), _)| s == share && g == glade_id && k.as_slice() == key)
            .filter_map(|(chain, log)| self.tip(chain, log).map(|(seq, _)| (chain.3.clone(), seq)))
            .collect()
    }

    /// A chain's tip: its last op's seq and hash, or its floor while it holds
    /// no op above it (plan Step 4.5c).
    fn tip(&self, chain: &ChainId, log: &[Op]) -> Option<Floor> {
        let last = log.last().map(|op| (op.seq, op_hash(op)));
        last.or_else(|| self.register.get(chain).map(|(_, floor)| *floor))
    }
}

/// The first of `<stem><ext>`, `<stem>-2<ext>`, `<stem>-3<ext>` and so on in
/// `dir` that names nothing yet: a legacy file never replaces another.
pub(crate) fn unused_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut n = 1;
    loop {
        let path = match n {
            1 => dir.join(format!("{stem}{ext}")),
            _ => dir.join(format!("{stem}-{n}{ext}")),
        };
        if !path.exists() {
            return path;
        }
        n += 1;
    }
}

/// The append verdict for one op against its chain's current tail. Split out so
/// `append` can decide without holding a borrow of `logs` across a proof write.
enum Verdict {
    Appended,
    Duplicate,
    BelowRetained,
    Gap { expected: i64, got: i64 },
    ChainBreak,
    /// An op already sits at this `(origin, seq)` with a different hash — a fork.
    /// Carries the stored op so the proof can be assembled. At a chain's base
    /// (plan Step 4.5c), the pair is the op there and a checkpoint naming
    /// another hash.
    Equivocation(Op),
    /// A checkpoint the register takes (plan Step 4.5c): the chain it folds,
    /// and the floor it gives that chain.
    Placed(ChainId, Floor),
    /// A checkpoint whose base moves back from the one held, or names another
    /// hash there (plan Step 4.5c). Carries the one held, for the proof.
    Rewrite(Op),
}

/// [`classify`], with the `home` share's rules. A checkpoint is judged for
/// the register ([`classify_checkpoint`]). An op of a chain with a floor (plan
/// Step 4.5c) is judged against it, and above it from its base. Any other
/// chain starts at seq 0 (plan Step 4.1b): an op whose predecessor is not
/// held cannot have it checked (B5).
fn classify_home(logs: &Logs, register: &Register, op: &Op) -> Verdict {
    if op.glade_id == G_CHECKPOINTS {
        return classify_checkpoint(logs, register, op);
    }
    let chain = chain_of(op);
    let log = logs.get(&chain).filter(|log| !log.is_empty());
    let Some((checkpoint, floor)) = register.get(&chain) else {
        return match log {
            None if op.seq != 0 => Verdict::Gap {
                expected: 0,
                got: op.seq,
            },
            _ => classify(log, op),
        };
    };
    match checkpoint::against(*floor, op) {
        Against::Covered => Verdict::BelowRetained,
        Against::Fork => Verdict::Equivocation(checkpoint.clone()),
        Against::Above if log.is_none() => follows(*floor, op),
        Against::Above => classify(log, op),
    }
}

/// Where a checkpoint lands (plan Step 4.5c): [`checkpoint::place`] against
/// the one held for the chain it folds, then against the op held at its
/// base, which must hash to it. One its own check refuses is judged kept, so
/// that [`checked`] refuses it: `verify` makes that check.
fn classify_checkpoint(logs: &Logs, register: &Register, op: &Op) -> Verdict {
    let Ok(Checkpoint { stream, floor }) = checkpoint::check(op) else {
        return Verdict::Appended;
    };
    let folded = (HOME.to_string(), stream, Vec::new(), op.origin.clone());
    if let Some((held, at)) = register.get(&folded) {
        match checkpoint::place((op, floor), Some((held, *at))) {
            Placement::Placed => {}
            Placement::Duplicate => return Verdict::Duplicate,
            Placement::Seen => return Verdict::BelowRetained,
            Placement::Fork => return Verdict::Equivocation(held.clone()),
            Placement::ChainBreak => return Verdict::ChainBreak,
            Placement::Rewrite => return Verdict::Rewrite(held.clone()),
        }
    }
    let log = logs.get(&folded).map(Vec::as_slice).unwrap_or_default();
    match log.iter().find(|held| held.seq == floor.0) {
        Some(held) if checkpoint::against(floor, held) == Against::Fork => {
            Verdict::Equivocation(held.clone())
        }
        _ => Verdict::Placed(folded, floor),
    }
}

/// Place `op`, a checkpoint that folds `folded` at `floor` (plan Step 4.5c):
/// that chain keeps no op at or below the base, and the checkpoint replaces
/// the one held for it, in the register and in the log of its own chain,
/// which holds its origin's newest checkpoint of each chain.
fn place(logs: &mut Logs, register: &mut Register, op: Op, folded: ChainId, floor: Floor) {
    let log = logs.entry(folded.clone()).or_default();
    log.retain(|held| held.seq > floor.0);
    let own = chain_of(&op);
    register.insert(folded, (op, floor));
    let origins = register.iter().filter(|(chain, _)| chain.3 == own.3);
    let mut newest: Vec<Op> = origins.map(|(_, (op, _))| op.clone()).collect();
    newest.sort_by_key(|op| op.seq);
    logs.insert(own, newest);
}

/// A `home` op that would be kept, or would convict its origin of a fork or
/// a rewrite, must verify (plan Steps 4.1b and 4.5c).
fn checked(verdict: &Verdict, op: &Op) -> Result<(), StoreError> {
    use Verdict::{Appended, Equivocation, Placed, Rewrite};
    let convicts = matches!(verdict, Equivocation(_) | Rewrite(_));
    if !convicts && !matches!(verdict, Appended | Placed(..)) {
        return Ok(());
    }
    envelope::verify(op).map_err(|why| {
        let (origin, seq) = (op.origin.clone(), op.seq);
        StoreError::Unverified { origin, seq, why }
    })
}

/// A `home` journal's ops, each as [`Store::append`] would take it after the
/// ones before it (plan Step 4.1b), checkpoints first, so that each chain
/// they fold starts at its floor (plan Step 4.5c): the chains they leave, the
/// register, and whether one was covered, and so left out; or `None`, if one
/// would not be taken.
fn replayed(ops: &[Op]) -> Option<(Logs, Register, bool)> {
    let (mut logs, mut register, mut covered) = (Logs::new(), Register::new(), false);
    let (checkpoints, rest): (Vec<&Op>, Vec<&Op>) =
        ops.iter().partition(|op| op.glade_id == G_CHECKPOINTS);
    for op in checkpoints.into_iter().chain(rest) {
        let verdict = classify_home(&logs, &register, op);
        let taken = checked(&verdict, op).is_ok();
        match verdict {
            Verdict::BelowRetained => covered = true,
            Verdict::Appended if taken => logs.entry(chain_of(op)).or_default().push(op.clone()),
            Verdict::Placed(folded, floor) if taken => {
                place(&mut logs, &mut register, op.clone(), folded, floor)
            }
            _ => return None,
        }
    }
    Some((logs, register, covered))
}

fn classify(log: Option<&Vec<Op>>, op: &Op) -> Verdict {
    let Some(last) = log.and_then(|l| l.last()) else { return Verdict::Appended };
    if op.seq <= last.seq {
        // Safe to unwrap the log: we found `last` in it.
        return match log.unwrap().iter().find(|o| o.seq == op.seq) {
            Some(stored) if op_hash(stored) == op_hash(op) => Verdict::Duplicate,
            Some(stored) => Verdict::Equivocation(stored.clone()),
            None => Verdict::BelowRetained, // below retained range — seen, not held
        };
    }
    follows((last.seq, op_hash(last)), op)
}

/// Whether `op` follows its chain's tip, `(seq, hash)`: it must be at the
/// next seq, and a `prev`, when present, must be the tip's hash.
fn follows((seq, hash): (i64, [u8; 32]), op: &Op) -> Verdict {
    if op.seq != seq + 1 {
        return Verdict::Gap {
            expected: seq + 1,
            got: op.seq,
        };
    }
    if let Some(prev) = &op.prev {
        if prev.as_slice() != hash {
            return Verdict::ChainBreak;
        }
    }
    Verdict::Appended
}

fn proofs_path(root: &Path) -> PathBuf {
    root.join("proofs").join("equivocations.log")
}

/// Append a proof as two length-prefixed op CBORs (a then b), mirroring the op
/// journal's framing, in one write. The chain/seq is recoverable from the ops
/// themselves.
fn append_proof(path: &Path, proof: &EquivProof) -> Result<(), StoreError> {
    fs::create_dir_all(path.parent().unwrap())?;
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let mut both = framed(&proof.a);
    both.extend(framed(&proof.b));
    f.write_all(&both)?;
    Ok(())
}

/// One journal record: the op's CBOR, prefixed by its length (u32, little
/// endian), written as one buffer so a crash cannot fall between the two
/// (plan Step 4.4).
fn framed(op: &Op) -> Vec<u8> {
    let bytes = cbor::encode(&op.to_cbor());
    let mut record = Vec::with_capacity(4 + bytes.len());
    record.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    record.extend_from_slice(&bytes);
    record
}

fn read_proofs(path: &Path) -> Result<Vec<EquivProof>, StoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    // Same framing: a flat run of ops, paired up. A pair holding an op that
    // cannot be read is dropped whole, so the pairs after it stay pairs (F15b).
    let ops = read_records(path)?;
    let pair = |p: &[Option<Op>]| {
        let (a, b) = (p[0].clone()?, p[1].clone()?);
        Some(EquivProof { a, b })
    };
    Ok(ops.chunks_exact(2).filter_map(pair).collect())
}

fn log_path(root: &Path, share: &str, origin: &str) -> PathBuf {
    root.join(hex(share)).join(format!("{}.log", hex(origin)))
}

fn append_to_log(root: &Path, op: &Op) -> Result<(), StoreError> {
    let path = log_path(root, &op.share, &op.origin);
    fs::create_dir_all(path.parent().unwrap())?;
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(&framed(op))?;
    Ok(())
}

/// Every op one journal holds, in order: [`read_records`]'s, less the ones
/// it could not read.
fn read_log(path: &Path) -> Result<Vec<Op>, StoreError> {
    Ok(read_records(path)?.into_iter().flatten().collect())
}

/// Every complete record in one journal, in order. A tail too short for its
/// record is what an interrupted append leaves: it is skipped, as it always
/// was, and now also cut from the file, so the next append starts on a record
/// boundary instead of after the torn bytes (plan Step 4.4). A journal whose
/// records are all complete is not written to. A record `cbor::try_decode`
/// or `Op::from_cbor` refuses is `None`, and said on stderr in one line
/// naming its place (F15b): the file keeps it.
fn read_records(path: &Path) -> Result<Vec<Option<Op>>, StoreError> {
    let data = fs::read(path)?;
    let mut ops = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let len = u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        let end = i + 4 + len;
        if end > data.len() {
            break; // truncated tail — the partial record is cut below
        }
        let op = envelope::decode_op(&data[i + 4..end]);
        if let Err(why) = &op {
            let (n, file) = (ops.len() + 1, path.display());
            eprintln!("skipped an op that cannot be read: record {n} of {file} ({why})");
        }
        ops.push(op.ok());
        i = end;
    }
    if i < data.len() {
        OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(i as u64)?;
    }
    Ok(ops)
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

// A journal written as a node wrote it before plan Step 4.1b, for other
// modules' tests. A braced module, so the condition encloses the whole
// section.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Append `op` to its journal under `root` with no check, as the store's
    /// append did before plan Step 4.1b.
    pub(crate) fn journal(root: &Path, op: &Op) {
        append_to_log(root, op).expect("the journal takes the op");
    }

    /// The ops of `origin`'s journal of `share` under `root`, in its order.
    pub(crate) fn journal_of(root: &Path, share: &str, origin: &str) -> Vec<Op> {
        read_log(&log_path(root, share, origin)).expect("the journal reads")
    }

    /// The stream and seq of each op, in order.
    pub(crate) fn slots(ops: &[Op]) -> Vec<(&str, i64)> {
        ops.iter()
            .map(|op| (op.glade_id.as_str(), op.seq))
            .collect()
    }

    /// The claims chain of ten of the node whose key is `[seed; 32]`, sealed
    /// as its registry appends them, and its checkpoint at `base`, the first
    /// of its `dir.checkpoints` chain (plan Step 4.5c): the node's id, the
    /// claims and the checkpoint.
    pub(crate) fn folded(seed: u8, base: usize) -> (String, Vec<Op>, Op) {
        use crate::registry::{Record, Registry, G_CLAIMS};
        use crate::sysdata::{ChainCheckpoint, ServeClaim};
        let identity = crate::peer::NodeIdentity::from_key([seed; 32]);
        let id = crate::transport::hex(&identity.node_id);
        let mut registry = Registry::sealed(identity);
        let mut claims = Vec::new();
        for lease_expiry_ms in 1..=10 {
            let (node, share, epoch) = (id.clone(), "ws".into(), 1);
            let claim = ServeClaim {
                node,
                share,
                lease_expiry_ms,
                epoch,
            };
            let claim = registry.append_returning(Record::Serve(claim), &id);
            claims.push(claim.unwrap());
        }
        let hash = op_hash(&claims[base]).to_vec();
        let (node, stream, seq) = (id.clone(), G_CLAIMS.into(), base as i64);
        let record = ChainCheckpoint {
            node,
            stream,
            seq,
            hash,
        };
        let checkpoint = registry.append_returning(Record::Checkpoint(record), &id);
        (id, claims, checkpoint.unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::generated::{Op, Shape};
    use glade_wire::swmr::{encode_swmr, SwmrAction};

    fn op(share: &str, origin: &str, seq: i64, payload: &[u8]) -> Op {
        Op {
            share: share.into(),
            glade_id: "g".into(),
            key: vec![],
            origin: origin.into(),
            seq,
            prev: None,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-store-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn swmr_op(origin: &str, seq: i64, action: SwmrAction, body: &[u8]) -> Op {
        Op {
            shape: Shape::Swmr,
            payload: encode_swmr(action, body),
            ..op("sh", origin, seq, b"")
        }
    }

    /// A sealed record on `home`, the first of its stream in the chain of
    /// the node whose key is `seed`.
    fn sealed(seed: u8, principal: &str) -> Op {
        let record = crate::registry::Record::Principal(crate::sysdata::PrincipalRecord {
            principal: principal.into(),
        });
        crate::envelope::testing::sealed([seed; 32], record)
    }

    /// Plan Step 4.1b: the served store takes a `home` op only if it
    /// verifies, from its chain's seq 0. A bare record and a forged envelope
    /// are refused and not kept; a signed one lands, and its repeat is a
    /// duplicate; one that would begin a chain above seq 0 is a gap. An app
    /// op is taken bare, as before (D5).
    #[test]
    fn a_home_op_lands_only_signed_and_from_seq_0() {
        let mut s = Store::open(fresh("home-signed")).unwrap();
        let signed = sealed(3, "alice");
        let bare = Op {
            payload: envelope::record_bytes(&signed.payload),
            ..signed.clone()
        };
        let mut forged = signed.clone();
        let last = forged.payload.len() - 1;
        forged.payload[last] ^= 1;
        let mut refused = |op: Op, want: Refused| match s.append(op) {
            Err(StoreError::Unverified { why, .. }) => assert_eq!(why, want),
            other => panic!("expected {want:?}, got {other:?}"),
        };
        refused(bare, Refused::Unsigned);
        refused(forged, Refused::Signature);
        assert_eq!(s.all_heads(), vec![], "nothing kept");
        assert_eq!(s.append(signed.clone()).unwrap(), Append::Appended);
        assert_eq!(s.append(signed).unwrap(), Append::Duplicate);
        let late = sealed(4, "bob");
        let late = Op {
            seq: 3,
            prev: Some(vec![0; 32]),
            ..late
        };
        assert!(matches!(
            s.append(late),
            Err(StoreError::Gap {
                expected: 0,
                got: 3
            })
        ));
        assert_eq!(
            s.append(op("sh", "a", 1, b"app")).unwrap(),
            Append::Appended
        );
    }

    /// Plan Step 4.1b (D8): `open` sets aside, whole, a `home` journal that
    /// does not verify, here one a node wrote before the step, renaming it to
    /// `<name>.legacy-<date>`, and replays the rest: another node's signed
    /// journal and an app share's journal, whose bare ops it never checks.
    /// It reports what it set aside; a second `open` sets nothing aside.
    #[test]
    fn open_sets_aside_a_home_journal_that_does_not_verify() {
        let root = fresh("home-aside");
        let signed = sealed(5, "carol");
        let unsigned = Op {
            payload: envelope::record_bytes(&signed.payload),
            origin: "0b".repeat(32),
            ..signed.clone()
        };
        for op in [&unsigned, &signed, &op("sh", "a", 1, b"app")] {
            append_to_log(&root, op).unwrap();
        }
        let s = Store::open(&root).unwrap();
        let aside = s.set_aside().expect("the unsigned journal set aside");
        assert_eq!((aside.journals, aside.records), (1, 1));
        let origin = &unsigned.origin;
        assert!(
            s.scan(HOME, &signed.glade_id, &[], origin, -1).is_empty(),
            "not replayed"
        );
        let kept = s.scan(HOME, &signed.glade_id, &[], &signed.origin, -1);
        assert_eq!(kept, std::slice::from_ref(&signed));
        assert_eq!(
            s.scan("sh", "g", &[], "a", -1).len(),
            1,
            "app data untouched"
        );
        let journal = root.join(hex(HOME)).join(format!("{}.log", hex(origin)));
        assert!(!journal.exists());
        let legacy = format!("{}.legacy-{}", journal.display(), today());
        assert!(Path::new(&legacy).exists(), "kept, renamed");
        drop(s);
        assert!(Store::open(&root).unwrap().set_aside().is_none(), "once");
    }

    /// Part 2's hardening: a `home` journal holding a record in a format this
    /// build does not know (a newer build's, it may be) refuses the open,
    /// naming the journal and the record, and is left as it is, not set
    /// aside.
    #[test]
    fn open_refuses_a_home_journal_in_a_format_it_does_not_know_and_leaves_it() {
        let root = fresh("home-newer");
        let newer = envelope::testing::newer([6; 32]);
        append_to_log(&root, &newer).unwrap();
        let journal = root
            .join(hex(HOME))
            .join(format!("{}.log", hex(&newer.origin)));
        let written = fs::read(&journal).unwrap();
        let err = match Store::open(&root) {
            Err(StoreError::Io(e)) => e,
            other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let named = format!(
            "{} holds a home record this build cannot read (dir.key-rotations of node",
            journal.display()
        );
        assert!(err.to_string().starts_with(&named), "{err}");
        assert_eq!(fs::read(&journal).unwrap(), written, "left as it is");
    }

    #[test]
    fn append_and_scan_from_seq() {
        let mut s = Store::open(fresh("scan")).unwrap();
        for n in 1..=3 {
            assert_eq!(s.append(op("sh", "a", n, &[n as u8])).unwrap(), Append::Appended);
        }
        assert_eq!(s.scan("sh", "g", &[], "a", 0).len(), 3); // all
        assert_eq!(s.scan("sh", "g", &[], "a", 1).len(), 2); // seq > 1
        assert_eq!(s.scan("sh", "g", &[], "a", 3).len(), 0); // caught up
        assert_eq!(s.scan("sh", "g", &[], "missing", 0).len(), 0);
    }

    #[test]
    fn heads_per_origin() {
        let mut s = Store::open(fresh("heads")).unwrap();
        s.append(op("sh", "a", 1, b"x")).unwrap();
        s.append(op("sh", "a", 2, b"y")).unwrap();
        s.append(op("sh", "b", 1, b"z")).unwrap();
        let mut h = s.heads("sh", "g", &[]);
        h.sort();
        assert_eq!(h, vec![("a".to_string(), 2), ("b".to_string(), 1)]);
    }

    /// Zones (keys) are independent chains: the *same* (share, glade_id, origin)
    /// in two different keys keeps two separate seq sequences, and one zone's
    /// heads/scan never sees the other's ops (the privacy-by-keying property).
    #[test]
    fn keys_are_independent_chains() {
        let mut s = Store::open(fresh("zones")).unwrap();
        let commons = |seq, p: &[u8]| op("sh", "a", seq, p); // key = []
        let private = |seq, p: &[u8]| Op { key: b"self:a".to_vec(), ..op("sh", "a", seq, p) };
        // both chains start at seq 0 — independent, no equivocation across keys
        s.append(commons(0, b"c0")).unwrap();
        s.append(private(0, b"p0")).unwrap();
        s.append(commons(1, b"c1")).unwrap();
        // each zone sees only its own ops
        assert_eq!(s.heads("sh", "g", &[]), vec![("a".to_string(), 1)]);
        assert_eq!(s.heads("sh", "g", b"self:a"), vec![("a".to_string(), 0)]);
        assert_eq!(s.scan("sh", "g", &[], "a", -1).len(), 2);
        let priv_ops = s.scan("sh", "g", b"self:a", "a", -1);
        assert_eq!(priv_ops.len(), 1);
        assert_eq!(priv_ops[0].payload, b"p0");
    }

    #[test]
    fn duplicate_is_idempotent_and_gap_errors() {
        let mut s = Store::open(fresh("dupgap")).unwrap();
        s.append(op("sh", "a", 1, b"x")).unwrap();
        s.append(op("sh", "a", 2, b"y")).unwrap();
        assert_eq!(s.append(op("sh", "a", 2, b"y")).unwrap(), Append::Duplicate); // re-delivery
        assert_eq!(s.append(op("sh", "a", 1, b"x")).unwrap(), Append::Duplicate); // older
        match s.append(op("sh", "a", 5, b"q")) {
            Err(StoreError::Gap { expected, got }) => {
                assert_eq!((expected, got), (3, 5));
            }
            other => panic!("expected Gap, got {other:?}"),
        }
    }

    #[test]
    fn swmr_accepts_one_writer_snapshot_delta_and_empty_reset() {
        let mut s = Store::open(fresh("swmr-one-writer")).unwrap();
        assert_eq!(
            s.append(swmr_op("writer-a", 0, SwmrAction::Snapshot, b"whole-0")).unwrap(),
            Append::Appended,
        );
        assert_eq!(
            s.append(swmr_op("writer-a", 1, SwmrAction::Delta, b"whole-1")).unwrap(),
            Append::Appended,
        );
        assert_eq!(
            s.append(swmr_op("writer-a", 2, SwmrAction::Reset, b"")).unwrap(),
            Append::Appended,
        );
        assert_eq!(s.scan("sh", "g", &[], "writer-a", -1).len(), 3);
    }

    #[test]
    fn swmr_rejects_malformed_action_before_store_mutation() {
        let mut s = Store::open(fresh("swmr-malformed")).unwrap();
        let malformed = Op { shape: Shape::Swmr, payload: vec![1, 99], ..op("sh", "writer-a", 0, b"") };

        assert!(matches!(
            s.append(malformed),
            Err(StoreError::InvalidSwmrPayload { .. })
        ));
        assert!(s.heads("sh", "g", &[]).is_empty());
    }

    #[test]
    fn swmr_rejects_second_writer_and_shape_mixing_before_mutation() {
        let mut s = Store::open(fresh("swmr-conflicts")).unwrap();
        s.append(swmr_op("writer-a", 0, SwmrAction::Snapshot, b"whole")).unwrap();

        assert!(matches!(
            s.append(swmr_op("writer-b", 0, SwmrAction::Snapshot, b"other")),
            Err(StoreError::SwmrWriterConflict { expected, got })
                if expected == "writer-a" && got == "writer-b"
        ));
        assert!(matches!(
            s.append(op("sh", "writer-b", 0, b"value")),
            Err(StoreError::ShapeConflict { expected: Shape::Swmr, got: Shape::Value })
        ));
        assert!(s.scan("sh", "g", &[], "writer-b", -1).is_empty());
        assert_eq!(s.scan("sh", "g", &[], "writer-a", -1).len(), 1);
    }

    #[test]
    fn crdt_accepts_multiple_writers_but_rejects_shape_mixing() {
        let mut s = Store::open(fresh("crdt-multi-writer")).unwrap();
        let alice = Op { shape: Shape::Crdt, ..op("sh", "alice", 0, b"A") };
        let bob = Op { shape: Shape::Crdt, ..op("sh", "bob", 0, b"B") };
        assert_eq!(s.append(alice).unwrap(), Append::Appended);
        assert_eq!(s.append(bob).unwrap(), Append::Appended);

        assert!(matches!(
            s.append(op("sh", "legacy", 0, b"whole-value")),
            Err(StoreError::ShapeConflict { expected: Shape::Crdt, got: Shape::Value })
        ));
        assert_eq!(s.scan("sh", "g", &[], "alice", -1).len(), 1);
        assert_eq!(s.scan("sh", "g", &[], "bob", -1).len(), 1);
        assert!(s.scan("sh", "g", &[], "legacy", -1).is_empty());
    }

    #[test]
    fn valid_chain_appends() {
        let mut s = Store::open(fresh("chain-ok")).unwrap();
        let a0 = op("sh", "a", 0, b"p0"); // prev None (baseline)
        s.append(a0.clone()).unwrap();
        let mut a1 = op("sh", "a", 1, b"p1");
        a1.prev = Some(crate::chain::op_hash(&a0).to_vec());
        s.append(a1.clone()).unwrap();
        let mut a2 = op("sh", "a", 2, b"p2");
        a2.prev = Some(crate::chain::op_hash(&a1).to_vec());
        assert_eq!(s.append(a2).unwrap(), Append::Appended);
    }

    #[test]
    fn equivocation_rejected_redelivery_idempotent() {
        let mut s = Store::open(fresh("equiv")).unwrap();
        s.append(op("sh", "a", 0, b"p0")).unwrap();
        // same (origin, seq), different payload -> forked chain, rejected
        match s.append(op("sh", "a", 0, b"p0-fork")) {
            Err(StoreError::Equivocation { origin, seq }) => assert_eq!((origin.as_str(), seq), ("a", 0)),
            other => panic!("expected Equivocation, got {other:?}"),
        }
        // exact re-delivery of the real op is still idempotent
        assert_eq!(s.append(op("sh", "a", 0, b"p0")).unwrap(), Append::Duplicate);
    }

    #[test]
    fn chain_break_rejected() {
        let mut s = Store::open(fresh("break")).unwrap();
        s.append(op("sh", "a", 0, b"p0")).unwrap();
        let mut a1 = op("sh", "a", 1, b"p1");
        a1.prev = Some(vec![0xde, 0xad, 0xbe, 0xef]); // does not match hash(a0)
        match s.append(a1) {
            Err(StoreError::ChainBreak { origin, seq }) => assert_eq!((origin.as_str(), seq), ("a", 1)),
            other => panic!("expected ChainBreak, got {other:?}"),
        }
    }

    /// SY4: two signed ops in one (origin, zone, seq) slot are detected AND the
    /// proof (both ops) is persisted as a record — it survives a restart.
    #[test]
    fn equivocation_records_and_persists_proof() {
        let root = fresh("equiv-proof");
        {
            let mut s = Store::open(&root).unwrap();
            s.append(op("sh", "a", 0, b"p0")).unwrap();
            let err = s.append(op("sh", "a", 0, b"p0-fork")).unwrap_err();
            assert!(matches!(err, StoreError::Equivocation { .. }));
            assert_eq!(s.equivocation_proofs().len(), 1);
            let p = &s.equivocation_proofs()[0];
            assert_eq!(p.a.payload, b"p0"); // the op we held
            assert_eq!(p.b.payload, b"p0-fork"); // the conflicting arrival
            assert_eq!(p.slot(), ("sh".into(), "g".into(), vec![], "a".into(), 0));
        }
        // reopen: the real op is in the journal, the proof in its own record.
        let s = Store::open(&root).unwrap();
        assert_eq!(s.equivocation_proofs().len(), 1);
        assert_eq!(s.equivocation_proofs()[0].b.payload, b"p0-fork");
        assert_eq!(s.scan("sh", "g", &[], "a", -1).len(), 1); // fork never folded
    }

    /// `all_heads` yields one `StreamHeads` per zone, each head carrying the
    /// origin's 32-byte chain-head hash (the same-seq/different-head tripwire).
    #[test]
    fn all_heads_are_per_zone_with_chain_hash() {
        let mut s = Store::open(fresh("all-heads")).unwrap();
        s.append(op("sh", "a", 0, b"x")).unwrap(); // commons zone
        s.append(Op { key: b"self:a".to_vec(), ..op("sh", "a", 0, b"y") }).unwrap(); // private zone
        let ah = s.all_heads();
        assert_eq!(ah.len(), 2); // two zones, independent
        for sh in &ah {
            assert_eq!(sh.heads.len(), 1);
            assert_eq!(sh.heads[0].seq, 0);
            assert_eq!(sh.heads[0].hash.as_ref().unwrap().len(), 32);
        }
    }

    /// An interrupted append (plan Step 4.4): a log whose tail holds part of a
    /// record, as a crash inside an append leaves it. Open loads the complete
    /// ops and cuts the partial one off, so the next append starts on a record
    /// boundary and a reopen loads every op. It writes the torn bytes itself,
    /// so it proves the repair, not what a real crash leaves on a real disk.
    #[test]
    fn a_torn_tail_is_cut_so_the_next_append_reopens_whole() {
        let root = fresh("torn-tail");
        {
            let mut s = Store::open(&root).unwrap();
            s.append(op("sh", "a", 0, b"zero")).unwrap();
        }
        let torn = cbor::encode(&op("sh", "a", 1, b"one").to_cbor());
        let mut log = OpenOptions::new()
            .append(true)
            .open(log_path(&root, "sh", "a"))
            .unwrap();
        log.write_all(&(torn.len() as u32).to_le_bytes()).unwrap();
        log.write_all(&torn[..torn.len() / 2]).unwrap();
        drop(log);
        {
            let mut s = Store::open(&root).unwrap();
            assert_eq!(
                s.scan("sh", "g", &[], "a", -1).len(),
                1,
                "the complete op loads"
            );
            assert_eq!(
                s.append(op("sh", "a", 1, b"uno")).unwrap(),
                Append::Appended
            );
        }
        let s = Store::open(&root).unwrap();
        let payloads: Vec<Vec<u8>> = s
            .scan("sh", "g", &[], "a", -1)
            .into_iter()
            .map(|o| o.payload)
            .collect();
        assert_eq!(payloads, [b"zero".to_vec(), b"uno".to_vec()]);
    }

    #[test]
    fn survives_restart() {
        let root = fresh("restart");
        {
            let mut s = Store::open(&root).unwrap();
            s.append(op("sh", "a", 1, b"one")).unwrap();
            s.append(op("sh", "a", 2, b"two")).unwrap();
            s.append(op("sh", "b", 1, b"bee")).unwrap();
        } // dropped — only the on-disk log remains
        let s = Store::open(&root).unwrap();
        let a = s.scan("sh", "g", &[], "a", 0);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].payload, b"one");
        assert_eq!(a[1].payload, b"two");
        let mut h = s.heads("sh", "g", &[]);
        h.sort();
        assert_eq!(h, vec![("a".to_string(), 2), ("b".to_string(), 1)]);
    }

    /// F15b: an op nested 100,000 deep in a journal, which the wire codec's
    /// decode recursed on until the stack overflowed, is skipped, and the ops
    /// around it load. In the proofs journal the pair it belongs to is
    /// dropped, so the pairs after it stay pairs.
    #[test]
    fn open_skips_an_op_it_cannot_read_and_its_proof_pair() {
        let root = fresh("nested-op");
        let mut nested = vec![0x81; 100_000];
        nested.push(0);
        let framed = |bytes: &[u8]| [&(bytes.len() as u32).to_le_bytes()[..], bytes].concat();
        let odd = cbor::encode(&op("sh", "b", 0, b"odd").to_cbor());
        let equivocate = |seq: i64| {
            let mut s = Store::open(&root).unwrap();
            s.append(op("sh", "a", seq, b"held")).unwrap();
            s.append(op("sh", "a", seq, b"fork")).unwrap_err();
        };
        equivocate(0);
        let written = [
            (log_path(&root, "sh", "a"), framed(&nested)),
            (proofs_path(&root), [framed(&nested), framed(&odd)].concat()),
        ];
        for (path, bytes) in written {
            let mut file = OpenOptions::new().append(true).open(path).unwrap();
            file.write_all(&bytes).unwrap();
        }
        equivocate(1);

        let s = Store::open(&root).unwrap();
        let held = s.scan("sh", "g", &[], "a", -1);
        assert_eq!(held.iter().map(|o| o.seq).collect::<Vec<_>>(), [0, 1]);
        let proofs = s.equivocation_proofs().iter();
        let slots: Vec<(i64, i64)> = proofs.map(|p| (p.a.seq, p.b.seq)).collect();
        assert_eq!(slots, [(0, 0), (1, 1)]);
    }

    /// Plan Step 4.5c: a peer's claims chain of ten, then its checkpoint at 7.
    /// The chain keeps 8 and 9, and the journal is rewritten to the ops held,
    /// checkpoint first; a reopen holds the same. The heads name 9, and a
    /// store holding the checkpoint alone names its floor, (7, H). An op at 5
    /// is below what is held, and a repeat of 8 a duplicate.
    #[test]
    fn a_checkpoint_moves_a_chains_floor_and_rewrites_its_journal() {
        use crate::registry::{G_CHECKPOINTS, G_CLAIMS};
        let root = fresh("floor");
        let mut s = Store::open(&root).unwrap();
        let (peer, claims, checkpoint) = testing::folded(18, 7);
        for op in &claims {
            s.append(op.clone()).unwrap();
        }
        assert_eq!(s.append(checkpoint.clone()).unwrap(), Append::Appended);
        let journal = testing::journal_of(&root, HOME, &peer);
        let retained = [(G_CHECKPOINTS, 0), (G_CLAIMS, 8), (G_CLAIMS, 9)];
        assert_eq!(testing::slots(&journal), retained, "the journal");
        let held = [vec![checkpoint.clone()], claims[8..].to_vec()].concat();
        assert_eq!(journal, held);
        let again = Store::open(&root).unwrap();
        for st in [&s, &again] {
            assert_eq!(st.scan(HOME, G_CLAIMS, &[], &peer, -1), claims[8..]);
            let checkpoints = st.scan(HOME, G_CHECKPOINTS, &[], &peer, -1);
            assert_eq!(checkpoints, std::slice::from_ref(&checkpoint));
            assert_eq!(st.heads(HOME, G_CLAIMS, &[]), [(peer.clone(), 9)]);
        }
        assert_eq!(s.append(claims[5].clone()).unwrap(), Append::BelowRetained);
        assert_eq!(s.append(claims[8].clone()).unwrap(), Append::Duplicate);
        let mut alone = Store::open(fresh("floor-alone")).unwrap();
        alone.append(checkpoint).unwrap();
        let hash = Some(op_hash(&claims[7]).to_vec());
        let floor = Head {
            origin: peer,
            seq: 7,
            hash,
        };
        assert_eq!(alone.zone_heads(HOME, G_CLAIMS, &[]).heads, [floor]);
    }

    /// Plan Step 4.5c: `open` replays a `home` journal checkpoints first. One
    /// that begins with its checkpoint, as a rewrite leaves it, opens with
    /// nothing set aside. One that holds the ops its checkpoint covers, with
    /// the checkpoint after them, as a crash before the rewrite would leave
    /// it, opens without them, is not set aside, and is rewritten.
    #[test]
    fn open_takes_a_rewritten_journal_and_one_the_rewrite_never_reached() {
        use crate::registry::G_CLAIMS;
        let (peer, claims, checkpoint) = testing::folded(19, 7);
        let rewritten = [vec![checkpoint.clone()], claims[8..].to_vec()].concat();
        let unreached = [claims.clone(), vec![checkpoint]].concat();
        for (name, journal) in [("rewritten", rewritten.clone()), ("unreached", unreached)] {
            let root = fresh(&format!("reopen-{name}"));
            for op in &journal {
                testing::journal(&root, op);
            }
            let s = Store::open(&root).unwrap();
            let aside = s.set_aside().map(ToString::to_string);
            assert_eq!(aside, None, "{name}");
            let held = s.scan(HOME, G_CLAIMS, &[], &peer, -1);
            assert_eq!(held, claims[8..], "{name}");
            assert_eq!(testing::journal_of(&root, HOME, &peer), rewritten, "{name}");
        }
    }

    /// Plan Step 4.5c: a checkpoint naming at its base another hash than the
    /// op held there is refused as a fork, the op and the checkpoint kept as
    /// its proof, and nothing is dropped. One whose base moves back from the
    /// checkpoint held is refused as a rewrite, the two kept as its proof.
    #[test]
    fn a_checkpoint_that_contradicts_its_base_is_a_fork_and_drops_nothing() {
        use crate::envelope::testing::checkpoint as sealed;
        use crate::registry::{G_CHECKPOINTS, G_CLAIMS};
        use crate::sysdata::ChainCheckpoint;
        let root = fresh("contradicts");
        let mut s = Store::open(&root).unwrap();
        let (peer, claims, checkpoint) = testing::folded(20, 7);
        for op in &claims {
            s.append(op.clone()).unwrap();
        }
        let naming = |base: i64, op: &Op| ChainCheckpoint {
            node: peer.clone(),
            stream: G_CLAIMS.into(),
            seq: base,
            hash: op_hash(op).to_vec(),
        };
        let forked = sealed([20; 32], naming(7, &claims[6]), 0, None);
        let refused = s.append(forked.clone());
        let fork = matches!(refused, Err(StoreError::Equivocation { .. }));
        assert!(fork, "{refused:?}");
        let proof = EquivProof {
            a: claims[7].clone(),
            b: forked,
        };
        assert_eq!(s.equivocation_proofs(), [proof]);
        let held = s.scan(HOME, G_CLAIMS, &[], &peer, -1);
        assert_eq!(held, claims, "nothing dropped");
        s.append(checkpoint.clone()).unwrap();
        let prev = Some(op_hash(&checkpoint).to_vec());
        let back = sealed([20; 32], naming(5, &claims[5]), 1, prev);
        let refused = s.append(back.clone());
        let rewrite = matches!(refused, Err(StoreError::Rewrite { .. }));
        assert!(rewrite, "{refused:?}");
        let proof = EquivProof {
            a: checkpoint.clone(),
            b: back,
        };
        assert_eq!(s.equivocation_proofs().last(), Some(&proof));
        assert_eq!(Store::open(&root).unwrap().equivocation_proofs().len(), 2);
        assert_eq!(s.scan(HOME, G_CHECKPOINTS, &[], &peer, -1), [checkpoint]);
    }
}
