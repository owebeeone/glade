//! Home records signed (plan Step 4.1b; the owner's rulings on
//! `glade/dev-docs/GladeNodeSigning.md`, D4, D5 and D7). Every op on the `home`
//! share carries its record inside a `SignedRecord` envelope, sealed by the
//! op's origin: an Ed25519 signature over the `origin-op` tag, then the op's
//! fields 1 to 10 with the record as the payload, so it covers the op's chain
//! position. The origin is a node id, which is its key, so a check needs no
//! lookup. [`verify`] is the check every `home` ingest makes, and [`record`]
//! is how a fold reads what an op carries. The design is
//! `glade/dev-docs/GladeNodeAssembly.md`, "Home records signed (plan Step
//! 4.1b)".

use std::fmt;

use glade_signer_api::{Purpose, SignatureStatus};
use glade_wire::cbor::{self, Cbor};
use glade_wire::generated::{Op, Shape};

use crate::peer::NodeIdentity;
use crate::registry::{
    G_BINDINGS, G_BINDING_RETRACTIONS, G_CLAIMS, G_GRANTS, G_NODES, G_PRINCIPALS, G_REVOCATIONS,
    G_SERVICES, G_TRANSPORT_BINDINGS, G_TRANSPORT_REVOCATIONS, G_WORKSPACES, HOME,
};
use crate::signing;
use crate::sysdata::SignedRecord;
use crate::transport::key_of;

/// Why a `home` op was refused: the first of [`verify`]'s rules it breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Its payload is not an envelope: a record written before plan Step
    /// 4.1b, or one never signed.
    Unsigned,
    /// Not the directory's form: a zone key, a shape other than `log`, refs.
    Form,
    /// Seq 0 naming a predecessor, or a later seq naming none (B5).
    Prev,
    /// Not one of the directory's streams.
    Stream,
    /// The record is not its stream's kind, canonically encoded.
    Kind,
    /// The origin is not a node id, 64 lower-case hex digits.
    Origin,
    /// The signature does not verify under the origin's key.
    Signature,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refused::Unsigned => "unsigned",
            Refused::Form => "not the directory's form",
            Refused::Prev => "its seq and prev disagree",
            Refused::Stream => "not a directory stream",
            Refused::Kind => "not its stream's kind",
            Refused::Origin => "its origin is not a node id",
            Refused::Signature => "its signature does not verify",
        })
    }
}

/// `op`'s payload, a record, sealed by `identity`: the envelope's bytes. The
/// caller builds `op` with every other field final, since the signature
/// covers them all.
pub fn seal(identity: &NodeIdentity, op: &Op) -> Vec<u8> {
    let sig = identity.sign(Purpose::OriginOp, &cbor::encode(&op.to_cbor()));
    let record = op.payload.clone();
    cbor::encode(&SignedRecord { record, sig }.to_cbor())
}

/// The record and the signature `payload` holds, if it is an envelope:
/// exactly `{1: bytes, 2: 64 bytes}`, canonically encoded.
fn open(payload: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let Some(Cbor::Map(fields)) = parse(payload) else {
        return None;
    };
    let [(1, Cbor::Bytes(record)), (2, Cbor::Bytes(sig))] = fields.as_slice() else {
        return None;
    };
    let canonical = sig.len() == 64 && cbor::encode(&Cbor::Map(fields.clone())) == payload;
    canonical.then(|| (record.clone(), sig.clone()))
}

/// Whether `payload` is an envelope at all, signed or not: what a record
/// written before plan Step 4.1b is not.
pub fn is_envelope(payload: &[u8]) -> bool {
    open(payload).is_some()
}

/// The record `payload` carries: the envelope's, or the payload itself when
/// it is not one. Only an unsealed registry holds a bare record, and only
/// tests and the journeys' record host use one: the verified stores hold
/// envelopes alone.
pub fn record_bytes(payload: &[u8]) -> Vec<u8> {
    open(payload).map_or_else(|| payload.to_vec(), |(record, _)| record)
}

/// The record `op` carries, decoded by `from`, a record kind's `from_cbor`.
/// A verified store's records are each their stream's kind, so none panics
/// the wire codec's decoders.
pub fn record<T>(op: &Op, from: impl FnOnce(&Cbor) -> T) -> T {
    from(&cbor::decode(&record_bytes(&op.payload)))
}

