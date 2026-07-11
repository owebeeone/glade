//! Per-origin chain hash (GQ-9) — the client mirror of the node's
//! `chain::op_hash`: `op_hash(op) = sha256(canonical_cbor(op))` over the op's
//! frozen encoding (prev included). Identical CBOR ⇒ identical sha256, so the
//! chain agrees byte-for-byte with the node + the TS client + the taut oracle
//! (`corpus/glade_hashes.json`).

use glade_wire::cbor;
use glade_wire::generated::Op;
use sha2::{Digest, Sha256};

/// The op's identity hash: sha256 of its canonical CBOR encoding.
pub fn op_hash(op: &Op) -> [u8; 32] {
    Sha256::digest(cbor::encode(&op.to_cbor())).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::generated::Shape;

    /// Reproduce the taut op-hash oracle (vector `chain/a0`) — cross-language
    /// chain agreement with the node (`chain.rs`) and the TS client.
    #[test]
    fn op_hash_matches_oracle() {
        let op = Op {
            share: "sh".into(),
            glade_id: "g".into(),
            key: vec![],
            origin: "a".into(),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: vec![],
            shape: Shape::Value,
            payload: b"p0".to_vec(),
        };
        let hex: String = op_hash(&op).iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "8a87b62f11deec6937b784dd4bada44d6473aa304c9cfd9c8974b139539e6873");
    }
}
