//! Node<->node session (Lane R step 2): the HELLO seam + heads/gap sync.
//!
//! Carrier-free by construction — everything here runs over any `AsyncRead +
//! AsyncWrite` pair, so the protocol is unit-tested over an in-memory duplex and
//! rides real iroh QUIC (`iroh_carrier.rs`) unchanged. Two layers:
//!
//!   1. **Framed IO** — a `u32`-length prefix around each `Frame` (the exact
//!      framing the WS carrier uses, minus the websocket).
//!   2. **HELLO seam** — peers exchange node identities, each signed for the
//!      connection it rides (plan Step 4.1a): the node id is the node key's
//!      Ed25519 public key, and the signature covers the TLS session, both
//!      endpoint ids and the role. It proves who is on the link; sync integrity
//!      still comes from the origin chains, never from the carrier.
//!
//! The heads/gap sync driver (per-(origin, zone) chains, verify-as-ingest,
//! reject-suffix + re-fetch, equivocation proof) lives in the second half.

use std::collections::BTreeMap;
use std::fmt;
use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use glade_signer_api::{Purpose, SignatureStatus};
use glade_wire::cbor::{self, Cbor};
use glade_wire::generated::{Heads, NodeHello, NodeWelcome, Op, Ops, Priority};

use crate::frame::Frame;
use crate::session::missing_for;
use crate::signing;
use crate::store::{EquivProof, Store, StoreError};
use crate::transport::Door;

/// Wire protocol version spoken on the peer link: 2 from plan Step 4.1a, whose
/// HELLO is signed. The carrier's ALPN names it too, so a node that speaks 1
/// fails at connect.
pub const PROTOCOL: i64 = 2;

// ---- framed IO ------------------------------------------------------------

/// Write one frame, length-prefixed (`u32` LE) then flushed.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &Frame) -> io::Result<()> {
    let bytes = frame.to_bytes();
    w.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

/// Read one length-prefixed frame. A clean stream close at a frame boundary
/// surfaces as `UnexpectedEof` — the sync loop reads that as "peer done".
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let n = u32::from_le_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Frame::from_bytes(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ---- node identity + HELLO seam ------------------------------------------

/// A node's identity (plan Step 4.1a; `GladeNodeSigning.md` D2): `node.key` is
/// a 32-byte Ed25519 seed, and `node_id` is its public key, so a verifier
/// needs no lookup. The seed never leaves this value: `Debug` prints the id.
#[derive(Clone, Copy)]
pub struct NodeIdentity {
    seed: [u8; 32],
    pub node_id: [u8; 32],
}

impl NodeIdentity {
    /// The identity a 32-byte seed signs as: `node_id` is its public key.
    pub fn from_key(seed: [u8; 32]) -> Self {
        let node_id = signing::public_key(&seed);
        NodeIdentity { seed, node_id }
    }

    /// A fresh identity from the operating system's randomness, for an
    /// endpoint bound without an instance (`PeerEndpoint::bind`).
    pub fn generate() -> io::Result<Self> {
        signing::random_seed().map(NodeIdentity::from_key)
    }

    /// This node's signature on `message`, for `purpose`.
    pub(crate) fn sign(&self, purpose: Purpose, message: &[u8]) -> Vec<u8> {
        signing::sign(&self.seed, purpose, message).to_vec()
    }
}

impl fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id: String = self.node_id.iter().map(|b| format!("{b:02x}")).collect();
        f.debug_struct("NodeIdentity")
            .field("node_id", &id)
            .finish_non_exhaustive()
    }
}

/// The end of a connection a HELLO speaks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Dialer,
    Acceptor,
}

/// What both ends of one connection know without sending it (D6): the two
/// iroh endpoint ids, and 32 bytes exported from the connection's TLS session
/// (`iroh_carrier.rs`). Another connection has other bytes, so a HELLO signed
/// for one does not verify on another. The in-memory tests fix one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    pub dialer: [u8; 32],
    pub acceptor: [u8; 32],
    pub exported: [u8; 32],
}

