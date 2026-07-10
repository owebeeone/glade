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

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use glade_wire::cbor;
use glade_wire::generated::{Head, Op, StreamHeads};

use crate::chain::op_hash;

/// Outcome of an append.
#[derive(Debug, PartialEq)]
pub enum Append {
    /// New op, persisted and indexed.
    Appended,
    /// `seq` already present with the *same* hash (idempotent re-delivery).
    Duplicate,
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
    Io(std::io::Error),
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// A chain identity: `(share, glade_id, key, origin)`. The zone `key` joins the
/// axis so each zone is an independently contiguous, independently shippable
/// chain (GladeZones.md).
type ChainId = (String, String, Vec<u8>, String);

fn chain_of(op: &Op) -> ChainId {
    (op.share.clone(), op.glade_id.clone(), op.key.clone(), op.origin.clone())
}

/// Append-only per-chain op store, persisted under `root`.
pub struct Store {
    root: PathBuf,
    logs: BTreeMap<ChainId, Vec<Op>>,
    /// Recorded equivocation proofs (persisted under `<root>/proofs/`), in
    /// detection order. A fork is data with a signature on it — kept, not lost.
    proofs: Vec<EquivProof>,
}

impl Store {
    /// Open (and replay) a store rooted at `root`, creating it if absent. The
    /// journal is per-`(share, origin)` file; each op is regrouped into its
    /// chain `(share, glade_id, key, origin)` from its own fields, so one file
    /// can feed several zone-chains. File order preserves per-chain seq order.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let root = root.into();
        let mut logs: BTreeMap<ChainId, Vec<Op>> = BTreeMap::new();
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
                for log_ent in fs::read_dir(share_ent.path())? {
                    let log_ent = log_ent?;
                    let fname = log_ent.file_name().to_string_lossy().to_string();
                    if fname.ends_with(".log") {
                        for op in read_log(&log_ent.path())? {
                            logs.entry(chain_of(&op)).or_default().push(op);
                        }
                    }
                }
            }
        }
        let proofs = read_proofs(&proofs_path(&root))?;
        Ok(Store { root, logs, proofs })
    }

    /// Append `op` to its `(share, glade_id, key, origin)` chain, with per-chain
    /// checks (P1.S4, GQ-9):
    /// - `seq <= last.seq`: idempotent if the stored op has the same hash;
    ///   **equivocation** (rejected) if a different hash — a forked chain.
    /// - `seq == last.seq + 1`: if `prev` is present it must equal the
    ///   predecessor's hash (else **chain break**); absent `prev` is accepted
    ///   unverified (M-LIMP lenient — honest clients always set it).
    /// - otherwise a forward **gap**.
    pub fn append(&mut self, op: Op) -> Result<Append, StoreError> {
        let chain = chain_of(&op);
        // Classify against the current tail without holding a borrow of `logs`
        // across the proof write / push (equivocation records into `proofs`).
        match classify(self.logs.get(&chain), &op) {
            Verdict::Duplicate => Ok(Append::Duplicate),
            Verdict::Gap { expected, got } => Err(StoreError::Gap { expected, got }),
            Verdict::ChainBreak => Err(StoreError::ChainBreak { origin: op.origin, seq: op.seq }),
            Verdict::Equivocation(stored) => {
                // Two signed ops, one slot — persist the fork proof, then reject.
                self.record_equivocation(EquivProof { a: stored, b: op.clone() })?;
                Err(StoreError::Equivocation { origin: op.origin, seq: op.seq })
            }
            Verdict::Appended => {
                append_to_log(&self.root, &op)?;
                self.logs.entry(chain).or_default().push(op);
                Ok(Append::Appended)
            }
        }
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
            .map(|(share, glade_id, key)| {
                let heads = self
                    .logs
                    .iter()
                    .filter(|((s, g, k, _), _)| *s == share && *g == glade_id && *k == key)
                    .filter_map(|((_, _, _, origin), log)| {
                        log.last().map(|o| Head { origin: origin.clone(), seq: o.seq, hash: Some(op_hash(o).to_vec()) })
                    })
                    .collect();
                StreamHeads { share, glade_id, key, heads }
            })
            .collect()
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
    /// vector (origin -> max seq). Different zones (keys) never mix.
    pub fn heads(&self, share: &str, glade_id: &str, key: &[u8]) -> Vec<(String, i64)> {
        self.logs
            .iter()
            .filter(|((s, g, k, _), _)| s == share && g == glade_id && k.as_slice() == key)
            .filter_map(|((_, _, _, origin), log)| log.last().map(|o| (origin.clone(), o.seq)))
            .collect()
    }
}

