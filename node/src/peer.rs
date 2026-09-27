//! Node<->node session (Lane R step 2): the HELLO seam + heads/gap sync.
//!
//! Carrier-free by construction — everything here runs over any `AsyncRead +
//! AsyncWrite` pair, so the protocol is unit-tested over an in-memory duplex and
//! rides real iroh QUIC (`iroh_carrier.rs`) unchanged. HELLO also runs on any
//! `CarrierLink`, as a link's first frame each way (plan Step 4.5b). Two layers:
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
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use glade_carrier_api::{CarrierError, CarrierLink};
use glade_grant_api::{GrantPort, Holder};
use glade_signer_api::{Purpose, SignatureStatus};
use glade_wire::cbor::{self, Cbor};
use glade_wire::generated::{Heads, NodeHello, NodeWelcome, Op, Ops, Priority};

use crate::frame::{frame_len, Frame};
use crate::grants::READ_SUBSCRIBE;
use crate::registry::HOME;
use crate::session::missing_for;
use crate::signing;
use crate::store::{EquivProof, Store, StoreError};
use crate::transport::Door;

/// Wire protocol version spoken on the peer link: 2 from plan Step 4.1a, whose
/// HELLO is signed, and 3 from plan Step 4.1b, whose `home` records are
/// signed envelopes that an older node cannot read. The carrier's ALPN names
/// it too, so a node of an older protocol fails at connect.
pub const PROTOCOL: i64 = 3;

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
/// surfaces as `UnexpectedEof` — the sync loop reads that as "peer done". A
/// length over `MAX_FRAME_BYTES` is refused before its body, as `InvalidData`
/// (F15).
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let n = frame_len(u32::from_le_bytes(len).into())?;
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

// ---- HELLO on a carrier link (plan Step 4.5b) ------------------------------

/// The label a link's channel binding is drawn under for HELLO: the label its
/// bytes are exported under today, so D6's transcript is unchanged.
pub const HELLO_LABEL: &[u8] = b"glade/v1/peer-hello";

/// How long HELLO has on a link (plan Step 4.5b, the owner's ruling of
/// 2026-09-27): from the moment the port hands the link over to a verified
/// `NodeHello`, at the acceptor, or `NodeWelcome`, at the dialer.
pub const HELLO_WITHIN: Duration = Duration::from_secs(10);

/// A bound as a line says it: `10 s` for whole seconds, else milliseconds.
pub(crate) fn spelled(bound: Duration) -> String {
    match bound.subsec_nanos() {
        0 => format!("{} s", bound.as_secs()),
        _ => format!("{} ms", bound.as_millis()),
    }
}

/// A HELLO this end refuses, and why.
fn refused(why: impl fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("HELLO refused: {why}"),
    )
}

/// What HELLO signs on `link` (D6), seen from `role`'s end, whose own
/// endpoint key is `own`: the far end's key, as the link's transport proved
/// it, and the bytes the link's session derives under [`HELLO_LABEL`]. A link
/// that names no 32-byte key, or has no binding, cannot complete HELLO.
fn link_channel(link: &dyn CarrierLink, own: &[u8; 32], role: Role) -> io::Result<Channel> {
    let far = link
        .remote_id()
        .and_then(|id| <[u8; 32]>::try_from(id.0).ok());
    let far = far.ok_or_else(|| refused("the link names no endpoint key"))?;
    let binding = link.channel_binding(HELLO_LABEL);
    let exported = binding
        .ok_or_else(|| refused("the link binds no session"))?
        .0;
    let (dialer, acceptor) = match role {
        Role::Dialer => (*own, far),
        Role::Acceptor => (far, *own),
    };
    Ok(Channel {
        dialer,
        acceptor,
        exported,
    })
}

/// A carrier's refusal, as an I/O error.
fn carried(e: CarrierError) -> io::Error {
    match e {
        CarrierError::Transport(why) => io::Error::other(why),
        other => io::Error::other(format!("{other:?}")),
    }
}

/// Send `frame` bare, as HELLO's two frames are sent.
async fn send_bare(link: &dyn CarrierLink, frame: &Frame) -> io::Result<()> {
    link.send(&frame.to_bytes()).await.map_err(carried)
}

/// The link's next frame, bare; if the link ends first, it ended before the
/// `awaited` frame.
async fn recv_bare(link: &dyn CarrierLink, awaited: &str) -> io::Result<Frame> {
    match link.recv().await {
        Ok(Some(bytes)) => {
            Frame::from_bytes(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Ok(None) => {
            let why = format!("the link ended before a {awaited}");
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, why))
        }
        Err(e) => Err(carried(e)),
    }
}

/// A frame where `expected` belongs.
fn unexpected(expected: &str, got: &Frame) -> io::Error {
    let why = format!("expected {expected}, got {got:?}");
    io::Error::new(io::ErrorKind::InvalidData, why)
}

