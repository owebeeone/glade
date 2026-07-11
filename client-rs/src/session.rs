//! The glade session — one origin, its own append-only per-chain log, and the
//! folds over what it has seen. The rust mirror of the TS client's
//! `session.ts` + `store.ts` + `fold.ts`: the same chain identity
//! `(share, glade_id, key, origin)`, the same lww / log folds (pure functions of
//! the op-set, so every replica converges), the same op-hash `prev` links. A
//! supplier's session materializes a served surface exactly as a browser tap
//! does — one shape (§2 of `GladeSupplierModel.md`).

use std::collections::HashMap;

use glade_wire::generated::{Op, Shape};

use crate::hash::op_hash;

/// Map a surface's shape word to the wire `Shape`. Only value/log op-serve
/// (exchange is directed, never appended; window is P3), so anything not `log`
/// folds as a value — byte-for-byte with the TS client's `shape === "log"`.
pub fn shape_of(shape: &str) -> Shape {
    if shape == "log" {
        Shape::Log
    } else {
        Shape::Value
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// In-memory per-chain append-log — keyed `share\0glade_id\0keyhex\0origin`, so
/// each zone is independently contiguous (a private key never shares a chain
/// with the commons). Mirrors the node + TS store's chain rules enough for
/// client convergence: dedup by `seq`, drop gaps (a real client would surface an
/// Error for equivocation; here convergence simply ignores bad ops).
#[derive(Default)]
struct Store {
    logs: HashMap<String, Vec<Op>>,
}

impl Store {
    fn chain_key(share: &str, glade_id: &str, key: &[u8], origin: &str) -> String {
        format!("{share}\u{0}{glade_id}\u{0}{}\u{0}{origin}", hex(key))
    }

    /// Append with per-chain checks; returns true if the op was newly stored.
    fn append(&mut self, op: Op) -> bool {
        let k = Self::chain_key(&op.share, &op.glade_id, &op.key, &op.origin);
        let log = self.logs.entry(k).or_default();
        if let Some(last) = log.last() {
            if op.seq <= last.seq {
                return false; // duplicate / equivocation: convergence ignores
            }
            if op.seq != last.seq + 1 {
                return false; // gap: a real client resumes; here we drop
            }
        }
        log.push(op);
        true
    }

    /// One origin's chain (share, glade_id, key) with seq > `from`, in order.
    fn scan(&self, share: &str, glade_id: &str, key: &[u8], origin: &str, from: i64) -> Vec<&Op> {
        self.logs
            .get(&Self::chain_key(share, glade_id, key, origin))
            .map(|l| l.iter().filter(|o| o.seq > from).collect())
            .unwrap_or_default()
    }

    /// Every op in a zone-surface (share, glade_id, key) across origins — the
    /// fold input for one bound surface. A different zone's ops never appear.
    fn ops_for(&self, share: &str, glade_id: &str, key: &[u8]) -> Vec<&Op> {
        let prefix = format!("{share}\u{0}{glade_id}\u{0}{}\u{0}", hex(key));
        let mut out = Vec::new();
        for (k, log) in &self.logs {
            if k.starts_with(&prefix) {
                out.extend(log.iter());
            }
        }
        out
    }
}

/// One origin's session: append to its own chains, apply remote ops, fold.
pub struct Session {
    pub origin: String,
    lamport: i64,
    store: Store,
}

impl Session {
    pub fn new(origin: impl Into<String>) -> Self {
        Session { origin: origin.into(), lamport: 0, store: Store::default() }
    }

    /// Append a local op to this origin's chain within a zone (default commons)
    /// and return it. The zone `key` selects the chain — its own seq/prev.
    pub fn append(&mut self, share: &str, glade_id: &str, shape: Shape, payload: Vec<u8>, key: Vec<u8>) -> Op {
        let (seq, prev) = {
            let own = self.store.scan(share, glade_id, &key, &self.origin, i64::MIN);
            match own.last() {
                Some(last) => (last.seq + 1, Some(op_hash(last).to_vec())),
                None => (0, None),
            }
        };
        self.lamport += 1;
        let op = Op {
            share: share.into(),
            glade_id: glade_id.into(),
            key,
            origin: self.origin.clone(),
            seq,
            prev,
            lamport: self.lamport,
            refs: vec![],
            shape,
            payload,
        };
        self.store.append(op.clone());
        op
    }

    /// Apply ops received from the node; advance the lamport clock.
    pub fn apply_remote(&mut self, ops: &[Op]) {
        for op in ops {
            let lam = op.lamport;
            if self.store.append(op.clone()) && lam > self.lamport {
                self.lamport = lam;
            }
        }
    }

    /// lww value fold: winner = max by (lamport, origin). `None` if empty.
    pub fn fold_value(&self, share: &str, glade_id: &str, key: &[u8]) -> Option<Vec<u8>> {
        fold_value(&self.store.ops_for(share, glade_id, key))
    }

    /// log fold: deterministic order by (lamport, origin, seq).
    pub fn fold_log(&self, share: &str, glade_id: &str, key: &[u8]) -> Vec<Vec<u8>> {
        fold_log(&self.store.ops_for(share, glade_id, key))
    }
}

/// Dedup by (origin, seq) keeping the first seen — the equivocation slot check
/// the node enforces; here duplicates simply collapse.
fn dedup<'a>(ops: &[&'a Op]) -> Vec<&'a Op> {
    let mut seen: HashMap<(String, i64), &'a Op> = HashMap::new();
    for op in ops {
        seen.entry((op.origin.clone(), op.seq)).or_insert(op);
    }
    seen.into_values().collect()
}