/// What a HELLO signs (D6), as canonical CBOR: `{1: protocol, 2: role,
/// 3: node id, 4: dialer endpoint id, 5: acceptor endpoint id, 6: exported
/// bytes}`, the role being `"dialer"` or `"acceptor"`.
fn transcript(role: Role, node_id: &[u8; 32], channel: &Channel) -> Vec<u8> {
    let role = match role {
        Role::Dialer => "dialer",
        Role::Acceptor => "acceptor",
    };
    cbor::encode(&Cbor::Map(vec![
        (1, Cbor::Int(PROTOCOL)),
        (2, Cbor::Text(role.into())),
        (3, Cbor::Bytes(node_id.to_vec())),
        (4, Cbor::Bytes(channel.dialer.to_vec())),
        (5, Cbor::Bytes(channel.acceptor.to_vec())),
        (6, Cbor::Bytes(channel.exported.to_vec())),
    ]))
}

/// This node's signature on the HELLO it sends as `role` on `channel`.
fn hello_sig(me: &NodeIdentity, role: Role, channel: &Channel) -> Option<Vec<u8>> {
    Some(me.sign(Purpose::PeerHello, &transcript(role, &me.node_id, channel)))
}

/// The verified peer: a node that proved on this connection that it holds the
/// key of the id it named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeerHello {
    pub peer_id: [u8; 32],
}

fn peer_id_of(node_id: &[u8]) -> io::Result<[u8; 32]> {
    node_id
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "peer node_id not 32 bytes"))
}

/// Check the HELLO the other end sent as `role` on `channel` (D6): protocol 2,
/// and a signature that verifies under the id it names for the transcript
/// this end computes. The id is the key, so first contact needs no lookup.
/// Anything else is refused (`PermissionDenied`).
fn verify_peer(
    node_id: &[u8],
    protocol: i64,
    sig: &Option<Vec<u8>>,
    role: Role,
    channel: &Channel,
) -> io::Result<PeerHello> {
    let peer_id = peer_id_of(node_id)?;
    let refused = |why: String| {
        let why = format!("HELLO refused: {why}");
        io::Error::new(io::ErrorKind::PermissionDenied, why)
    };
    if protocol != PROTOCOL {
        return Err(refused(format!("protocol {protocol}, not {PROTOCOL}")));
    }
    let Some(sig) = sig else {
        return Err(refused("no signature".into()));
    };
    let transcript = transcript(role, &peer_id, channel);
    match signing::verify(&peer_id, Purpose::PeerHello, &transcript, sig) {
        SignatureStatus::Valid => Ok(PeerHello { peer_id }),
        SignatureStatus::Invalid => Err(refused("its signature is not for this connection".into())),
    }
}

/// The door's HELLO check (plan Step 4.2b): the node that spoke must be bound
/// to the endpoint key its connection came from, by a record or, on first
/// contact, by the operator's configuration. No door, no check.
fn bound(door: Option<&Door>, peer: &PeerHello, endpoint: &[u8; 32]) -> io::Result<()> {
    let Some(door) = door else {
        return Ok(());
    };
    door.binds(&peer.peer_id, endpoint).map_err(|why| {
        let why = format!("HELLO refused: {why}");
        io::Error::new(io::ErrorKind::PermissionDenied, why)
    })
}

/// Dialer side of the node<->node HELLO: send `NodeHello`, await `NodeWelcome`,
/// and return the peer once its WELCOME verifies for `channel` and, with a
/// door, its node is bound to the acceptor's endpoint key.
pub async fn hello_dial<R, W>(
    r: &mut R,
    w: &mut W,
    me: &NodeIdentity,
    channel: &Channel,
    door: Option<&Door>,
) -> io::Result<PeerHello>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let hello = NodeHello {
        node_id: me.node_id.to_vec(),
        protocol: PROTOCOL,
        sig: hello_sig(me, Role::Dialer, channel),
    };
    write_frame(w, &Frame::NodeHello(hello)).await?;
    let peer = match read_frame(r).await? {
        Frame::NodeWelcome(nw) => {
            verify_peer(&nw.node_id, nw.protocol, &nw.sig, Role::Acceptor, channel)?
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected NodeWelcome, got {other:?}"),
            ))
        }
    };
    bound(door, &peer, &channel.acceptor)?;
    Ok(peer)
}

