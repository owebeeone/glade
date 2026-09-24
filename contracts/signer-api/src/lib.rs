//! Sign under this node's key and verify under node keys, shaped for the node's
//! three stub seams: the HELLO (`glade/node/src/peer.rs:99`), the per-op origin
//! signature (`peer.rs:143`) and the node-self-signed local overlay
//! (`sysdir.rs:263`). Not key custody, rotation, trust policy, authorization or
//! freshness.
//!
//! The outcomes follow the discovery `Signer`/`Verifier`
//! (`glade-discover-signature-api`, SG-001..SG-003) without depending on them:
//! those verify a discovery `SignedOp`, which none of the three seams has, and
//! return `impl Future`, so they cannot be injected as `dyn`. One ed25519
//! adapter may implement both and run both suites (plan Step 4.1).

/// A node's identity: its node key's Ed25519 public key, the key being the
/// 32-byte seed in `node.key`, as `NodeIdentity::from_key` derives it (plan
/// Step 4.1a). A verifier needs no lookup: the id is the key.
pub type NodeId = [u8; 32];

/// What a signature is for. Each purpose is its own signing domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// A `NodeHello` or `NodeWelcome` (`peer.rs:99`).
    PeerHello,
    /// An op's origin signature over its canonical bytes (`peer.rs:143`).
    OriginOp,
    /// The node-self-signed local overlay (`sysdir.rs:263`).
    LocalOverlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignError {
    /// The node key cannot be used, e.g. its material is unreadable.
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationError {
    /// Validity cannot be established: no key is known for the signer, or key
    /// material is unavailable. This is neither `Valid` nor `Invalid`.
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureStatus {
    /// The signer's key signed exactly these bytes for this purpose. Integrity
    /// only: not authorization, not freshness.
    Valid,
    /// Verification completed and the signature does not bind these bytes,
    /// this purpose and this signer.
    Invalid,
}

/// `node_id` is this handle's identity and is stable for its lifetime; a
/// rotated key is a new handle. `sign` signs exactly `message` for `purpose`,
/// and a signature made for one purpose MUST NOT verify for another: the purpose
/// is part of what is signed. Algorithm, domain encoding and key resolution are
/// the implementation's (the proof profile names them); none accepts anything.
///
/// `verify` is `Valid` only when `signer`'s key signed exactly `message` for
/// `purpose`. Inability to verify (no key known for `signer`, material
/// unavailable) is `Err(Unavailable)`, never `Valid` or `Invalid`. Callers fail
/// closed on anything but `Valid` and still check authorization and freshness.
///
/// Synchronous on purpose: the three seams are synchronous call sites and the
/// node key is a local file, so neither method waits on the network.
///
/// ```compile_fail
/// use glade_signer_api::SignerPort;
/// struct Missing;
/// impl SignerPort for Missing {}
/// ```
///
/// It bridges onto an injector's facade with `'static` asked of the
/// implementation (`Interface` is `shaku::Interface` as Shaku defines it):
///
/// ```
/// use glade_signer_api::{Purpose, SignError, SignerPort};
/// use std::any::Any;
/// trait Interface: Any + Send + Sync {}
/// impl<T: Any + Send + Sync> Interface for T {}
/// trait Signer: SignerPort + Interface {}
/// impl<T: SignerPort + 'static> Signer for T {}
/// fn is_interface<I: Interface + ?Sized>() {}
/// is_interface::<dyn Signer>();
/// fn build<T: SignerPort + 'static>(provider: T) -> Box<dyn Signer> { Box::new(provider) }
/// fn hello(signer: &dyn Signer) -> Result<Vec<u8>, SignError> { signer.sign(Purpose::PeerHello, b"") }
/// ```
///
/// The witness's form does not compile, because the port names no `Any`:
///
/// ```compile_fail,E0310
/// # use glade_signer_api::SignerPort;
/// # trait Interface: std::any::Any + Send + Sync {}
/// # impl<T: std::any::Any + Send + Sync> Interface for T {}
/// trait Signer: SignerPort + Interface {}
/// impl<T: SignerPort> Signer for T {}
/// ```
pub trait SignerPort: Send + Sync {
    fn node_id(&self) -> NodeId;
    fn sign(&self, purpose: Purpose, message: &[u8]) -> Result<Vec<u8>, SignError>;
    fn verify(
        &self,
        signer: &NodeId,
        purpose: Purpose,
        message: &[u8],
        signature: &[u8],
    ) -> Result<SignatureStatus, VerificationError>;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Probes; the fixture states which keys the port resolves. No algorithm
    //! vectors: a real adapter adds its own, and key-resolution security tests.
    use crate::{NodeId, Purpose, SignError, SignatureStatus, SignerPort, VerificationError};

    const PURPOSES: [Purpose; 3] = [Purpose::PeerHello, Purpose::OriginOp, Purpose::LocalOverlay];

    /// SI-001. Every purpose signs and verifies as this node; the identity is stable.
    pub fn round_trip(port: &dyn SignerPort) {
        let me = port.node_id();
        for purpose in PURPOSES {
            for message in [&b"canonical bytes"[..], b""] {
                let signature = port.sign(purpose, message).expect("SI-001 signing");
                let status = port.verify(&me, purpose, message, &signature);
                assert_eq!(
                    status,
                    Ok(SignatureStatus::Valid),
                    "SI-001 {purpose:?} as this node"
                );
            }
        }
        assert_eq!(port.node_id(), me, "SI-001 stable identity");
    }

    /// SI-002. A signature binds exactly its bytes, its purpose and its signer.
    /// The port MUST resolve `other`'s key and MUST NOT resolve `stranger`'s.
    pub fn binding(port: &dyn SignerPort, other: &NodeId, stranger: &NodeId) {
        let me = port.node_id();
        let signature = port.sign(Purpose::OriginOp, b"op").expect("SI-002 signing");
        let mut forged = signature.clone();
        if let Some(first) = forged.first_mut() {
            *first ^= 0xff;
        } else {
            forged.push(0xff);
        }
        let invalid = Ok(SignatureStatus::Invalid);
        let status = port.verify(&me, Purpose::OriginOp, b"oq", &signature);
        assert_eq!(status, invalid, "SI-002 message binding");
        let status = port.verify(&me, Purpose::OriginOp, b"op", &forged);
        assert_eq!(status, invalid, "SI-002 signature binding");
        for purpose in [Purpose::PeerHello, Purpose::LocalOverlay] {
            let status = port.verify(&me, purpose, b"op", &signature);
            assert_eq!(status, invalid, "SI-002 purpose binding");
        }
        let status = port.verify(other, Purpose::OriginOp, b"op", &signature);
        assert_eq!(status, invalid, "SI-002 signer binding");
        let status = port.verify(stranger, Purpose::OriginOp, b"op", &signature);
        let unknown = Err(VerificationError::Unavailable);
        assert_eq!(
            status, unknown,
            "SI-002 an unknown key is not proof of invalidity"
        );
    }

    /// SI-003. Requires key material configured unavailable: both directions
    /// refuse, and neither guesses.
    pub fn unavailable(port: &dyn SignerPort) {
        let me = port.node_id();
        let signed = port.sign(Purpose::OriginOp, b"op");
        assert_eq!(
            signed,
            Err(SignError::Unavailable),
            "SI-003 signing refuses"
        );
        let status = port.verify(&me, Purpose::OriginOp, b"op", &[0; 64]);
        let refused = Err(VerificationError::Unavailable);
        assert_eq!(status, refused, "SI-003 verification refuses");
    }
}
