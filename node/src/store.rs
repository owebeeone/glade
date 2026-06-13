//! Per-(share, origin) append-log store (P1.S1).
//!
//! The authoritative unit is the **(share, origin) log**: one monotonic `seq`
//! sequence per origin within a share (GladeSubstrateV1 §6, Decisions D8). An
//! op carries `(glade_id, key)` as routing/fold addressing, not a separate log
//! axis. Restart-safe: each log is a length-prefixed CBOR append file replayed
//! on `open`. Chain-hash / equivocation verification arrives in P1.S4; here the
//! store enforces per-origin seq contiguity and idempotent re-delivery.

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

/// Append-only per-(share, origin) op store, persisted under `root`.
pub struct Store {
    root: PathBuf,
    logs: BTreeMap<(String, String), Vec<Op>>,
}

impl Store {
    /// Open (and replay) a store rooted at `root`, creating it if absent.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let root = root.into();
        let mut logs: BTreeMap<(String, String), Vec<Op>> = BTreeMap::new();
        if root.exists() {
            for share_ent in fs::read_dir(&root)? {
                let share_ent = share_ent?;
                if !share_ent.file_type()?.is_dir() {
                    continue;
                }
                let share = unhex(&share_ent.file_name().to_string_lossy());
                for log_ent in fs::read_dir(share_ent.path())? {
                    let log_ent = log_ent?;
                    let fname = log_ent.file_name().to_string_lossy().to_string();
                    if let Some(origin_hex) = fname.strip_suffix(".log") {
                        let origin = unhex(origin_hex);
                        logs.insert((share.clone(), origin), read_log(&log_ent.path())?);
                    }
                }
            }
        }
        Ok(Store { root, logs })
    }

    /// Append `op` to its `(share, origin)` log, with per-origin chain checks
    /// (P1.S4, GQ-9):
    /// - `seq <= last.seq`: idempotent if the stored op has the same hash;
    ///   **equivocation** (rejected) if a different hash — a forked chain.
    /// - `seq == last.seq + 1`: if `prev` is present it must equal the
    ///   predecessor's hash (else **chain break**); absent `prev` is accepted
    ///   unverified (M-LIMP lenient — honest clients always set it).
    /// - otherwise a forward **gap**.
    pub fn append(&mut self, op: Op) -> Result<Append, StoreError> {
        let key = (op.share.clone(), op.origin.clone());
        let log = self.logs.entry(key).or_default();
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

    /// Ops for `(share, origin)` with `seq > from_seq`, in order (the resume tail).
    pub fn scan(&self, share: &str, origin: &str, from_seq: i64) -> Vec<Op> {
        self.logs
            .get(&(share.to_string(), origin.to_string()))
            .map(|log| log.iter().filter(|o| o.seq > from_seq).cloned().collect())
            .unwrap_or_default()
    }

    /// Per-origin head seq for `share` — the resume vector (origin -> max seq).
    pub fn heads(&self, share: &str) -> Vec<(String, i64)> {
        self.logs
            .iter()
            .filter(|((s, _), _)| s == share)
            .filter_map(|((_, origin), log)| log.last().map(|o| (origin.clone(), o.seq)))
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

fn unhex(s: &str) -> String {
    let bytes: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    String::from_utf8(bytes).unwrap_or_default()
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
        assert_eq!(s.scan("sh", "a", 0).len(), 3); // all
        assert_eq!(s.scan("sh", "a", 1).len(), 2); // seq > 1
        assert_eq!(s.scan("sh", "a", 3).len(), 0); // caught up
        assert_eq!(s.scan("sh", "missing", 0).len(), 0);
    }

    #[test]
    fn heads_per_origin() {
        let mut s = Store::open(fresh("heads")).unwrap();
        s.append(op("sh", "a", 1, b"x")).unwrap();
        s.append(op("sh", "a", 2, b"y")).unwrap();
        s.append(op("sh", "b", 1, b"z")).unwrap();
        let mut h = s.heads("sh");
        h.sort();
        assert_eq!(h, vec![("a".to_string(), 2), ("b".to_string(), 1)]);
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
        let a = s.scan("sh", "a", 0);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].payload, b"one");
        assert_eq!(a[1].payload, b"two");
        let mut h = s.heads("sh");
        h.sort();
        assert_eq!(h, vec![("a".to_string(), 2), ("b".to_string(), 1)]);
    }
}
