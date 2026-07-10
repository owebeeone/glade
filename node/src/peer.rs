//! Node<->node session (Lane R step 2): the HELLO seam + heads/gap sync.
//!
//! Carrier-free by construction — everything here runs over any `AsyncRead +
//! AsyncWrite` pair, so the protocol is unit-tested over an in-memory duplex and
//! rides real iroh QUIC (`iroh_carrier.rs`) unchanged. Two layers:
//!
//!   1. **Framed IO** — a `u32`-length prefix around each `Frame` (the exact
//!      framing the WS carrier uses, minus the websocket).
//!   2. **HELLO seam** — peers exchange a node identity (`node_id =
//!      sha256(node key)`, the stubbed-but-structure-real posture from
//!      GladeSystemDataSeamNotes; ed25519 swaps in behind `verify_peer`). The
//!      s-sync DIAL gate: operator chains "verify", but NOTHING downstream trusts
//!      this — sync integrity is end-to-end from origin chains, never the carrier.
//!
//! The heads/gap sync driver (per-(origin, zone) chains, verify-as-ingest,
//! reject-suffix + re-fetch, equivocation proof) lives in the second half.

use std::io;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use glade_wire::generated::{NodeHello, NodeWelcome};

use crate::frame::Frame;

/// Wire protocol version spoken on the peer link.
pub const PROTOCOL: i64 = 1;

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

/// A node's stubbed-but-structure-real identity: `node_id = sha256(key)`
/// (GladeSystemDataSeamNotes). The `key` is a 32-byte node key — today an
/// arbitrary seed (the iroh secret-key public bytes in the carrier); ed25519
/// swaps in behind the same shape without a wire change.
#[derive(Clone, Copy, Debug)]
pub struct NodeIdentity {
    pub key: [u8; 32],
    pub node_id: [u8; 32],
}

impl NodeIdentity {
    /// Derive the identity from a node key: `node_id = sha256(key)`.
    pub fn from_key(key: [u8; 32]) -> Self {
        NodeIdentity { key, node_id: Sha256::digest(key).into() }
    }

    /// The origin/operator signature seam over the handshake. STUBBED: a
    /// domain-separated digest, not a real signature — `verify_peer` accepts
    /// unconditionally today. Real ed25519 over `node_id` drops in here.
    fn stub_sig(&self) -> Vec<u8> {
        let mut h = Sha256::new();
        h.update(b"glade/peer/hello");
        h.update(self.key);
        h.finalize().to_vec()
    }
}

/// The verified peer, as far as the (stubbed) seam vouches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeerHello {
    pub peer_id: [u8; 32],
}

fn peer_id_of(node_id: &[u8]) -> io::Result<[u8; 32]> {
    node_id
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "peer node_id not 32 bytes"))
}

/// The HELLO verification seam. STUBBED: structure is real (we parse the claimed
/// node_id and carry the signature) but the check always accepts — matching the
/// s-sync gate note that sync integrity never depends on this handshake.
fn verify_peer(node_id: &[u8], _sig: &Option<Vec<u8>>) -> io::Result<PeerHello> {
    Ok(PeerHello { peer_id: peer_id_of(node_id)? })
}

/// Dialer side of the node<->node HELLO: send `NodeHello`, await `NodeWelcome`,
/// return the (stubbed-)verified peer identity.
pub async fn hello_dial<R, W>(r: &mut R, w: &mut W, me: &NodeIdentity) -> io::Result<PeerHello>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let hello = NodeHello { node_id: me.node_id.to_vec(), protocol: PROTOCOL, sig: Some(me.stub_sig()) };
    write_frame(w, &Frame::NodeHello(hello)).await?;
    match read_frame(r).await? {
        Frame::NodeWelcome(nw) => verify_peer(&nw.node_id, &nw.sig),
        other => Err(io::Error::new(io::ErrorKind::InvalidData, format!("expected NodeWelcome, got {other:?}"))),
    }
}

/// Acceptor side: await `NodeHello`, reply `NodeWelcome`, return the peer.
pub async fn hello_accept<R, W>(r: &mut R, w: &mut W, me: &NodeIdentity) -> io::Result<PeerHello>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let peer = match read_frame(r).await? {
        Frame::NodeHello(nh) => verify_peer(&nh.node_id, &nh.sig)?,
        other => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("expected NodeHello, got {other:?}"))),
    };
    let welcome = NodeWelcome { node_id: me.node_id.to_vec(), protocol: PROTOCOL, sig: Some(me.stub_sig()) };
    write_frame(w, &Frame::NodeWelcome(welcome)).await?;
    Ok(peer)
}

#[cfg(test)]
mod hello_tests {
    use super::*;
    use tokio::io::split;

    /// node_id is sha256(key) — deterministic, and distinct keys give distinct ids.
    #[test]
    fn node_id_is_sha256_of_key() {
        let a = NodeIdentity::from_key([1u8; 32]);
        let b = NodeIdentity::from_key([1u8; 32]);
        let c = NodeIdentity::from_key([2u8; 32]);
        assert_eq!(a.node_id, b.node_id); // deterministic
        assert_ne!(a.node_id, c.node_id); // key-bound
        assert_eq!(a.node_id.to_vec(), sha2::Sha256::digest([1u8; 32]).to_vec());
    }

    /// The DIAL gate over an in-memory duplex: dialer and acceptor complete the
    /// HELLO and each learns the OTHER's node_id (not its own).
    #[tokio::test]
    async fn hello_handshake_exchanges_identities() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);

        // duplex(a,b): writing a is readable on b. Split each end into (r, w).
        let (a, b) = tokio::io::duplex(4096);
        let (mut ar, mut aw) = split(a);
        let (mut br, mut bw) = split(b);

        let acc = tokio::spawn(async move { hello_accept(&mut br, &mut bw, &acceptor).await });
        let seen_by_dialer = hello_dial(&mut ar, &mut aw, &dialer).await.unwrap();
        let seen_by_acceptor = acc.await.unwrap().unwrap();

        assert_eq!(seen_by_dialer.peer_id, acceptor.node_id, "dialer learns acceptor id");
        assert_eq!(seen_by_acceptor.peer_id, dialer.node_id, "acceptor learns dialer id");
    }
}