/// Dialer side of HELLO on a carrier link (plan Step 4.5b): `NodeHello` is
/// the link's first frame, bare, and the acceptor's `NodeWelcome` must verify
/// for the link's channel within `within`; with a door, its node must be bound
/// to the acceptor's endpoint key. `own` is this end's endpoint key.
pub async fn hello_dial_link(
    link: &dyn CarrierLink,
    own: &[u8; 32],
    me: &NodeIdentity,
    door: Option<&Door>,
    within: Duration,
) -> io::Result<PeerHello> {
    let channel = link_channel(link, own, Role::Dialer)?;
    let welcomed = async {
        let hello = NodeHello {
            node_id: me.node_id.to_vec(),
            protocol: PROTOCOL,
            sig: hello_sig(me, Role::Dialer, &channel),
        };
        send_bare(link, &Frame::NodeHello(hello)).await?;
        match recv_bare(link, "WELCOME").await? {
            Frame::NodeWelcome(nw) => {
                verify_peer(&nw.node_id, nw.protocol, &nw.sig, Role::Acceptor, &channel)
            }
            other => Err(unexpected("NodeWelcome", &other)),
        }
    };
    let late = format!("no WELCOME within {}", spelled(within));
    let late = || io::Error::new(io::ErrorKind::TimedOut, late);
    let peer = tokio::time::timeout(within, welcomed)
        .await
        .map_err(|_| late())??;
    bound(door, &peer, &channel.acceptor)?;
    Ok(peer)
}

/// Acceptor side of HELLO on a carrier link (plan Step 4.5b): the link's first
/// frame must be a `NodeHello` that verifies for the link's channel within
/// `within`, from a node that, with a door, is bound to the dialer's endpoint
/// key; only then is `NodeWelcome` sent, bare. A refused HELLO gets no answer,
/// and the door reports it by the dialer's tag. `own` is this end's endpoint
/// key.
pub async fn hello_accept_link(
    link: &dyn CarrierLink,
    own: &[u8; 32],
    me: &NodeIdentity,
    door: Option<&Door>,
    within: Duration,
) -> io::Result<PeerHello> {
    let channel = link_channel(link, own, Role::Acceptor)?;
    let heard = async {
        let peer = match recv_bare(link, "HELLO").await? {
            Frame::NodeHello(nh) => {
                verify_peer(&nh.node_id, nh.protocol, &nh.sig, Role::Dialer, &channel)?
            }
            other => return Err(unexpected("NodeHello", &other)),
        };
        bound(door, &peer, &channel.dialer)?;
        Ok(peer)
    };
    let late = || refused(format!("no HELLO within {}", spelled(within)));
    let heard = tokio::time::timeout(within, heard).await;
    let peer = heard.unwrap_or_else(|_| Err(late()));
    let peer = peer.inspect_err(|e| report(door, &channel.dialer, e))?;
    let welcome = NodeWelcome {
        node_id: me.node_id.to_vec(),
        protocol: PROTOCOL,
        sig: hello_sig(me, Role::Acceptor, &channel),
    };
    send_bare(link, &Frame::NodeWelcome(welcome)).await?;
    Ok(peer)
}

/// Report a refused HELLO through the door, naming the endpoint key it came
/// from, as `PeerEndpoint::accept` does on a stream.
fn report(door: Option<&Door>, endpoint: &[u8; 32], e: &io::Error) {
    if let (Some(door), io::ErrorKind::PermissionDenied) = (door, e.kind()) {
        door.refused(endpoint, e);
    }
}

// ---- heads/gap sync -------------------------------------------------------

/// Max ops per streamed chunk. Bulk backfill is size-capped so it never
/// head-of-line-blocks interactive traffic — the §6 scheduler guarantee applied
/// to sync. Resume is free: the receiver's HEADS advance as ops land.
pub const OPS_PER_CHUNK: usize = 64;

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
    /// break, gap, or a `home` op that does not verify (plan Step 4.1b).
    /// Nothing that peer sent after the break is kept; the caller re-fetches
    /// these chains elsewhere (resume is exact).
    pub rejected: Vec<ChainKey>,
    /// `home` chains deferred FROM THIS PEER (plan Step 4.1b's part 2, D9):
    /// their origin is not a node the puller knows. Nothing of them is kept,
    /// and the next pull asks for them again.
    pub deferred: Vec<ChainKey>,
    /// Equivocation proofs newly recorded while ingesting this stream — a signed
    /// fork by the ORIGIN (SY4), not the carrier's fault.
    pub equivocations: Vec<EquivProof>,
}

