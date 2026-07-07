//! Resume / convergence logic (P1.S2), carrier-independent.
//!
//! Resync is a heads exchange then a gap ship, both directions (GladeSubstrateV1
//! §6). Transport ordering is not load-bearing — ops carry `(origin, seq)` — so
//! this logic is identical over a websocket or iroh. `Store` is the replica; a
//! peer announces its per-origin heads and receives exactly the ops it lacks.

use std::collections::BTreeMap;

use glade_wire::generated::{Error, ErrorCode, Op};

use crate::frame::Frame;
use crate::store::{Store, StoreError};

/// A peer's per-origin heads for `share` (origin -> highest seq held).
pub type Heads = BTreeMap<String, i64>;

pub fn heads_map(store: &Store, share: &str, glade_id: &str, key: &[u8]) -> Heads {
    store.heads(share, glade_id, key).into_iter().collect()
}

/// The ops in `store` for a zone `(share, glade_id, key)` that a peer holding
/// `their` heads is missing: for each origin, everything above the peer's head
/// (or the whole chain if the peer has never seen that origin). Scoped to the
/// zone so a subscriber never receives another zone's (e.g. private) ops.
pub fn missing_for(store: &Store, share: &str, glade_id: &str, key: &[u8], their: &Heads) -> Vec<Op> {
    let mut out = Vec::new();
    for (origin, _head) in store.heads(share, glade_id, key) {
        let from = their.get(&origin).copied().unwrap_or(i64::MIN);
        out.extend(store.scan(share, glade_id, key, &origin, from));
    }
    out
}

/// Map a rejected append to a diagnostic `Error` frame (P1.S4): a fork is
/// surfaced, never propagated or silently dropped.
pub fn error_frame(err: &StoreError, share: &str, glade_id: &str) -> Frame {
    let (code, message) = match err {
        StoreError::Equivocation { origin, seq } => {
            (ErrorCode::Equivocation, format!("forked chain at ({origin},{seq})"))
        }
        StoreError::ChainBreak { origin, seq } => {
            (ErrorCode::Protocol, format!("chain break at ({origin},{seq})"))
        }
        StoreError::Gap { expected, got } => {
            (ErrorCode::Protocol, format!("gap: expected {expected}, got {got}"))
        }
        StoreError::Io(e) => (ErrorCode::Internal, format!("io: {e}")),
    };
    Frame::Error(Error {
        code,
        message,
        share: Some(share.into()),
        glade_id: Some(glade_id.into()),
        corr: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use glade_wire::generated::{ErrorCode, Op, Shape};
    use std::path::PathBuf;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-session-test-{name}"));
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
    /// Sorted (origin, seq, payload) identity of a zone's full op set.
    fn snapshot(store: &Store) -> Vec<(String, i64, Vec<u8>)> {
        let mut all = Vec::new();
        for (origin, _) in store.heads("sh", "g", &[]) {
            for o in store.scan("sh", "g", &[], &origin, i64::MIN) {
                all.push((o.origin, o.seq, o.payload));
            }
        }
        all.sort();
        all
    }
    /// Ship `store`'s missing ops into `dst` (a one-direction sync).
    fn ship(src: &Store, dst: &mut Store) {
        for o in missing_for(src, "sh", "g", &[], &heads_map(dst, "sh", "g", &[])) {
            dst.append(o).unwrap();
        }
    }

    #[test]
    fn reconnect_resumes_and_converges_both_directions() {
        let mut server = Store::open(fresh("server")).unwrap();
        let mut client = Store::open(fresh("client")).unwrap();

        // client writes A:1 and syncs it up to the server.
        client.append(op("a", 1, b"a1")).unwrap();
        ship(&client, &mut server);
        assert_eq!(snapshot(&server), snapshot(&client));

        // client disconnects. Meanwhile origin B writes to the server (elsewhere)...
        server.append(op("b", 1, b"b1")).unwrap();
        server.append(op("b", 2, b"b2")).unwrap();
        // ...and the client keeps writing locally while offline.
        client.append(op("a", 2, b"a2")).unwrap();

        // reconnect: heads exchanged, gaps shipped both ways.
        ship(&server, &mut client); // client gains B:1,2
        ship(&client, &mut server); // server gains A:2

        // converged: both replicas hold A:1,2 and B:1,2, nothing lost.
        let want = vec![
            ("a".to_string(), 1, b"a1".to_vec()),
            ("a".to_string(), 2, b"a2".to_vec()),
            ("b".to_string(), 1, b"b1".to_vec()),
            ("b".to_string(), 2, b"b2".to_vec()),
        ];
        assert_eq!(snapshot(&server), want);
        assert_eq!(snapshot(&client), want);
    }

    #[test]
    fn forked_op_surfaces_error_frame_not_silent() {
        let mut s = Store::open(fresh("err")).unwrap();
        s.append(op("a", 0, b"p0")).unwrap();
        let err = s.append(op("a", 0, b"p0-fork")).unwrap_err(); // forked chain
        let frame = error_frame(&err, "sh", "g");
        match &frame {
            Frame::Error(e) => assert_eq!(e.code, ErrorCode::Equivocation),
            other => panic!("expected Error frame, got {other:?}"),
        }
        // and it survives the wire
        assert!(matches!(Frame::from_bytes(&frame.to_bytes()).unwrap(), Frame::Error(_)));
    }

    #[test]
    fn no_gap_when_already_in_sync() {
        let mut server = Store::open(fresh("insync-s")).unwrap();
        let mut client = Store::open(fresh("insync-c")).unwrap();
        client.append(op("a", 1, b"a1")).unwrap();
        ship(&client, &mut server);
        // second sync ships nothing
        assert!(missing_for(&server, "sh", "g", &[], &heads_map(&client, "sh", "g", &[])).is_empty());
        assert!(missing_for(&client, "sh", "g", &[], &heads_map(&server, "sh", "g", &[])).is_empty());
    }
}
