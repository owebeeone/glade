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
//! served store at part 2. From part 3, a node makes its own: a renewal tick
//! folds its `dir.claims` chain once enough of its claims are superseded
//! ([`superseded`], [`tick`]).

use std::fmt;

use glade_wire::cbor::Cbor;
use glade_wire::generated::Op;

use crate::chain::op_hash;
use crate::envelope::{self, Refused};
use crate::registry::{Record, Registry, RegistryError, G_CLAIMS};
use crate::sysdata::{ChainCheckpoint, ServeClaim};

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

/// Which of `claims`, one node's in chain order, are superseded (section 3):
/// a claim is when a later one names the same node and share with an epoch
/// at least as high and a lease that ends no earlier. It is live only while
/// that one is, so every claims fold answers without it as with it, at every
/// reader's clock. Every other claim is state.
pub fn superseded(claims: &[ServeClaim]) -> Vec<bool> {
    // The later claims nothing supersedes: whatever a superseded one
    // supersedes, so does the claim that supersedes it.
    let mut standing: Vec<&ServeClaim> = Vec::new();
    let mut gone = vec![false; claims.len()];
    for (at, claim) in claims.iter().enumerate().rev() {
        let over = |later: &&ServeClaim| {
            let named = later.node == claim.node && later.share == claim.share;
            let ends = later.lease_expiry_ms >= claim.lease_expiry_ms;
            named && later.epoch >= claim.epoch && ends
        };
        gone[at] = standing.iter().any(over);
        if !gone[at] {
            standing.push(claim);
        }
    }
    gone
}

/// What a tick's checkpoint folded: its base, and how many of the claims at
/// or below it left, and were carried. Its `Display` is the line the root's
/// reporter prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folded {
    pub base: i64,
    pub dropped: usize,
    pub carried: usize,
}

impl fmt::Display for Folded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (base, dropped, carried) = (self.base, self.dropped, self.carried);
        let folded = format!("{G_CLAIMS} folded at seq {base}");
        let dropped = format!("{dropped} superseded claim(s) dropped");
        write!(f, "checkpoint: {folded}, {dropped}, {carried} carried")
    }
}

/// A renewal tick's appends to the staged `registry`, as `node` (section 4):
/// its `renewals`, and its checkpoint once at least `after` of the claims its
/// chain holds are superseded. Then B is the chain's tip before the tick: the
/// claims at or below B that are not superseded, the renewals counted, are
/// appended again, in their order; then the renewals; then the checkpoint at
/// B, which the registry places, dropping every claim at or below B and the
/// checkpoint before it. The ops in the order they were appended, which is
/// the order they are published in, the checkpoint last, and what it folded.
pub fn tick(
    registry: &mut Registry,
    node: &str,
    renewals: Vec<ServeClaim>,
    after: usize,
) -> Result<(Vec<Op>, Option<Folded>), RegistryError> {
    let (floor, carried, covered) = due(&registry.chain(G_CLAIMS, node), &renewals, after);
    let kept = carried.len();
    let mut ops = Vec::new();
    for claim in carried.into_iter().chain(renewals) {
        ops.push(registry.append_returning(Record::Serve(claim), node)?);
    }
    let Some((base, hash)) = floor else {
        return Ok((ops, None));
    };
    let record = ChainCheckpoint {
        node: node.into(),
        stream: G_CLAIMS.into(),
        seq: base,
        hash: hash.to_vec(),
    };
    ops.push(registry.append_returning(Record::Checkpoint(record), node)?);
    let (dropped, carried) = (covered - kept, kept);
    let folded = Folded {
        base,
        dropped,
        carried,
    };
    Ok((ops, Some(folded)))
}

