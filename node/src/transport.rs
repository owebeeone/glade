//! The transport-key binding (plan Step 4.2; the ruling
//! `transport_key_binding = binding_record`): a node's iroh endpoint key is
//! bound to its Glade identity by a record in the node's own chain, signed by
//! the node key, and a revocation withdraws it for good. Carrier-free: keys
//! are raw 32-byte Ed25519 keys and nothing here names iroh. The design is
//! `glade/dev-docs/GladeNodeAssembly.md`, "Transport binding and the door
//! (plan Step 4.2)".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::sync::{Mutex, PoisonError};

use glade_signer_api::SignatureStatus;
use glade_wire::cbor::{self, Cbor};
use glade_wire::generated::Op;

use crate::registry::{Record, Registry, G_TRANSPORT_BINDINGS, G_TRANSPORT_REVOCATIONS, HOME};
use crate::signing::{self, TRANSPORT_BINDING, TRANSPORT_REVOCATION};
use crate::store::Store;
use crate::sysdata::{NodeTransportBinding, NodeTransportRevocation};
use crate::sysdir::now_ms;

/// A node's iroh endpoint key (the signing note's F1): `endpoint.key`, a
/// second class-1 secret beside `node.key` and not derived from it, so the
/// node's identity survives the key's replacement. Its Ed25519 public key is
/// the endpoint id that peers dial and bindings name. `Debug` prints only
/// the id.
#[derive(Clone, Copy)]
pub struct EndpointKey {
    seed: [u8; 32],
    pub endpoint_id: [u8; 32],
}

impl EndpointKey {
    pub fn from_seed(seed: [u8; 32]) -> EndpointKey {
        let endpoint_id = signing::public_key(&seed);
        EndpointKey { seed, endpoint_id }
    }

    /// The seed, for the carrier that binds with it.
    pub(crate) fn seed(&self) -> [u8; 32] {
        self.seed
    }
}

impl fmt::Debug for EndpointKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = hex(&self.endpoint_id);
        f.debug_struct("EndpointKey")
            .field("endpoint_id", &id)
            .finish_non_exhaustive()
    }
}

/// A key as the directory writes it: 64 lower-case hex digits.
pub fn hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// The key `text` writes, if it is exactly 64 lower-case hex digits.
fn key_of(text: &str) -> Option<[u8; 32]> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let text = text.as_bytes();
    if text.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (byte, pair) in key.iter_mut().zip(text.chunks(2)) {
        *byte = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Some(key)
}

/// What a record's signature covers: the canonical CBOR of its fields before
/// `sig`, which are `node`, `endpoint_id` and, for a binding, `valid_from`.
fn message(node: &str, endpoint_id: &str, valid_from: Option<i64>) -> Vec<u8> {
    let mut fields = vec![
        (1, Cbor::Text(node.into())),
        (2, Cbor::Text(endpoint_id.into())),
    ];
    fields.extend(valid_from.map(|at| (3, Cbor::Int(at))));
    cbor::encode(&Cbor::Map(fields))
}

/// The binding of `endpoint` to the node whose key is `seed`, from
/// `valid_from`, signed by that key.
pub fn sign_binding(seed: &[u8; 32], endpoint: &[u8; 32], valid_from: i64) -> NodeTransportBinding {
    let (node, endpoint_id) = (hex(&signing::public_key(seed)), hex(endpoint));
    let signed = message(&node, &endpoint_id, Some(valid_from));
    let sig = signing::sign_in(seed, TRANSPORT_BINDING, &signed).to_vec();
    NodeTransportBinding {
        node,
        endpoint_id,
        valid_from,
        sig,
    }
}

/// The revocation of `endpoint` by the node whose key is `seed`.
pub fn sign_revocation(seed: &[u8; 32], endpoint: &[u8; 32]) -> NodeTransportRevocation {
    let (node, endpoint_id) = (hex(&signing::public_key(seed)), hex(endpoint));
    let signed = message(&node, &endpoint_id, None);
    let sig = signing::sign_in(seed, TRANSPORT_REVOCATION, &signed).to_vec();
    NodeTransportRevocation {
        node,
        endpoint_id,
        sig,
    }
}

/// A binding's pair, `(endpoint key, node key)`.
type Pair = ([u8; 32], [u8; 32]);