/// The check every `home` ingest makes, its rules in this order: an envelope;
/// the directory's form; seq 0 with no `prev` and every later seq with one;
/// a directory stream; that stream's kind; a node id as origin; and the
/// origin's signature, checked strictly. Chain continuity is the caller's.
pub fn verify(op: &Op) -> Result<(), Refused> {
    let (record, sig) = open(&op.payload).ok_or(Refused::Unsigned)?;
    let form = op.share == HOME && op.key.is_empty() && op.shape == Shape::Log;
    if !form || !op.refs.is_empty() {
        return Err(Refused::Form);
    }
    if op.seq < 0 || op.prev.is_some() != (op.seq > 0) {
        return Err(Refused::Prev);
    }
    let fields = kind(&op.glade_id).ok_or(Refused::Stream)?;
    if !is_kind(&record, fields) {
        return Err(Refused::Kind);
    }
    let signer = key_of(&op.origin).ok_or(Refused::Origin)?;
    let signed = Op {
        payload: record,
        ..op.clone()
    };
    let message = cbor::encode(&signed.to_cbor());
    match signing::verify(&signer, Purpose::OriginOp, &message, &sig) {
        SignatureStatus::Valid => Ok(()),
        SignatureStatus::Invalid => Err(Refused::Signature),
    }
}

/// Whether `glade_id` is one of the directory's streams: the ones the
/// directory profile hosts (`assembly::DirectoryRules`).
pub fn directory_stream(glade_id: &str) -> bool {
    kind(glade_id).is_some()
}

/// A record field's type, as `node/ir/sysdata.taut.py` declares it.
#[derive(Clone, Copy, Debug)]
enum Field {
    Text,
    Int,
    Bytes,
    /// A list of text.
    Texts,
}

/// The kind each directory stream holds: its fields, numbered from 1.
fn kind(glade_id: &str) -> Option<&'static [Field]> {
    use Field::{Bytes, Int, Text, Texts};
    let fields: &'static [Field] = match glade_id {
        G_NODES | G_REVOCATIONS | G_BINDING_RETRACTIONS => &[Text, Text],
        G_WORKSPACES | G_GRANTS => &[Text, Text, Texts],
        G_CLAIMS => &[Text, Text, Int, Int],
        G_BINDINGS => &[Text, Text, Text, Text, Text, Text],
        G_SERVICES => &[Text, Text, Text],
        G_PRINCIPALS => &[Text],
        G_TRANSPORT_BINDINGS => &[Text, Text, Int, Bytes],
        G_TRANSPORT_REVOCATIONS => &[Text, Text, Bytes],
        _ => return None,
    };
    Some(fields)
}

/// Whether `record` is a record of `fields`, exactly: the canonical CBOR of a
/// map whose keys are 1 to n, each value of its field's type.
fn is_kind(record: &[u8], fields: &[Field]) -> bool {
    let Some(value) = parse(record) else {
        return false;
    };
    let Cbor::Map(entries) = &value else {
        return false;
    };
    let typed = |(i, ((key, value), field)): (usize, (&(i64, Cbor), &Field))| {
        let field_type = match (field, value) {
            (Field::Text, Cbor::Text(_)) | (Field::Int, Cbor::Int(_)) => true,
            (Field::Bytes, Cbor::Bytes(_)) => true,
            (Field::Texts, Cbor::Array(items)) => {
                items.iter().all(|item| matches!(item, Cbor::Text(_)))
            }
            _ => false,
        };
        i64::try_from(i + 1) == Ok(*key) && field_type
    };
    let shaped = entries.len() == fields.len() && entries.iter().zip(fields).enumerate().all(typed);
    shaped && cbor::encode(&value) == record
}

/// The CBOR item `bytes` holds, read without panicking: integers, byte and
/// text strings, and arrays and integer-keyed maps of them, nested at most
/// two deep. `None` for anything else, or for bytes left over. The wire
/// codec's `decode` panics on bytes it cannot read, and a peer's bytes are
/// read before they are trusted.
pub(crate) fn parse(bytes: &[u8]) -> Option<Cbor> {
    let mut at = 0;
    let value = item(bytes, &mut at, 2)?;
    (at == bytes.len()).then_some(value)
}

