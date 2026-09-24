//! iroh QUIC carrier for the node<->node link (Lane R step 2).
//!
//! The client-facing WS carrier (`ws.rs`) is untouched: iroh rides ONLY the peer
//! path. A `PeerEndpoint` binds a localhost QUIC endpoint (relay + discovery
//! disabled — `presets::Minimal`, direct dial by socket address only), dials a
//! peer (the s-sync DIAL), runs the `peer::hello_*` seam over a bidirectional
//! stream, and hands back a `PeerLink` whose framed streams the sync driver
//! then speaks over — the SAME `Frame` bytes the websocket carries.
//!
//! The iroh key is transport-only. The glade identity is the node key, whose
//! Ed25519 public key is the node id (plan Step 4.1a), and each HELLO is signed
//! for the connection it rides: this module reads both endpoint ids and 32
//! bytes exported from the connection's TLS session, and hands them to
//! `peer::hello_*` as the [`Channel`]. A booted node's endpoint key is its
//! `endpoint.key`, the same at every start (plan Step 4.2), which a record in
//! its chain binds to the node (`transport.rs`). A booted node's endpoint has a
//! door (plan Step 4.2b): an accept hook refuses an endpoint key the door does
//! not know, and HELLO refuses a node not bound to its connection's key.

use std::future::{ready, Future};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use iroh::endpoint::presets;
use iroh::endpoint::{AfterHandshakeOutcome, EndpointHooks, Side, VarInt};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr};

use crate::peer::{hello_accept, hello_dial, Channel, NodeIdentity, PeerHello};
use crate::transport::{Door, EndpointKey};

/// ALPN for the glade node<->node protocol 2 (`peer::PROTOCOL`, plan Step
/// 4.1a), whose HELLO is signed: a node of protocol 1 fails at connect, not
/// mid-sync.
pub const ALPN: &[u8] = b"glade/node/2";

fn other<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// The exporter label the HELLO's keying material is drawn under (RFC 8446
/// §7.5), with no context.
const HELLO_EXPORTER: &[u8] = b"glade/v1/peer-hello";

/// What both ends of `conn` know without sending it: the endpoint ids, and 32
/// bytes exported from its TLS session, the same at both ends.
fn channel(conn: &Connection, dialer: EndpointId, acceptor: EndpointId) -> io::Result<Channel> {
    let mut exported = [0u8; 32];
    conn.export_keying_material(&mut exported, HELLO_EXPORTER, b"")
        .map_err(|_| other("the TLS session exported no keying material"))?;
    Ok(Channel {
        dialer: *dialer.as_bytes(),
        acceptor: *acceptor.as_bytes(),
        exported,
    })
}

/// The one endpoint recipe every constructor shares: localhost,
/// `presets::Minimal` (relay + discovery disabled), the glade ALPN, the
/// endpoint key `key`, and the accept hook of `door`, if any.
async fn bind_endpoint(key: EndpointKey, door: Option<Arc<Door>>) -> io::Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&key.seed()))
        .alpns(vec![ALPN.to_vec()]);
    if let Some(door) = door {
        builder = builder.hooks(DoorHook(door));
    }
    builder
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .map_err(other)?
        .bind()
        .await
        .map_err(other)
}

/// The door's accept-time half (plan Step 4.2b), in iroh's one hook after
/// TLS, which knows the far end's endpoint key: an inbound connection whose
/// key the door refuses is closed with code 0 and no reason, before any
/// stream opens, and the refusal is reported here. An outbound one waits for
/// HELLO. The hook holds the door, never the endpoint: that would be a cycle.
#[derive(Debug)]
struct DoorHook(Arc<Door>);

impl EndpointHooks for DoorHook {
    fn after_handshake<'a>(
        &'a self,
        conn: &'a Connection,
    ) -> impl Future<Output = AfterHandshakeOutcome> + Send + 'a {
        let key = *conn.remote_id().as_bytes();
        let verdict = match conn.side() {
            Side::Client => Ok(()),
            Side::Server => self.0.admits(&key),
        };
        ready(match verdict {
            Ok(()) => AfterHandshakeOutcome::Accept,
            Err(why) => {
                self.0.refused(&key, &why);
                let error_code = VarInt::from_u32(0);
                AfterHandshakeOutcome::Reject {
                    error_code,
                    reason: Vec::new(),
                }
            }
        })
    }
}

