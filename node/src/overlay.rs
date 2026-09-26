//! Class 3: `local.json`, the node-private overlay (GDL-036; plan Step 4.1c;
//! `GladeNodeSigning.md` D7). Authority overlay, suspect marks, resume
//! vectors: assertions a node makes about itself, never shipped. The overlay
//! only ever NARROWS granted rights, so tamper cannot exceed a grant, and
//! every assertion has a fail-closed default: a failed check discards each to
//! its most restrictive value, never to "off".
//!
//! The file holds the canonical CBOR of a `SignedRecord`, the form a `home`
//! record's envelope takes: `record` is the overlay, the canonical CBOR of a
//! map of assertions numbered from 1, and `sig` is the node key's Ed25519
//! signature over `glade/v1/local-overlay\0` then `record`. This build knows
//! no assertion, so the only overlay it applies is the empty map. Nothing
//! writes the file yet. The design is `glade/dev-docs/GladeNodeAssembly.md`,
//! "Custody and the local overlay's check", section 6.

use std::fs;
use std::io;
use std::path::Path;

use glade_signer_api::{Purpose, SignatureStatus};
use glade_wire::cbor::{self, Cbor};

use crate::envelope;
use crate::peer::NodeIdentity;
use crate::signing;

/// The overlay's file in an instance directory.
pub const FILE: &str = "local.json";

/// The node-private authority overlay: the assertions a node applies. This
/// build knows none, so it is empty, which is every assertion's fail-closed
/// default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LocalOverlay;

/// What a boot made of `local.json`: the overlay the node applies, and, when
/// the file failed its check, the line that says so.
#[derive(Debug, PartialEq)]
pub struct Checked {
    pub overlay: LocalOverlay,
    pub discarded: Option<String>,
}

/// Check the `local.json` in `dir` as `identity`'s. No file is the defaults,
/// and nothing is said; a file that fails the check is the defaults, and the
/// line says why.
pub fn load(dir: &Path, identity: &NodeIdentity) -> Checked {
    let path = dir.join(FILE);
    let checked = match fs::read(&path) {
        Ok(bytes) => check(&bytes, identity),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(LocalOverlay),
        Err(e) => Err(format!("cannot be read ({e})")),
    };
    match checked {
        Ok(overlay) => Checked {
            overlay,
            discarded: None,
        },
        Err(why) => {
            let path = path.display();
            let line = format!(
                "{path}: {why}; its assertions are discarded to their fail-closed defaults"
            );
            Checked {
                overlay: LocalOverlay,
                discarded: Some(line),
            }
        }
    }
}

/// The overlay `bytes` hold, if this node sealed them and this build knows
/// what they assert; else why not.
fn check(bytes: &[u8], identity: &NodeIdentity) -> Result<LocalOverlay, String> {
    let (record, sig) = envelope::open(bytes).ok_or("not a signed overlay")?;
    let signed = signing::verify(&identity.node_id, Purpose::LocalOverlay, &record, &sig);
    if let SignatureStatus::Invalid = signed {
        return Err("its signature is not this node's".into());
    }
    match envelope::parse(&record) {
        Some(Cbor::Map(items)) if !items.is_empty() => Err(format!(
            "it holds {} assertion(s) this build does not know",
            items.len()
        )),
        Some(Cbor::Map(_)) if record == cbor::encode(&Cbor::Map(Vec::new())) => Ok(LocalOverlay),
        _ => Err("its overlay is not a canonical map".into()),
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysdata::SignedRecord;

    const SEED: [u8; 32] = [41; 32];

    /// `record` sealed as the node whose key is `seed` seals, for `purpose`.
    fn sealed(seed: [u8; 32], purpose: Purpose, record: Vec<u8>) -> Vec<u8> {
        let sig = signing::sign(&seed, purpose, &record).to_vec();
        cbor::encode(&SignedRecord { record, sig }.to_cbor())
    }

    /// D7's check (plan Step 4.1c): the empty overlay, sealed by this node
    /// under the local-overlay tag, is taken; and each flaw is refused for
    /// its own reason: a byte of the signature changed, the tag of another
    /// purpose, another node's key, the overlay bare, an assertion this build
    /// does not know, and an empty map encoded otherwise than canonically.
    /// Until 4.1c the file was read and never checked.
    #[test]
    fn local_json_is_taken_only_as_this_nodes_empty_overlay_under_its_tag() {
        let identity = NodeIdentity::from_key(SEED);
        let empty = cbor::encode(&Cbor::Map(Vec::new()));
        let taken = sealed(SEED, Purpose::LocalOverlay, empty.clone());
        assert_eq!(check(&taken, &identity), Ok(LocalOverlay));

        let mut flipped = taken.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        let asserting = cbor::encode(&Cbor::Map(vec![(1, Cbor::Int(1))]));
        let not_this_nodes = "its signature is not this node's";
        let refused = [
            (not_this_nodes, flipped),
            (
                not_this_nodes,
                sealed(SEED, Purpose::OriginOp, empty.clone()),
            ),
            (
                not_this_nodes,
                sealed([42; 32], Purpose::LocalOverlay, empty.clone()),
            ),
            ("not a signed overlay", empty),
            (
                "it holds 1 assertion(s) this build does not know",
                sealed(SEED, Purpose::LocalOverlay, asserting),
            ),
            (
                "its overlay is not a canonical map",
                sealed(SEED, Purpose::LocalOverlay, vec![0xb8, 0x00]),
            ),
        ];
        for (why, bytes) in refused {
            assert_eq!(check(&bytes, &identity), Err(why.to_owned()), "{why}");
        }
    }
}
