//! iroh QUIC carrier for the node<->node link (Lane R step 2).
//!
//! The client-facing WS carrier (`ws.rs`) is untouched: iroh rides ONLY the peer
//! path. A `PeerEndpoint` binds a localhost QUIC endpoint (relay + discovery
//! disabled — `presets::Minimal`, direct dial by socket address only), dials a
//! peer (the s-sync DIAL), runs the `peer::hello_*` seam over a bidirectional
//! stream, and hands back a `PeerLink` whose framed streams the sync driver
//! then speaks over — the SAME `Frame` bytes the websocket carries.
//!
//! The node key is the iroh secret key's public bytes, so the glade
//! `node_id = sha256(iroh pubkey)` — the stubbed-but-structure-real identity
//! (GladeSystemDataSeamNotes); swapping the seam for ed25519 needs no wire change.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use iroh::endpoint::presets;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};

use crate::peer::{hello_accept, hello_dial, NodeIdentity, PeerHello};

/// ALPN for the glade node<->node protocol (protocol 1).
pub const ALPN: &[u8] = b"glade/node/1";

fn other<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// A dialable address for a peer: its endpoint id + a direct socket address
/// (localhost, no relay). Enough for `Endpoint::connect` with discovery off.
#[derive(Clone, Copy, Debug)]
pub struct PeerAddr {
    pub endpoint_id: EndpointId,
    pub socket: SocketAddr,
}

/// An established peer connection after HELLO: the verified peer identity plus
/// the bidirectional stream (kept as split halves for the sync driver).
pub struct PeerLink {
    pub peer: PeerHello,
    pub conn: Connection,
    pub send: SendStream,
    pub recv: RecvStream,
}

/// A bound iroh endpoint that speaks the glade peer protocol.
///
/// `Clone` shares the one underlying iroh endpoint (it is `Arc`-backed). A node
/// owns a `PeerEndpoint` for its whole lifetime; **it MUST outlive every
/// `PeerLink` it produces** — dropping the last handle closes the endpoint and
/// tears down live connections. Clone it into an accept loop rather than moving
/// the sole handle in.
#[derive(Clone)]
pub struct PeerEndpoint {
    endpoint: Endpoint,
    identity: NodeIdentity,
}

impl PeerEndpoint {
    /// Bind a localhost QUIC endpoint (relay + discovery disabled). The glade
    /// identity is derived from the iroh key: `node_id = sha256(iroh pubkey)`.
    pub async fn bind() -> io::Result<PeerEndpoint> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind_addr((Ipv4Addr::LOCALHOST, 0))
            .map_err(other)?
            .bind()
            .await
            .map_err(other)?;
        let key = *endpoint.secret_key().public().as_bytes();
        Ok(PeerEndpoint { endpoint, identity: NodeIdentity::from_key(key) })
    }

    pub fn identity(&self) -> &NodeIdentity {
        &self.identity
    }

    /// This endpoint's dialable address (its id + first IPv4 bound socket).
    pub fn addr(&self) -> io::Result<PeerAddr> {
        let socket = self
            .endpoint
            .bound_sockets()
            .into_iter()
            .find(|s| s.is_ipv4())
            .ok_or_else(|| other("no bound IPv4 socket"))?;
        let socket = SocketAddr::from((Ipv4Addr::LOCALHOST, socket.port()));
        Ok(PeerAddr { endpoint_id: self.endpoint.id(), socket })
    }

    /// Dial a peer (DIAL), open a bidirectional stream, and run the HELLO seam.
    pub async fn dial(&self, addr: &PeerAddr) -> io::Result<PeerLink> {
        let ea = EndpointAddr::from_parts(addr.endpoint_id, [TransportAddr::Ip(addr.socket)]);
        let conn = self.endpoint.connect(ea, ALPN).await.map_err(other)?;
        let (mut send, mut recv) = conn.open_bi().await.map_err(other)?;
        let peer = hello_dial(&mut recv, &mut send, &self.identity).await?;
        Ok(PeerLink { peer, conn, send, recv })
    }

    /// Accept one inbound peer connection and run the HELLO seam. Returns
    /// `Ok(None)` when the endpoint is closed.
    pub async fn accept(&self) -> io::Result<Option<PeerLink>> {
        let Some(incoming) = self.endpoint.accept().await else { return Ok(None) };
        let conn = incoming.accept().map_err(other)?.await.map_err(other)?;
        let (mut send, mut recv) = conn.accept_bi().await.map_err(other)?;
        let peer = hello_accept(&mut recv, &mut send, &self.identity).await?;
        Ok(Some(PeerLink { peer, conn, send, recv }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DIAL over REAL iroh QUIC: dialer binds, acceptor binds, dialer dials
    /// by direct address, both complete the node<->node HELLO and each learns
    /// the other's node_id. No relay, no discovery — pure localhost QUIC.
    #[tokio::test(flavor = "multi_thread")]
    async fn dial_and_hello_over_iroh() {
        let acceptor = PeerEndpoint::bind().await.unwrap();
        let dialer = PeerEndpoint::bind().await.unwrap();
        let acc_id = acceptor.identity().node_id;
        let dial_id = dialer.identity().node_id;
        let acc_addr = acceptor.addr().unwrap();

        // Clone the acceptor into the accept task; the original stays alive here
        // so the endpoint (hence the connection) outlives the link.
        let acc_ep = acceptor.clone();
        let acc = tokio::spawn(async move { acc_ep.accept().await });
        let link = dialer.dial(&acc_addr).await.unwrap();
        assert_eq!(link.peer.peer_id, acc_id, "dialer learns acceptor node_id over iroh");

        let served = acc.await.unwrap().unwrap().unwrap();
        assert_eq!(served.peer.peer_id, dial_id, "acceptor learns dialer node_id over iroh");
    }
}