/// A dialable address for a peer: its endpoint id + a direct socket address
/// (localhost, no relay). Enough for `Endpoint::connect` with discovery off.
#[derive(Clone, Copy, Debug)]
pub struct PeerAddr {
    pub endpoint_id: EndpointId,
    pub socket: SocketAddr,
}

impl PeerAddr {
    /// Parse a `--peer` target, `<endpoint-id-hex>@<ip:port>`: the two values
    /// a node prints as `peer <id> <addr>`.
    pub fn parse(s: &str) -> Option<PeerAddr> {
        let (id, sock) = s.split_once('@')?;
        Some(PeerAddr {
            endpoint_id: id.parse().ok()?,
            socket: sock.parse().ok()?,
        })
    }
}

/// A `--peer` entry (plan Step 4.2b): `<endpoint-id>`, a key the door admits
/// on first contact and nothing dials, or `<endpoint-id>@<ip:port>`, which is
/// dialed as well. Either way the operator has configured the key.
#[derive(Clone, Copy, Debug)]
pub enum PeerEntry {
    Known(EndpointId),
    Dial(PeerAddr),
}

impl PeerEntry {
    pub fn parse(s: &str) -> Option<PeerEntry> {
        if s.contains('@') {
            return PeerAddr::parse(s).map(PeerEntry::Dial);
        }
        s.parse().ok().map(PeerEntry::Known)
    }

    /// The endpoint key the entry configures.
    pub fn key(&self) -> [u8; 32] {
        match self {
            PeerEntry::Known(id) => *id.as_bytes(),
            PeerEntry::Dial(addr) => *addr.endpoint_id.as_bytes(),
        }
    }
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
    door: Option<Arc<Door>>,
}

impl PeerEndpoint {
    /// Bind a localhost QUIC endpoint (relay + discovery disabled) with a fresh
    /// random glade identity, which dies with it: for tests, which boot no
    /// instance. A booted node binds with its own ([`PeerEndpoint::bind_with`]).
    pub async fn bind() -> io::Result<PeerEndpoint> {
        let identity = NodeIdentity::generate()?;
        PeerEndpoint::bind_with(identity).await
    }

    /// Bind with an EXPLICIT glade identity and a fresh endpoint key, which
    /// dies with the endpoint: for tests and the async witness, which boot no
    /// instance. A booted node binds with its own key ([`PeerEndpoint::bind_as`]).
    /// The iroh key stays transport-only; the glade node_id spoken on the HELLO
    /// seam is the identity the directory's records attribute — which is what
    /// lets a folded `ServeClaim.node` match a live peer link.
    pub async fn bind_with(identity: NodeIdentity) -> io::Result<PeerEndpoint> {
        let key = EndpointKey::from_seed(crate::signing::random_seed()?);
        PeerEndpoint::bind_as(identity, key).await
    }

    /// Bind as a booted node: its identity, from `node.key`
    /// (`sysdir::Boot::identity`), and its endpoint key, `endpoint.key`
    /// (`sysdir::Boot::endpoint_key`), so its endpoint id is the same at every
    /// start (plan Step 4.2) and its binding record names it.
    pub async fn bind_as(identity: NodeIdentity, key: EndpointKey) -> io::Result<PeerEndpoint> {
        let endpoint = bind_endpoint(key, None).await?;
        Ok(PeerEndpoint {
            endpoint,
            identity,
            door: None,
        })
    }

    /// [`PeerEndpoint::bind_as`], behind `door` (plan Step 4.2b): how both
    /// roots bind a booted node, whose door is closed to keys it does not know.
    pub async fn bind_door(
        identity: NodeIdentity,
        key: EndpointKey,
        door: Arc<Door>,
    ) -> io::Result<PeerEndpoint> {
        let endpoint = bind_endpoint(key, Some(door.clone())).await?;
        Ok(PeerEndpoint {
            endpoint,
            identity,
            door: Some(door),
        })
    }