/// One item at `*at`, containers only while `depth` allows.
fn item(bytes: &[u8], at: &mut usize, depth: u8) -> Option<Cbor> {
    let (major, n) = head(bytes, at)?;
    match major {
        0 => i64::try_from(n).ok().map(Cbor::Int),
        1 => i64::try_from(n).ok().map(|n| Cbor::Int(-1 - n)),
        2 | 3 => {
            let end = at.checked_add(usize::try_from(n).ok()?)?;
            let raw = bytes.get(*at..end)?.to_vec();
            *at = end;
            match major {
                2 => Some(Cbor::Bytes(raw)),
                _ => String::from_utf8(raw).ok().map(Cbor::Text),
            }
        }
        4 | 5 if depth > 0 => {
            // An entry takes a byte at least, so a count past the bytes left
            // is refused before anything is allocated for it.
            let n = usize::try_from(n)
                .ok()
                .filter(|n| *n <= bytes.len() - *at)?;
            if major == 4 {
                let items = (0..n).map(|_| item(bytes, at, depth - 1));
                return items.collect::<Option<_>>().map(Cbor::Array);
            }
            let entry = |at: &mut usize| {
                let Cbor::Int(key) = item(bytes, at, 0)? else {
                    return None;
                };
                Some((key, item(bytes, at, depth - 1)?))
            };
            let entries = (0..n).map(|_| entry(at));
            entries.collect::<Option<_>>().map(Cbor::Map)
        }
        _ => None,
    }
}

/// One CBOR item's head at `*at`: its major type and its argument.
fn head(bytes: &[u8], at: &mut usize) -> Option<(u8, u64)> {
    let first = *bytes.get(*at)?;
    *at += 1;
    let width = match first & 0x1f {
        n @ 0..=23 => return Some((first >> 5, u64::from(n))),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        _ => return None,
    };
    let raw = bytes.get(*at..*at + width)?;
    *at += width;
    Some((
        first >> 5,
        raw.iter().fold(0u64, |n, b| (n << 8) | u64::from(*b)),
    ))
}

