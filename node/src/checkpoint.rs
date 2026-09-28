//! Signed checkpoints (plan Step 4.5c; AZ-12; the owner's rulings of
//! 2026-09-27 on `glade/dev-docs/GladeDirectoryCheckpoints.md`). A checkpoint,
//! `ChainCheckpoint {node, stream, seq, hash}`, is a record in node N's own
//! chain on `dir.checkpoints`: N's chain on `stream` is folded at `seq`, the
//! base B, whose op hashes to `hash`, H. It gives that chain a floor, (B, H):
//! the first op held above it is the one at B+1, whose `prev` is H, and an op
//! at or below B is covered, taken as seen and not held. `dir.checkpoints` is
//! a register, not a log: per origin and stream, the newest is kept.
//!
//! Here are the record's own check and the two chain rules, the floor and the
//! register, as pure functions: the registry applies them from part 1, and the
//! served store at part 2. Nothing makes a checkpoint before part 3.

use glade_wire::cbor::Cbor;
use glade_wire::generated::Op;

use crate::chain::op_hash;
use crate::envelope::{self, Refused};
use crate::registry::G_CLAIMS;
use crate::sysdata::ChainCheckpoint;

/// A chain's floor: the base B its newest checkpoint names, and H, the hash
/// of its op at B.
pub type Floor = (i64, [u8; 32]);

/// A checkpoint that passed [`check`]: the stream whose chain it folds, the
/// op's origin's, and the floor it gives that chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub stream: String,
    pub floor: Floor,
}

/// Whether this build compacts `stream`: `dir.claims` alone (section 3).
pub fn compacts(stream: &str) -> bool {
    stream == G_CLAIMS
}

/// The checkpoint `record` holds, read without panicking: exactly
/// `{1: text, 2: text, 3: int, 4: bytes}`, as `envelope::parse` reads it.
pub(crate) fn read(record: &[u8]) -> Option<ChainCheckpoint> {
    use Cbor::{Bytes, Int, Text};
    let Some(Cbor::Map(fields)) = envelope::parse(record) else {
        return None;
    };
    let fields = fields.as_slice();
    let [(1, Text(node)), (2, Text(stream)), (3, Int(seq)), (4, Bytes(hash))] = fields else {
        return None;
    };
    let (node, stream, seq, hash) = (node.clone(), stream.clone(), *seq, hash.clone());
    Some(ChainCheckpoint {
        node,
        stream,
        seq,
        hash,
    })
}

/// The record's own check (section 5), which [`envelope::verify`] makes after
/// its seven rules: a checkpoint names its op's origin, a stream this build
/// compacts, a base of 0 or more and a 32-byte hash. Anything else is refused
/// [`Refused::Checkpoint`].
pub fn check(op: &Op) -> Result<Checkpoint, Refused> {
    let record = read(&envelope::record_bytes(&op.payload)).ok_or(Refused::Checkpoint)?;
    let hash = <[u8; 32]>::try_from(record.hash.as_slice()).ok();
    let taken = record.node == op.origin && compacts(&record.stream) && record.seq >= 0;
    match (taken, hash) {
        (true, Some(hash)) => Ok(Checkpoint {
            stream: record.stream,
            floor: (record.seq, hash),
        }),
        _ => Err(Refused::Checkpoint),
    }
}

/// Where an op lands against its chain's floor (section 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Against {
    /// Above the base: the chain's own rules take it.
    Above,
    /// At or below the base: seen, and not held. Nothing it chains to is
    /// held, so it is not judged.
    Covered,
    /// At the base, with a hash other than the checkpoint's: a fork.
    Fork,
}

/// Where `op` lands against `floor`, its chain's.
pub fn against(floor: Floor, op: &Op) -> Against {
    let (base, hash) = floor;
    if op.seq > base {
        return Against::Above;
    }
    match op.seq == base && op_hash(op) != hash {
        true => Against::Fork,
        false => Against::Covered,
    }
}

