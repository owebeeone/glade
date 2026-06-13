//! Subscription routing + outbound priority (P1.S3).
//!
//! The router fans an op out to the sessions subscribed to its
//! `(share, glade_id)` — **minus the origin's own session** (no self-echo).
//! Each session's outbound is a two-lane priority queue: interactive/control
//! frames preempt bulk log backfill, so a keystroke never waits behind a big
//! backfill on the single socket (GladeSubstrateV1 §6).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use glade_wire::generated::Priority;

pub type SessionId = u64;

/// `(share, glade_id) -> subscribed sessions`.
#[derive(Default)]
pub struct Router {
    subs: BTreeMap<(String, String), BTreeSet<SessionId>>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&mut self, session: SessionId, share: &str, glade_id: &str) {
        self.subs.entry((share.into(), glade_id.into())).or_default().insert(session);
    }

    pub fn unsubscribe(&mut self, session: SessionId, share: &str, glade_id: &str) {
        if let Some(set) = self.subs.get_mut(&(share.into(), glade_id.into())) {
            set.remove(&session);
        }
    }

    /// Drop a session from every subscription (connection teardown).
    pub fn unsubscribe_all(&mut self, session: SessionId) {
        for set in self.subs.values_mut() {
            set.remove(&session);
        }
    }

    /// Sessions subscribed to `(share, glade_id)` except `from` — the fan-out
    /// targets for an op originating at `from` (never echoed to itself).
    pub fn route(&self, from: SessionId, share: &str, glade_id: &str) -> Vec<SessionId> {
        self.subs
            .get(&(share.to_string(), glade_id.to_string()))
            .map(|set| set.iter().copied().filter(|&s| s != from).collect())
            .unwrap_or_default()
    }
}

/// A session's outbound queue: a high lane (control/interactive) drained before
/// a low lane (bulk). FIFO within a lane.
#[derive(Default)]
pub struct OutQueue {
    hi: VecDeque<Vec<u8>>,
    lo: VecDeque<Vec<u8>>,
}

impl OutQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, pri: Priority, frame: Vec<u8>) {
        match pri {
            Priority::Bulk => self.lo.push_back(frame),
            _ => self.hi.push_back(frame), // control + interactive preempt bulk
        }
    }

    /// Next frame to send: high lane first, then bulk.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        self.hi.pop_front().or_else(|| self.lo.pop_front())
    }

    pub fn is_empty(&self) -> bool {
        self.hi.is_empty() && self.lo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use crate::store::Store;
    use glade_wire::generated::{Op, Ops, Shape};
    use std::path::PathBuf;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-router-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }
    fn op(origin: &str, seq: i64, payload: &[u8]) -> Op {
        Op {
            share: "sh".into(),
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

    #[test]
    fn route_reaches_others_minus_origin() {
        let mut r = Router::new();
        r.subscribe(1, "sh", "g");
        r.subscribe(2, "sh", "g");
        r.subscribe(3, "sh", "other"); // different stream
        assert_eq!(r.route(1, "sh", "g"), vec![2]); // 2 gets it, 1 (origin) does not, 3 not subscribed
        assert_eq!(r.route(9, "sh", "g"), vec![1, 2]); // a non-member origin reaches both
        assert!(r.route(1, "sh", "absent").is_empty());
    }

    #[test]
    fn interactive_preempts_bulk_backfill() {
        let mut q = OutQueue::new();
        q.push(Priority::Bulk, vec![0u8; 1_000_000]); // 1 MB backfill enqueued first
        q.push(Priority::Interactive, b"keystroke".to_vec());
        assert_eq!(q.pop().unwrap(), b"keystroke"); // ...but the keystroke goes first
        assert_eq!(q.pop().unwrap().len(), 1_000_000);
        assert!(q.is_empty());
    }

    #[test]
    fn two_sessions_exchange_ops_through_router() {
        // node-side store + router + per-session outbound queues
        let mut store = Store::open(fresh("exchange")).unwrap();
        let mut router = Router::new();
        let mut out: std::collections::BTreeMap<SessionId, OutQueue> = Default::default();
        for s in [1u64, 2] {
            router.subscribe(s, "sh", "g");
            out.insert(s, OutQueue::new());
        }
        // session 1 (origin "a") submits an op: node stores it, fans out to others
        let o = op("a", 0, b"hello");
        store.append(o.clone()).unwrap();
        let frame = Frame::Ops(Ops { ops: vec![o.clone()], pri: None }).to_bytes();
        for target in router.route(1, "sh", "g") {
            out.get_mut(&target).unwrap().push(Priority::Interactive, frame.clone());
        }
        // session 2 receives the op; session 1 (origin) gets nothing
        let got = out.get_mut(&2).unwrap().pop().expect("session 2 should receive");
        match Frame::from_bytes(&got).unwrap() {
            Frame::Ops(ops) => assert_eq!(ops.ops[0].payload, b"hello"),
            other => panic!("expected Ops, got {other:?}"),
        }
        assert!(out.get(&1).unwrap().is_empty(), "origin must not be echoed");
    }
}
