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
//!
//! Plan Step 4.5b, part 1: the adapter's half of the mesh's move onto the
//! carrier port. [`IrohCarrier`] binds as a composition root lends it
//! ([`Lent`]: the key, the door, the relays and the first word's bound), on
//! every socket its configuration names; dials every address a carrier
//! address names; waits for an inbound attempt's first word within its bound;
//! gives each link's TLS exporter bytes as its channel binding; and notes each
//! link's path and the home relays' states through the node-local
//! `LinkNotes` port. No root lends it anything yet, so the mesh still runs on
//! `PeerEndpoint`.

use std::fmt;
use std::future::{ready, Future};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, ChannelBinding, PortFuture,
    TransportId,
};
use iroh::endpoint::presets;
use iroh::endpoint::RelayStatus;
use iroh::endpoint::{AfterHandshakeOutcome, EndpointHooks, PortmapperConfig, Side, VarInt};
use iroh::endpoint::{Connection, ConnectionError, ReadError, RecvStream, SendStream};
use iroh::Watcher;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr};

use crate::assembly::{LinkNotes, PathSeen, RelayState};
use crate::netconf::{Network, PeerEntry, Relays, Via};
use crate::peer::{hello_accept, hello_dial, spelled, Channel, NodeIdentity, PeerHello};
use crate::transport::{hex, key_of, tag, Door, EndpointKey};

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

    /// Watch this endpoint's home relays until it closes (plan Step 4.5), as
    /// [`watch_relays`] does.
    pub(crate) fn relay_watch(
        &self,
        seen: impl FnMut(Vec<RelayState>) + Send + 'static,
    ) -> impl Future<Output = ()> + Send + 'static {
        watch_relays(&self.endpoint, seen)
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

/// A home relay's state, as iroh reports it, in a line's words.
fn relay_state(status: &RelayStatus) -> RelayState {
    RelayState {
        url: status.url().to_string(),
        connected: status.is_connected(),
        error: status.last_error().map(|e| e.to_string()),
    }
}

