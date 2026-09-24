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

/// The domains of the transport-key binding (plan Step 4.2), outside the
/// port's three purposes: a binding's signature covers the record, not an
/// op, so it is not `OriginOp`'s. Like HELLO, they are signed and checked by
/// the node's own functions, not through `SignerPort`.
pub const TRANSPORT_BINDING: &[u8] = b"glade/v1/transport-binding\0";
pub const TRANSPORT_REVOCATION: &[u8] = b"glade/v1/transport-revocation\0";

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
    sign_in(seed, tag(purpose), message)
}

/// The signature of the key `seed` expands to on `domain`, a tag, followed
/// by `message`.
pub fn sign_in(seed: &[u8; 32], domain: &[u8], message: &[u8]) -> [u8; 64] {
    let key = SigningKey::from_bytes(seed);
    key.sign(&[domain, message].concat()).to_bytes()
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
    verify_in(signer, tag(purpose), message, signature)
}

/// [`verify`] for a signature made in `domain`, a tag, by [`sign_in`].
pub fn verify_in(
    signer: &NodeId,
    domain: &[u8],
    message: &[u8],
    signature: &[u8],
) -> SignatureStatus {
    let key = VerifyingKey::from_bytes(signer).ok();
    let signature = Signature::from_slice(signature).ok();
    let valid = key.zip(signature).is_some_and(|(key, signature)| {
        let bytes = [domain, message].concat();
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

    /// D7's rule, with plan Step 4.2's two domains: every tag is ASCII and
    /// ends in a zero byte, and no tag is a prefix of another, so no
    /// signature crosses domains. It guards the table; it has no red form.
    #[test]
    fn no_tag_is_a_prefix_of_another() {
        let purposes = [Purpose::PeerHello, Purpose::OriginOp, Purpose::LocalOverlay];
        let mut tags: Vec<&[u8]> = purposes.into_iter().map(tag).collect();
        tags.extend([TRANSPORT_BINDING, TRANSPORT_REVOCATION]);
        for (i, a) in tags.iter().enumerate() {
            assert!(a.is_ascii() && a.ends_with(b"\0"), "{a:?}");
            for b in &tags[i + 1..] {
                assert!(!a.starts_with(b) && !b.starts_with(a), "{a:?} {b:?}");
            }
        }
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
        let message = [tag(Purpose::PeerHello), &b"any transcript"[..]].concat();
        let forged_signature = Signature::from_bytes(&forged);
        assert!(
            lax.verify(&message, &forged_signature).is_ok(),
            "the lax check takes it"
        );
        let status = verify(&weak, Purpose::PeerHello, b"any transcript", &forged);
        assert_eq!(status, SignatureStatus::Invalid);
    }
}
