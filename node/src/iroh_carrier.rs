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

/// The one endpoint recipe both constructors share: localhost, `presets::Minimal`
/// (relay + discovery disabled), the glade ALPN.
async fn bind_endpoint() -> io::Result<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .map_err(other)?
        .bind()
        .await
        .map_err(other)
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
        let endpoint = bind_endpoint().await?;
        let key = *endpoint.secret_key().public().as_bytes();
        Ok(PeerEndpoint { endpoint, identity: NodeIdentity::from_key(key) })
    }

    /// Bind with an EXPLICIT glade identity (a booted node passes the identity
    /// derived from its `node.key`, `sysdir::Boot::identity`). The iroh key
    /// stays transport-only; the glade node_id spoken on the HELLO seam is then
    /// the same identity the directory's records attribute — which is what lets
    /// a folded `ServeClaim.node` match a live peer link.
    pub async fn bind_with(identity: NodeIdentity) -> io::Result<PeerEndpoint> {
        let endpoint = bind_endpoint().await?;
        Ok(PeerEndpoint { endpoint, identity })
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
    use crate::peer::{pull_sync, serve_sync};
    use crate::store::Store;
    use glade_wire::generated::{Op, Shape};

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

    /// Full s-sync over REAL iroh QUIC: the acceptor serves a store with a
    /// prev-linked chain; the dialer pulls it over the same HELLO'd connection
    /// and converges, verified per op. Carrier + sync, end to end on localhost.
    #[tokio::test(flavor = "multi_thread")]
    async fn sync_over_iroh() {
        let dir = std::env::temp_dir().join("glade-iroh-sync-srv");
        let _ = std::fs::remove_dir_all(&dir);
        let mut server = Store::open(&dir).unwrap();
        let mut prev = None;
        for seq in 0..4 {
            let o = Op {
                share: "sh".into(), glade_id: "g".into(), key: vec![], origin: "a".into(),
                seq, prev: prev.clone(), lamport: seq, refs: vec![], shape: Shape::Value,
                payload: format!("a{seq}").into_bytes(),
            };
            server.append(o.clone()).unwrap();
            prev = Some(crate::chain::op_hash(&o).to_vec());
        }

        let acceptor = PeerEndpoint::bind().await.unwrap();
        let dialer = PeerEndpoint::bind().await.unwrap();
        let acc_addr = acceptor.addr().unwrap();

        let acc_ep = acceptor.clone();
        let acc = tokio::spawn(async move {
            let mut link = acc_ep.accept().await.unwrap().unwrap();
            let sent = serve_sync(&mut link.recv, &mut link.send, &server).await;
            // Keep `link` (hence the connection) alive until the dialer has read
            // the finished stream — dropping it early would reset the stream.
            (link, sent)
        });

        let cdir = std::env::temp_dir().join("glade-iroh-sync-cli");
        let _ = std::fs::remove_dir_all(&cdir);
        let mut client = Store::open(&cdir).unwrap();
        let mut link = dialer.dial(&acc_addr).await.unwrap();
        let out = pull_sync(&mut link.recv, &mut link.send, &mut client).await.unwrap();
        let (_served, sent) = acc.await.unwrap();
        let sent = sent.unwrap();

        assert_eq!(sent, 4);
        assert_eq!(out.applied, 4);
        assert!(out.rejected.is_empty());
        assert_eq!(client.scan("sh", "g", &[], "a", -1).len(), 4);
    }
}