/// Watch `endpoint`'s home relays until it closes: `seen` has their states
/// now, and again at each change (plan Step 4.5). iroh's status becomes text
/// and flags here, and the future holds no handle on the endpoint, so it
/// keeps no socket bound.
fn watch_relays(
    endpoint: &Endpoint,
    mut seen: impl FnMut(Vec<RelayState>) + Send + 'static,
) -> impl Future<Output = ()> + Send + 'static {
    let mut statuses = endpoint.home_relay_status();
    let closed = endpoint.closed();
    async move {
        let states = |held: &[RelayStatus]| held.iter().map(relay_state).collect();
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

/// How long an inbound attempt has, from the moment the endpoint hands it
/// over, to finish its handshake, pass the door, open its stream and send the
/// first word (plan Step 4.5b, the owner's ruling of 2026-09-27): more than
/// ten times the crossing's whole HELLO through n0's relay.
pub const FIRST_WORD: Duration = Duration::from_secs(10);

/// What a composition root lends the adapter (plan Step 4.5b): the node's
/// endpoint key; the door its accept hook asks, if any; whether it has n0's
/// relays; and how long an inbound attempt has to send its first word,
/// [`FIRST_WORD`] but in tests.
#[derive(Clone, Debug)]
pub struct Lent {
    pub key: EndpointKey,
    pub door: Option<Arc<Door>>,
    pub relays: Relays,
    pub first_word: Duration,
}

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
/// a peer entry's form with the full id, `<endpoint-id>@<via>[,<via>]`;
/// `remote_id` is the id the TLS session proved, and `channel_binding` the
/// bytes its exporter gives (plan Step 4.5b).
/// `close` ends every link the port tracks and takes their handles, since a
/// surviving connection keeps the port bound (the async witness's finding).
/// A link holds the endpoint too, so a port dropped without `close` does not
/// take its links' transport with it. It binds the sockets
/// `CarrierConfig::local` names, behind the door and with the relays it was
/// lent (plan Step 4.5b), and lent nothing refuses to bind. `Clone` shares
/// the one port: every clone binds, dials, accepts and closes one endpoint.
#[derive(Clone)]
pub struct IrohCarrier(Arc<Adapter>);

/// The port every clone of an [`IrohCarrier`] shares.
struct Adapter {
    lent: Option<Lent>,
    state: Mutex<PortState>,
    /// How many [`Loan`]s of the endpoint pending accepts and dials hold.
    loans: AtomicUsize,
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
    /// A port that binds as `lent` says; lent nothing, it refuses to.
    pub fn new(lent: Option<Lent>) -> IrohCarrier {
        let (state, loans) = (Mutex::new(PortState::Unbound), AtomicUsize::new(0));
        IrohCarrier(Arc::new(Adapter { lent, state, loans }))
    }

    /// The relays this port was lent: none unless lent n0's.
    fn relays(&self) -> Relays {
        self.0.lent.as_ref().map_or(Relays::Off, |lent| lent.relays)
    }

    /// Keep `endpoint` as this port's, unless a bind or a close came first.
    fn place(&self, endpoint: &Endpoint, max: usize) -> Result<(), CarrierError> {
        let mut state = lock(&self.0.state);
        if let Some(refused) = refusal(&state) {
            return Err(refused);
        }
        let (ep, links) = (endpoint.clone(), Vec::new());
        *state = PortState::Bound { ep, max, links };
        Ok(())
    }

    /// The bound endpoint: none before `bind` and after `close`.
    fn endpoint(&self) -> Option<Endpoint> {
        match &*lock(&self.0.state) {
            PortState::Bound { ep, .. } => Some(ep.clone()),
            _ => None,
        }
    }

    /// The bound endpoint, lent to a pending accept or dial and counted until
    /// its [`Loan`] ends: none before `bind` and after `close`.
    fn loan(&self) -> Option<(Endpoint, Loan<'_>)> {
        let state = lock(&self.0.state);
        let PortState::Bound { ep, .. } = &*state else {
            return None;
        };
        self.0.loans.fetch_add(1, Ordering::SeqCst);
        let port = &*self.0;
        Some((ep.clone(), Loan { port, out: true }))
    }

    /// Track a new link; none, dropping it, once the port has closed.
    fn track(&self, conn: Connection, send: SendStream, recv: RecvStream) -> Option<Link> {
        let mut state = lock(&self.0.state);
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

    /// One inbound attempt, under one bound from the moment the endpoint
    /// hands it over: the handshake, the door's hook, the stream and the first
    /// word (plan Step 4.5b). At the bound the attempt is refused and its
    /// connection closed with code 0 and no reason, and once the handshake has
    /// proved a key the door reports it; the port then takes the next.
    async fn accept_on(
        &self,
        endpoint: &Endpoint,
        lent: &Lent,
    ) -> Result<Option<Link>, CarrierError> {
        let Some(incoming) = endpoint.accept().await else {
            return Ok(None);
        };
        let deadline = tokio::time::Instant::now() + lent.first_word;
        let late = format!("no first word within {}", spelled(lent.first_word));
        let accepting = incoming.accept().map_err(transport)?;
        let conn = tokio::time::timeout_at(deadline, accepting).await;
        let conn = conn.map_err(|_| transport(&late))?.map_err(transport)?;
        let Ok(opened) = tokio::time::timeout_at(deadline, first_stream(&conn)).await else {
            conn.close(0u32.into(), b"");
            if let Some(door) = &lent.door {
                door.refused(conn.remote_id().as_bytes(), &late);
            }
            return Err(transport(late));
        };
        let (send, recv) = opened?;
        Ok(self.track(conn, send, recv))
    }

    /// One dial of `peer` from `endpoint`: the connection, the stream and the
    /// first word, then the link, tracked.
    async fn dial_on(&self, endpoint: &Endpoint, peer: &CarrierAddr) -> Result<Link, CarrierError> {
        let at = dial_target(peer, self.relays())?;
        let conn = endpoint.connect(at, CARRIER_ALPN).await;
        let conn = conn.map_err(transport)?;
        let (mut send, recv) = conn.open_bi().await.map_err(transport)?;
        send.write_all(PREAMBLE).await.map_err(transport)?;
        self.track(conn, send, recv).ok_or(CarrierError::Closed)
    }
}

/// The stream a dialer opened on `conn`, once it has sent the adapter's first
/// word; another word refuses the attempt.
async fn first_stream(conn: &Connection) -> Result<(SendStream, RecvStream), CarrierError> {
    let (send, mut recv) = conn.accept_bi().await.map_err(transport)?;
    let mut word = [0; 4];
    recv.read_exact(&mut word).await.map_err(transport)?;
    if &word != PREAMBLE {
        return Err(transport("not a carrier link"));
    }
    Ok((send, recv))
}

/// The sockets a carrier address names to bind, `<ip:port>[,<ip:port>]`, at
/// most one per family, as a configuration file's `bind` lines allow: the
/// address says where, and the key a port is lent says who, so an
/// `<endpoint-id>@` prefix is ignored (plan Steps 4.5 and 4.5b).
fn local_sockets(local: &CarrierAddr) -> Option<Vec<SocketAddr>> {
    let at = match local.0.rsplit_once('@') {
        Some((_, at)) => at,
        None => &local.0,
    };
    let parse = |socket: &str| socket.parse::<SocketAddr>().ok();
    let sockets: Vec<SocketAddr> = at.split(',').map(parse).collect::<Option<_>>()?;
    let v4 = sockets.iter().filter(|socket| socket.is_ipv4()).count();
    (v4 <= 1 && sockets.len() - v4 <= 1).then_some(sockets)
}

/// A bound endpoint's carrier address: its id and every socket it bound,
/// IPv4 first (plan Step 4.5b).
fn bound_at(endpoint: &Endpoint) -> io::Result<CarrierAddr> {
    let mut sockets = endpoint.bound_sockets();
    sockets.sort_by_key(|socket| !socket.is_ipv4());
    let via = sockets.into_iter().map(Via::Ip).collect();
    let entry = PeerEntry {
        key: *endpoint.id().as_bytes(),
        via,
    };
    carrier_addr(&entry).ok_or_else(|| other("no socket bound"))
}

/// A peer entry as the adapter dials it, `<endpoint-id>@<via>[,<via>]`: its
/// full id and every address it names (plan Step 4.5b). An entry that names
/// none is no address to dial.
pub fn carrier_addr(entry: &PeerEntry) -> Option<CarrierAddr> {
    let vias: Vec<String> = entry.via.iter().map(Via::to_string).collect();
    let id = hex(&entry.key);
    (!vias.is_empty()).then(|| CarrierAddr(format!("{id}@{}", vias.join(","))))
}

/// The peer entry a carrier address names, `<endpoint-id>@<via>[,<via>]`,
/// each via an `ip:port` or one of n0's relay URLs, as `bind` answers it and
/// `dial` takes it (plan Step 4.5b).
pub fn entry_of(addr: &CarrierAddr) -> Option<PeerEntry> {
    let (id, vias) = addr.0.split_once('@')?;
    let key = key_of(id)?;
    let via = |text: &str| match text.parse() {
        Ok(socket) => Some(Via::Ip(socket)),
        Err(_) => n0_relay(text).map(Via::Relay),
    };
    let via = vias.split(',').map(via).collect::<Option<Vec<Via>>>()?;
    Some(PeerEntry { key, via })
}

/// Where `addr` is dialed: one iroh address with every via it names. A relay
/// URL needs a port lent n0's relays, as a configuration file's load demands
/// (plan Step 4.5b).
fn dial_target(addr: &CarrierAddr, relays: Relays) -> Result<EndpointAddr, CarrierError> {
    let malformed = || transport("expected <endpoint-id>@<ip:port or relay-url>[,…] to dial");
    let entry = entry_of(addr).ok_or_else(malformed)?;
    let relayed = entry.via.iter().any(|via| matches!(via, Via::Relay(_)));
    if relayed && relays != Relays::N0 {
        return Err(transport("a relay URL needs relay n0"));
    }
    endpoint_addr(&entry).map_err(transport)
}

impl CarrierPort for IrohCarrier {
    fn bind(&self, config: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>> {
        Box::pin(async move {
            if let Some(refused) = refusal(&lock(&self.0.state)) {
                return Err(refused);
            }
            let unkeyed = || transport("the iroh adapter was lent no endpoint key");
            let lent = self.0.lent.as_ref().ok_or_else(unkeyed)?;
            let nowhere = || transport("expected <ip:port>[,<ip:port>], one per family, to bind");
            let bind = local_sockets(&config.local).ok_or_else(nowhere)?;
            let (relays, peers) = (lent.relays, Vec::new());
            let network = Network {
                relays,
                bind,
                peers,
            };
            let door = lent.door.clone();
            let endpoint = bind_endpoint(lent.key, door, CARRIER_ALPN, &network).await;
            let endpoint = endpoint.map_err(transport)?;
            let at = bound_at(&endpoint).map_err(transport)?;
            if let Err(refused) = self.place(&endpoint, config.max_frame_bytes.get()) {
                endpoint.close().await;
                return Err(refused);
            }
            Ok(at)
        })
    }

    fn dial<'a>(&'a self, peer: &'a CarrierAddr) -> PortFuture<'a, Result<Link, CarrierError>> {
        Box::pin(async move {
            let (endpoint, loan) = self.loan().ok_or(CarrierError::Closed)?;
            let dialed = self.dial_on(&endpoint, peer).await;
            loan.end(endpoint).await;
            dialed
        })
    }

    fn accept(&self) -> PortFuture<'_, Result<Option<Link>, CarrierError>> {
        Box::pin(async move {
            let Some(lent) = &self.0.lent else {
                return Ok(None);
            };
            let Some((endpoint, loan)) = self.loan() else {
                return Ok(None);
            };
            let accepted = self.accept_on(&endpoint, lent).await;
            loan.end(endpoint).await;
            match accepted {
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
            let taken = std::mem::replace(&mut *lock(&self.0.state), PortState::Closed);
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
            // A loan out keeps the sockets bound until it ends, and the last
            // one to end waits for their release: close does not.
            if self.0.loans.load(Ordering::SeqCst) == 0 {
                released(&bound).await;
            }
        })
    }
}

/// The notes port (plan Step 4.5b): a link's path, read off its connection,
/// and the home relays' states, for a port lent n0's relays.
impl LinkNotes for IrohCarrier {
    fn path(&self, remote: &TransportId) -> Option<PathSeen> {
        let links: Vec<Arc<LinkState>> = match &*lock(&self.0.state) {
            PortState::Bound { links, .. } => links.iter().filter_map(Weak::upgrade).collect(),
            _ => Vec::new(),
        };
        let mut to_remote = links.iter().rev().filter(|link| link.remote == *remote);
        let newest_live = to_remote.find_map(|link| {
            let held = lock(&link.held);
            held.as_ref().map(|(conn, _)| selected_path(conn))
        });
        newest_live.flatten()
    }

    fn relay_watch(&self, seen: Box<dyn FnMut(Vec<RelayState>) + Send>) -> PortFuture<'static, ()> {
        let relays = self.relays() == Relays::N0;
        match self.endpoint() {
            Some(endpoint) if relays => Box::pin(watch_relays(&endpoint, seen)),
            _ => Box::pin(ready(())),
        }
    }
}

/// Wait, within `LINGER`, until each socket in `bound` can be bound again
/// (plan Step 4.5). iroh lets a closed endpoint's sockets go a few
/// milliseconds after its last handle drops, and gives no signal when it
/// has, so a port whose `close` frees its address by value (CA-004), now
/// that it binds where `CarrierConfig::local` says, waits for it: binding
/// each address itself is the only witness. `close` waits here when no
/// [`Loan`] is out, and otherwise the last loan to end does.
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

/// The endpoint's handle, out on loan to a pending accept or dial (the
/// owner's ruling of 2026-09-27). iroh frees the sockets only once every
/// handle is gone, so a `close` that finds a loan out does not wait for the
/// release, which it would bar: the last loan to end waits for it, and its
/// accept or dial answers after. A loan dropped with its future is
/// uncounted all the same, and waits for nothing.
struct Loan<'a> {
    port: &'a Adapter,
    out: bool,
}

impl Loan<'_> {
    /// Give `endpoint` back. The last loan out of a closed port waits,
    /// within `LINGER`, until its sockets bind again.
    async fn end(mut self, endpoint: Endpoint) {
        let bound = endpoint.bound_sockets();
        drop(endpoint);
        if self.uncount() {
            released(&bound).await;
        }
    }

    /// Uncount the loan, once: true when it was the last out of a port that
    /// has closed.
    fn uncount(&mut self) -> bool {
        if !std::mem::take(&mut self.out) {
            return false;
        }
        let last = self.port.loans.fetch_sub(1, Ordering::SeqCst) == 1;
        last && matches!(*lock(&self.port.state), PortState::Closed)
    }
}

