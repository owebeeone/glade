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
//! [`IrohCarrier`] is the `CarrierPort` over iroh (plan Step 4.2c), which
//! tracks its links; the mesh still runs on `PeerEndpoint`.
//!
//! Plan Step 4.5: the endpoint's recipe takes the node's network
//! (`netconf.rs`): the sockets it binds and whether it has n0's relays, and
//! nothing else. The portmapper stays off, no address lookup is added, and
//! the node calls none of iroh's helpers that read the environment: it names
//! n0's production relays itself. A dial names every address its entry
//! gives, IP or relay, and iroh chooses among them. Lines name an endpoint by
//! its tag, never its id. For the notes the crossing reads (part 2), this
//! module describes a link's selected path and the home relays' states as
//! text and numbers, so iroh's types stop here.

use std::fmt;
use std::future::{ready, Future};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture, TransportId,
};
use iroh::endpoint::presets;
use iroh::endpoint::RelayStatus;
use iroh::endpoint::{AfterHandshakeOutcome, EndpointHooks, PortmapperConfig, Side, VarInt};
use iroh::endpoint::{Connection, ConnectionError, ReadError, RecvStream, SendStream};
use iroh::Watcher;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr};

use crate::netconf::{Network, PeerEntry, Relays, Via};
use crate::peer::{hello_accept, hello_dial, Channel, NodeIdentity, PeerHello};
use crate::transport::{tag, Door, EndpointKey};

/// ALPN for the glade node<->node protocol 3 (`peer::PROTOCOL`, plan Step
/// 4.1b), whose `home` records are signed envelopes and whose HELLO is signed
/// (4.1a): a node of protocol 1 or 2 fails at connect, not mid-sync.
pub const ALPN: &[u8] = b"glade/node/3";

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

/// The one endpoint recipe every constructor shares (plan Step 4.5):
/// `presets::Minimal` (no relay, no address lookup), the ALPN `alpn`, the
/// endpoint key `key`, the accept hook of `door`, if any, and `network`'s
/// sockets and relays. iroh comes with `0.0.0.0` and `[::]` pre-bound, every
/// interface, so both are cleared first and only `network`'s sockets bind;
/// and its portmapper, which opens a UDP socket on every interface to find
/// the router over UPnP, is off whatever the network says. With relays off
/// no relay call is made. So `Network::default()` gives the endpoint it
/// always had, call for call: loopback alone, no relay, and nothing for
/// macOS's firewall to ask about. No address lookup, proxy or net-report
/// setting is made, and none of iroh's helpers that read the environment is
/// called.
async fn bind_endpoint(
    key: EndpointKey,
    door: Option<Arc<Door>>,
    alpn: &[u8],
    network: &Network,
) -> io::Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&key.seed()))
        .alpns(vec![alpn.to_vec()]);
    if let Some(door) = door {
        builder = builder.hooks(DoorHook(door));
    }
    let mut builder = builder
        .portmapper_config(PortmapperConfig::Disabled)
        .clear_ip_transports();
    for socket in &network.bind {
        builder = builder.bind_addr(*socket).map_err(other)?;
    }
    if network.relays == Relays::N0 {
        builder = builder.relay_mode(relay_mode(network.relays));
    }
    builder.bind().await.map_err(other)
}

/// The relay mode `relays` names: none, or n0's production relays, named
/// here rather than by `default_relay_mode()`, which an environment variable
/// can turn to n0's staging relays.
fn relay_mode(relays: Relays) -> RelayMode {
    match relays {
        Relays::Off => RelayMode::Disabled,
        Relays::N0 => RelayMode::Default,
    }
}

/// Whether `key` is an endpoint id iroh accepts, a point on Ed25519's curve:
/// the configuration's check at load (`netconf.rs`).
pub(crate) fn accepts_endpoint_id(key: &[u8; 32]) -> bool {
    EndpointId::from_bytes(key).is_ok()
}

/// The relay `text` names, as the node prints its URL, if it is one of the
/// relays `relay n0` gives: n0's production map, as iroh defines it. The
/// configuration's check at load (`netconf.rs`).
pub(crate) fn n0_relay(text: &str) -> Option<String> {
    let url: RelayUrl = text.parse().ok()?;
    let n0 = relay_mode(Relays::N0).relay_map();
    n0.contains(&url).then(|| url.to_string())
}