// Sealed ops for other modules' tests. A braced module, so the condition
// encloses the whole section.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::registry::{Record, Registry};

    /// `record`, sealed as the first op of its stream in the chain of the
    /// node whose key is `seed`, as that node's registry appends it.
    pub(crate) fn sealed(seed: [u8; 32], record: Record) -> Op {
        let identity = NodeIdentity::from_key(seed);
        let origin = crate::transport::hex(&identity.node_id);
        let appended = Registry::sealed(identity).append_returning(record, &origin);
        appended.expect("a sealed registry appends as its own node")
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Record, Registry};
    use crate::sysdata::{
        BindingDecl, BindingRetraction, CapabilityGrant, CapabilityRevocation, NodeRecord,
        NodeTransportBinding, NodeTransportRevocation, PrincipalRecord, ServeClaim,
        ServiceDefinition, WorkspaceEntry,
    };
    use ed25519_dalek::{Signature, SigningKey};

    const SEED: [u8; 32] = [31; 32];

    fn claim(epoch: i64) -> Record {
        Record::Serve(ServeClaim {
            node: "n".into(),
            share: "ws-a".into(),
            lease_expiry_ms: 5_000,
            epoch,
        })
    }

    /// The first two ops of this node's claims chain, sealed.
    fn chain() -> (Op, Op) {
        let identity = NodeIdentity::from_key(SEED);
        let origin = crate::transport::hex(&identity.node_id);
        let mut registry = Registry::sealed(identity);
        let first = registry.append_returning(claim(1), &origin).unwrap();
        let second = registry.append_returning(claim(2), &origin).unwrap();
        (first, second)
    }

    /// Section 3's rules, each broken once, in order: the sealed op verifies,
    /// and each flaw is refused for its own reason, the signature covering
    /// the op's position as well as its record. It tries one flaw of each
    /// kind, not every byte.
    #[test]
    fn a_sealed_record_verifies_and_each_flaw_is_refused() {
        let (first, second) = chain();
        assert_eq!(verify(&first), Ok(()));
        assert_eq!(verify(&second), Ok(()));
        let bare = Op {
            payload: claim(1).encode(),
            ..first.clone()
        };
        let reseal = |op: &Op, seed: [u8; 32]| {
            let unsealed = Op {
                payload: record_bytes(&op.payload),
                ..op.clone()
            };
            Op {
                payload: seal(&NodeIdentity::from_key(seed), &unsealed),
                ..op.clone()
            }
        };
        let mut flipped = first.clone();
        let last = flipped.payload.len() - 1;
        flipped.payload[last] ^= 1;
        let another_key = reseal(&first, [32; 32]);
        let moved = Op {
            seq: 5,
            ..second.clone()
        };
        let no_prev = Op {
            prev: None,
            ..second.clone()
        };
        let keyed = Op {
            key: b"k".to_vec(),
            ..first.clone()
        };
        let elsewhere = Op {
            glade_id: "dir.elsewhere".into(),
            ..first.clone()
        };
        let another_kind = reseal(
            &Op {
                glade_id: G_NODES.into(),
                ..first.clone()
            },
            SEED,
        );
        let shouting = Op {
            origin: first.origin.to_uppercase(),
            ..first.clone()
        };
        let mut long_form = first.payload.clone();
        long_form.splice(1..2, [0x18, 1]);
        let non_canonical = Op {
            payload: long_form,
            ..first.clone()
        };
        let flaws = [
            ("a bare record", bare, Refused::Unsigned),
            ("a non-canonical envelope", non_canonical, Refused::Unsigned),
            ("a zone key", keyed, Refused::Form),
            ("seq 1 with no prev", no_prev, Refused::Prev),
            ("another stream", elsewhere, Refused::Stream),
            ("a claim on dir.nodes", another_kind, Refused::Kind),
            ("an upper-case origin", shouting, Refused::Origin),
            ("a flipped signature byte", flipped, Refused::Signature),
            ("another key's signature", another_key, Refused::Signature),
            ("another seq", moved, Refused::Signature),
        ];
        for (what, op, why) in flaws {
            assert_eq!(verify(&op), Err(why), "{what}");
        }
    }

    /// D7's encoding: the signature is pure Ed25519, by the origin's key, over
    /// the `origin-op` tag then the canonical CBOR of the op's ten fields with
    /// the record as its payload, built here by hand. Another Ed25519 library
    /// checks it by putting the tag first. It pins the layout it was written
    /// with; the `proof_family` corpus has no vectors yet.
    #[test]
    fn the_signature_is_pure_ed25519_over_the_tag_then_the_op_with_its_record() {
        let (_, op) = chain();
        let record = claim(2).encode();
        let fields = Cbor::Map(vec![
            (1, Cbor::Text(HOME.into())),
            (2, Cbor::Text(G_CLAIMS.into())),
            (3, Cbor::Bytes(vec![])),
            (4, Cbor::Text(op.origin.clone())),
            (5, Cbor::Int(1)),
            (6, Cbor::Bytes(op.prev.clone().unwrap())),
            (7, Cbor::Int(1)),
            (8, Cbor::Array(vec![])),
            (9, Cbor::Int(Shape::Log.wire())),
            (10, Cbor::Bytes(record.clone())),
        ]);
        let signed = [&b"glade/v1/origin-op\0"[..], &cbor::encode(&fields)].concat();
        let (inside, sig) = open(&op.payload).unwrap();
        assert_eq!(inside, record);
        let key = SigningKey::from_bytes(&SEED).verifying_key();
        let sig = Signature::from_slice(&sig).unwrap();
        assert!(key.verify_strict(&signed, &sig).is_ok());
    }

    /// The kind table against the generated codecs: each of the eleven
    /// directory streams takes its kind as the codec encodes it, and not with
    /// a byte left over; a claim is no node record, verbs are text, and a
    /// stream outside the directory has no kind.
    #[test]
    fn each_directory_kind_passes_its_own_check_and_a_misshapen_one_fails() {
        let text = |s: &str| s.to_string();
        let records = [
            (
                G_NODES,
                NodeRecord {
                    node_id: text("n"),
                    operator: text("o"),
                }
                .to_cbor(),
            ),
            (
                G_WORKSPACES,
                WorkspaceEntry {
                    workspace: text("w"),
                    name: text("n"),
                    eligible_hosts: vec![text("h")],
                }
                .to_cbor(),
            ),
            (
                G_CLAIMS,
                ServeClaim {
                    node: text("n"),
                    share: text("s"),
                    lease_expiry_ms: -1,
                    epoch: 1,
                }
                .to_cbor(),
            ),
            (
                G_GRANTS,
                CapabilityGrant {
                    principal: text("p"),
                    share: text("s"),
                    verbs: vec![],
                }
                .to_cbor(),
            ),
            (
                G_REVOCATIONS,
                CapabilityRevocation {
                    principal: text("p"),
                    share: text("s"),
                }
                .to_cbor(),
            ),
            (
                G_BINDINGS,
                BindingDecl {
                    app: text("a"),
                    glade_id: text("g"),
                    ..BindingDecl::default()
                }
                .to_cbor(),
            ),
            (
                G_BINDING_RETRACTIONS,
                BindingRetraction {
                    app: text("a"),
                    glade_id: text("g"),
                }
                .to_cbor(),
            ),
            (
                G_SERVICES,
                ServiceDefinition {
                    app: text("a"),
                    name: text("n"),
                    glade_id: text("g"),
                }
                .to_cbor(),
            ),
            (
                G_PRINCIPALS,
                PrincipalRecord {
                    principal: text("p"),
                }
                .to_cbor(),
            ),
            (
                G_TRANSPORT_BINDINGS,
                NodeTransportBinding {
                    valid_from: 7,
                    sig: vec![1; 64],
                    ..NodeTransportBinding::default()
                }
                .to_cbor(),
            ),
            (
                G_TRANSPORT_REVOCATIONS,
                NodeTransportRevocation {
                    sig: vec![1; 64],
                    ..NodeTransportRevocation::default()
                }
                .to_cbor(),
            ),
        ];
        for (stream, record) in &records {
            let bytes = cbor::encode(record);
            assert!(is_kind(&bytes, kind(stream).unwrap()), "{stream}");
            let mut listed = bytes.clone();
            listed.push(0);
            assert!(
                !is_kind(&listed, kind(stream).unwrap()),
                "{stream}: a byte left over"
            );
        }
        let claim = cbor::encode(&records[2].1);
        assert!(
            !is_kind(&claim, kind(G_NODES).unwrap()),
            "a claim is no node record"
        );
        let texts = cbor::encode(&Cbor::Map(vec![
            (1, Cbor::Text(text("p"))),
            (2, Cbor::Text(text("s"))),
            (3, Cbor::Array(vec![Cbor::Int(1)])),
        ]));
        assert!(!is_kind(&texts, kind(G_GRANTS).unwrap()), "verbs are text");
        assert!(!directory_stream("dir.elsewhere"));
    }

    /// The checked decoder reads what the codec writes, and answers `None`,
    /// never a panic, for bytes it cannot read: a torn item, one left over, a
    /// float, a map keyed by text, nesting past two, a count past the bytes.
    #[test]
    fn the_checked_decoder_reads_the_codecs_bytes_and_refuses_the_rest() {
        let written = Cbor::Map(vec![
            (1, Cbor::Text("t".into())),
            (2, Cbor::Array(vec![Cbor::Int(-3), Cbor::Bytes(vec![9])])),
        ]);
        let bytes = cbor::encode(&written);
        assert_eq!(parse(&bytes), Some(written));
        let nested = cbor::encode(&Cbor::Array(vec![Cbor::Array(vec![Cbor::Array(vec![])])]));
        let unreadable: [&[u8]; 6] = [
            &bytes[..bytes.len() - 1],
            &[bytes.as_slice(), &[0]].concat(),
            &[0xf9, 0x3c, 0x00],
            &[0xa1, 0x61, 0x6b, 0x01],
            &nested,
            &[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        ];
        for bytes in unreadable {
            assert_eq!(parse(bytes), None, "{bytes:02x?}");
        }
    }
}