/// Server side of a pull (the s-sync responder): read the peer's HEADS, stream
/// exactly the ops it lacks for every zone we hold, in size-capped BULK chunks,
/// then close the write half — that close is the "gap complete" terminator.
///
/// Offers every zone of `home`, how grants arrive, and every other zone whose
/// share `grants` lets `holder` read (plan Step 4.3's grant check,
/// `read.subscribe`). A zone refused is left out whole: the per-(origin, zone)
/// shape makes it an absence, not a hole.
pub async fn serve_sync<R, W>(
    r: &mut R,
    w: &mut W,
    store: &Store,
    holder: &Holder,
    grants: &dyn GrantPort,
) -> io::Result<usize>
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
        if share != HOME && grants.check(holder, READ_SUBSCRIBE, &share).is_err() {
            continue;
        }
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
/// prev-hash continuity + seq monotonic + equivocation, and a `home` op's
/// origin signature, plan Step 4.1b; app ops carry none, D5). On a
/// chain-check failure the whole suffix of that (origin, zone) chain from this
/// peer is dropped and reported for re-fetch; equivocation records a proof.
/// A `home` op whose origin `known` does not answer for is deferred with its
/// chain's suffix, and kept nowhere (D9). Ends at the peer's stream close.
pub async fn pull_sync<R, W>(
    r: &mut R,
    w: &mut W,
    store: &mut Store,
    known: &dyn Fn(&str) -> bool,
) -> io::Result<SyncOutcome>
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
            if out.rejected.contains(&ck) || out.deferred.contains(&ck) {
                continue; // suffix of an already-broken chain from this peer
            }
            if op.share == HOME && !known(&op.origin) {
                out.deferred.push(ck);
                continue;
            }
            match store.append(op) {
                Ok(_) => out.applied += 1,
                Err(StoreError::ChainBreak { .. })
                | Err(StoreError::Gap { .. })
                | Err(StoreError::Unverified { .. })
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

    /// `genuine`, tampered four ways: a flipped signature byte, another
    /// node's id under the signature, protocol 1, and no signature.
    fn tampered(genuine: &NodeHello) -> [(&'static str, NodeHello); 4] {
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
        [
            ("a flipped signature byte", flipped),
            ("another node's id", renamed),
            ("protocol 1", old),
            ("no signature", unsigned),
        ]
    }

    /// Doors for the dialer's key, `CHANNEL.dialer`, each with whether it
    /// admits the HELLO of the node `[7; 32]`, the dialer: unknown, no;
    /// configured, yes, on first contact; bound to another node, no; bound to
    /// the dialer, yes.
    fn doors() -> [(&'static str, Door, bool); 4] {
        use crate::transport::testing::bound_by;
        [
            ("unknown", Door::new([], |_: &str| {}), false),
            (
                "configured",
                Door::new([CHANNEL.dialer], |_: &str| {}),
                true,
            ),
            (
                "bound to another node",
                bound_by(&[8u8; 32], &CHANNEL.dialer),
                false,
            ),
            (
                "bound to the dialer",
                bound_by(&[7u8; 32], &CHANNEL.dialer),
                true,
            ),
        ]
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
        for (what, hello) in tampered(&genuine) {
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
        for (what, door, admitted) in doors() {
            let (verdict, answered) = present_to(hello.clone(), CHANNEL, Some(&door)).await;
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

    /// F12, on the peer link: a frame holding a value its enum does not name
    /// is refused as `InvalidData`, naming the value, where decoding it
    /// panicked the task reading the link. A HELLO whose tag names no frame
    /// type fails the acceptor's handshake, and an `Ops` frame holding an
    /// unknown shape fails `read_frame`, the mesh's framed read.
    #[tokio::test]
    async fn a_frame_with_an_unknown_value_is_refused_as_invalid_data() {
        use glade_wire::cbor::{self, Cbor};
        use glade_wire::generated::Op;
        let mut op = Op::default().to_cbor();
        if let Cbor::Map(entries) = &mut op {
            entries.retain(|(key, _)| *key != 9);
            entries.push((9, Cbor::Int(9)));
        }
        let ops = Cbor::Map(vec![(1, Cbor::Array(vec![op])), (2, Cbor::Null)]);
        let empty = cbor::encode(&Cbor::Map(vec![]));
        let hello = [&[15][..], &empty].concat();
        let shaped = [&[4][..], &cbor::encode(&ops)].concat();
        let cases = [(hello, "frame type 15", true), (shaped, "shape 9", false)];
        for (bytes, value, handshake) in cases {
            let (a, b) = tokio::io::duplex(4096);
            let ((_ar, mut aw), (mut br, mut bw)) = (split(a), split(b));
            let len = (bytes.len() as u32).to_le_bytes();
            aw.write_all(&len).await.unwrap();
            aw.write_all(&bytes).await.unwrap();
            let me = NodeIdentity::from_key([9u8; 32]);
            let refused = if handshake {
                let accepted = hello_accept(&mut br, &mut bw, &me, &CHANNEL, None).await;
                accepted.map(|_| ())
            } else {
                read_frame(&mut br).await.map(|_| ())
            };
            let refused = refused.expect_err(value);
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{value}");
            assert_eq!(refused.to_string(), format!("bad frame: unknown {value}"));
        }
    }

    /// F15, on the peer stream: a frame `read_frame` cannot decode is refused
    /// as `InvalidData`, where decoding it panicked the task reading the
    /// stream (a truncated frame, one holding a CBOR tag) or, nested 100,000
    /// deep, overflowed its thread's stack and aborted the node. The stream
    /// stays framed: the frame after each is read.
    #[tokio::test]
    async fn a_truncated_malformed_or_nested_frame_is_refused_as_invalid_data() {
        let me = NodeIdentity::from_key([9u8; 32]);
        let hello = Frame::NodeHello(NodeHello {
            node_id: me.node_id.to_vec(),
            protocol: PROTOCOL,
            sig: None,
        });
        let good = hello.to_bytes();
        let tag = &good[..1];
        let tagged = "bad frame: CBOR the wire does not take (0xc0)";
        let cases = [
            (good[..good.len() - 1].to_vec(), "bad frame: truncated"),
            ([tag, &[0xc0, 0x00]].concat(), tagged),
            (
                [tag, &[0x81].repeat(100_000), &[0x80]].concat(),
                "bad frame: nested deeper than 32",
            ),
        ];
        for (bytes, said) in cases {
            let (mut near, mut far) = tokio::io::duplex(1 << 20);
            for frame in [&bytes, &good] {
                let len = (frame.len() as u32).to_le_bytes();
                near.write_all(&len).await.unwrap();
                near.write_all(frame).await.unwrap();
            }
            let refused = read_frame(&mut far).await.expect_err(said);
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{said}");
            assert_eq!(refused.to_string(), said);
            let after = read_frame(&mut far).await.unwrap();
            assert_eq!(after, hello, "after {said}");
        }
    }

    /// F15: a length over `MAX_FRAME_BYTES`, by one or up to `u32::MAX`, is
    /// refused as `InvalidData` before its body, where `read_frame` allocated
    /// the length and waited for the body. A frame of exactly the limit is
    /// read.
    #[tokio::test]
    async fn a_length_over_the_frame_limit_is_refused_before_its_body() {
        use crate::frame::MAX_FRAME_BYTES;
        use glade_wire::generated::ChannelData;
        let over = MAX_FRAME_BYTES as u32 + 1;
        for claimed in [over, u32::MAX] {
            let (mut near, mut far) = tokio::io::duplex(64);
            near.write_all(&claimed.to_le_bytes()).await.unwrap();
            let read = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut far));
            let refused = read.await.expect("refused before its body");
            let refused = refused.expect_err("over the limit");
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{claimed}");
            let said = format!("bad frame: {claimed} bytes, over the limit of {MAX_FRAME_BYTES}");
            assert_eq!(refused.to_string(), said);
        }
        // The tag, a map of two, their keys, "c", and the head of the data
        // take 11 bytes.
        let data = vec![7; MAX_FRAME_BYTES - 11];
        let channel = "c".into();
        let at_limit = Frame::ChannelData(ChannelData { channel, data }).to_bytes();
        assert_eq!(at_limit.len(), MAX_FRAME_BYTES);
        let (mut near, mut far) = tokio::io::duplex(MAX_FRAME_BYTES + 4);
        let len = (MAX_FRAME_BYTES as u32).to_le_bytes();
        near.write_all(&len).await.unwrap();
        near.write_all(&at_limit).await.unwrap();
        let read = read_frame(&mut far).await.unwrap();
        let whole = matches!(read, Frame::ChannelData(c) if c.data.len() == MAX_FRAME_BYTES - 11);
        assert!(whole, "the frame at the limit was not read whole");
    }

    // ---- HELLO on a carrier link (plan Step 4.5b) ------------------------
    //
    // Each rule above, on an in-memory link pair, and the rules a link adds:
    // its transport session's binding, its protocol gate, a link that cannot
    // HELLO, and HELLO's bound.

    use crate::conversation::testing::{endpoint_key, iroh_link, iroh_port, pair, MemLink};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// An in-memory link on `channel`: each end names the other's key, and
    /// both answer its exported bytes under HELLO's label.
    fn on(channel: &Channel) -> [Arc<MemLink>; 2] {
        let ids = [channel.dialer.to_vec(), channel.acceptor.to_vec()];
        pair(ids, Some(channel.exported), 1 << 16)
    }

    /// [`present_to`], on a link on `channel`.
    async fn presented_on_link(
        hello: NodeHello,
        channel: Channel,
        door: Option<&Door>,
    ) -> (io::Result<PeerHello>, bool) {
        let acceptor = NodeIdentity::from_key([9u8; 32]);
        let [dialer, accepting] = on(&channel);
        dialer
            .send(&Frame::NodeHello(hello).to_bytes())
            .await
            .unwrap();
        let (me, own) = (&acceptor, &channel.acceptor);
        let verdict = hello_accept_link(&*accepting, own, me, door, HELLO_WITHIN).await;
        accepting.close().await;
        let answered = matches!(dialer.recv().await, Ok(Some(_)));
        (verdict, answered)
    }

    /// Both ends' HELLOs on a link on `channel`: the node `[7; 32]` dials,
    /// behind `door`, and the node `[9; 32]` accepts.
    async fn hello_on_link(
        channel: &Channel,
        door: Option<&Door>,
    ) -> (io::Result<PeerHello>, io::Result<PeerHello>) {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);
        let [dialing, accepting] = on(channel);
        tokio::join!(
            hello_dial_link(&*dialing, &channel.dialer, &dialer, door, HELLO_WITHIN),
            hello_accept_link(
                &*accepting,
                &channel.acceptor,
                &acceptor,
                None,
                HELLO_WITHIN
            ),
        )
    }

    /// A door that admits `keys` on first contact, and the lines it reports.
    fn noting(keys: &[[u8; 32]]) -> (Door, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        let door = Door::new(keys.iter().copied(), move |line: &str| {
            sink.lock().unwrap().push(line.into());
        });
        (door, lines)
    }

    /// `future`, within 5 s: a test that would hang fails.
    async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
        let bounded = tokio::time::timeout(Duration::from_secs(5), future).await;
        bounded.expect("within 5 s")
    }

    /// Plan Step 4.5b: HELLO as a link's first frame each way; each end
    /// learns the other's node id.
    #[tokio::test]
    async fn hello_on_a_link_exchanges_identities() {
        let (dialed, accepted) = hello_on_link(&CHANNEL, None).await;
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);
        assert_eq!(
            dialed.unwrap().peer_id,
            acceptor.node_id,
            "the dialer learns"
        );
        assert_eq!(
            accepted.unwrap().peer_id,
            dialer.node_id,
            "the acceptor learns"
        );
    }

    /// Plan Step 4.5b: on a link, as on a stream, a tampered HELLO is refused
    /// and gets no answer, and the genuine one is taken and answered.
    #[tokio::test]
    async fn a_tampered_hello_is_refused_on_a_link() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let genuine = hello_from(&dialer, &CHANNEL);
        for (what, hello) in tampered(&genuine) {
            let (verdict, answered) = presented_on_link(hello, CHANNEL, None).await;
            assert!(verdict.is_err(), "{what}: accepted");
            assert!(!answered, "{what}: answered");
        }
        let (verdict, answered) = presented_on_link(genuine, CHANNEL, None).await;
        assert_eq!(verdict.unwrap().peer_id, dialer.node_id);
        assert!(answered, "the genuine HELLO is answered");
    }

    /// Plan Step 4.5b: a HELLO recorded on one link, as its dialer sent it,
    /// is refused on another whose session binds other bytes, or whose
    /// dialer's key differs.
    #[tokio::test]
    async fn a_hello_replayed_from_another_link_is_refused() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let [dialing, recording] = on(&CHANNEL);
        let unanswered = Duration::from_millis(50);
        let dialed = hello_dial_link(&*dialing, &CHANNEL.dialer, &dialer, None, unanswered);
        let (_, recorded) = tokio::join!(dialed, recording.recv());
        let recorded = recorded
            .unwrap()
            .expect("the dialer's HELLO, as it crossed");
        let Ok(Frame::NodeHello(recorded)) = Frame::from_bytes(&recorded) else {
            panic!("expected the dialer's HELLO");
        };
        let another_session = Channel {
            exported: [4; 32],
            ..CHANNEL
        };
        let another_endpoint = Channel {
            dialer: [5; 32],
            ..CHANNEL
        };
        for channel in [another_session, another_endpoint] {
            let (verdict, answered) = presented_on_link(recorded.clone(), channel, None).await;
            assert!(verdict.is_err(), "replayed onto {channel:?}: accepted");
            assert!(!answered, "replayed onto {channel:?}: answered");
        }
    }

    /// Plan Step 4.5b: on a link, as on a stream, a HELLO reflected with the
    /// roles swapped is refused: the dialer's own, mirrored back as the
    /// WELCOME, and an acceptor's WELCOME presented as a HELLO.
    #[tokio::test]
    async fn a_reflected_hello_is_refused_on_a_link() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let [dialing, mirror] = on(&CHANNEL);
        let reflect = async {
            let bytes = mirror.recv().await.unwrap().unwrap();
            let Ok(Frame::NodeHello(hello)) = Frame::from_bytes(&bytes) else {
                panic!("expected the dialer's HELLO");
            };
            let welcome = NodeWelcome {
                node_id: hello.node_id,
                protocol: hello.protocol,
                sig: hello.sig,
            };
            mirror
                .send(&Frame::NodeWelcome(welcome).to_bytes())
                .await
                .unwrap();
        };
        let dialed = hello_dial_link(&*dialing, &CHANNEL.dialer, &dialer, None, HELLO_WITHIN);
        let (verdict, ()) = tokio::join!(dialed, reflect);
        assert!(verdict.is_err(), "the dialer took its own HELLO back");

        let acceptor = NodeIdentity::from_key([9u8; 32]);
        let reflected = NodeHello {
            node_id: acceptor.node_id.to_vec(),
            protocol: PROTOCOL,
            sig: hello_sig(&acceptor, Role::Acceptor, &CHANNEL),
        };
        let (verdict, answered) = presented_on_link(reflected, CHANNEL, None).await;
        assert!(verdict.is_err(), "the acceptor took its own WELCOME back");
        assert!(!answered);
    }

    /// Plan Step 4.5b: on a link, as on a stream, HELLO completes only for a
    /// node bound to the endpoint key its link came from, as [`doors`] says;
    /// and the dialer refuses a WELCOME whose node its door binds to no key
    /// the acceptor's end holds.
    #[tokio::test]
    async fn a_hello_on_a_link_completes_only_for_a_node_bound_to_its_endpoint_key() {
        use crate::transport::testing::bound_by;
        let hello = hello_from(&NodeIdentity::from_key([7u8; 32]), &CHANNEL);
        for (what, door, admitted) in doors() {
            let (verdict, answered) = presented_on_link(hello.clone(), CHANNEL, Some(&door)).await;
            assert_eq!(verdict.is_ok(), admitted, "{what}: {verdict:?}");
            assert_eq!(answered, admitted, "{what}: answered");
        }
        let not_its = bound_by(&[8u8; 32], &CHANNEL.acceptor);
        let (dialed, accepted) = hello_on_link(&CHANNEL, Some(&not_its)).await;
        assert!(accepted.is_ok(), "the acceptor answered");
        let refused = dialed.expect_err("the dialer took a WELCOME from an unbound node");
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    }

    /// Plan Step 4.5b, over two iroh ports on loopback: HELLO completes both
    /// ways on a link; a HELLO recorded on one link is refused, unanswered,
    /// on another between the same two ports, whose TLS session binds other
    /// bytes; and one mirrored back as the WELCOME is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hello_on_a_link_binds_its_transport_session() {
        let (a_key, b_key) = (endpoint_key(), endpoint_key());
        let ((a, _), (b, at_b)) = (
            iroh_port(a_key, 64 << 10).await,
            iroh_port(b_key, 64 << 10).await,
        );
        let (own_a, own_b) = (a_key.endpoint_id, b_key.endpoint_id);
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);

        let [dialing, accepting] = iroh_link(&a, &at_b, &b).await;
        let (dialed, accepted) = tokio::join!(
            hello_dial_link(&*dialing, &own_a, &dialer, None, HELLO_WITHIN),
            hello_accept_link(&*accepting, &own_b, &acceptor, None, HELLO_WITHIN),
        );
        assert_eq!(dialed.unwrap().peer_id, acceptor.node_id);
        assert_eq!(accepted.unwrap().peer_id, dialer.node_id);

        let [dialing, recording] = iroh_link(&a, &at_b, &b).await;
        let unanswered = Duration::from_millis(200);
        let dialed = hello_dial_link(&*dialing, &own_a, &dialer, None, unanswered);
        let (_, recorded) = tokio::join!(dialed, recording.recv());
        let recorded = recorded
            .unwrap()
            .expect("the dialer's HELLO, as it crossed");
        let [replaying, accepting] = iroh_link(&a, &at_b, &b).await;
        replaying.send(&recorded).await.unwrap();
        let replayed = hello_accept_link(&*accepting, &own_b, &acceptor, None, HELLO_WITHIN);
        let refused = replayed
            .await
            .expect_err("a HELLO replayed from another link");
        let why = "HELLO refused: its signature is not for this connection";
        assert_eq!(refused.to_string(), why);
        accepting.close().await;
        assert_eq!(
            within(replaying.recv()).await,
            Ok(None),
            "the replay was answered"
        );

        let [dialing, mirror] = iroh_link(&a, &at_b, &b).await;
        let reflect = async {
            let bytes = mirror.recv().await.unwrap().unwrap();
            let Ok(Frame::NodeHello(hello)) = Frame::from_bytes(&bytes) else {
                panic!("expected the dialer's HELLO");
            };
            let welcome = NodeWelcome {
                node_id: hello.node_id,
                protocol: hello.protocol,
                sig: hello.sig,
            };
            mirror
                .send(&Frame::NodeWelcome(welcome).to_bytes())
                .await
                .unwrap();
        };
        let dialed = hello_dial_link(&*dialing, &own_a, &dialer, None, HELLO_WITHIN);
        let (verdict, ()) = tokio::join!(dialed, reflect);
        assert!(verdict.is_err(), "the dialer took its own HELLO back");
    }

    /// Plan Step 4.5b: HELLO's `protocol` is the node protocol's only gate on
    /// a link. A `NodeHello` of protocol 4, its signature good, is refused and
    /// unanswered, and the door reports it: the gate plan Step 4.5c turns.
    #[tokio::test]
    async fn a_hello_of_another_protocol_is_refused_on_a_link() {
        let mut hello = hello_from(&NodeIdentity::from_key([7u8; 32]), &CHANNEL);
        hello.protocol = 4;
        let (door, lines) = noting(&[CHANNEL.dialer]);
        let (verdict, answered) = presented_on_link(hello, CHANNEL, Some(&door)).await;
        let refused = verdict.expect_err("a HELLO of protocol 4 taken");
        let why = format!("HELLO refused: protocol 4, not {PROTOCOL}");
        assert_eq!(refused.to_string(), why);
        assert!(!answered, "a HELLO of protocol 4 answered");
        let tag = crate::transport::tag(&CHANNEL.dialer);
        let line = format!("peer refused: endpoint {tag}: {why}");
        assert_eq!(*lines.lock().unwrap(), [line]);
    }

    /// Plan Step 4.5b: a link whose far end has no 32-byte endpoint key, as
    /// the fakes' links have none, or whose transport binds no session,
    /// cannot HELLO, at either end.
    #[tokio::test]
    async fn a_link_without_an_endpoint_key_or_a_binding_cannot_hello() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);
        // An 8-byte key, and this end's own key those 8 bytes and zeros.
        let padded = |byte: u8| std::array::from_fn(|at| if at < 8 { byte } else { 0 });
        let short = pair([vec![1; 8], vec![2; 8]], Some(CHANNEL.exported), 1 << 16);
        let unbound = pair(
            [CHANNEL.dialer.to_vec(), CHANNEL.acceptor.to_vec()],
            None,
            1 << 16,
        );
        let cases = [
            ("an 8-byte key", short, [padded(1), padded(2)]),
            ("no binding", unbound, [CHANNEL.dialer, CHANNEL.acceptor]),
        ];
        for (what, [dialing, accepting], [own_d, own_a]) in cases {
            let (dialed, accepted) = tokio::join!(
                hello_dial_link(&*dialing, &own_d, &dialer, None, HELLO_WITHIN),
                hello_accept_link(&*accepting, &own_a, &acceptor, None, HELLO_WITHIN),
            );
            for (end, verdict) in [("dialer", dialed), ("acceptor", accepted)] {
                let refused = verdict.expect_err(&format!("{what}: the {end} completed HELLO"));
                let kind = refused.kind();
                assert_eq!(kind, io::ErrorKind::PermissionDenied, "{what}: {refused}");
            }
        }
    }

    /// Plan Step 4.5b: HELLO on a link has a bound. With 200 ms, an acceptor
    /// whose dialer never says HELLO, and a dialer whose acceptor never
    /// answers, are each refused at the bound, and the acceptor's door
    /// reports it by the dialer's tag.
    #[tokio::test]
    async fn hello_on_a_link_is_bounded() {
        let bound = Duration::from_millis(200);
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);
        let (door, lines) = noting(&[CHANNEL.dialer]);

        let [_silent, accepting] = on(&CHANNEL);
        let began = Instant::now();
        let own = &CHANNEL.acceptor;
        let heard = hello_accept_link(&*accepting, own, &acceptor, Some(&door), bound);
        let refused = within(heard)
            .await
            .expect_err("a HELLO never sent was taken");
        assert!(began.elapsed() >= bound, "refused before its bound");
        let why = "HELLO refused: no HELLO within 200 ms";
        assert_eq!(refused.to_string(), why);
        let tag = crate::transport::tag(&CHANNEL.dialer);
        let line = format!("peer refused: endpoint {tag}: {why}");
        assert_eq!(*lines.lock().unwrap(), [line]);

        let [dialing, _mute] = on(&CHANNEL);
        let began = Instant::now();
        let welcomed = hello_dial_link(&*dialing, &CHANNEL.dialer, &dialer, None, bound);
        let refused = within(welcomed)
            .await
            .expect_err("a WELCOME never sent was taken");
        assert!(began.elapsed() >= bound, "refused before its bound");
        assert_eq!(refused.to_string(), "no WELCOME within 200 ms");
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

    /// A puller that knows every node, so D9's rule defers nothing (plan Step
    /// 4.1b's part 2).
    fn anyone(_: &str) -> bool {
        true
    }

    /// The holder the duplex tests serve, as its HELLO would claim it, and a
    /// grant fold that lets it read `sh` (plan Step 4.3).
    fn reader() -> (Holder, crate::grants::PolicyView) {
        let mut policy = crate::grants::Policy::default();
        policy.grant(&"ab".repeat(32), "sh", [READ_SUBSCRIBE.to_string()]);
        let fold = crate::grants::PolicyView::of(Some(policy));
        (Holder::Node([0xab; 32]), fold)
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

        let (holder, grants) = reader();
        let serve = async move { serve_sync(&mut br, &mut bw, &server, &holder, &grants).await };
        let srv = tokio::spawn(serve);
        let out = pull_sync(&mut ar, &mut aw, &mut client, &anyone)
            .await
            .unwrap();
        let sent = srv.await.unwrap().unwrap();

        assert_eq!(sent, 8);
        assert_eq!(out.applied, 8);
        assert!(out.rejected.is_empty());
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 5);
        assert_eq!(client.scan("sh", "g", b"", "b", -1).len(), 3);
    }

    /// Plan Step 4.3: the responder offers `home` and each zone its holder may
    /// read, and leaves every other zone out whole. The holder is granted
    /// `sh`; the store also holds `other`, and `home` with a signed record
    /// (plan Step 4.1b).
    #[tokio::test]
    async fn serve_sync_leaves_out_a_zone_the_claimed_holder_may_not_read() {
        let mut server = Store::open(fresh("grant-srv")).unwrap();
        chained(&mut server, "a", b"", 2);
        let mut prev = None;
        for seq in 0..3 {
            let o = Op {
                share: "other".into(),
                ..op("b", seq, b"", prev.clone(), b"x")
            };
            server.append(o.clone()).unwrap();
            prev = Some(crate::chain::op_hash(&o).to_vec());
        }
        let b = crate::registry::Record::Principal(crate::sysdata::PrincipalRecord {
            principal: "b".into(),
        });
        let home = crate::envelope::testing::sealed([2; 32], b);
        server.append(home.clone()).unwrap();
        let mut client = Store::open(fresh("grant-cli")).unwrap();
        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca);
        let (mut br, mut bw) = split(cb);
        let (holder, grants) = reader();
        let serve = async move { serve_sync(&mut br, &mut bw, &server, &holder, &grants).await };
        let srv = tokio::spawn(serve);
        let out = pull_sync(&mut ar, &mut aw, &mut client, &anyone)
            .await
            .unwrap();
        assert_eq!(
            srv.await.unwrap().unwrap(),
            3,
            "sh's two ops and home's one"
        );
        assert_eq!(out.applied, 3);
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 2);
        let (glade_id, origin) = (&home.glade_id, &home.origin);
        let held = client.scan(HOME, glade_id, b"", origin, -1);
        assert_eq!(held, std::slice::from_ref(&home));
        assert!(
            client.scan("other", "g", b"", "b", -1).is_empty(),
            "a zone its holder may not read"
        );
    }

    /// Plan Step 4.1b's part 2 (D9): a pull takes a `home` record only from a
    /// node the puller knows. The peer serves two signed `home` chains of two
    /// records each, and an app zone; the puller knows one of the two nodes,
    /// so the other's chain is deferred whole and kept nowhere, while the
    /// known node's chain and the app zone land as before.
    #[tokio::test]
    async fn pull_sync_defers_a_home_chain_whose_origin_is_not_known() {
        use crate::registry::{Record, Registry, G_PRINCIPALS};
        use crate::sysdata::PrincipalRecord;
        let mut server = Store::open(fresh("d9-srv")).unwrap();
        chained(&mut server, "a", b"", 2);
        let mut origins = Vec::new();
        for seed in [3, 4] {
            let identity = NodeIdentity::from_key([seed; 32]);
            let origin = crate::transport::hex(&identity.node_id);
            let mut registry = Registry::sealed(identity);
            for name in ["p0", "p1"] {
                let principal = name.into();
                let record = Record::Principal(PrincipalRecord { principal });
                let op = registry.append_returning(record, &origin).unwrap();
                server.append(op).unwrap();
            }
            origins.push(origin);
        }
        let (known, stranger) = (origins[0].clone(), origins[1].clone());
        let mut client = Store::open(fresh("d9-cli")).unwrap();
        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca);
        let (mut br, mut bw) = split(cb);
        let (holder, grants) = reader();
        let serve = async move { serve_sync(&mut br, &mut bw, &server, &holder, &grants).await };
        let srv = tokio::spawn(serve);
        let knows = |origin: &str| origin == known;
        let out = pull_sync(&mut ar, &mut aw, &mut client, &knows)
            .await
            .unwrap();
        srv.await.unwrap().unwrap();

        let deferred = (
            HOME.to_string(),
            G_PRINCIPALS.to_string(),
            vec![],
            stranger.clone(),
        );
        assert_eq!(out.deferred, [deferred]);
        assert_eq!(
            out.applied, 4,
            "the app zone's two and the known node's two"
        );
        assert!(out.rejected.is_empty());
        assert_eq!(client.scan(HOME, G_PRINCIPALS, &[], &known, -1).len(), 2);
        assert_eq!(client.scan(HOME, G_PRINCIPALS, &[], &stranger, -1), []);
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
        let out = pull_sync(&mut ar, &mut aw, &mut client, &anyone)
            .await
            .unwrap();
        carrier.await.unwrap();

        assert_eq!(out.applied, 3); // 0,1,2 landed; 3 broke, 4 (suffix) dropped
        assert_eq!(out.rejected, vec![("sh".into(), "g".into(), vec![], "a".into())]);
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 3);

        // Re-fetch from the honest replica: resume from head 2, gain 3 and 4.
        let (ca2, cb2) = tokio::io::duplex(64 * 1024);
        let (mut ar2, mut aw2) = split(ca2);
        let (mut br2, mut bw2) = split(cb2);
        let (holder, grants) = reader();
        let serve = async move { serve_sync(&mut br2, &mut bw2, &truth, &holder, &grants).await };
        let srv = tokio::spawn(serve);
        let out2 = pull_sync(&mut ar2, &mut aw2, &mut client, &anyone)
            .await
            .unwrap();
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
        let out = pull_sync(&mut ar, &mut aw, &mut client, &anyone)
            .await
            .unwrap();
        carrier.await.unwrap();

        assert_eq!(out.applied, 0);
        assert_eq!(out.equivocations.len(), 1);
        assert_eq!(out.equivocations[0].a.payload, b"A");
        assert_eq!(out.equivocations[0].b.payload, b"B");
        assert_eq!(client.equivocation_proofs().len(), 1); // persisted in the store
    }
}