/// Where `entry` is dialed: one iroh address holding its id and every
/// address it names, IP or relay. iroh sends a connection's first packets to
/// every address it knows, and chooses among them.
fn endpoint_addr(entry: &PeerEntry) -> io::Result<EndpointAddr> {
    let id = EndpointId::from_bytes(&entry.key).map_err(other)?;
    let mut addrs = Vec::new();
    for via in &entry.via {
        addrs.push(match via {
            Via::Ip(socket) => TransportAddr::Ip(*socket),
            Via::Relay(url) => TransportAddr::Relay(url.parse().map_err(other)?),
        });
    }
    Ok(EndpointAddr::from_parts(id, addrs))
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

/// An endpoint's dialable address: its id, and a socket as bound, IPv4 first
/// (plan Step 4.5). For the default bind that is `127.0.0.1:<port>`, as it
/// always was.
fn bound_addr(endpoint: &Endpoint) -> io::Result<PeerAddr> {
    let mut sockets = endpoint.bound_sockets();
    sockets.sort_by_key(|socket| !socket.is_ipv4());
    let socket = sockets.first().copied();
    let socket = socket.ok_or_else(|| other("no socket bound"))?;
    Ok(PeerAddr {
        endpoint_id: endpoint.id(),
        socket,
    })
}

/// A dialable address for a peer: its endpoint id + a direct socket address.
/// Enough for `Endpoint::connect` with no address lookup.
#[derive(Clone, Copy, Debug)]
pub struct PeerAddr {
    pub endpoint_id: EndpointId,
    pub socket: SocketAddr,
}

impl PeerAddr {
    /// Parse `<endpoint-id-hex>@<ip:port>`, a carrier address.
    pub fn parse(s: &str) -> Option<PeerAddr> {
        let (id, sock) = s.split_once('@')?;
        Some(PeerAddr {
            endpoint_id: id.parse().ok()?,
            socket: sock.parse().ok()?,
        })
    }

    /// The endpoint's tag, which the `peer` line prints for its id.
    pub fn tag(&self) -> String {
        tag(self.endpoint_id.as_bytes())
    }
}

/// A peer's own address as a dial target: its key, dialed at that one
/// socket.
impl From<&PeerAddr> for PeerEntry {
    fn from(addr: &PeerAddr) -> PeerEntry {
        PeerEntry {
            key: *addr.endpoint_id.as_bytes(),
            via: vec![Via::Ip(addr.socket)],
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
    /// Whether the endpoint was bound with relays (plan Step 4.5).
    relays: Relays,
}

impl PeerEndpoint {
    /// Bind a localhost QUIC endpoint (no relay, no address lookup) with a
    /// fresh random glade identity, which dies with it: for tests, which boot
    /// no instance. A booted node binds with its own
    /// ([`PeerEndpoint::bind_with`]).
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
    /// It binds the default network, `127.0.0.1:0` alone.
    pub async fn bind_as(identity: NodeIdentity, key: EndpointKey) -> io::Result<PeerEndpoint> {
        let endpoint = bind_endpoint(key, None, ALPN, &Network::default()).await?;
        Ok(PeerEndpoint {
            endpoint,
            identity,
            door: None,
            relays: Relays::Off,
        })
    }

    /// [`PeerEndpoint::bind_as`], behind `door` (plan Step 4.2b) and on
    /// `network` (plan Step 4.5): how both roots bind a booted node, whose
    /// door is closed to keys it does not know.
    pub async fn bind_door(
        identity: NodeIdentity,
        key: EndpointKey,
        door: Arc<Door>,
        network: &Network,
    ) -> io::Result<PeerEndpoint> {
        let endpoint = bind_endpoint(key, Some(door.clone()), ALPN, network).await?;
        Ok(PeerEndpoint {
            endpoint,
            identity,
            door: Some(door),
            relays: network.relays,
        })
    }

    pub fn identity(&self) -> &NodeIdentity {
        &self.identity
    }

    /// The door this endpoint was bound behind, if any: the mesh feeds it.
    pub fn door(&self) -> Option<Arc<Door>> {
        self.door.clone()
    }

    /// Whether the endpoint was bound with n0's relays, whose state the mesh
    /// then watches (plan Step 4.5).
    pub(crate) fn relays(&self) -> Relays {
        self.relays
    }

    /// Watch this endpoint's home relays until it closes: `seen` has their
    /// states now, and again at each change (plan Step 4.5). iroh's status
    /// becomes text and flags here, and the future holds no handle on the
    /// endpoint, so it keeps no socket bound.
    pub(crate) fn relay_watch(
        &self,
        mut seen: impl FnMut(Vec<RelayState>) + Send + 'static,
    ) -> impl Future<Output = ()> + Send + 'static {
        let mut statuses = self.endpoint.home_relay_status();
        let closed = self.endpoint.closed();
        async move {
            let states = |held: &[RelayStatus]| held.iter().map(RelayState::of).collect();
            seen(states(&statuses.get()));
            let mut closed = std::pin::pin!(closed);
            loop {
                tokio::select! {
                    () = &mut closed => {
                        return;
                    }
                    changed = statuses.updated() => {
                        let Ok(now) = changed else {
                            return;
                        };
                        seen(states(&now));
                    }
                }
            }
        }
    }

    /// Report a refused HELLO, naming the endpoint key it came from.
    fn report(&self, endpoint: &[u8; 32], e: &io::Error) {
        if let (Some(door), io::ErrorKind::PermissionDenied) = (&self.door, e.kind()) {
            door.refused(endpoint, e);
        }
    }

    /// This endpoint's dialable address: its id, and a socket as bound, IPv4
    /// first.
    pub fn addr(&self) -> io::Result<PeerAddr> {
        bound_addr(&self.endpoint)
    }

    /// Dial a peer (DIAL) at every address `target` names, open a
    /// bidirectional stream, and run the HELLO seam.
    pub async fn dial(&self, target: impl Into<PeerEntry>) -> io::Result<PeerLink> {
        let ea = endpoint_addr(&target.into())?;
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

// ---- the notes the crossing reads (plan Step 4.5, part 2) -------------------

/// The path a link sends on, as its `link` line reads it: where it goes,
/// `relay <url>` or `direct <ip:port>`, and iroh's round-trip estimate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathSeen {
    pub(crate) via: String,
    pub(crate) rtt_ms: u128,
}

/// Where a path to `addr` goes, in a line's words.
fn described(addr: &TransportAddr) -> String {
    match addr {
        TransportAddr::Relay(url) => format!("relay {url}"),
        TransportAddr::Ip(socket) => format!("direct {socket}"),
        other => format!("another transport, {other:?}"),
    }
}

/// The path iroh has selected for `conn`'s data, if it has selected one.
pub(crate) fn selected_path(conn: &Connection) -> Option<PathSeen> {
    let paths = conn.paths();
    let selected = paths.iter().find(|path| path.is_selected())?;
    let via = described(selected.remote_addr());
    let rtt_ms = selected.rtt().as_millis();
    Some(PathSeen { via, rtt_ms })
}

/// A home relay's state, as the `relay` lines read it: its URL as the node
/// prints it, whether the endpoint is connected to it, and while it is not,
/// the last error, if one has been seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RelayState {
    pub(crate) url: String,
    pub(crate) connected: bool,
    pub(crate) error: Option<String>,
}

impl RelayState {
    fn of(status: &RelayStatus) -> RelayState {
        RelayState {
            url: status.url().to_string(),
            connected: status.is_connected(),
            error: status.last_error().map(|e| e.to_string()),
        }
    }
}

// ---- the CarrierPort adapter (plan Step 4.2c) --------------------------------

/// The adapter's ALPN, apart from the node's: its links carry opaque frames,
/// not the node protocol, so a node's endpoint and an adapter never connect.
pub const CARRIER_ALPN: &[u8] = b"glade/carrier/1";

/// What a dialer sends first, so that its acceptor sees the link at once:
/// QUIC shows a stream to its peer only when bytes cross it.
const PREAMBLE: &[u8; 4] = b"gcl1";

/// How long a close waits for the peer to acknowledge what was sent: iroh's
/// own bound for a drain on a bad link.
const LINGER: Duration = Duration::from_secs(3);

fn transport(e: impl fmt::Display) -> CarrierError {
    CarrierError::Transport(e.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

type Link = Box<dyn CarrierLink>;

/// `glade_carrier_api::CarrierPort` over iroh (plan Step 4.2c): one endpoint,
/// bound with the key the port was lent, and a QUIC connection per link, of
/// frames that are a `u32` little-endian length and the bytes. The address is
/// `<endpoint-id>@<ip:port>`; `remote_id` is the id the TLS session proved.
/// `close` ends every link the port tracks and takes their handles, since a
/// surviving connection keeps the port bound (the async witness's finding).
/// A link holds the endpoint too, so a port dropped without `close` does not
/// take its links' transport with it. It binds the socket
/// `CarrierConfig::local` names (plan Step 4.5), with no relay, has no door,
/// and lent no key refuses to bind.
pub struct IrohCarrier {
    key: Option<EndpointKey>,
    state: Mutex<PortState>,
}

enum PortState {
    Unbound,
    /// The endpoint, the frame limit, and every link the port has made.
    Bound {
        ep: Endpoint,
        max: usize,
        links: Vec<Weak<LinkState>>,
    },
    Closed,
}

/// Why a port in `state` cannot bind: it is bound or closed already.
fn refusal(state: &PortState) -> Option<CarrierError> {
    match state {
        PortState::Unbound => None,
        PortState::Bound { .. } => Some(CarrierError::AlreadyBound),
        PortState::Closed => Some(CarrierError::Closed),
    }
}

impl IrohCarrier {
    /// A port that binds with `key`; lent none, it refuses to.
    pub fn new(key: Option<EndpointKey>) -> IrohCarrier {
        let state = Mutex::new(PortState::Unbound);
        IrohCarrier { key, state }
    }

    /// Keep `endpoint` as this port's, unless a bind or a close came first.
    fn place(&self, endpoint: &Endpoint, max: usize) -> Result<(), CarrierError> {
        let mut state = lock(&self.state);
        if let Some(refused) = refusal(&state) {
            return Err(refused);
        }
        let (ep, links) = (endpoint.clone(), Vec::new());
        *state = PortState::Bound { ep, max, links };
        Ok(())
    }

    /// The bound endpoint: none before `bind` and after `close`.
    fn endpoint(&self) -> Option<Endpoint> {
        match &*lock(&self.state) {
            PortState::Bound { ep, .. } => Some(ep.clone()),
            _ => None,
        }
    }

    /// Track a new link; none, dropping it, once the port has closed.
    fn track(&self, conn: Connection, send: SendStream, recv: RecvStream) -> Option<Link> {
        let mut state = lock(&self.state);
        let PortState::Bound { ep, max, links } = &mut *state else {
            return None;
        };
        let remote = TransportId(conn.remote_id().as_bytes().to_vec());
        let send = Some(Sending {
            stream: send,
            torn: false,
        });
        let recv = Some(Receiving {
            stream: recv,
            buf: Vec::new(),
        });
        let link = Arc::new(LinkState {
            remote,
            max: *max,
            ended: AtomicBool::new(false),
            held: Mutex::new(Some((conn, ep.clone()))),
            send: tokio::sync::Mutex::new(send),
            recv: tokio::sync::Mutex::new(recv),
        });
        links.retain(|link| link.strong_count() > 0);
        links.push(Arc::downgrade(&link));
        Some(Box::new(IrohLink(link)))
    }

    async fn accept_on(&self, endpoint: &Endpoint) -> Result<Option<Link>, CarrierError> {
        let Some(incoming) = endpoint.accept().await else {
            return Ok(None);
        };
        let accepting = incoming.accept().map_err(transport)?;
        let conn = accepting.await.map_err(transport)?;
        let (send, mut recv) = conn.accept_bi().await.map_err(transport)?;
        let mut preamble = [0; 4];
        recv.read_exact(&mut preamble).await.map_err(transport)?;
        if &preamble != PREAMBLE {
            return Err(transport("not a carrier link"));
        }
        Ok(self.track(conn, send, recv))
    }
}

/// The socket a carrier address names, `<ip:port>` or
/// `<endpoint-id>@<ip:port>`: the address says where, and the key a port is
/// lent says who, so an id is ignored (plan Step 4.5).
fn local_socket(local: &CarrierAddr) -> Option<SocketAddr> {
    let at = match local.0.rsplit_once('@') {
        Some((_, at)) => at,
        None => &local.0,
    };
    at.parse().ok()
}

impl CarrierPort for IrohCarrier {
    fn bind(&self, config: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>> {
        Box::pin(async move {
            if let Some(refused) = refusal(&lock(&self.state)) {
                return Err(refused);
            }
            let unkeyed = || transport("the iroh adapter was lent no endpoint key");
            let key = self.key.ok_or_else(unkeyed)?;
            let nowhere = || transport("expected <ip:port> or <endpoint-id>@<ip:port> to bind");
            let socket = local_socket(&config.local).ok_or_else(nowhere)?;
            let network = Network {
                bind: vec![socket],
                ..Network::default()
            };
            let endpoint = bind_endpoint(key, None, CARRIER_ALPN, &network).await;
            let endpoint = endpoint.map_err(transport)?;
            let at = bound_addr(&endpoint).map_err(transport)?;
            if let Err(refused) = self.place(&endpoint, config.max_frame_bytes.get()) {
                endpoint.close().await;
                return Err(refused);
            }
            Ok(CarrierAddr(format!("{}@{}", at.endpoint_id, at.socket)))
        })
    }

    fn dial<'a>(&'a self, peer: &'a CarrierAddr) -> PortFuture<'a, Result<Link, CarrierError>> {
        Box::pin(async move {
            let endpoint = self.endpoint().ok_or(CarrierError::Closed)?;
            let malformed = || transport(format!("{}: expected <endpoint-id>@<ip:port>", peer.0));
            let at = PeerAddr::parse(&peer.0).ok_or_else(malformed)?;
            let at = EndpointAddr::from_parts(at.endpoint_id, [TransportAddr::Ip(at.socket)]);
            let conn = endpoint.connect(at, CARRIER_ALPN).await;
            let conn = conn.map_err(transport)?;
            let (mut send, recv) = conn.open_bi().await.map_err(transport)?;
            send.write_all(PREAMBLE).await.map_err(transport)?;
            self.track(conn, send, recv).ok_or(CarrierError::Closed)
        })
    }

    fn accept(&self) -> PortFuture<'_, Result<Option<Link>, CarrierError>> {
        Box::pin(async move {
            let Some(endpoint) = self.endpoint() else {
                return Ok(None);
            };
            match self.accept_on(&endpoint).await {
                // A close that cuts a handshake short ends the accept too.
                Err(_) if self.endpoint().is_none() => Ok(None),
                accepted => accepted,
            }
        })
    }

    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            // Out of the port at the first poll, the links' handles too: a
            // close dropped part-way gives up the drain, never the handles.
            let taken = std::mem::replace(&mut *lock(&self.state), PortState::Closed);
            let PortState::Bound { ep, links, .. } = taken else {
                return;
            };
            let live = links.iter().filter_map(Weak::upgrade);
            let mut ended: Vec<Ended> = live.map(|link| link.end()).collect();
            let drained = async {
                for link in &mut ended {
                    link.drain().await;
                }
            };
            let _ = tokio::time::timeout(LINGER, drained).await;
            drop(ended);
            let bound = ep.bound_sockets();
            ep.close().await;
            drop(ep);
            released(&bound).await;
        })
    }
}

/// Wait, within `LINGER`, until each socket in `bound` can be bound again
/// (plan Step 4.5). iroh lets a closed endpoint's sockets go a few
/// milliseconds after its last handle drops, and gives no signal when it
/// has, so a port whose `close` frees its address by value (CA-004), now
/// that it binds where `CarrierConfig::local` says, waits for it: binding
/// each address itself is the only witness.
async fn released(bound: &[SocketAddr]) {
    let deadline = tokio::time::Instant::now() + LINGER;
    for socket in bound {
        while std::net::UdpSocket::bind(socket).is_err() {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// One link, shared by its [`IrohLink`] and, weakly, its port. Each half has
/// its own lock, held across awaits, so a send and a receive run together.
struct LinkState {
    remote: TransportId,
    max: usize,
    ended: AtomicBool,
    /// The connection, and the endpoint it rides, which lives as long.
    held: Mutex<Option<(Connection, Endpoint)>>,
    send: tokio::sync::Mutex<Option<Sending>>,
    recv: tokio::sync::Mutex<Option<Receiving>>,
}

struct Sending {
    stream: SendStream,
    /// A send is under way, or was dropped part-way: its frame may be torn.
    torn: bool,
}

struct Receiving {
    stream: RecvStream,
    /// What has arrived of the next frame: a receive dropped part-way loses
    /// none of it.
    buf: Vec<u8>,
}

impl LinkState {
    fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// End the link: from now on `send` answers `Closed` and `recv`
    /// `Ok(None)`. What it holds comes out by value; a half an operation
    /// holds is given up when the operation lets it go.
    fn end(&self) -> Ended {
        self.ended.store(true, Ordering::SeqCst);
        if let Ok(mut half) = self.recv.try_lock() {
            half.take();
        }
        let sending = self.send.try_lock().ok().and_then(|mut h| h.take());
        let held = lock(&self.held).take();
        Ended { sending, held }
    }

    async fn hold<'a, T>(&'a self, half: &'a tokio::sync::Mutex<Option<T>>) -> Held<'a, T> {
        let (half, ended) = (half.lock().await, &self.ended);
        Held { half, ended }
    }
}

/// What an ended link held: its send half, to drain, and its connection,
/// closed (code 0, no reason) when this drops.
struct Ended {
    sending: Option<Sending>,
    held: Option<(Connection, Endpoint)>,
}

impl Ended {
    /// Finish sending and wait until the peer has it all: what was sent
    /// reaches the peer before its end of stream.
    async fn drain(&mut self) {
        if let Some(half) = self.sending.as_mut().filter(|half| !half.torn) {
            if half.stream.finish().is_ok() {
                let _ = half.stream.stopped().await;
            }
        }
    }
}

impl Drop for Ended {
    fn drop(&mut self) {
        if let Some((conn, _endpoint)) = self.held.take() {
            conn.close(0u32.into(), b"");
        }
    }
}

/// A half, held by one operation; let go once its link has ended, it gives
/// the half up, so that no handle outlives the end.
struct Held<'a, T> {
    half: tokio::sync::MutexGuard<'a, Option<T>>,
    ended: &'a AtomicBool,
}

impl<T> Held<'_, T> {
    /// The half, unless the link has ended.
    fn live(&mut self) -> Option<&mut T> {
        let ended = self.ended.load(Ordering::SeqCst);
        self.half.as_mut().filter(|_| !ended)
    }
}

impl<T> Drop for Held<'_, T> {
    fn drop(&mut self) {
        if self.ended.load(Ordering::SeqCst) {
            self.half.take();
        }
    }
}

/// The next whole frame in `buf`, taken out of it: none until one has
/// arrived, and `FrameTooLarge` for a length over `max`, before its body.
fn next_frame(buf: &mut Vec<u8>, max: usize) -> Option<Result<Vec<u8>, CarrierError>> {
    let head: [u8; 4] = buf.get(..4)?.try_into().ok()?;
    let len = u32::from_le_bytes(head) as usize;
    if len > max {
        return Some(Err(CarrierError::FrameTooLarge));
    }
    let frame = buf.get(4..4 + len)?.to_vec();
    buf.drain(..4 + len);
    Some(Ok(frame))
}

/// One link of an [`IrohCarrier`].
struct IrohLink(Arc<LinkState>);

impl CarrierLink for IrohLink {
    fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
        Box::pin(async move {
            let link = &self.0;
            let mut held = link.hold(&link.send).await;
            let Some(half) = held.live() else {
                return Err(CarrierError::Closed);
            };
            let fits = frame.len() <= link.max;
            let len = u32::try_from(frame.len()).ok().filter(|_| fits);
            let len = len.ok_or(CarrierError::FrameTooLarge)?;
            if half.torn {
                // Rather than follow a frame that may be torn, the link ends.
                drop(link.end());
                return Err(CarrierError::Closed);
            }
            half.torn = true;
            let bytes = [&len.to_le_bytes()[..], frame].concat();
            let written = half.stream.write_all(&bytes).await;
            half.torn = written.is_err();
            match written {
                Ok(()) => Ok(()),
                Err(_) if link.ended() => Err(CarrierError::Closed),
                Err(e) => Err(transport(e)).inspect_err(|_| drop(link.end())),
            }
        })
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
        Box::pin(async move {
            let link = &self.0;
            let mut held = link.hold(&link.recv).await;
            loop {
                let Some(half) = held.live() else {
                    return Ok(None);
                };
                if let Some(frame) = next_frame(&mut half.buf, link.max) {
                    return frame.map(Some).inspect_err(|_| drop(link.end()));
                }
                let mut chunk = [0; 4096];
                let read = half.stream.read(&mut chunk).await;
                let between_frames = half.buf.is_empty();
                match read {
                    Ok(Some(n)) => half.buf.extend_from_slice(&chunk[..n]),
                    // The peer finished, or closed, between two frames.
                    Ok(None)
                    | Err(ReadError::ConnectionLost(ConnectionError::ApplicationClosed(_)))
                        if between_frames =>
                    {
                        return Ok(None);
                    }
                    Err(_) if link.ended() => return Ok(None),
                    failed => {
                        drop(link.end());
                        let why = failed.err().map(|e| e.to_string());
                        let why = why.unwrap_or_else(|| "the stream ended inside a frame".into());
                        return Err(CarrierError::Transport(why));
                    }
                }
            }
        })
    }

    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let mut ended = self.0.end();
            let _ = tokio::time::timeout(LINGER, ended.drain()).await;
        })
    }

    fn remote_id(&self) -> Option<TransportId> {
        Some(self.0.remote.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::{pull_sync, serve_sync};
    use crate::store::Store;
    use glade_wire::generated::{Op, Shape};
    use std::collections::BTreeSet;
    use std::net::Ipv4Addr;
    use std::path::{Path, PathBuf};

    /// Plan Step 4.5: `relay off` is no relay, and `relay n0` is n0's
    /// production relays, the four of iroh's `defaults::prod`, never its
    /// staging relays, which an environment variable can choose through
    /// iroh's own default. A peer's relay URL may name those four alone, as
    /// the node prints them. Pure: nothing binds.
    #[test]
    fn the_recipe_maps_off_to_disabled_and_n0_to_the_production_relays() {
        assert_eq!(relay_mode(Relays::Off), RelayMode::Disabled);
        assert_eq!(relay_mode(Relays::N0), RelayMode::Default);
        let urls = |map: iroh::RelayMap| map.urls::<BTreeSet<RelayUrl>>();
        let n0 = urls(relay_mode(Relays::N0).relay_map());
        assert_eq!(n0, urls(iroh::defaults::prod::default_relay_map()));
        assert_eq!(n0.len(), 4);
        for url in &n0 {
            let printed = url.to_string();
            assert_eq!(n0_relay(&printed), Some(printed.clone()));
        }
        let staging = urls(iroh::defaults::staging::default_relay_map());
        for url in &staging {
            assert_eq!(n0_relay(&url.to_string()), None, "{url}");
        }
    }

    /// Plan Step 4.5: a `link` line names where a path goes, `relay <url>`
    /// for a relay path, its URL as the node prints it, and `direct
    /// <ip:port>` for an IP one. Pure: nothing binds.
    #[test]
    fn a_path_is_described_by_where_it_goes() {
        let url: RelayUrl = "https://aps1-1.relay.n0.iroh.link./".parse().unwrap();
        let relay = described(&TransportAddr::Relay(url));
        assert_eq!(relay, "relay https://aps1-1.relay.n0.iroh.link./");
        let socket: SocketAddr = "10.1.1.236:4545".parse().unwrap();
        let direct = described(&TransportAddr::Ip(socket));
        assert_eq!(direct, "direct 10.1.1.236:4545");
    }

    /// Plan Step 4.5: an endpoint binds where its network says, and the
    /// `peer` line's address is the socket bound. On loopback, at a port
    /// found free, so nothing listens beyond this machine.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_endpoint_binds_where_its_network_says() {
        let free = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let socket = free.local_addr().unwrap();
        drop(free);
        let network = Network {
            bind: vec![socket],
            ..Network::default()
        };
        let key = EndpointKey::from_seed([8; 32]);
        let endpoint = bind_endpoint(key, None, ALPN, &network).await.unwrap();
        assert_eq!(endpoint.bound_sockets(), [socket]);
        let printed = bound_addr(&endpoint).unwrap().socket;
        assert_eq!(printed, socket, "the peer line's address");
        endpoint.close().await;
    }

    /// Plan Step 4.5: a dial target names each address its entry gives, IP
    /// and relay, in one iroh address, and a line names the entry by the tag
    /// iroh itself prints for the key. Pure: nothing dials.
    #[test]
    fn a_dial_target_names_each_address_it_was_given() {
        let key = EndpointKey::from_seed([9; 32]).endpoint_id;
        let socket: SocketAddr = "10.1.1.236:4545".parse().unwrap();
        let relay = "https://aps1-1.relay.n0.iroh.link./";
        let via = vec![Via::Ip(socket), Via::Relay(relay.into())];
        let entry = PeerEntry { key, via };
        let target = endpoint_addr(&entry).unwrap();
        assert_eq!(target.id.as_bytes(), &key);
        let ips: Vec<SocketAddr> = target.ip_addrs().copied().collect();
        let relays: Vec<String> = target.relay_urls().map(|url| url.to_string()).collect();
        assert_eq!((ips, relays), (vec![socket], vec![relay.to_string()]));
        let short = target.id.fmt_short().to_string();
        assert_eq!(entry.to_string(), format!("{short}@{socket},{relay}"));
    }

    /// The `.rs` files under `dir`, at any depth.
    fn rust_files(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(rust_files(&path));
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
        files
    }

    /// A source file's production code, line by line with its number: each
    /// file's code before its first `#[cfg(test)]`, where this crate's test
    /// modules begin, with every comment set aside.
    fn production_lines(text: &str) -> Vec<(usize, &str)> {
        let lines = text.lines().enumerate();
        let code = lines.take_while(|(_, line)| line.trim() != "#[cfg(test)]");
        let code = code.map(|(i, line)| (i + 1, line.split("//").next().unwrap_or_default()));
        code.collect()
    }

    /// Plan Step 4.5: no production code asks iroh for what reads the
    /// environment or adds n0's lookups: `presets::N0` (its DNS lookup and
    /// `default_relay_mode()`), `presets::N0DisableRelay`,
    /// `default_relay_mode` and `force_staging_infra`
    /// (`IROH_FORCE_STAGING_RELAYS`), and `Builder::proxy_from_env` (the
    /// proxy variables). A source check over `src/`.
    #[test]
    fn no_production_code_names_irohs_environment_readers() {
        let readers = [
            "presets::N0",
            "N0DisableRelay",
            "default_relay_mode",
            "force_staging_infra",
            "proxy_from_env",
        ];
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = rust_files(&src);
        assert!(files.len() > 20, "the check read {} files", files.len());
        let mut named = Vec::new();
        for path in &files {
            let text = std::fs::read_to_string(path).unwrap();
            for (n, code) in production_lines(&text) {
                let found = readers.iter().filter(|reader| code.contains(*reader));
                named.extend(found.map(|reader| format!("{}:{n}: {reader}", path.display())));
            }
        }
        assert_eq!(named, Vec::<String>::new());
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

    /// Every endpoint the node binds listens on loopback alone. iroh
    /// pre-binds `0.0.0.0` and `[::]`, and a loopback IPv4 bind replaced only
    /// the first: the `[::]` socket stayed, open to the LAN over IPv6, and
    /// macOS's firewall asked about every new node and test binary.
    /// Plan Step 4.5: `Network::default()`, what every profile binds with no
    /// `--config` file.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_endpoint_listens_on_loopback_alone() {
        let key = EndpointKey::from_seed([7; 32]);
        let network = Network::default();
        let endpoint = bind_endpoint(key, None, ALPN, &network).await.unwrap();
        let sockets = endpoint.bound_sockets();
        assert!(!sockets.is_empty(), "no socket bound");
        for socket in &sockets {
            assert!(
                socket.ip().is_loopback(),
                "{socket} listens beyond this machine: {sockets:?}"
            );
        }
        endpoint.close().await;
    }

    /// Plan Steps 4.1a and 4.1b: the ALPN names the protocol, 3 since 4.1b,
    /// so a node of protocol 2, an endpoint offering only `glade/node/2` as
    /// every build from 4.1a to 4.3 does, fails at connect in either
    /// direction, before any HELLO is sent.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_protocol_2_node_fails_at_connect() {
        let bound = std::time::Duration::from_secs(10);
        let v2: &[u8] = b"glade/node/2";
        let old = Endpoint::builder(presets::Minimal)
            .alpns(vec![v2.to_vec()])
            .portmapper_config(PortmapperConfig::Disabled)
            .clear_ip_transports()
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
        let dialed = tokio::time::timeout(bound, old.connect(ea, v2)).await;
        let refused = dialed.expect("bounded").is_err();
        assert!(refused, "a protocol-2 dialer connected");

        let sockets = old.bound_sockets();
        let port = sockets.iter().find(|s| s.is_ipv4()).unwrap().port();
        let socket = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let at_old = PeerAddr {
            endpoint_id: old.id(),
            socket,
        };
        let dialed = tokio::time::timeout(bound, new.dial(&at_old)).await;
        let refused = dialed.expect("bounded").is_err();
        assert!(refused, "it accepted a protocol-3 dialer");
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
            // The dialer, as its HELLO proved it, may read `sh` (plan Step 4.3).
            let dialer = link.peer.peer_id;
            let mut policy = crate::grants::Policy::default();
            policy.grant(
                &crate::mesh::hex_id(&dialer),
                "sh",
                ["read.subscribe".to_string()],
            );
            let grants = crate::grants::PolicyView::of(Some(policy));
            let holder = glade_grant_api::Holder::Node(dialer);
            let sent = serve_sync(&mut link.recv, &mut link.send, &server, &holder, &grants).await;
            // Keep `link` (hence the connection) alive until the dialer has read
            // the finished stream — dropping it early would reset the stream.
            (link, sent)
        });

        let cdir = std::env::temp_dir().join("glade-iroh-sync-cli");
        let _ = std::fs::remove_dir_all(&cdir);
        let mut client = Store::open(&cdir).unwrap();
        let mut link = dialer.dial(&acc_addr).await.unwrap();
        let anyone = |_: &str| true;
        let out = pull_sync(&mut link.recv, &mut link.send, &mut client, &anyone)
            .await
            .unwrap();
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

    // ---- the CarrierPort adapter (plan Step 4.2c) ----

    use glade_carrier_api::conformance::{self as carrier, Fixture};
    use std::num::NonZeroUsize;

    /// A port lent a fresh endpoint key, as a booted node would lend its own.
    fn keyed() -> IrohCarrier {
        let seed = crate::signing::random_seed().unwrap();
        IrohCarrier::new(Some(EndpointKey::from_seed(seed)))
    }

    /// A configuration with this frame limit. The address is plan Step
    /// 4.5's to read.
    fn limit(max: usize) -> CarrierConfig {
        let local = CarrierAddr("127.0.0.1:0".into());
        let max_frame_bytes = NonZeroUsize::new(max).unwrap();
        CarrierConfig {
            local,
            max_frame_bytes,
        }
    }

    /// Three ports on loopback, for the contract's probes.
    fn iroh_fixture() -> Fixture {
        let port = || -> Arc<dyn CarrierPort> { Arc::new(keyed()) };
        let anywhere = CarrierAddr("127.0.0.1:0".into());
        let (at_a, at_b) = (anywhere.clone(), anywhere);
        Fixture {
            a: port(),
            b: port(),
            fresh: port(),
            at_a,
            at_b,
        }
    }

    /// A probe left waiting on a peer that never comes fails, not hangs.
    async fn bounded(probe: impl Future<Output = ()>) {
        let within = tokio::time::timeout(Duration::from_secs(20), probe).await;
        within.expect("the probe finished within 20 s");
    }

    #[tokio::test]
    async fn ca_001_iroh_carries_frames_whole_once_and_in_order() {
        bounded(carrier::frames(iroh_fixture())).await;
    }

    #[tokio::test]
    async fn ca_002_iroh_holds_the_frame_limit_both_ways() {
        bounded(carrier::frame_limit(iroh_fixture())).await;
    }

    #[tokio::test]
    async fn ca_003_irohs_futures_are_lazy_and_recv_is_cancel_safe() {
        bounded(carrier::cancellation(iroh_fixture())).await;
    }

    #[tokio::test]
    async fn ca_004_an_iroh_port_gives_its_endpoint_up_by_value() {
        bounded(carrier::close_by_value(iroh_fixture())).await;
    }

    #[tokio::test]
    async fn ca_005_iroh_names_each_far_end_by_its_endpoint_id() {
        bounded(carrier::remote_identity(iroh_fixture())).await;
    }

    /// Link `a` to `b`, each bound with this frame limit.
    async fn linked(a: &IrohCarrier, b: &IrohCarrier, max: usize) -> [Box<dyn CarrierLink>; 2] {
        let at_b = b.bind(limit(max)).await.unwrap();
        a.bind(limit(max)).await.unwrap();
        let (dialed, accepted) = tokio::join!(a.dial(&at_b), b.accept());
        [dialed.unwrap(), accepted.unwrap().unwrap()]
    }

    /// Plan Step 4.2c: closing the port ends every link it made and takes
    /// their handles out of them, so its port frees though the links
    /// themselves survive. The far end sees its stream end.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_closed_carrier_frees_its_port_though_its_links_survive() {
        let key = EndpointKey::from_seed(crate::signing::random_seed().unwrap());
        let (a, b) = (keyed(), IrohCarrier::new(Some(key)));
        let [dialed, accepted] = linked(&a, &b, 64).await;
        let proved = Some(TransportId(key.endpoint_id.to_vec()));
        assert_eq!(
            dialed.remote_id(),
            proved,
            "the endpoint id b's TLS session proved"
        );
        let port = match &*lock(&b.state) {
            PortState::Bound { ep, .. } => bound_addr(ep).unwrap().socket.port(),
            _ => unreachable!("b is bound"),
        };

        b.close().await;

        assert!(
            port_is_freed(port).await,
            "a link the port made keeps it bound"
        );
        assert_eq!(accepted.send(b"late").await, Err(CarrierError::Closed));
        assert_eq!(dialed.recv().await, Ok(None), "the far end's stream ends");
    }

    /// A dialer with the adapter's ALPN that writes `first` on its stream,
    /// raw.
    async fn raw_dial(to: &CarrierAddr, first: &[u8]) -> (Endpoint, SendStream) {
        let endpoint = Endpoint::builder(presets::Minimal)
            .portmapper_config(PortmapperConfig::Disabled)
            .clear_ip_transports()
            .bind_addr((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .unwrap();
        let at = PeerAddr::parse(&to.0).unwrap();
        let at = EndpointAddr::from_parts(at.endpoint_id, [TransportAddr::Ip(at.socket)]);
        let conn = endpoint.connect(at, CARRIER_ALPN).await.unwrap();
        let (mut send, _) = conn.open_bi().await.unwrap();
        send.write_all(first).await.unwrap();
        (endpoint, send)
    }

    /// Plan Step 4.2c: a link that does not open with the adapter's word is
    /// refused, and the port accepts the next. A receive dropped part-way
    /// through a frame keeps what it read, so the frame arrives whole.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_frame_read_in_two_parts_arrives_whole() {
        let port = keyed();
        let at = port.bind(limit(64)).await.unwrap();
        let (_stranger, refused) = tokio::join!(raw_dial(&at, b"gcl0"), port.accept());
        let why = CarrierError::Transport("not a carrier link".into());
        assert_eq!(refused.err(), Some(why), "another word is refused");
        let ((_dialer, mut send), link) =
            tokio::join!(raw_dial(&at, b"gcl1\x06\0\0\0abc"), port.accept());
        let link = link.unwrap().unwrap();

        let mut receiving = link.recv();
        let half = tokio::time::timeout(Duration::from_millis(200), &mut receiving).await;
        assert!(half.is_err(), "half a frame is no frame");
        drop(receiving);
        send.write_all(b"def\x02\0\0\0gh").await.unwrap();

        let whole = link.recv().await;
        assert_eq!(
            whole,
            Ok(Some(b"abcdef".to_vec())),
            "the dropped receive kept its part"
        );
        assert_eq!(link.recv().await, Ok(Some(b"gh".to_vec())));
    }

    /// Plan Step 4.2c: a send dropped part-way may leave its frame torn, and a
    /// torn frame is never followed by another: the next send ends the link.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_torn_frame_is_never_followed_by_another() {
        let (max, a, b) = (4 << 20, keyed(), keyed());
        let [dialed, accepted] = linked(&a, &b, max).await;
        let big = vec![7; max];
        let stuck = tokio::time::timeout(Duration::from_millis(200), dialed.send(&big)).await;
        assert!(
            stuck.is_err(),
            "a frame past the peer's window waits for the peer"
        );

        let next = tokio::time::timeout(Duration::from_secs(5), dialed.send(b"x")).await;
        assert_eq!(
            next,
            Ok(Err(CarrierError::Closed)),
            "the link ended instead"
        );
        let after = accepted.recv().await.map(|frame| frame.map(|f| f.len()));
        assert!(
            !matches!(after, Ok(Some(_))),
            "a frame arrived after a torn one: {after:?}"
        );
    }

    /// Plan Step 4.5: the adapter binds the socket `CarrierConfig::local`
    /// names, so CA-004's re-bind is real on iroh: a fresh port asked to bind
    /// where a closed one was lands at its very address, where before it
    /// bound anywhere on loopback.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_iroh_port_binds_where_its_config_says() {
        let (b, fresh) = (keyed(), keyed());
        let at_b = b.bind(limit(64)).await.unwrap();
        b.close().await;
        let again = CarrierConfig {
            local: at_b.clone(),
            ..limit(64)
        };
        let at_fresh = fresh.bind(again).await.unwrap();
        let socket = |at: &CarrierAddr| local_socket(at).unwrap();
        assert_eq!(socket(&at_fresh), socket(&at_b), "where b was");
        assert_ne!(at_fresh, at_b, "under another key");
        fresh.close().await;
    }

    /// Plan Step 4.2c: a link holds its endpoint, so a port dropped without
    /// `close`, as both are here, does not take the link's transport with it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_outlives_a_port_dropped_without_close() {
        let [dialed, accepted] = linked(&keyed(), &keyed(), 64).await;
        dialed.send(b"after").await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), accepted.recv()).await;
        assert_eq!(
            got,
            Ok(Ok(Some(b"after".to_vec()))),
            "the transport went with its port"
        );
    }
}