/// Acceptor side: await `NodeHello` and check it for `channel` and, with a
/// door, its node's binding to the dialer's endpoint key; only then reply
/// `NodeWelcome`. A refused HELLO gets no answer.
pub async fn hello_accept<R, W>(
    r: &mut R,
    w: &mut W,
    me: &NodeIdentity,
    channel: &Channel,
    door: Option<&Door>,
) -> io::Result<PeerHello>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let peer = match read_frame(r).await? {
        Frame::NodeHello(nh) => {
            verify_peer(&nh.node_id, nh.protocol, &nh.sig, Role::Dialer, channel)?
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected NodeHello, got {other:?}"),
            ))
        }
    };
    bound(door, &peer, &channel.dialer)?;
    let welcome = NodeWelcome {
        node_id: me.node_id.to_vec(),
        protocol: PROTOCOL,
        sig: hello_sig(me, Role::Acceptor, channel),
    };
    write_frame(w, &Frame::NodeWelcome(welcome)).await?;
    Ok(peer)
}

// ---- heads/gap sync -------------------------------------------------------

/// Max ops per streamed chunk. Bulk backfill is size-capped so it never
/// head-of-line-blocks interactive traffic — the §6 scheduler guarantee applied
/// to sync. Resume is free: the receiver's HEADS advance as ops land.
pub const OPS_PER_CHUNK: usize = 64;

/// Per-op origin-signature verification seam (GQ-9). STUBBED: called before an
/// op can land, but currently accepts. Real ed25519 over the op's canonical
/// bytes drops in here — closing the one-op tamper window (GladePeerSyncNotes §6).
pub fn verify_origin_sig(_op: &Op) -> bool {
    true
}

/// A `(share, glade_id, key, origin)` chain — the per-(origin, zone) unit (D8).
pub type ChainKey = (String, String, Vec<u8>, String);

fn chain_key(op: &Op) -> ChainKey {
    (op.share.clone(), op.glade_id.clone(), op.key.clone(), op.origin.clone())
}

/// What a pull produced.
#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// Ops that landed (appended or idempotent duplicate).
    pub applied: usize,
    /// (origin, zone) chains whose suffix was rejected FROM THIS PEER — chain
    /// break, gap, or bad signature. Nothing that peer sent after the break is
    /// kept; the caller re-fetches these chains elsewhere (resume is exact).
    pub rejected: Vec<ChainKey>,
    /// Equivocation proofs newly recorded while ingesting this stream — a signed
    /// fork by the ORIGIN (SY4), not the carrier's fault.
    pub equivocations: Vec<EquivProof>,
}

/// Server side of a pull (the s-sync responder): read the peer's HEADS, stream
/// exactly the ops it lacks for every zone we hold, in size-capped BULK chunks,
/// then close the write half — that close is the "gap complete" terminator.
///
/// Offers every zone this store holds. ACL zone-filtering (a peer withholding an
/// entire private-zone chain) is a drop-in here: filter `store.zones()` — the
/// per-(origin, zone) shape means absent chains are absences, not holes.
pub async fn serve_sync<R, W>(r: &mut R, w: &mut W, store: &Store) -> io::Result<usize>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let their = match read_frame(r).await? {
        Frame::Heads(h) => h.streams,
        other => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("expected Heads, got {other:?}"))),
    };
    // Index the peer's vectors by zone -> origin -> seq.
    let mut their_by_zone: BTreeMap<(String, String, Vec<u8>), BTreeMap<String, i64>> = BTreeMap::new();
    for sh in their {
        let m = their_by_zone.entry((sh.share.clone(), sh.glade_id.clone(), sh.key.clone())).or_default();
        for hd in sh.heads {
            m.insert(hd.origin, hd.seq);
        }
    }
    let mut sent = 0usize;
    for (share, glade_id, key) in store.zones() {
        let their_v = their_by_zone.get(&(share.clone(), glade_id.clone(), key.clone())).cloned().unwrap_or_default();
        let gap = missing_for(store, &share, &glade_id, &key, &their_v);
        for chunk in gap.chunks(OPS_PER_CHUNK) {
            write_frame(w, &Frame::Ops(Ops { ops: chunk.to_vec(), pri: Some(Priority::Bulk) })).await?;
            sent += chunk.len();
        }
    }
    w.shutdown().await?; // close = gap complete
    Ok(sent)
}