fn fold_value(ops: &[&Op]) -> Option<Vec<u8>> {
    let live = dedup(ops);
    let mut win: Option<&Op> = None;
    for o in live {
        let better = match win {
            None => true,
            Some(w) => o.lamport > w.lamport || (o.lamport == w.lamport && o.origin > w.origin),
        };
        if better {
            win = Some(o);
        }
    }
    win.map(|o| o.payload.clone())
}

fn fold_log(ops: &[&Op]) -> Vec<Vec<u8>> {
    let mut live = dedup(ops);
    live.sort_by(|a, b| {
        a.lamport
            .cmp(&b.lamport)
            .then_with(|| a.origin.cmp(&b.origin))
            .then_with(|| a.seq.cmp(&b.seq))
    });
    live.into_iter().map(|o| o.payload.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// lww: a later append wins; a lower-lamport remote loses. Order-independent.
    #[test]
    fn value_fold_is_lww() {
        let mut a = Session::new("a");
        a.append("s", "g", Shape::Value, b"a0".to_vec(), vec![]);
        assert_eq!(a.fold_value("s", "g", &[]), Some(b"a0".to_vec()));
        // a remote op from "b" with a higher lamport wins.
        let b_op = Op { origin: "b".into(), lamport: 9, ..sample("s", "g", b"b0") };
        a.apply_remote(&[b_op]);
        assert_eq!(a.fold_value("s", "g", &[]), Some(b"b0".to_vec()));
    }

    /// log: deterministic order by (lamport, origin, seq); a zone's key isolates.
    #[test]
    fn log_fold_orders_and_zones_isolate() {
        let mut s = Session::new("a");
        s.append("s", "g", Shape::Log, b"l0".to_vec(), vec![]);
        s.append("s", "g", Shape::Log, b"l1".to_vec(), vec![]);
        s.append("s", "g", Shape::Log, b"private".to_vec(), b"self:a".to_vec());
        assert_eq!(s.fold_log("s", "g", &[]), vec![b"l0".to_vec(), b"l1".to_vec()]);
        assert_eq!(s.fold_log("s", "g", b"self:a"), vec![b"private".to_vec()]);
    }

    fn sample(share: &str, glade_id: &str, payload: &[u8]) -> Op {
        Op {
            share: share.into(),
            glade_id: glade_id.into(),
            key: vec![],
            origin: "x".into(),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }
}
