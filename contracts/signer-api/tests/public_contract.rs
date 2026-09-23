use glade_signer_api::{
    NodeId, Purpose, SignError, SignatureStatus, SignerPort, VerificationError, conformance,
};
use std::collections::BTreeMap;

const ME: NodeId = [1; 32];
const OTHER: NodeId = [2; 32];
const STRANGER: NodeId = [3; 32];

/// Deliberately wrong behaviours, each caught by one probe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrong {
    AcceptsAnything,
    OneDomain,
    UnknownIsInvalid,
}

/// A keyed checksum standing in for a signature: FNV-1a over a per-node secret,
/// the purpose and the message. Anyone who reads this file can forge it; it
/// pins the contract's shape and is never cryptography.
fn checksum(secret: u64, domain: u8, message: &[u8]) -> Vec<u8> {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ secret;
    for byte in std::iter::once(&domain).chain(message) {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash.to_le_bytes().to_vec()
}

struct Keys {
    secrets: BTreeMap<NodeId, u64>,
    available: bool,
    wrong: Option<Wrong>,
}

fn keys(available: bool, wrong: Option<Wrong>) -> Keys {
    Keys {
        secrets: BTreeMap::from([(ME, 11), (OTHER, 22)]),
        available,
        wrong,
    }
}

impl Keys {
    fn domain(&self, purpose: Purpose) -> u8 {
        if self.wrong == Some(Wrong::OneDomain) {
            return 0;
        }
        purpose as u8
    }
}

impl SignerPort for Keys {
    fn node_id(&self) -> NodeId {
        ME
    }

    fn sign(&self, purpose: Purpose, message: &[u8]) -> Result<Vec<u8>, SignError> {
        if !self.available {
            return Err(SignError::Unavailable);
        }
        Ok(checksum(self.secrets[&ME], self.domain(purpose), message))
    }

    fn verify(
        &self,
        signer: &NodeId,
        purpose: Purpose,
        message: &[u8],
        signature: &[u8],
    ) -> Result<SignatureStatus, VerificationError> {
        if !self.available {
            return Err(VerificationError::Unavailable);
        }
        if self.wrong == Some(Wrong::AcceptsAnything) {
            return Ok(SignatureStatus::Valid);
        }
        let Some(secret) = self.secrets.get(signer) else {
            if self.wrong == Some(Wrong::UnknownIsInvalid) {
                return Ok(SignatureStatus::Invalid);
            }
            return Err(VerificationError::Unavailable);
        };
        if checksum(*secret, self.domain(purpose), message) == signature {
            return Ok(SignatureStatus::Valid);
        }
        Ok(SignatureStatus::Invalid)
    }
}

#[test]
fn si_001_every_purpose_signs_and_verifies_as_this_node() {
    conformance::round_trip(&keys(true, None));
}

#[test]
fn si_002_a_signature_binds_bytes_purpose_and_signer() {
    conformance::binding(&keys(true, None), &OTHER, &STRANGER);
}

#[test]
fn si_003_unavailable_key_material_refuses_both_ways() {
    conformance::unavailable(&keys(false, None));
}

#[test]
#[should_panic(expected = "SI-002 message binding")]
fn rejects_an_accept_anything_verifier() {
    conformance::binding(&keys(true, Some(Wrong::AcceptsAnything)), &OTHER, &STRANGER);
}

#[test]
#[should_panic(expected = "SI-002 purpose binding")]
fn rejects_one_domain_for_every_purpose() {
    conformance::binding(&keys(true, Some(Wrong::OneDomain)), &OTHER, &STRANGER);
}

#[test]
#[should_panic(expected = "SI-002 an unknown key is not proof of invalidity")]
fn rejects_an_unknown_key_reported_as_invalid() {
    conformance::binding(
        &keys(true, Some(Wrong::UnknownIsInvalid)),
        &OTHER,
        &STRANGER,
    );
}