/// Where a checkpoint lands in the register (section 6's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// None is held for its stream, or an older one naming a lower base, or
    /// the same base and hash: placed, and the held one leaves.
    Placed,
    /// The op held, again.
    Duplicate,
    /// A newer one is held: seen, and not held.
    Seen,
    /// Another op at the held one's seq: a fork.
    Fork,
    /// The held one is the checkpoint before it, which its `prev` does not
    /// name.
    ChainBreak,
    /// An older one is held that names a higher base, or the same base and
    /// another hash: a rewrite, refused.
    Rewrite,
}

/// Where `arriving`, a checkpoint's op and floor, lands against `held`, the
/// newest held for its origin and stream. With one compacted stream, the
/// checkpoint before it, where it is held, is the held one, so its `prev` is
/// checked there.
pub fn place(arriving: (&Op, Floor), held: Option<(&Op, Floor)>) -> Placement {
    let Some((held, (held_base, held_hash))) = held else {
        return Placement::Placed;
    };
    let (op, (base, hash)) = arriving;
    if op.seq < held.seq {
        return Placement::Seen;
    }
    if op.seq == held.seq {
        return match op_hash(op) == op_hash(held) {
            true => Placement::Duplicate,
            false => Placement::Fork,
        };
    }
    if base < held_base || (base == held_base && hash != held_hash) {
        return Placement::Rewrite;
    }
    let names_held = op.prev.as_deref() == Some(op_hash(held).as_slice());
    match op.seq == held.seq + 1 && !names_held {
        true => Placement::ChainBreak,
        false => Placement::Placed,
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::testing::checkpoint;
    use crate::peer::NodeIdentity;
    use crate::registry::{Ingested, Registry, RegistryError, G_PRINCIPALS};
    use crate::transport::hex;

    const SEED: [u8; 32] = [41; 32];

    /// Section 5's check, after the envelope's seven rules: a checkpoint is
    /// taken only of its own node's chain on a stream this build compacts,
    /// at a base of 0 or more, naming a 32-byte hash. Each flaw is refused
    /// `Checkpoint` by the check every `home` ingest makes, and by a sealed
    /// registry taking it in.
    #[test]
    fn a_checkpoint_is_taken_only_for_its_own_nodes_claims() {
        let me = hex(&NodeIdentity::from_key(SEED).node_id);
        let good = ChainCheckpoint {
            node: me.clone(),
            stream: G_CLAIMS.into(),
            seq: 3,
            hash: vec![7; 32],
        };
        let flawed = [
            (
                "another node's id",
                ChainCheckpoint {
                    node: hex(&NodeIdentity::from_key([42; 32]).node_id),
                    ..good.clone()
                },
            ),
            (
                "dir.principals",
                ChainCheckpoint {
                    stream: G_PRINCIPALS.into(),
                    ..good.clone()
                },
            ),
            (
                "an unknown stream",
                ChainCheckpoint {
                    stream: "dir.elsewhere".into(),
                    ..good.clone()
                },
            ),
            (
                "a negative seq",
                ChainCheckpoint {
                    seq: -1,
                    ..good.clone()
                },
            ),
            (
                "a 31-byte hash",
                ChainCheckpoint {
                    hash: vec![7; 31],
                    ..good.clone()
                },
            ),
        ];
        let node = || Registry::sealed(NodeIdentity::from_key([43; 32]));
        for (what, record) in flawed {
            let op = checkpoint(SEED, record, 0, None);
            assert_eq!(envelope::verify(&op), Err(Refused::Checkpoint), "{what}");
            let why = Refused::Checkpoint;
            let refused = RegistryError::Unverified {
                origin: me.clone(),
                why,
            };
            assert_eq!(node().ingest(op), Err(refused), "{what}");
        }
        let op = checkpoint(SEED, good, 0, None);
        assert_eq!(envelope::verify(&op), Ok(()));
        assert_eq!(
            node().ingest(op),
            Ok(Ingested::Appended),
            "a good one taken"
        );
    }
}