    pub fn identity(&self) -> &NodeIdentity {
        &self.identity
    }

    /// The door this endpoint was bound behind, if any: the mesh feeds it.
    pub fn door(&self) -> Option<Arc<Door>> {
        self.door.clone()
    }

    /// Report a refused HELLO, naming the endpoint key it came from.
    fn report(&self, endpoint: &[u8; 32], e: &io::Error) {
        if let (Some(door), io::ErrorKind::PermissionDenied) = (&self.door, e.kind()) {
            door.refused(endpoint, e);
        }
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
        let channel = channel(&conn, self.endpoint.id(), conn.remote_id())?;
        let (mut send, mut recv) = conn.open_bi().await.map_err(other)?;
        let door = self.door.as_deref();
        let peer = hello_dial(&mut recv, &mut send, &self.identity, &channel, door).await?;
        Ok(PeerLink { peer, conn, send, recv })
    }

    /// Accept one inbound peer connection and run the HELLO seam. Returns
    /// `Ok(None)` when the endpoint is closed.
    pub async fn accept(&self) -> io::Result<Option<PeerLink>> {
        let Some(incoming) = self.endpoint.accept().await else { return Ok(None) };
        let conn = incoming.accept().map_err(other)?.await.map_err(other)?;
        let channel = channel(&conn, conn.remote_id(), self.endpoint.id())?;
        let (mut send, mut recv) = conn.accept_bi().await.map_err(other)?;
        let door = self.door.as_deref();
        let peer = hello_accept(&mut recv, &mut send, &self.identity, &channel, door).await;
        let peer = peer.inspect_err(|e| self.report(&channel.dialer, e))?;
        Ok(Some(PeerLink { peer, conn, send, recv }))
    }

    /// Close the endpoint gracefully and give up this handle.
    ///
    /// Dropping the last handle also closes the endpoint, but abruptly: a peer
    /// sees its connection time out even when every byte arrived. `close` AWAITS
    /// iroh's own drain (about three seconds on a bad link, usually far less), so
    /// each open connection is told it is over. It consumes the handle because
    /// iroh frees the UDP socket only when EVERY clone is gone: once `close`
    /// resolves, `accept` on a remaining clone answers `Ok(None)`, the accept
    /// loop ends and drops its clone, and iroh's driver task then releases the
    /// port a few milliseconds later. iroh gives no signal for that moment, so a
    /// caller that must bind the same port again has to wait for it. A clone that
    /// never goes away keeps the port bound, which is how a leaked handle shows up.
    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::{pull_sync, serve_sync};
    use crate::store::Store;
    use glade_wire::generated::{Op, Shape};

    /// Plan Step 4.2b: a `--peer` entry is an endpoint id, perhaps with an
    /// address. Without one it only configures the door; with one it is
    /// dialed too; anything else is no entry.
    #[test]
    fn a_peer_entry_names_a_key_and_perhaps_where_to_dial_it() {
        let key = EndpointKey::from_seed([5; 32]).endpoint_id;
        let id = crate::transport::hex(&key);
        let known = PeerEntry::parse(&id);
        assert!(matches!(known, Some(PeerEntry::Known(_))), "{known:?}");
        let dialed = PeerEntry::parse(&format!("{id}@127.0.0.1:4711"));
        assert!(matches!(dialed, Some(PeerEntry::Dial(at)) if at.socket.port() == 4711));
        let keys = (known.map(|e| e.key()), dialed.map(|e| e.key()));
        assert_eq!(keys, (Some(key), Some(key)));
        for junk in ["", "nope", &format!("{id}@"), "@127.0.0.1:1", &id[1..]] {
            assert!(PeerEntry::parse(junk).is_none(), "{junk:?}");
        }
    }

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