/// Dialer side of a pull (the s-sync initiator): announce our HEADS, then ingest
/// the peer's gap stream, VERIFYING EACH OP AS IT LANDS (`store.append` =
/// prev-hash continuity + seq monotonic + equivocation, plus the origin-sig
/// seam). On a chain-check failure the whole suffix of that (origin, zone) chain
/// from this peer is dropped and reported for re-fetch; equivocation records a
/// proof. Ends at the peer's stream close.
pub async fn pull_sync<R, W>(r: &mut R, w: &mut W, store: &mut Store) -> io::Result<SyncOutcome>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    write_frame(w, &Frame::Heads(Heads { streams: store.all_heads() })).await?;
    let before = store.equivocation_proofs().len();
    let mut out = SyncOutcome::default();
    loop {
        let frame = match read_frame(r).await {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // peer closed = done
            Err(e) => return Err(e),
        };
        let Frame::Ops(ops) = frame else { continue }; // pull channel carries only Ops
        for op in ops.ops {
            let ck = chain_key(&op);
            if out.rejected.contains(&ck) {
                continue; // suffix of an already-broken chain from this peer
            }
            if !verify_origin_sig(&op) {
                out.rejected.push(ck);
                continue;
            }
            match store.append(op) {
                Ok(_) => out.applied += 1,
                Err(StoreError::ChainBreak { .. })
                | Err(StoreError::Gap { .. })
                | Err(StoreError::InvalidSwmrPayload { .. })
                | Err(StoreError::SwmrWriterConflict { .. })
                | Err(StoreError::ShapeConflict { .. }) => out.rejected.push(ck),
                Err(StoreError::Equivocation { .. }) => {} // proof recorded in the store
                Err(StoreError::Io(e)) => return Err(e),
            }
        }
    }
    out.equivocations = store.equivocation_proofs()[before..].to_vec();
    Ok(out)
}

#[cfg(test)]
mod hello_tests {
    use super::*;
    use tokio::io::split;

    fn hex32(hex: &str) -> [u8; 32] {
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap();
        std::array::from_fn(|i| byte(2 * i))
    }

    /// The id is the seed's Ed25519 public key (plan Step 4.1a, D2), checked
    /// against RFC 8032 §7.1, TEST 1, whose secret key is a 32-byte seed as
    /// `node.key` is. It proves the derivation, not how a key is stored.
    #[test]
    fn node_id_is_the_ed25519_public_key_of_the_seed() {
        let seed = hex32("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let public = hex32("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        assert_eq!(NodeIdentity::from_key(seed).node_id, public);
    }

    /// A fixed channel for the in-memory HELLOs: what the carrier reads from a
    /// real connection (`iroh_carrier.rs` tests the real one).
    const CHANNEL: Channel = Channel {
        dialer: [1; 32],
        acceptor: [2; 32],
        exported: [3; 32],
    };

    /// The DIAL gate over an in-memory duplex: dialer and acceptor complete the
    /// HELLO and each learns the OTHER's node_id (not its own). The genuine
    /// case: accepted before plan Step 4.1a signed it, and after.
    #[tokio::test]
    async fn hello_handshake_exchanges_identities() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);

        // duplex(a,b): writing a is readable on b. Split each end into (r, w).
        let (a, b) = tokio::io::duplex(4096);
        let (mut ar, mut aw) = split(a);
        let (mut br, mut bw) = split(b);

        let acc = tokio::spawn(async move {
            let (me, channel) = (&acceptor, &CHANNEL);
            hello_accept(&mut br, &mut bw, me, channel, None).await
        });
        let dialed = hello_dial(&mut ar, &mut aw, &dialer, &CHANNEL, None).await;
        let seen_by_dialer = dialed.unwrap();
        let seen_by_acceptor = acc.await.unwrap().unwrap();