/// What a tick that appends `renewals` to the claims chain `held` folds. Once
/// at least `after` of the held claims are superseded: the floor at the
/// chain's tip, the held claims that the renewals do not supersede either,
/// which are carried, and how many claims the floor covers. Else no floor.
fn due(
    held: &[&Op],
    renewals: &[ServeClaim],
    after: usize,
) -> (Option<Floor>, Vec<ServeClaim>, usize) {
    // A chain holding fewer claims than `after` has fewer superseded: most
    // ticks decode none.
    let Some(tip) = held.last().filter(|_| held.len() >= after) else {
        return (None, Vec::new(), 0);
    };
    let read = |op: &&Op| envelope::folded(op, ServeClaim::from_cbor);
    let mut claims: Vec<ServeClaim> = held.iter().filter_map(read).collect();
    let count = superseded(&claims).into_iter().filter(|gone| *gone).count();
    if count < after {
        return (None, Vec::new(), 0);
    }
    let before = claims.len();
    claims.extend_from_slice(renewals);
    let gone = superseded(&claims);
    claims.truncate(before);
    let carried = claims.into_iter().zip(gone).filter(|(_, gone)| !gone);
    let carried = carried.map(|(claim, _)| claim).collect();
    (Some((tip.seq, op_hash(tip))), carried, held.len())
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

    /// Section 3's rule, over one node's claims: two shares served, restarts
    /// that raise `ws-a`'s epoch on shorter leases, an exact repeat, the
    /// clock stepped back, and a share, `ws-gone`, that lapsed and was never
    /// renewed. Over the claims no later one supersedes, every claims fold
    /// answers as over the whole history, at instants either side of every
    /// expiry: `who_serves`, the registry's and the served store's,
    /// `max_claim_epoch`, `home_epoch` and `directory_knows`. What leaves is
    /// every renewal an equal or later lease at an equal or higher epoch
    /// supersedes, and nothing else.
    #[test]
    fn only_superseded_claims_leave_and_every_claims_fold_answers_alike() {
        use crate::claims::{home_epoch, max_claim_epoch};
        use crate::mesh::{directory_knows, who_serves};
        use crate::registry::{RegistryApi, HOME};
        use crate::store::Store;

        let me = hex(&NodeIdentity::from_key(SEED).node_id);
        let claim = |share: &str, epoch: i64, lease_expiry_ms: i64| ServeClaim {
            node: me.clone(),
            share: share.into(),
            lease_expiry_ms,
            epoch,
        };
        let history = [
            claim(HOME, 1, 1_000),
            claim("ws-a", 1, 3_000),
            claim("ws-gone", 1, 900),
            claim(HOME, 1, 1_300),
            claim("ws-a", 2, 2_000),
            claim(HOME, 1, 2_000),
            claim(HOME, 1, 2_000),
            claim(HOME, 1, 1_700),
            claim("ws-a", 2, 2_400),
            claim("ws-a", 3, 2_200),
        ];
        let gone = superseded(&history);
        let kept = history.iter().zip(&gone).filter(|(_, gone)| !**gone);
        let kept: Vec<ServeClaim> = kept.map(|(claim, _)| claim.clone()).collect();
        // Each set of claims appended by the node's registry and landed in a
        // served store of its own.
        let folds = |claims: &[ServeClaim], name: &str| {
            let mut registry = Registry::sealed(NodeIdentity::from_key(SEED));
            let root = std::env::temp_dir().join(format!("glade-checkpoint-{name}"));
            let _ = std::fs::remove_dir_all(&root);
            let mut store = Store::open(&root).unwrap();
            for claim in claims {
                let record = Record::Serve(claim.clone());
                let op = registry.append_returning(record, &me).unwrap();
                store.append(op).unwrap();
            }
            (registry, store)
        };
        let (whole, whole_store) = folds(&history, "whole");
        let (left, left_store) = folds(&kept, "kept");
        let expiries = history.iter().map(|claim| claim.lease_expiry_ms);
        let mut instants: Vec<i64> = expiries.flat_map(|end| [end - 1, end, end + 1]).collect();
        instants.sort_unstable();
        instants.dedup();
        let shares = [HOME, "ws-a", "ws-gone", "ws-never"];
        for at in instants {
            for share in shares {
                let what = format!("who_serves({share}) at {at}");
                let (answer, expected) = (left.who_serves(share, at), whole.who_serves(share, at));
                assert_eq!(answer, expected, "{what}, the registry's");
                let served = |store: &Store| who_serves(store, share, at);
                let (answer, expected) = (served(&left_store), served(&whole_store));
                assert_eq!(answer, expected, "{what}, the served store's");
            }
        }
        for share in shares {
            let epoch = |store: &Store| max_claim_epoch(store, share);
            let (answer, expected) = (epoch(&left_store), epoch(&whole_store));
            assert_eq!(answer, expected, "max_claim_epoch({share})");
            let knows = |store: &Store| directory_knows(store, share);
            let (answer, expected) = (knows(&left_store), knows(&whole_store));
            assert_eq!(answer, expected, "directory_knows({share})");
        }
        assert_eq!(home_epoch(&left_store, &me), home_epoch(&whole_store, &me));
        // `home`'s first two, `ws-a`'s at epoch 2 to 2,000, and the first of
        // the repeat.
        let leaving: Vec<usize> = (0..gone.len()).filter(|at| gone[*at]).collect();
        assert_eq!(leaving, [0, 3, 4, 5], "what leaves");
    }
}