/// The append verdict for one op against its chain's current tail. Split out so
/// `append` can decide without holding a borrow of `logs` across a proof write.
enum Verdict {
    Appended,
    Duplicate,
    Gap { expected: i64, got: i64 },
    ChainBreak,
    /// An op already sits at this `(origin, seq)` with a different hash — a fork.
    /// Carries the stored op so the proof can be assembled.
    Equivocation(Op),
}

fn classify(log: Option<&Vec<Op>>, op: &Op) -> Verdict {
    let Some(last) = log.and_then(|l| l.last()) else { return Verdict::Appended };
    if op.seq <= last.seq {
        // Safe to unwrap the log: we found `last` in it.
        return match log.unwrap().iter().find(|o| o.seq == op.seq) {
            Some(stored) if op_hash(stored) == op_hash(op) => Verdict::Duplicate,
            Some(stored) => Verdict::Equivocation(stored.clone()),
            None => Verdict::Duplicate, // below retained range — treat as seen
        };
    }
    if op.seq != last.seq + 1 {
        return Verdict::Gap { expected: last.seq + 1, got: op.seq };
    }
    if let Some(prev) = &op.prev {
        if prev.as_slice() != op_hash(last) {
            return Verdict::ChainBreak;
        }
    }
    Verdict::Appended
}

fn proofs_path(root: &Path) -> PathBuf {
    root.join("proofs").join("equivocations.log")
}

/// Append a proof as two length-prefixed op CBORs (a then b), mirroring the op
/// journal's framing. The chain/seq is recoverable from the ops themselves.
fn append_proof(path: &Path, proof: &EquivProof) -> Result<(), StoreError> {
    fs::create_dir_all(path.parent().unwrap())?;
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    for op in [&proof.a, &proof.b] {
        let bytes = cbor::encode(&op.to_cbor());
        f.write_all(&(bytes.len() as u32).to_le_bytes())?;
        f.write_all(&bytes)?;
    }
    Ok(())
}

fn read_proofs(path: &Path) -> Result<Vec<EquivProof>, StoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let ops = read_log(path)?; // same framing — a flat run of ops, paired up
    Ok(ops.chunks_exact(2).map(|p| EquivProof { a: p[0].clone(), b: p[1].clone() }).collect())
}

fn log_path(root: &Path, share: &str, origin: &str) -> PathBuf {
    root.join(hex(share)).join(format!("{}.log", hex(origin)))
}

fn append_to_log(root: &Path, op: &Op) -> Result<(), StoreError> {
    let path = log_path(root, &op.share, &op.origin);
    fs::create_dir_all(path.parent().unwrap())?;
    let bytes = cbor::encode(&op.to_cbor());
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(&(bytes.len() as u32).to_le_bytes())?;
    f.write_all(&bytes)?;
    Ok(())
}

fn read_log(path: &Path) -> Result<Vec<Op>, StoreError> {
    let data = fs::read(path)?;
    let mut ops = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let len = u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if i + len > data.len() {
            break; // truncated tail — ignore the partial record
        }
        ops.push(Op::from_cbor(&cbor::decode(&data[i..i + len])));
        i += len;
    }
    Ok(ops)
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::generated::{Op, Shape};

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
}