/// Where a pair (node, endpoint key) stands in the fold, at a reader's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// A binding names the pair, dated at or before the clock, and no
    /// revocation does.
    Live,
    /// Every binding of the pair is dated after the reader's clock.
    NotYet { valid_from: i64 },
    /// The node has revoked the pair: for good, whatever the clock.
    Revoked,
    /// No binding names the pair.
    Unbound,
    /// The reader's clock cannot be read (0 or less): nothing is live.
    ClockUncertain,
}

/// The transport-binding fold over an op-set: the registry's, or the served
/// store's `home` share. Set union, a revocation of a pair winning over every
/// binding of it; a pure function of the op-set, never of arrival order.
#[derive(Debug, Default)]
pub struct TransportFold {
    /// Each bound pair, with the earliest `valid_from` among its bindings.
    bindings: BTreeMap<Pair, i64>,
    revoked: BTreeSet<Pair>,
    /// Records on either stream that prove nothing, so count for nothing.
    pub ignored: usize,
}

impl TransportFold {
    /// Fold the transport records out of `ops`; other streams are skipped. A
    /// record that does not prove itself binds or revokes nothing, and is
    /// counted: only the node may withdraw its key.
    pub fn over<'a>(ops: impl IntoIterator<Item = &'a Op>) -> TransportFold {
        let mut fold = TransportFold::default();
        for op in ops {
            fold.note(op);
        }
        fold
    }

    /// Fold in one op, if it is on a transport stream: the pair it revoked,
    /// if it is a counted revocation new to the fold.
    fn note(&mut self, op: &Op) -> Option<Pair> {
        if ![G_TRANSPORT_BINDINGS, G_TRANSPORT_REVOCATIONS].contains(&op.glade_id.as_str()) {
            return None;
        }
        match proven(op) {
            Some((pair, Some(valid_from))) => {
                let earliest = self.bindings.entry(pair).or_insert(valid_from);
                *earliest = (*earliest).min(valid_from);
                None
            }
            Some((pair, None)) => self.revoked.insert(pair).then_some(pair),
            None => {
                self.ignored += 1;
                None
            }
        }
    }

    /// The fold of the served store's `home` share, every origin's records.
    pub fn of_store(store: &Store) -> TransportFold {
        let mut ops = Vec::new();
        for glade_id in [G_TRANSPORT_BINDINGS, G_TRANSPORT_REVOCATIONS] {
            for (origin, _) in store.heads(HOME, glade_id, &[]) {
                ops.extend(store.scan(HOME, glade_id, &[], &origin, i64::MIN));
            }
        }
        TransportFold::over(&ops)
    }

    /// Where `node`'s binding of `endpoint` stands at the reader's clock
    /// `now_ms`, judged when read: a revocation first, whatever the clock;
    /// then nothing is live while the clock cannot be read.
    pub fn binds(&self, node: &[u8; 32], endpoint: &[u8; 32], now_ms: i64) -> Bound {
        let pair = (*endpoint, *node);
        if self.revoked.contains(&pair) {
            return Bound::Revoked;
        }
        if now_ms <= 0 {
            return Bound::ClockUncertain;
        }
        match self.bindings.get(&pair) {
            Some(&valid_from) if valid_from <= now_ms => Bound::Live,
            Some(&valid_from) => Bound::NotYet { valid_from },
            None => Bound::Unbound,
        }
    }

    /// The door's rule (plan Step 4.2b) for endpoint key `endpoint` at
    /// `now_ms`: at accept, `node` being `None`, or for the HELLO of `node`.
    /// A live binding of the key admits it, at HELLO only for its own node;
    /// a key no record names is admitted only if `configured`, on first
    /// contact; nothing is admitted while the clock cannot be read.
    pub fn door(
        &self,
        endpoint: &[u8; 32],
        node: Option<&[u8; 32]>,
        configured: bool,
        now_ms: i64,
    ) -> Result<(), Refusal> {
        if now_ms <= 0 {
            return Err(Refusal::ClockUncertain);
        }
        let named = self.bindings.keys().chain(&self.revoked);
        let nodes: BTreeSet<[u8; 32]> = named
            .filter(|(e, _)| e == endpoint)
            .map(|(_, n)| *n)
            .collect();
        let Some(first) = nodes.first() else {
            return if configured {
                Ok(())
            } else {
                Err(Refusal::Unknown)
            };
        };
        let state = |n: &[u8; 32]| self.binds(n, endpoint, now_ms);
        let why = |n: &[u8; 32]| match state(n) {
            Bound::Revoked => Refusal::Revoked { node: *n },
            Bound::NotYet { valid_from } => Refusal::NotYet { valid_from },
            _ => Refusal::Unknown,
        };
        let live = nodes.iter().find(|n| state(n) == Bound::Live);
        match (node, live) {
            (None, Some(_)) => Ok(()),
            (Some(n), _) if state(n) == Bound::Live => Ok(()),
            (Some(n), _) if nodes.contains(n) => Err(why(n)),
            (Some(n), Some(m)) => Err(Refusal::BoundElsewhere {
                bound: *m,
                node: *n,
            }),
            _ => Err(why(first)),
        }
    }

    /// The endpoint keys `node` has bound and not revoked, whatever their
    /// dates.
    pub fn endpoints_of(&self, node: &[u8; 32]) -> Vec<[u8; 32]> {
        let bound = self.bindings.keys().filter(|(_, n)| n == node);
        let live = bound.filter(|pair| !self.revoked.contains(pair));
        live.map(|(endpoint, _)| *endpoint).collect()
    }
}