impl Drop for Loan<'_> {
    fn drop(&mut self) {
        self.uncount();
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

    /// The connection's TLS exporter under `label`, with no context, as
    /// HELLO draws its bytes today (D6), while the link lives (plan Step
    /// 4.5b).
    fn channel_binding(&self, label: &[u8]) -> Option<ChannelBinding> {
        let held = lock(&self.0.held);
        let (conn, _) = held.as_ref()?;
        let mut bytes = [0; 32];
        conn.export_keying_material(&mut bytes, label, b"").ok()?;
        Some(ChannelBinding(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::{pull_sync, serve_sync};
    use crate::store::Store;
    use glade_wire::generated::{Op, Shape};
    use std::collections::BTreeSet;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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

    /// A fresh endpoint key, as a booted node's own would be.
    fn key() -> EndpointKey {
        EndpointKey::from_seed(crate::signing::random_seed().unwrap())
    }

    /// What a root lends a port with `key`: no door, no relays, and the
    /// first word's bound a node has.
    fn lent(key: EndpointKey) -> Lent {
        let (door, relays, first_word) = (None, Relays::Off, FIRST_WORD);
        Lent {
            key,
            door,
            relays,
            first_word,
        }
    }

    /// A port lent a fresh endpoint key, as a booted node would lend its own.
    fn keyed() -> IrohCarrier {
        IrohCarrier::new(Some(lent(key())))
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

    /// Poll `future` once, with a waker that does nothing, as the contract's
    /// probes do.
    fn polled_once<T>(future: &mut PortFuture<'_, T>) -> std::task::Poll<T> {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        future.as_mut().poll(&mut cx)
    }

    /// A port bound on loopback, and the one socket it bound.
    async fn bound_on_loopback() -> (IrohCarrier, SocketAddr) {
        let port = keyed();
        let at = port.bind(limit(64)).await.unwrap();
        let socket = local_sockets(&at).unwrap()[0];
        (port, socket)
    }

    /// Hold `port`'s endpoint, on no loan, for 100 ms more: iroh's release,
    /// made late, so that a wait for it shows and a missing one fails every
    /// run, not only under load.
    fn release_late(port: &IrohCarrier) {
        let kept = port.endpoint();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(kept);
        });
    }

    /// A pending accept holds an endpoint handle through `close`, and iroh
    /// frees the sockets only once it lets go, so `close` does not wait for
    /// the release: the accept it ends does, before it answers (the owner's
    /// ruling of 2026-09-27).
    #[tokio::test]
    async fn close_leaves_the_release_to_a_pending_accept() {
        let (port, socket) = bound_on_loopback().await;
        let mut accepting = port.accept();
        assert!(polled_once(&mut accepting).is_pending(), "the accept waits");
        release_late(&port);

        let started = std::time::Instant::now();
        port.close().await;
        let closing = started.elapsed();
        let answer = accepting.await;
        let rebinds = std::net::UdpSocket::bind(socket).is_ok();

        let quick = closing < Duration::from_secs(1);
        assert!(quick, "close waits on no accept's handle: {closing:?}");
        assert!(matches!(answer, Ok(None)), "close ends the pending accept");
        assert!(rebinds, "the accept answers once the address binds again");
    }

    /// A pending dial holds a handle too, here to a peer that never accepts:
    /// `close` does not wait for the release, and the dial it ends answers
    /// once the address binds again.
    #[tokio::test]
    async fn close_leaves_the_release_to_a_pending_dial() {
        let (port, socket) = bound_on_loopback().await;
        let silent = keyed();
        let peer = silent.bind(limit(64)).await.unwrap();
        let mut dialing = port.dial(&peer);
        assert!(polled_once(&mut dialing).is_pending(), "the dial waits");
        release_late(&port);

        let started = std::time::Instant::now();
        port.close().await;
        let closing = started.elapsed();
        let answer = dialing.await;
        let rebinds = std::net::UdpSocket::bind(socket).is_ok();
        silent.close().await;

        let quick = closing < Duration::from_secs(1);
        assert!(quick, "close waits on no dial's handle: {closing:?}");
        assert!(answer.is_err(), "close ends the pending dial");
        assert!(rebinds, "the dial answers once the address binds again");
    }

    /// With two loans out, the first to end answers at once, since the other
    /// still holds the address, and the last answers once it binds again.
    #[tokio::test]
    async fn only_the_last_loan_to_end_waits_for_the_release() {
        let (port, socket) = bound_on_loopback().await;
        let (mut first, mut last) = (port.accept(), port.accept());
        assert!(polled_once(&mut first).is_pending(), "the first waits");
        assert!(polled_once(&mut last).is_pending(), "the last waits");
        port.close().await;

        let started = std::time::Instant::now();
        let first_ended = matches!(first.await, Ok(None));
        let answering = started.elapsed();
        let held = std::net::UdpSocket::bind(socket).is_err();
        let last_ended = matches!(last.await, Ok(None));
        let rebinds = std::net::UdpSocket::bind(socket).is_ok();

        let quick = answering < Duration::from_secs(1);
        assert!(quick, "the first waits for no release: {answering:?}");
        assert!(first_ended && last_ended, "close ends both accepts");
        assert!(held, "the last loan out holds the address");
        assert!(rebinds, "the last answers once the address binds again");
    }

    /// An accept dropped before it answers is uncounted with its loan, so a
    /// `close` that finds no loan out waits for the release itself.
    #[tokio::test]
    async fn a_dropped_accept_leaves_the_release_to_close() {
        let (port, socket) = bound_on_loopback().await;
        let mut accepting = port.accept();
        assert!(polled_once(&mut accepting).is_pending(), "the accept waits");
        drop(accepting);
        release_late(&port);

        port.close().await;

        let rebinds = std::net::UdpSocket::bind(socket).is_ok();
        assert!(rebinds, "close answers once the address binds again");
    }

    #[tokio::test]
    async fn ca_005_iroh_names_each_far_end_by_its_endpoint_id() {
        bounded(carrier::remote_identity(iroh_fixture())).await;
    }

    /// Link `a` to `b`, each bound with this frame limit.
    async fn linked(a: &IrohCarrier, b: &IrohCarrier, max: usize) -> [Box<dyn CarrierLink>; 2] {
        let at_b = b.bind(limit(max)).await.unwrap();
        a.bind(limit(max)).await.unwrap();
        dial_accept(a, &at_b, b).await
    }

    /// `a` dials `b` at `at`, and `b` accepts, within 5 s. A dial or an
    /// accept that fails fails the test at once, with its error, rather than
    /// leave the other waiting.
    async fn dial_accept(a: &IrohCarrier, at: &CarrierAddr, b: &IrohCarrier) -> [Link; 2] {
        let both = async { tokio::try_join!(a.dial(at), b.accept()) };
        let both = tokio::time::timeout(Duration::from_secs(5), both).await;
        let (dialed, accepted) = both.expect("linked within 5 s").expect("linked");
        [dialed, accepted.expect("an inbound link")]
    }

    /// Plan Step 4.2c: closing the port ends every link it made and takes
    /// their handles out of them, so its port frees though the links
    /// themselves survive. The far end sees its stream end.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_closed_carrier_frees_its_port_though_its_links_survive() {
        let b_key = key();
        let (a, b) = (keyed(), IrohCarrier::new(Some(lent(b_key))));
        let [dialed, accepted] = linked(&a, &b, 64).await;
        let proved = Some(TransportId(b_key.endpoint_id.to_vec()));
        assert_eq!(
            dialed.remote_id(),
            proved,
            "the endpoint id b's TLS session proved"
        );
        let port = bound_socket(&b).port();

        b.close().await;

        assert!(
            port_is_freed(port).await,
            "a link the port made keeps it bound"
        );
        assert_eq!(accepted.send(b"late").await, Err(CarrierError::Closed));
        assert_eq!(dialed.recv().await, Ok(None), "the far end's stream ends");
    }

    /// A raw dialer with `key`, on the adapter's ALPN: it connects to `to`
    /// and writes `first` on a stream, opening none when `first` is empty.
    /// It holds its endpoint, its connection, or why it has none, and its
    /// stream, which keep the attempt open.
    async fn raw(to: &CarrierAddr, key: EndpointKey, first: &[u8]) -> Raw {
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::from_bytes(&key.seed()))
            .portmapper_config(PortmapperConfig::Disabled)
            .clear_ip_transports()
            .bind_addr((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .unwrap();
        let at = dial_target(to, Relays::Off).unwrap();
        let conn = endpoint.connect(at, CARRIER_ALPN).await;
        let conn = conn.map_err(|e| format!("{e:?}"));
        let mut send = None;
        if let (Ok(conn), false) = (&conn, first.is_empty()) {
            if let Ok((mut stream, _)) = conn.open_bi().await {
                let _ = stream.write_all(first).await;
                send = Some(stream);
            }
        }
        (endpoint, conn, send)
    }

    type Raw = (Endpoint, Result<Connection, String>, Option<SendStream>);

    /// A dialer with the adapter's ALPN that writes `first` on its stream,
    /// raw.
    async fn raw_dial(to: &CarrierAddr, first: &[u8]) -> (Endpoint, SendStream) {
        let (endpoint, _, send) = raw(to, key(), first).await;
        (endpoint, send.expect("a stream"))
    }

    /// How the far end ended `conn`, as its `Debug` form reads, within 5 s:
    /// [`REFUSED`] for a refusal.
    async fn closed(conn: Result<Connection, String>) -> String {
        let Ok(conn) = conn else {
            return conn.err().unwrap_or_default();
        };
        let closed = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        format!("{:?}", closed.expect("closed within 5 s"))
    }

    /// A connection an adapter refused: closed with code 0 and no reason.
    const REFUSED: &str = r#"ApplicationClosed(ApplicationClose { error_code: 0, reason: b"" })"#;

    /// The socket `port` bound first, IPv4 first.
    fn bound_socket(port: &IrohCarrier) -> SocketAddr {
        match &*lock(&port.0.state) {
            PortState::Bound { ep, .. } => bound_addr(ep).unwrap().socket,
            _ => unreachable!("the port is bound"),
        }
    }

    /// The refusal lines a door reported.
    type Lines = Arc<Mutex<Vec<String>>>;

    /// A door that admits `keys` on first contact, and the lines it reports.
    fn door_of(keys: &[EndpointKey]) -> (Arc<Door>, Lines) {
        let (lines, configured) = (Lines::default(), keys.iter().map(|key| key.endpoint_id));
        let sink = lines.clone();
        let door = Door::new(configured, move |line: &str| lock(&sink).push(line.into()));
        (Arc::new(door), lines)
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
        let socket = |at: &CarrierAddr| local_sockets(at).unwrap();
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

    // ---- the adapter's half of the mesh's move (plan Step 4.5b, part 1) ----

    /// CA-006 on real iroh over loopback: each link's bytes are its TLS
    /// session's exporter. And under HELLO's label a link gives the very
    /// bytes HELLO exports on a connection today (D6), so its transcript is
    /// unchanged over the port.
    #[tokio::test]
    async fn ca_006_iroh_binds_each_link_to_its_tls_session() {
        bounded(carrier::channel_binding(iroh_fixture())).await;
        let (a, b) = (keyed(), keyed());
        let [dialed, _accepted] = linked(&a, &b, 64).await;
        let links = match &*lock(&a.0.state) {
            PortState::Bound { links, .. } => links.clone(),
            _ => unreachable!("a is bound"),
        };
        let link = links[0].upgrade().expect("a's link lives");
        let conn = lock(&link.held).as_ref().map(|(conn, _)| conn.clone());
        let conn = conn.expect("a's link holds its connection");
        let today = channel(&conn, conn.remote_id(), conn.remote_id()).unwrap();
        let binding = dialed.channel_binding(HELLO_EXPORTER);
        assert_eq!(
            binding,
            Some(ChannelBinding(today.exported)),
            "the bytes HELLO signs"
        );
    }

    /// Plan Step 4.5b: an inbound attempt has one bound, from its arrival to
    /// its first word. A dialer the door admits that sends nothing, and one
    /// that sends half the word, are each refused at the bound, closed with
    /// code 0 and no reason, and reported by their tags; the port then
    /// accepts a genuine link.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_first_word_is_awaited_within_its_bound() {
        let bound = Duration::from_millis(200);
        let (silent, short, genuine) = (key(), key(), key());
        let (door, lines) = door_of(&[silent, short, genuine]);
        let (door, first_word) = (Some(door), bound);
        let port = IrohCarrier::new(Some(Lent {
            door,
            first_word,
            ..lent(key())
        }));
        let at = port.bind(limit(64)).await.unwrap();
        for (dialer, first) in [(silent, &b""[..]), (short, &b"gc"[..])] {
            let began = tokio::time::Instant::now();
            let accepting = tokio::time::timeout(Duration::from_secs(5), port.accept());
            let (refused, (_endpoint, conn, _send)) =
                tokio::join!(accepting, raw(&at, dialer, first));
            let refused = refused.expect("the accept still waiting after 5 s");
            let late = "no first word within 200 ms";
            let why = CarrierError::Transport(late.into());
            assert_eq!(refused.err(), Some(why), "refused at the bound");
            assert!(began.elapsed() >= bound, "refused before its bound");
            let tag = tag(&dialer.endpoint_id);
            let line = format!("peer refused: endpoint {tag}: {late}");
            assert_eq!(lock(&lines).last(), Some(&line), "reported by its tag");
            let why = closed(conn).await;
            assert!(why.contains(REFUSED), "closed as a refusal: {why}");
        }
        let dialer = IrohCarrier::new(Some(lent(genuine)));
        dialer.bind(limit(64)).await.unwrap();
        let [dialed, accepted] = dial_accept(&dialer, &at, &port).await;
        dialed.send(b"word").await.unwrap();
        assert_eq!(accepted.recv().await, Ok(Some(b"word".to_vec())));
    }

    /// Plan Step 4.5b: the door's accept hook on the adapter's endpoint. A
    /// key the door does not know is refused at accept, though its dialer
    /// sends the word: its connection is closed with code 0 and no reason,
    /// and the refusal reported by its tag. A key the door was configured
    /// with links.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_carrier_behind_a_door_refuses_an_unknown_key_at_accept() {
        let (known, unknown) = (key(), key());
        let (door, lines) = door_of(&[known]);
        let door = Some(door);
        let port = IrohCarrier::new(Some(Lent {
            door,
            ..lent(key())
        }));
        let at = port.bind(limit(64)).await.unwrap();
        let (refused, (_endpoint, conn, _send)) =
            tokio::join!(port.accept(), raw(&at, unknown, PREAMBLE));
        assert!(refused.is_err(), "the unknown key linked");
        let why = closed(conn).await;
        assert!(why.contains(REFUSED), "closed as a refusal: {why}");
        let line = format!(
            "peer refused: endpoint {}: unknown endpoint key",
            tag(&unknown.endpoint_id)
        );
        assert_eq!(*lock(&lines), [line]);
        let dialer = IrohCarrier::new(Some(lent(known)));
        dialer.bind(limit(64)).await.unwrap();
        let [dialed, accepted] = dial_accept(&dialer, &at, &port).await;
        dialed.send(b"known").await.unwrap();
        assert_eq!(accepted.recv().await, Ok(Some(b"known".to_vec())));
    }

    /// Plan Step 4.5b: `local` names a socket per family; the port binds both
    /// and answers with both, IPv4 first; and a dial of that answer reaches
    /// it. On loopback, at ports found free. Then, pure: an `ip:port` and a
    /// relay URL become one iroh address with both, and the relay URL needs
    /// a port lent n0's relays.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_carrier_binds_the_sockets_it_is_given_and_dials_every_address() {
        let free = |ip: IpAddr| {
            let socket = std::net::UdpSocket::bind((ip, 0)).unwrap();
            socket.local_addr().unwrap()
        };
        let v4 = free(Ipv4Addr::LOCALHOST.into());
        let v6 = free(Ipv6Addr::LOCALHOST.into());
        let b_key = key();
        let b = IrohCarrier::new(Some(lent(b_key)));
        let local = CarrierAddr(format!("{v6},{v4}"));
        let at_b = b.bind(CarrierConfig { local, ..limit(64) }).await.unwrap();
        let id = hex(&b_key.endpoint_id);
        assert_eq!(at_b.0, format!("{id}@{v4},{v6}"), "both, IPv4 first");
        let a = keyed();
        a.bind(limit(64)).await.unwrap();
        let [dialed, accepted] = dial_accept(&a, &at_b, &b).await;
        dialed.send(b"both").await.unwrap();
        assert_eq!(accepted.recv().await, Ok(Some(b"both".to_vec())));

        let (socket, relay) = ("10.1.1.236:4545", "https://aps1-1.relay.n0.iroh.link./");
        let far = CarrierAddr(format!("{id}@{socket},{relay}"));
        let target = dial_target(&far, Relays::N0).unwrap();
        assert_eq!(target.id.as_bytes(), &b_key.endpoint_id);
        let ips: Vec<String> = target.ip_addrs().map(|ip| ip.to_string()).collect();
        let relays: Vec<String> = target.relay_urls().map(|url| url.to_string()).collect();
        assert_eq!((ips, relays), (vec![socket.into()], vec![relay.into()]));
        let unrelayed = dial_target(&far, Relays::Off).err();
        let needs = transport("a relay URL needs relay n0");
        assert_eq!(unrelayed, Some(needs), "a port with no relays");
    }

    /// Plan Step 4.5b: the notes port reads the path of an endpoint's newest
    /// live link, `direct <ip:port>` on loopback, the far end's socket, and
    /// nothing for an endpoint no link reaches. A port with no relays watches
    /// none: its watch ends at once, having seen nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_carrier_notes_each_links_path() {
        let b_key = key();
        let (a, b) = (keyed(), IrohCarrier::new(Some(lent(b_key))));
        let [_dialed, _accepted] = linked(&a, &b, 64).await;
        let far = TransportId(b_key.endpoint_id.to_vec());
        let mut path = a.path(&far);
        for _ in 0..200 {
            if path.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            path = a.path(&far);
        }
        let path = path.expect("a path noted within 2 s");
        assert_eq!(path.via, format!("direct {}", bound_socket(&b)));
        let nobody = TransportId(key().endpoint_id.to_vec());
        assert_eq!(a.path(&nobody), None, "no link reaches it");

        let seen = Lines::default();
        let noting = seen.clone();
        let watch = a.relay_watch(Box::new(move |states: Vec<RelayState>| {
            lock(&noting).extend(states.into_iter().map(|state| state.url));
        }));
        let ended = tokio::time::timeout(Duration::from_secs(1), watch).await;
        ended.expect("the watch ends at once");
        assert!(lock(&seen).is_empty(), "a port with no relays notes none");
    }
}