        assert_eq!(
            seen_by_dialer.peer_id, acceptor.node_id,
            "dialer learns acceptor id"
        );
        assert_eq!(
            seen_by_acceptor.peer_id, dialer.node_id,
            "acceptor learns dialer id"
        );
    }

    /// The HELLO `me` sends as the dialer on `channel`.
    fn hello_from(me: &NodeIdentity, channel: &Channel) -> NodeHello {
        NodeHello {
            node_id: me.node_id.to_vec(),
            protocol: PROTOCOL,
            sig: hello_sig(me, Role::Dialer, channel),
        }
    }

    /// Present `hello` to an acceptor on `channel`: its verdict, and whether
    /// it answered with a WELCOME.
    async fn present(hello: NodeHello, channel: Channel) -> (io::Result<PeerHello>, bool) {
        present_to(hello, channel, None).await
    }

    /// [`present`], to an acceptor behind `door`.
    async fn present_to(
        hello: NodeHello,
        channel: Channel,
        door: Option<&Door>,
    ) -> (io::Result<PeerHello>, bool) {
        let acceptor = NodeIdentity::from_key([9u8; 32]);
        let (a, b) = tokio::io::duplex(4096);
        let (mut ar, mut aw) = split(a);
        let (mut br, mut bw) = split(b);
        let hello = Frame::NodeHello(hello);
        write_frame(&mut aw, &hello).await.unwrap();
        let verdict = hello_accept(&mut br, &mut bw, &acceptor, &channel, door).await;
        drop((br, bw));
        let answered = read_frame(&mut ar).await.is_ok();
        (verdict, answered)
    }

    /// Plan Step 4.1a: a tampered HELLO is refused and gets no answer: a
    /// flipped signature byte, another node's id under the signature, protocol
    /// 1, and no signature. The untouched HELLO is accepted and answered. It
    /// does not try every byte.
    #[tokio::test]
    async fn a_tampered_hello_is_refused() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let genuine = hello_from(&dialer, &CHANNEL);
        let mut flipped = genuine.clone();
        if let Some(sig) = flipped.sig.as_mut() {
            sig[10] ^= 1;
        }
        let mut renamed = genuine.clone();
        renamed.node_id = NodeIdentity::from_key([8u8; 32]).node_id.to_vec();
        let mut old = genuine.clone();
        old.protocol = 1;
        let mut unsigned = genuine.clone();
        unsigned.sig = None;
        let tampered = [
            ("a flipped signature byte", flipped),
            ("another node's id", renamed),
            ("protocol 1", old),
            ("no signature", unsigned),
        ];
        for (what, hello) in tampered {
            let (verdict, answered) = present(hello, CHANNEL).await;
            assert!(verdict.is_err(), "{what}: accepted");
            assert!(!answered, "{what}: answered");
        }
        let (verdict, answered) = present(genuine, CHANNEL).await;
        assert_eq!(verdict.unwrap().peer_id, dialer.node_id);
        assert!(answered, "the genuine HELLO is answered");
    }

    /// Plan Step 4.1a: a HELLO recorded on one connection is refused on
    /// another, where the exported bytes differ, or the endpoint ids do. The
    /// real carrier's bytes differ per connection (`iroh_carrier.rs`).
    #[tokio::test]
    async fn a_hello_replayed_from_another_connection_is_refused() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let recorded = hello_from(&dialer, &CHANNEL);
        let another_session = Channel {
            exported: [4; 32],
            ..CHANNEL
        };
        let another_endpoint = Channel {
            dialer: [5; 32],
            ..CHANNEL
        };
        for channel in [another_session, another_endpoint] {
            let (verdict, answered) = present(recorded.clone(), channel).await;
            assert!(verdict.is_err(), "replayed onto {channel:?}: accepted");
            assert!(!answered, "replayed onto {channel:?}: answered");
        }
    }

    /// Plan Step 4.1a: a HELLO reflected with the roles swapped is refused. The
    /// dialer's own HELLO, mirrored back as the WELCOME, is checked as the
    /// acceptor's and fails; an acceptor's WELCOME, presented to it as a HELLO,
    /// is checked as a dialer's and fails.
    #[tokio::test]
    async fn a_reflected_hello_is_refused() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let (a, b) = tokio::io::duplex(4096);
        let (mut ar, mut aw) = split(a);
        let (mut br, mut bw) = split(b);
        let mirror = tokio::spawn(async move {
            let Frame::NodeHello(hello) = read_frame(&mut br).await.unwrap() else {
                panic!("expected the dialer's HELLO");
            };
            let welcome = NodeWelcome {
                node_id: hello.node_id,
                protocol: hello.protocol,
                sig: hello.sig,
            };
            write_frame(&mut bw, &Frame::NodeWelcome(welcome)).await.unwrap();
        });
        let verdict = hello_dial(&mut ar, &mut aw, &dialer, &CHANNEL, None).await;
        mirror.await.unwrap();
        assert!(verdict.is_err(), "the dialer took its own HELLO back");

        let acceptor = NodeIdentity::from_key([9u8; 32]);
        let reflected = NodeHello {
            node_id: acceptor.node_id.to_vec(),
            protocol: PROTOCOL,
            sig: hello_sig(&acceptor, Role::Acceptor, &CHANNEL),
        };
        let (verdict, answered) = present(reflected, CHANNEL).await;
        assert!(verdict.is_err(), "the acceptor took its own WELCOME back");
        assert!(!answered);
    }

    /// Plan Step 4.2b: HELLO completes only for a node bound to the endpoint
    /// key its connection came from (the channel's endpoint ids). The
    /// dialer's key `[1; 32]`: unknown to the door, refused, unanswered;
    /// configured, admitted on first contact; bound to another node, refused;
    /// bound to the dialer, admitted. And the dialer refuses a WELCOME whose
    /// node its door binds to no key the acceptor's endpoint holds.
    #[tokio::test]
    async fn a_hello_completes_only_for_a_node_bound_to_its_endpoint_key() {
        use crate::transport::testing::bound_by;
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let hello = hello_from(&dialer, &CHANNEL);
        let unknown = Door::new([], |_: &str| {});
        let configured = Door::new([CHANNEL.dialer], |_: &str| {});
        let elsewhere = bound_by(&[8u8; 32], &CHANNEL.dialer);
        let its_own = bound_by(&[7u8; 32], &CHANNEL.dialer);
        for (what, door, admitted) in [
            ("unknown", &unknown, false),
            ("configured", &configured, true),
            ("bound to another node", &elsewhere, false),
            ("bound to the dialer", &its_own, true),
        ] {
            let (verdict, answered) = present_to(hello.clone(), CHANNEL, Some(door)).await;
            assert_eq!(verdict.is_ok(), admitted, "{what}: {verdict:?}");
            assert_eq!(answered, admitted, "{what}: answered");
        }
        let acceptor = NodeIdentity::from_key([9u8; 32]);
        let (a, b) = tokio::io::duplex(4096);
        let ((mut ar, mut aw), (mut br, mut bw)) = (split(a), split(b));
        let welcomes = tokio::spawn(async move {
            let (me, channel) = (&acceptor, &CHANNEL);
            hello_accept(&mut br, &mut bw, me, channel, None).await
        });
        let not_its = bound_by(&[8u8; 32], &CHANNEL.acceptor);
        let verdict = hello_dial(&mut ar, &mut aw, &dialer, &CHANNEL, Some(&not_its)).await;
        assert!(welcomes.await.unwrap().is_ok(), "the acceptor answered");
        let refused = verdict.expect_err("the dialer took a WELCOME from an unbound node");
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use glade_wire::generated::Shape;
    use std::path::PathBuf;
    use tokio::io::split;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-peer-sync-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn op(origin: &str, seq: i64, key: &[u8], prev: Option<Vec<u8>>, payload: &[u8]) -> Op {
        Op {
            share: "sh".into(),
            glade_id: "g".into(),
            key: key.to_vec(),
            origin: origin.into(),
            seq,
            prev,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }

    /// Append a valid prev-linked chain of `n` ops for `origin` in `key`'s zone,
    /// returning the ops (so a test can replay/tamper them as a carrier would).
    fn chained(store: &mut Store, origin: &str, key: &[u8], n: i64) -> Vec<Op> {
        let mut prev = None;
        let mut ops = Vec::new();
        for seq in 0..n {
            let o = op(origin, seq, key, prev.clone(), format!("{origin}{seq}").as_bytes());
            store.append(o.clone()).unwrap();
            prev = Some(crate::chain::op_hash(&o).to_vec());
            ops.push(o);
        }
        ops
    }

    /// SY1+SY2: a fresh replica pulls the exact gap over a duplex and verifies
    /// every op as it lands — two chains, both converge byte-for-byte.
    #[tokio::test]
    async fn pull_converges_and_verifies() {
        let mut server = Store::open(fresh("srv")).unwrap();
        chained(&mut server, "a", b"", 5);
        chained(&mut server, "b", b"", 3);
        let mut client = Store::open(fresh("cli")).unwrap();

        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca); // client end
        let (mut br, mut bw) = split(cb); // server end

        let srv = tokio::spawn(async move { serve_sync(&mut br, &mut bw, &server).await });
        let out = pull_sync(&mut ar, &mut aw, &mut client).await.unwrap();
        let sent = srv.await.unwrap().unwrap();

        assert_eq!(sent, 8);
        assert_eq!(out.applied, 8);
        assert!(out.rejected.is_empty());
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 5);
        assert_eq!(client.scan("sh", "g", b"", "b", -1).len(), 3);
    }

    /// SY3: a tampering carrier flips op 3's `prev`. The chain check rejects op 3
    /// and the whole suffix FROM THIS PEER; re-fetching the chain from an honest
    /// replica resumes exactly (the retry costs one range, not one share).
    #[tokio::test]
    async fn tampered_suffix_rejected_then_refetched() {
        let mut truth = Store::open(fresh("truth")).unwrap();
        let ops = chained(&mut truth, "a", b"", 5);
        let mut client = Store::open(fresh("tamper-cli")).unwrap();

        // Malicious peer: ops 0,1,2 valid, op 3 with a tampered prev, then op 4.
        let mut bad3 = ops[3].clone();
        bad3.prev = Some(vec![0u8; 32]); // != hash(op2) -> chain break at seq 3
        let malicious = vec![ops[0].clone(), ops[1].clone(), ops[2].clone(), bad3, ops[4].clone()];

        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca);
        let (mut br, mut bw) = split(cb);
        let carrier = tokio::spawn(async move {
            read_frame(&mut br).await.unwrap(); // client HEADS
            write_frame(&mut bw, &Frame::Ops(Ops { ops: malicious, pri: Some(Priority::Bulk) })).await.unwrap();
            bw.shutdown().await.unwrap();
        });
        let out = pull_sync(&mut ar, &mut aw, &mut client).await.unwrap();
        carrier.await.unwrap();

        assert_eq!(out.applied, 3); // 0,1,2 landed; 3 broke, 4 (suffix) dropped
        assert_eq!(out.rejected, vec![("sh".into(), "g".into(), vec![], "a".into())]);
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 3);

        // Re-fetch from the honest replica: resume from head 2, gain 3 and 4.
        let (ca2, cb2) = tokio::io::duplex(64 * 1024);
        let (mut ar2, mut aw2) = split(ca2);
        let (mut br2, mut bw2) = split(cb2);
        let srv = tokio::spawn(async move { serve_sync(&mut br2, &mut bw2, &truth).await });
        let out2 = pull_sync(&mut ar2, &mut aw2, &mut client).await.unwrap();
        srv.await.unwrap().unwrap();

        assert!(out2.rejected.is_empty());
        assert_eq!(out2.applied, 2);
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 5); // full chain restored
    }

    /// SY4: the peer streams a second signed op into a slot the client already
    /// holds — an origin fork. Ingest rejects it and surfaces the proof.
    #[tokio::test]
    async fn pull_surfaces_equivocation_proof() {
        let mut client = Store::open(fresh("equiv")).unwrap();
        client.append(op("a", 0, b"", None, b"A")).unwrap();
        let conflict = op("a", 0, b"", None, b"B"); // same slot, different hash

        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca);
        let (mut br, mut bw) = split(cb);
        let carrier = tokio::spawn(async move {
            read_frame(&mut br).await.unwrap();
            write_frame(&mut bw, &Frame::Ops(Ops { ops: vec![conflict], pri: Some(Priority::Bulk) })).await.unwrap();
            bw.shutdown().await.unwrap();
        });
        let out = pull_sync(&mut ar, &mut aw, &mut client).await.unwrap();
        carrier.await.unwrap();

        assert_eq!(out.applied, 0);
        assert_eq!(out.equivocations.len(), 1);
        assert_eq!(out.equivocations[0].a.payload, b"A");
        assert_eq!(out.equivocations[0].b.payload, b"B");
        assert_eq!(client.equivocation_proofs().len(), 1); // persisted in the store
    }
}