/// Why the door refused an endpoint key at accept, or a HELLO through it
/// (plan Step 4.2b). Its text is the refusal line's reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The reader's clock cannot be read.
    ClockUncertain,
    /// No record names the key, and the operator did not configure it.
    Unknown,
    /// The key's node revoked it.
    Revoked { node: [u8; 32] },
    /// The key is bound only from a date after the reader's clock.
    NotYet { valid_from: i64 },
    /// The key is bound to another node than the one that spoke.
    BoundElsewhere { bound: [u8; 32], node: [u8; 32] },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::ClockUncertain => write!(f, "clock uncertain"),
            Refusal::Unknown => write!(f, "unknown endpoint key"),
            Refusal::Revoked { node } => write!(f, "revoked by node {}", hex(node)),
            Refusal::NotYet { valid_from } => write!(f, "bound from {valid_from}"),
            Refusal::BoundElsewhere { bound, node } => {
                write!(f, "bound to node {}, not {}", hex(bound), hex(node))
            }
        }
    }
}

/// The door (plan Step 4.2b): a booted node's check of an endpoint key, at
/// accept and at HELLO, against the transport records it holds, which are
/// its served store's `home` share with every peer's records, and the keys
/// its operator configured (`--peer`). The endpoint's accept hook and the
/// mesh share it; the mesh feeds it each record that lands.
pub struct Door {
    configured: BTreeSet<[u8; 32]>,
    fold: Mutex<TransportFold>,
    report: Box<dyn Fn(&str) + Send + Sync>,
}

impl fmt::Debug for Door {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let configured = self.configured.len();
        f.debug_struct("Door")
            .field("configured", &configured)
            .finish_non_exhaustive()
    }
}

impl Door {
    /// A door that admits `configured` keys on first contact, and reports
    /// each refusal to `report`: a stderr line, for the node.
    pub fn new(
        configured: impl IntoIterator<Item = [u8; 32]>,
        report: impl Fn(&str) + Send + Sync + 'static,
    ) -> Door {
        let (configured, report) = (configured.into_iter().collect(), Box::new(report));
        let fold = Mutex::new(TransportFold::default());
        Door {
            configured,
            fold,
            report,
        }
    }

    fn fold(&self) -> std::sync::MutexGuard<'_, TransportFold> {
        self.fold.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take in what the served store holds, before the first connection.
    pub fn load(&self, store: &Store) {
        *self.fold() = TransportFold::of_store(store);
    }

    /// Take in `op`, landed in the served store: if it is a counted
    /// revocation new to the door, the pair it revoked, `(endpoint key,
    /// node)`, whose live link must now close.
    pub fn note(&self, op: &Op) -> Option<([u8; 32], [u8; 32])> {
        self.fold().note(op)
    }

    /// At accept: whether the key `endpoint` may connect, by the clock now.
    pub fn admits(&self, endpoint: &[u8; 32]) -> Result<(), Refusal> {
        let configured = self.configured.contains(endpoint);
        self.fold().door(endpoint, None, configured, now_ms())
    }

    /// At HELLO: whether `node` may speak through the key `endpoint`, now.
    pub fn binds(&self, node: &[u8; 32], endpoint: &[u8; 32]) -> Result<(), Refusal> {
        let configured = self.configured.contains(endpoint);
        self.fold().door(endpoint, Some(node), configured, now_ms())
    }

