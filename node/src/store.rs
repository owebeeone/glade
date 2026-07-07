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
use glade_wire::generated::Op;

use crate::chain::op_hash;

/// Outcome of an append.
#[derive(Debug, PartialEq)]
pub enum Append {
    /// New op, persisted and indexed.
    Appended,
    /// `seq` already present with the *same* hash (idempotent re-delivery).
    Duplicate,
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
        Ok(Store { root, logs })
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
        let log = self.logs.entry(chain_of(&op)).or_default();
        if let Some(last) = log.last() {
            if op.seq <= last.seq {
                return match log.iter().find(|o| o.seq == op.seq) {
                    Some(stored) if op_hash(stored) == op_hash(&op) => Ok(Append::Duplicate),
                    Some(_) => Err(StoreError::Equivocation { origin: op.origin, seq: op.seq }),
                    None => Ok(Append::Duplicate), // below retained range — treat as seen
                };
            }
            if op.seq != last.seq + 1 {
                return Err(StoreError::Gap { expected: last.seq + 1, got: op.seq });
            }
            if let Some(prev) = &op.prev {
                if prev.as_slice() != op_hash(last) {
                    return Err(StoreError::ChainBreak { origin: op.origin, seq: op.seq });
                }
            }
        }
        append_to_log(&self.root, &op)?;
        log.push(op);
        Ok(Append::Appended)
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