        // Plan Step 4.1a's premise: both ends of one connection export the
        // same bytes, and a second connection between the same two endpoints
        // exports other bytes, which is what refuses a replayed HELLO.
        let acc_ep = acceptor.clone();
        let again = tokio::spawn(async move { acc_ep.accept().await });
        let second = dialer.dial(&acc_addr).await.unwrap();
        let _served_again = again.await.unwrap().unwrap().unwrap();
        let (me, it) = (dialer.endpoint.id(), acceptor.endpoint.id());
        let at_dialer = channel(&link.conn, me, link.conn.remote_id()).unwrap();
        let at_acceptor = channel(&served.conn, served.conn.remote_id(), it).unwrap();
        let other = channel(&second.conn, me, second.conn.remote_id()).unwrap();
        assert_eq!(at_dialer, at_acceptor, "one connection, one channel");
        assert_ne!(
            at_dialer.exported, other.exported,
            "another connection, other bytes"
        );
    }

    /// Plan Step 4.1a: the ALPN names protocol 2, so a node of protocol 1, an
    /// endpoint offering only `glade/node/1`, fails at connect in either
    /// direction, before any HELLO is sent.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_protocol_1_node_fails_at_connect() {
        let bound = std::time::Duration::from_secs(10);
        let v1: &[u8] = b"glade/node/1";
        let old = Endpoint::builder(presets::Minimal)
            .alpns(vec![v1.to_vec()])
            .bind_addr((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .unwrap();
        let old_accepts = old.clone();
        tokio::spawn(async move {
            while let Some(incoming) = old_accepts.accept().await {
                if let Ok(connecting) = incoming.accept() {
                    let _ = connecting.await;
                }
            }
        });
        let new = PeerEndpoint::bind().await.unwrap();
        let new_accepts = new.clone();
        tokio::spawn(async move { new_accepts.accept().await });

        let at_new = new.addr().unwrap();
        let ea = EndpointAddr::from_parts(at_new.endpoint_id, [TransportAddr::Ip(at_new.socket)]);
        let dialed = tokio::time::timeout(bound, old.connect(ea, v1)).await;
        let refused = dialed.expect("bounded").is_err();
        assert!(refused, "a protocol-1 dialer connected");

        let sockets = old.bound_sockets();
        let port = sockets.iter().find(|s| s.is_ipv4()).unwrap().port();
        let socket = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let at_old = PeerAddr {
            endpoint_id: old.id(),
            socket,
        };
        let dialed = tokio::time::timeout(bound, new.dial(&at_old)).await;
        let refused = dialed.expect("bounded").is_err();
        assert!(refused, "it accepted a protocol-2 dialer");
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

    /// iroh gives no signal for "the socket is released": its driver task ends a
    /// few milliseconds AFTER `close` resolves and the last handle drops (measured
    /// at 6 to 10 ms with the whole suite running in parallel), and only then is
    /// the port free. So these tests wait for the release, bounded at two seconds.
    async fn port_is_freed(port: u16) -> bool {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// `close` awaits iroh's drain and gives up the handle, so with no clone left
    /// the UDP socket is released: the recorded port binds again.
    #[tokio::test(flavor = "multi_thread")]
    async fn close_frees_the_bound_port() {
        let endpoint = PeerEndpoint::bind().await.unwrap();
        let port = endpoint.addr().unwrap().socket.port();
        assert!(
            std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_err(),
            "the port is held while the endpoint lives"
        );

        endpoint.close().await;

        assert!(port_is_freed(port).await, "the port is free once the last handle has closed");
    }

    /// After `close` a clone's `accept` answers `Ok(None)`, which is what ends an
    /// accept loop; until that clone is dropped it keeps the port, so a handle
    /// that never goes away shows up as a port that never frees.
    #[tokio::test(flavor = "multi_thread")]
    async fn close_ends_an_accept_loop_and_a_kept_clone_keeps_the_port() {
        let endpoint = PeerEndpoint::bind().await.unwrap();
        let kept = endpoint.clone();
        let port = endpoint.addr().unwrap().socket.port();

        endpoint.close().await;

        assert!(kept.accept().await.unwrap().is_none(), "a closed endpoint accepts nothing");
        assert!(
            std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_err(),
            "a clone that is still alive still holds the port"
        );
        drop(kept);
        assert!(port_is_freed(port).await, "the port is free once every clone is gone");
    }
}