    /// Report a refusal of the key `endpoint`, with its reason.
    pub fn refused(&self, endpoint: &[u8; 32], why: &dyn fmt::Display) {
        (self.report)(&format!("peer refused: endpoint {}: {why}", hex(endpoint)));
    }
}

/// What a boot did to its node's bindings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rebound {
    /// A binding of the current endpoint key was appended.
    pub minted: bool,
    /// How many bindings of replaced keys were revoked.
    pub revoked: usize,
}

impl Rebound {
    /// The line both roots print after `node` when replaced keys' bindings
    /// were revoked.
    pub fn line(&self) -> Option<String> {
        let revoked = self.revoked;
        (revoked > 0).then(|| format!("revoked {revoked} binding(s) of replaced endpoint key(s)"))
    }
}

/// At boot (plan Step 4.2): bind `endpoint` to the node whose key is `seed`,
/// unless a binding by it already names the key, dated at `now_ms`, which
/// must be readable; and revoke every other key the node has bound. Refused
/// (`InvalidData`), with nothing appended, if the node has revoked this key.
pub(crate) fn bind_at_boot(
    registry: &mut Registry,
    seed: &[u8; 32],
    endpoint: &EndpointKey,
    now_ms: i64,
) -> io::Result<Rebound> {
    let (fold, node) = (registry.transport(), signing::public_key(seed));
    let current = endpoint.endpoint_id;
    let refused = |why: String| io::Error::new(io::ErrorKind::InvalidData, why);
    if fold.binds(&node, &current, now_ms) == Bound::Revoked {
        let why = "endpoint.key is revoked for this node: move it aside, and the next boot mints a new key";
        return Err(refused(why.into()));
    }
    let minted = !fold.bindings.contains_key(&(current, node)) && now_ms > 0;
    let binding = minted.then(|| Record::Transport(sign_binding(seed, &current, now_ms)));
    let replaced: Vec<[u8; 32]> = fold
        .endpoints_of(&node)
        .into_iter()
        .filter(|key| *key != current)
        .collect();
    let revocations = replaced
        .iter()
        .map(|key| Record::TransportRevoke(sign_revocation(seed, key)));
    for record in binding.into_iter().chain(revocations) {
        let appended = registry.append_returning(record, &hex(&node));
        appended.map_err(|e| refused(format!("registry append rejected: {e:?}")))?;
    }
    let revoked = replaced.len();
    Ok(Rebound { minted, revoked })
}

/// The pair a transport record proves, and a binding's date (`None` for a
/// revocation): only if its payload is its kind's canonical encoding, its
/// ids are 64 lower-case hex digits, its signature verifies strictly under
/// its node for its kind's domain, and it rides `home` in that node's own
/// chain.
fn proven(op: &Op) -> Option<(Pair, Option<i64>)> {
    let fields = flat_map(&op.payload)?;
    let kind = (op.glade_id.as_str(), fields.as_slice());
    let (node, endpoint_id, valid_from, sig, domain) = match kind {
        (
            G_TRANSPORT_BINDINGS,
            [(1, Cbor::Text(node)), (2, Cbor::Text(endpoint_id)), (3, Cbor::Int(at)), (4, Cbor::Bytes(sig))],
        ) => (node, endpoint_id, Some(*at), sig, TRANSPORT_BINDING),
        (
            G_TRANSPORT_REVOCATIONS,
            [(1, Cbor::Text(node)), (2, Cbor::Text(endpoint_id)), (3, Cbor::Bytes(sig))],
        ) => (node, endpoint_id, None, sig, TRANSPORT_REVOCATION),
        _ => return None,
    };
    let canonical = cbor::encode(&Cbor::Map(fields.clone())) == op.payload;
    let pair = (key_of(endpoint_id)?, key_of(node)?);
    let signed = message(node, endpoint_id, valid_from);
    let valid = signing::verify_in(&pair.1, domain, &signed, sig) == SignatureStatus::Valid;
    let chain = op.share == HOME && op.origin == *node;
    (canonical && valid && chain).then_some((pair, valid_from))
}

