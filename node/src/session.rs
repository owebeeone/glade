//! Resume / convergence logic (P1.S2), carrier-independent.
//!
//! Resync is a heads exchange then a gap ship, both directions (GladeSubstrateV1
//! §6). Transport ordering is not load-bearing — ops carry `(origin, seq)` — so
//! this logic is identical over a websocket or iroh. `Store` is the replica; a
//! peer announces its per-origin heads and receives exactly the ops it lacks.

use std::collections::BTreeMap;

use glade_wire::generated::Op;

use crate::store::Store;

/// A peer's per-origin heads for `share` (origin -> highest seq held).
pub type Heads = BTreeMap<String, i64>;

pub fn heads_map(store: &Store, share: &str) -> Heads {
    store.heads(share).into_iter().collect()
}

/// The ops in `store` for `share` that a peer holding `their` heads is missing:
/// for each origin, everything above the peer's head (or the whole log if the
/// peer has never seen that origin).
pub fn missing_for(store: &Store, share: &str, their: &Heads) -> Vec<Op> {
    let mut out = Vec::new();
    for (origin, _head) in store.heads(share) {
        let from = their.get(&origin).copied().unwrap_or(i64::MIN);
        out.extend(store.scan(share, &origin, from));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use glade_wire::generated::{Op, Shape};
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
    /// Sorted (origin, seq, payload) identity of a share's full op set.
    fn snapshot(store: &Store) -> Vec<(String, i64, Vec<u8>)> {
        let mut all = Vec::new();
        for (origin, _) in store.heads("sh") {
            for o in store.scan("sh", &origin, i64::MIN) {
                all.push((o.origin, o.seq, o.payload));
            }
        }
        all.sort();
        all
    }
    /// Ship `store`'s missing ops into `dst` (a one-direction sync).
    fn ship(src: &Store, dst: &mut Store) {
        for o in missing_for(src, "sh", &heads_map(dst, "sh")) {
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
    fn no_gap_when_already_in_sync() {
        let mut server = Store::open(fresh("insync-s")).unwrap();
        let mut client = Store::open(fresh("insync-c")).unwrap();
        client.append(op("a", 1, b"a1")).unwrap();
        ship(&client, &mut server);
        // second sync ships nothing
        assert!(missing_for(&server, "sh", &heads_map(&client, "sh")).is_empty());
        assert!(missing_for(&client, "sh", &heads_map(&server, "sh")).is_empty());
    }
}
