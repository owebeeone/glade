//! The node key as an Ed25519 signer (plan Step 4.1a; the owner's rulings on
//! `glade/dev-docs/GladeNodeSigning.md`, D1, D2 and D7). `node.key` holds a
//! 32-byte seed and the node id is its public key, so a verifier needs no
//! lookup. Every signature is pure Ed25519 over a purpose's tag followed by the
//! message, and every check is `verify_strict`. The design is
//! `glade/dev-docs/GladeNodeAssembly.md`, "Signing: the key, the id and HELLO".

use std::collections::BTreeSet;
use std::io;
use std::sync::{Mutex, PoisonError};

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use glade_signer_api::{
    NodeId, Purpose, SignError, SignatureStatus, SignerPort, VerificationError,
};

use crate::peer::NodeIdentity;

/// A purpose's signing domain (D7): an ASCII tag ending in a zero byte, put
/// before the message. No tag is a prefix of another, so a signature made for
/// one purpose verifies for no other. Ed25519's context variant is not used:
/// WebCrypto lacks it.
pub fn tag(purpose: Purpose) -> &'static [u8] {
    match purpose {
        Purpose::PeerHello => b"glade/v1/peer-hello\0",
        Purpose::OriginOp => b"glade/v1/origin-op\0",
        Purpose::LocalOverlay => b"glade/v1/local-overlay\0",
    }
}

fn signed_bytes(purpose: Purpose, message: &[u8]) -> Vec<u8> {
    [tag(purpose), message].concat()
}

/// 32 bytes from the operating system's randomness, the seed of a new key:
/// `getrandom(2)` on Linux, `getentropy` on macOS, `ProcessPrng` on Windows.
pub fn random_seed() -> io::Result<[u8; 32]> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(io::Error::other)?;
    Ok(seed)
}

/// The id a seed signs as: its Ed25519 public key.
pub fn public_key(seed: &[u8; 32]) -> NodeId {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

/// The signature of the key `seed` expands to on `message`, for `purpose`.
pub fn sign(seed: &[u8; 32], purpose: Purpose, message: &[u8]) -> [u8; 64] {
    let key = SigningKey::from_bytes(seed);
    key.sign(&signed_bytes(purpose, message)).to_bytes()
}

/// Whether `signature` is `signer`'s on `message` for `purpose`, the id being
/// the key. `Valid` only when the id is a point, neither it nor the
/// signature's R is of small order, and the equation holds (`verify_strict`):
/// under a lax check a small-order key "signs" almost every message.
pub fn verify(
    signer: &NodeId,
    purpose: Purpose,
    message: &[u8],
    signature: &[u8],
) -> SignatureStatus {
    let key = VerifyingKey::from_bytes(signer).ok();
    let signature = Signature::from_slice(signature).ok();
    let valid = key.zip(signature).is_some_and(|(key, signature)| {
        let bytes = signed_bytes(purpose, message);
        key.verify_strict(&bytes, &signature).is_ok()
    });
    if valid {
        SignatureStatus::Valid
    } else {
        SignatureStatus::Invalid
    }
}

/// The node's `SignerPort` adapter. It signs with the node key, and checks
/// signatures by this node and by the nodes recorded as authenticated, taking
/// each id as its key. Any other signer is `Unavailable`, not `Invalid`: an
/// unknown key is not proof of invalidity (SI-002). Built without a key, as
/// the legacy form builds it, it refuses both ways (SI-003).
pub struct NodeSigner {
    identity: Option<NodeIdentity>,
    authenticated: Mutex<BTreeSet<NodeId>>,
}

impl NodeSigner {
    /// A signer over `identity`'s key, or over none.
    pub fn new(identity: Option<NodeIdentity>) -> NodeSigner {
        NodeSigner {
            identity,
            authenticated: Mutex::new(BTreeSet::new()),
        }
    }

    /// Record `node` as authenticated, its HELLO having verified on a link:
    /// from now on its signatures are checked. Plan Step 4.1b calls it.
    pub fn authenticated(&self, node: NodeId) {
        let mut known = self
            .authenticated
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        known.insert(node);
    }

    fn knows(&self, me: &NodeIdentity, signer: &NodeId) -> bool {
        let known = self
            .authenticated
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *signer == me.node_id || known.contains(signer)
    }
}

impl SignerPort for NodeSigner {
    fn node_id(&self) -> NodeId {
        self.identity.map_or([0; 32], |identity| identity.node_id)
    }

    fn sign(&self, purpose: Purpose, message: &[u8]) -> Result<Vec<u8>, SignError> {
        let identity = self.identity.as_ref().ok_or(SignError::Unavailable)?;
        Ok(identity.sign(purpose, message))
    }

    fn verify(
        &self,
        signer: &NodeId,
        purpose: Purpose,
        message: &[u8],
        signature: &[u8],
    ) -> Result<SignatureStatus, VerificationError> {
        match &self.identity {
            Some(me) if self.knows(me, signer) => Ok(verify(signer, purpose, message, signature)),
            _ => Err(VerificationError::Unavailable),
        }
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier as _;

    const SEED: [u8; 32] = [7; 32];

    /// D7's encoding: a signature is pure Ed25519, by the key the seed expands
    /// to, over the purpose's tag and then the message, so any Ed25519
    /// library checks it by putting the tag first. The tag is part of what is
    /// signed. It does not show another language agrees: the `proof_family`
    /// corpus's vectors are not written yet.
    #[test]
    fn a_signature_is_pure_ed25519_over_the_tag_then_the_message() {
        let signature = Signature::from_bytes(&sign(&SEED, Purpose::OriginOp, b"op"));
        let key = SigningKey::from_bytes(&SEED).verifying_key();
        let tagged = [&b"glade/v1/origin-op\0"[..], b"op"].concat();
        assert!(key.verify_strict(&tagged, &signature).is_ok());
        assert!(
            key.verify_strict(b"op", &signature).is_err(),
            "the tag is signed"
        );
    }

    /// Strict verification (D1). The identity point is a public key of small
    /// order: with R the identity and S zero, its "signature" holds for every
    /// message under the lax check, so a HELLO naming that id would pass on
    /// first contact. `verify` refuses it. It does not test the other
    /// small-order points.
    #[test]
    fn a_small_order_id_signs_nothing() {
        let mut weak = [0u8; 32];
        weak[0] = 1;
        let mut forged = [0u8; 64];
        forged[0] = 1;
        let lax = VerifyingKey::from_bytes(&weak).unwrap();
        let message = signed_bytes(Purpose::PeerHello, b"any transcript");
        let forged_signature = Signature::from_bytes(&forged);
        assert!(
            lax.verify(&message, &forged_signature).is_ok(),
            "the lax check takes it"
        );
        let status = verify(&weak, Purpose::PeerHello, b"any transcript", &forged);
        assert_eq!(status, SignatureStatus::Invalid);
    }
}