/// The payload's flat CBOR map of integer keys to ints, byte strings and
/// text, or `None`. The wire codec's `decode` panics on bytes it cannot read,
/// and until plan Step 4.1b any peer can write `home` (F2).
fn flat_map(bytes: &[u8]) -> Option<Vec<(i64, Cbor)>> {
    let mut at = 0;
    let (5, len) = head(bytes, &mut at)? else {
        return None;
    };
    let mut entries = Vec::new();
    for _ in 0..len {
        let ((0, key), (major, n)) = (head(bytes, &mut at)?, head(bytes, &mut at)?) else {
            return None;
        };
        let value = match major {
            0 => Cbor::Int(i64::try_from(n).ok()?),
            1 => Cbor::Int(-1 - i64::try_from(n).ok()?),
            2 | 3 => {
                let end = at.checked_add(usize::try_from(n).ok()?)?;
                let raw = bytes.get(at..end)?.to_vec();
                at = end;
                if major == 2 {
                    Cbor::Bytes(raw)
                } else {
                    Cbor::Text(String::from_utf8(raw).ok()?)
                }
            }
            _ => return None,
        };
        entries.push((i64::try_from(key).ok()?, value));
    }
    (at == bytes.len()).then_some(entries)
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

// Records and doors for other modules' tests. A braced module, so the
// condition encloses the whole section.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use glade_wire::generated::Shape;

    /// `record` as the `seq`th op of the chain of the node whose key is
    /// `seed`, on its stream in `home`. The fold reads no chain link.
    pub(crate) fn op_of(seed: &[u8; 32], record: Record, seq: i64) -> Op {
        let (glade_id, payload) = (record.glade_id().into(), record.encode());
        let origin = hex(&signing::public_key(seed));
        let (share, shape) = (HOME.into(), Shape::Log);
        Op {
            share,
            glade_id,
            origin,
            seq,
            lamport: seq,
            shape,
            payload,
            ..Op::default()
        }
    }

    /// A door that configures nothing and holds one binding: of `endpoint`
    /// to the node whose key is `seed`, from epoch millisecond 1.
    pub(crate) fn bound_by(seed: &[u8; 32], endpoint: &[u8; 32]) -> Door {
        let door = Door::new([], |_: &str| {});
        door.note(&op_of(
            seed,
            Record::Transport(sign_binding(seed, endpoint, 1)),
            0,
        ));
        door
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryApi;
    use glade_wire::generated::Shape;

    const NODE: [u8; 32] = [7; 32];
    const OTHER: [u8; 32] = [8; 32];
    const E1: [u8; 32] = [1; 32];
    const E2: [u8; 32] = [2; 32];

    fn id(seed: &[u8; 32]) -> [u8; 32] {
        signing::public_key(seed)
    }

    /// `record` as the `seq`th op of `origin`'s chain on its stream. The fold
    /// reads no chain link, so `prev` is left out.
    fn op(record: Record, origin: &str, seq: i64) -> Op {
        Op {
            share: HOME.into(),
            glade_id: record.glade_id().into(),
            key: vec![],
            origin: origin.into(),
            seq,
            prev: None,
            lamport: seq,
            refs: vec![],
            shape: Shape::Log,
            payload: record.encode(),
        }
    }

    fn bind(seed: &[u8; 32], endpoint: &[u8; 32], valid_from: i64, seq: i64) -> Op {
        let record = Record::Transport(sign_binding(seed, endpoint, valid_from));
        op(record, &hex(&id(seed)), seq)
    }

    fn revoke(seed: &[u8; 32], endpoint: &[u8; 32], seq: i64) -> Op {
        let record = Record::TransportRevoke(sign_revocation(seed, endpoint));
        op(record, &hex(&id(seed)), seq)
    }

    /// Bytes written out by hand, part by part.
    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    /// A 64-character text item, as canonical CBOR writes one.
    fn text(t: &str) -> Vec<u8> {
        cat(&[&[0x78, 0x40], t.as_bytes()])
    }

    /// The layout of section 2, checked against bytes built here by hand: a
    /// binding's `sig` is pure Ed25519 by the node key over the tag
    /// `glade/v1/transport-binding\0` then the canonical CBOR of fields 1-3,
    /// and verifies in no other domain; a revocation's likewise over fields
    /// 1-2 in its own. Written with the records: it pins them, so it has no
    /// red form. It does not show another language agrees.
    #[test]
    fn each_record_is_signed_in_its_own_domain_over_its_own_fields() {
        use ed25519_dalek::{Signature, SigningKey};
        let key = SigningKey::from_bytes(&NODE).verifying_key();
        let (node, endpoint) = (hex(&id(&NODE)), hex(&E1));
        let valid_from: i64 = 1_790_208_000_000;
        let fields = cat(&[
            &[0xa3, 0x01],
            &text(&node),
            &[0x02],
            &text(&endpoint),
            &[0x03, 0x1b],
            &valid_from.to_be_bytes(),
        ]);
        let binding = sign_binding(&NODE, &E1, valid_from);
        assert_eq!((&binding.node, &binding.endpoint_id), (&node, &endpoint));
        let sig = Signature::from_slice(&binding.sig).unwrap();
        let tagged = |tag: &[u8]| cat(&[tag, &fields]);
        assert!(key
            .verify_strict(&tagged(b"glade/v1/transport-binding\0"), &sig)
            .is_ok());
        let others: [&[u8]; 4] = [
            b"glade/v1/transport-revocation\0",
            b"glade/v1/origin-op\0",
            b"glade/v1/peer-hello\0",
            b"",
        ];
        for other in others {
            assert!(
                key.verify_strict(&tagged(other), &sig).is_err(),
                "{other:?}"
            );
        }
        let pair = cat(&[&[0xa2, 0x01], &text(&node), &[0x02], &text(&endpoint)]);
        let revocation = sign_revocation(&NODE, &E1);
        let sig = Signature::from_slice(&revocation.sig).unwrap();
        let revoked = cat(&[b"glade/v1/transport-revocation\0", &pair]);
        assert!(key.verify_strict(&revoked, &sig).is_ok());
        let as_binding = cat(&[b"glade/v1/transport-binding\0", &pair]);
        assert!(key.verify_strict(&as_binding, &sig).is_err());
    }

    /// Section 3: set union, the earliest date of a pair's bindings counting;
    /// a revocation clears every binding of its pair, earlier and later, and
    /// no other pair, another node's binding of the same key included; the
    /// same answers from the ops in reverse order.
    #[test]
    fn the_fold_is_a_set_union_and_a_revocation_wins_for_good() {
        let ops = vec![
            bind(&NODE, &E1, 2_000, 0),
            bind(&NODE, &E1, 1_000, 1),
            bind(&NODE, &E2, 1_000, 2),
            revoke(&NODE, &E2, 0),
            bind(&NODE, &E2, 500, 3),
            bind(&OTHER, &E2, 1_000, 0),
        ];
        let fold = TransportFold::over(&ops);
        let (node, other) = (id(&NODE), id(&OTHER));
        assert_eq!(
            fold.binds(&node, &E1, 1_500),
            Bound::Live,
            "the earliest date"
        );
        assert_eq!(fold.binds(&node, &E2, 9_000), Bound::Revoked);
        assert_eq!(fold.binds(&other, &E2, 9_000), Bound::Live, "another pair");
        assert_eq!(fold.binds(&other, &E1, 9_000), Bound::Unbound);
        assert_eq!(fold.endpoints_of(&node), [E1]);
        assert_eq!(fold.ignored, 0);
        let mut reversed = ops.clone();
        reversed.reverse();
        let back = TransportFold::over(&reversed);
        for (n, e) in [(node, E1), (node, E2), (other, E1), (other, E2)] {
            assert_eq!(back.binds(&n, &e, 1_500), fold.binds(&n, &e, 1_500));
        }
    }

    /// Section 4: `valid_from` is judged at the reader's clock, with no
    /// margin; a clock that cannot be read (0 or less) makes nothing live,
    /// and a revocation holds whatever the clock.
    #[test]
    fn a_binding_is_live_from_its_valid_from_at_the_readers_clock() {
        let ops = [
            bind(&NODE, &E1, 1_000, 0),
            bind(&NODE, &E2, 1, 1),
            revoke(&NODE, &E2, 0),
        ];
        let fold = TransportFold::over(&ops);
        let node = id(&NODE);
        let at = |now| fold.binds(&node, &E1, now);
        assert_eq!(at(999), Bound::NotYet { valid_from: 1_000 });
        assert_eq!((at(1_000), at(1_001)), (Bound::Live, Bound::Live));
        assert_eq!(
            (at(0), at(-5)),
            (Bound::ClockUncertain, Bound::ClockUncertain)
        );
        assert_eq!(fold.binds(&node, &E2, 0), Bound::Revoked);
    }

    /// Section 3's first rule: a record that does not prove itself binds or
    /// revokes nothing, and is counted, and nothing panics. A flipped
    /// signature byte, another key's signature, another origin's chain, a
    /// payload that is not canonical, an id in upper case, a torn payload,
    /// bytes that are not CBOR, a revocation on the bindings stream, and a
    /// revocation of the pair signed by another node, which leaves the
    /// genuine binding live.
    #[test]
    fn a_record_that_does_not_prove_itself_binds_nothing() {
        let genuine = sign_binding(&NODE, &E1, 1_000);
        let own = hex(&id(&NODE));
        let on = |payload: Vec<u8>, origin: &str| Op {
            payload,
            ..op(Record::Transport(genuine.clone()), origin, 0)
        };
        let mut flipped = genuine.clone();
        flipped.sig[3] ^= 1;
        let mut foreign = sign_binding(&OTHER, &E1, 1_000);
        foreign.node = own.clone();
        let loud = own.to_uppercase();
        let signed = message(&loud, &hex(&E1), Some(1_000));
        let upper = NodeTransportBinding {
            node: loud.clone(),
            sig: signing::sign_in(&NODE, TRANSPORT_BINDING, &signed).to_vec(),
            ..genuine.clone()
        };
        let wide = cat(&[
            &[0xa4, 0x01],
            &text(&own),
            &[0x02],
            &text(&hex(&E1)),
            &[0x03, 0x1b, 0, 0, 0, 0, 0, 0, 0x03, 0xe8, 0x04, 0x58, 0x40],
            &genuine.sig,
        ]);
        let bytes = Record::Transport(genuine.clone()).encode();
        let mut not_ours = sign_revocation(&OTHER, &E1);
        not_ours.node = own.clone();
        let revocation = Record::TransportRevoke(sign_revocation(&NODE, &E1)).encode();
        let bad = vec![
            op(Record::Transport(flipped), &own, 1),
            op(Record::Transport(foreign), &own, 2),
            op(Record::Transport(genuine.clone()), &hex(&id(&OTHER)), 0),
            on(wide, &own),
            op(Record::Transport(upper), &loud, 0),
            on(bytes[..bytes.len() - 1].to_vec(), &own),
            on(vec![0xff, 0x00], &own),
            on(revocation, &own),
            op(Record::TransportRevoke(not_ours), &own, 0),
        ];
        let alone = TransportFold::over(&bad);
        assert_eq!(alone.binds(&id(&NODE), &E1, 2_000), Bound::Unbound);
        assert_eq!(alone.ignored, bad.len());
        let mut all = bad.clone();
        all.push(op(Record::Transport(genuine), &own, 0));
        let fold = TransportFold::over(&all);
        assert_eq!(fold.binds(&id(&NODE), &E1, 2_000), Bound::Live);
        assert_eq!(fold.ignored, bad.len());
    }

    /// Plan Step 4.2b's rule, row by row, at one clock: a live binding
    /// admits, at HELLO only its own node; a revoked or not-yet-dated key is
    /// refused, configured or not; a key no record names is admitted only if
    /// configured; nothing is admitted while the clock cannot be read.
    #[test]
    fn the_door_admits_a_live_or_first_configured_key_and_refuses_the_rest() {
        const E3: [u8; 32] = [3; 32];
        const E4: [u8; 32] = [4; 32];
        let ops = [
            bind(&NODE, &E1, 1_000, 0),
            bind(&NODE, &E2, 1_000, 1),
            revoke(&NODE, &E2, 0),
            bind(&NODE, &E3, 5_000, 2),
        ];
        let fold = TransportFold::over(&ops);
        let (node, other) = (id(&NODE), id(&OTHER));
        let door = |e: &[u8; 32], hello: Option<&[u8; 32]>, configured| {
            fold.door(e, hello, configured, 2_000)
        };
        assert_eq!(door(&E1, None, false), Ok(()), "a live binding");
        assert_eq!(door(&E1, Some(&node), false), Ok(()), "its own node");
        let elsewhere = Refusal::BoundElsewhere {
            bound: node,
            node: other,
        };
        assert_eq!(door(&E1, Some(&other), false), Err(elsewhere));
        let revoked = Err(Refusal::Revoked { node });
        assert_eq!(door(&E2, None, true), revoked, "configured, and revoked");
        assert_eq!(door(&E2, Some(&node), true), revoked);
        let not_yet = Err(Refusal::NotYet { valid_from: 5_000 });
        assert_eq!(
            (door(&E3, None, false), door(&E3, Some(&node), true)),
            (not_yet.clone(), not_yet)
        );
        assert_eq!(
            door(&E4, None, true),
            Ok(()),
            "first contact on a configured key"
        );
        assert_eq!(door(&E4, Some(&other), true), Ok(()));
        let unknown = Err(Refusal::Unknown);
        assert_eq!(
            (door(&E4, None, false), door(&E4, Some(&node), false)),
            (unknown.clone(), unknown)
        );
        let unread = Err(Refusal::ClockUncertain);
        assert_eq!(fold.door(&E1, None, false, 0), unread);
        assert_eq!(fold.door(&E4, Some(&other), true, 0), unread);
    }

    /// Plan Step 4.2b: a door takes each record as it lands, answering a
    /// counted revocation with the key it revoked, and judges at the clock
    /// now; it reports a refusal as one line naming the key and the reason.
    #[test]
    fn a_door_takes_records_as_they_land_and_reports_its_refusals() {
        let lines = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        let door = Door::new([E2], move |line: &str| {
            sink.lock().unwrap().push(line.to_string())
        });
        assert_eq!(door.admits(&E1), Err(Refusal::Unknown));
        assert_eq!(door.admits(&E2), Ok(()), "configured");
        assert_eq!(door.note(&bind(&NODE, &E1, 1, 0)), None);
        assert_eq!(door.binds(&id(&NODE), &E1), Ok(()));
        let lands = door.note(&revoke(&NODE, &E1, 0));
        assert_eq!(lands, Some((E1, id(&NODE))), "a revocation lands");
        let revoked = Refusal::Revoked { node: id(&NODE) };
        assert_eq!(door.admits(&E1), Err(revoked.clone()));
        door.refused(&E1, &revoked);
        let line = format!(
            "peer refused: endpoint {}: revoked by node {}",
            hex(&E1),
            hex(&id(&NODE))
        );
        assert_eq!(*lines.lock().unwrap(), [line]);
    }

    fn rebound(minted: bool, revoked: usize) -> Rebound {
        Rebound { minted, revoked }
    }

    /// Two endpoint keys, as two `endpoint.key` files would hold them.
    fn keys() -> (EndpointKey, EndpointKey) {
        (
            EndpointKey::from_seed([11; 32]),
            EndpointKey::from_seed([12; 32]),
        )
    }

    /// Section 5, over a registry: the first boot binds the key; the next
    /// binds nothing more; a boot with another key binds it and revokes the
    /// replaced one; a boot with the revoked key is refused and appends
    /// nothing.
    #[test]
    fn a_boot_binds_its_key_once_and_revokes_a_replaced_one() {
        let (mut registry, (k1, k2)) = (Registry::new(), keys());
        let first = bind_at_boot(&mut registry, &NODE, &k1, 1_000).unwrap();
        assert_eq!(first, rebound(true, 0));
        let again = bind_at_boot(&mut registry, &NODE, &k1, 2_000).unwrap();
        assert_eq!(again, rebound(false, 0), "bound once");
        let replaced = bind_at_boot(&mut registry, &NODE, &k2, 3_000).unwrap();
        assert_eq!(replaced, rebound(true, 1));
        let (fold, node) = (registry.transport(), id(&NODE));
        assert_eq!(fold.binds(&node, &k1.endpoint_id, 4_000), Bound::Revoked);
        assert_eq!(fold.binds(&node, &k2.endpoint_id, 4_000), Bound::Live);
        let held = registry.snapshot();
        let refused = bind_at_boot(&mut registry, &NODE, &k1, 5_000).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
        assert_eq!(registry.snapshot(), held, "nothing appended");
    }

    /// Section 4 at boot: a clock that cannot be read mints no binding, and
    /// a revocation, which needs no clock, is still minted.
    #[test]
    fn a_boot_on_an_unreadable_clock_binds_nothing_and_still_revokes() {
        let (mut registry, (k1, k2)) = (Registry::new(), keys());
        bind_at_boot(&mut registry, &NODE, &k1, 1_000).unwrap();
        let unread = bind_at_boot(&mut registry, &NODE, &k2, 0).unwrap();
        assert_eq!(unread, rebound(false, 1));
        let (fold, node) = (registry.transport(), id(&NODE));
        assert_eq!(fold.binds(&node, &k2.endpoint_id, 5_000), Bound::Unbound);
        assert_eq!(fold.binds(&node, &k1.endpoint_id, 5_000), Bound::Revoked);
    }
}
