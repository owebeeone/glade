//! Per-origin chain hash (P1.S4, GQ-9). `op_hash(op) = sha256(canonical_cbor(op))`
//! over the op's frozen encoding (prev included). Reproduces taut's op-hash
//! oracle (`corpus/glade_hashes.json`) byte-for-byte — agreement is inherited
//! from the wire corpus (identical CBOR -> identical sha256).

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
    use glade_wire::generated::{Op, Shape};

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{:02x}", x)).collect()
    }

    fn chain_a0() -> Op {
        Op {
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
        }
    }

    /// Rust reproduces the Python op-hash oracle (taut/corpus/glade_hashes.json,
    /// vector `chain/a0`) — cross-language chain agreement (D10).
    #[test]
    fn op_hash_matches_python_oracle() {
        assert_eq!(
            hex(&op_hash(&chain_a0())),
            "8a87b62f11deec6937b784dd4bada44d6473aa304c9cfd9c8974b139539e6873"
        );
    }

    #[test]
    fn different_payload_forks_the_hash() {
        let mut fork = chain_a0();
        fork.payload = b"p0-fork".to_vec();
        assert_ne!(op_hash(&fork), op_hash(&chain_a0())); // same (origin,seq), different hash
    }
}
