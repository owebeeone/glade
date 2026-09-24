//! Plan Step 3.2's assembly: the node's bindings as one Shaku module,
//! [`NodeAssembly`], built once per node scope. `glade-node` resolves from it
//! when started with `GLADE_NODE_ASSEMBLED=1` (the binary's docs say how), and
//! a test composition overrides every real provider in it with a deterministic
//! one (`tests/assembly`). The design, with each binding's port, providers and
//! consumers, is `glade/dev-docs/GladeNodeAssembly.md`.
//!
//! | Binding | Interface | Port | Provider in the module |
//! | --- | --- | --- | --- |
//! | `clock_binding` | [`Clock`] | `ClockPort` | [`SystemClock`] |
//! | `peer_carrier_binding` | [`PeerCarrier`] | `CarrierPort` | [`IrohCarrier`], over iroh (plan Step 4.2c) |
//! | `client_carrier_binding` | [`ClientCarrier`] | `CarrierPort` | [`PendingWebSocketAdapter`] |
//! | `record_transport_binding` | [`RecordTransport`] | [`TransportPort`] | [`CarrierTransport`], over the peer occurrence |
//! | `directory_host_binding` | [`RecordHost`] | [`RecordHostPort`] | [`Records`] |
//! | `directory_profile_binding` | [`RecordProfile`] | [`RecordProfilePort`] | [`DirectoryRules`] |
//! | grant | [`Grants`] | `GrantPort` | [`PendingGrantFold`] |
//! | signer | [`Signer`] | `SignerPort` | [`NodeSigner`], Ed25519 (plan Step 4.1a) |
//! | configuration | [`Config`] | [`ConfigPort`] | [`CommandLine`] |
//!
//! Every port is bridged by a facade declared here, `trait F: Port +
//! shaku::Interface` with `impl<T: Port + 'static> F for T {}`, so no port
//! names Shaku (`dev-docs/arch1/AsyncWitnessResult.md` §5, caveat 4). A
//! consumer is handed each port back as the port, never as the facade.
//! Resolution is by interface type, through `HasComponent<I>`: there is no
//! global `get_service<T>()`, no string-keyed bag, and no constructor that
//! falls back to real I/O (`dev-docs/arch1/RuntimeAndAssurance.md:73-76`). A
//! provider built without the handle the composition root acquires refuses;
//! it never acquires one itself.
//!
//! # Refused before startup (DI-E03)
//!
//! The complete module builds and resolves:
//!
//! ```
//! use std::sync::Arc;
//!
//! use glade_node::assembly::{Directory, NodeAssembly};
//! use shaku::HasComponent;
//!
//! let node = NodeAssembly::builder().build();
//! let _: Arc<dyn Directory> = node.resolve();
//! ```
//!
//! A missing binding does not compile (E0277): the directory facade, with
//! neither its clock nor its record host bound.
//!
//! ```compile_fail,E0277
//! use glade_node::assembly::DirectoryFacade;
//!
//! shaku::module! {
//!     Unbound {
//!         components = [DirectoryFacade],
//!         providers = []
//!     }
//! }
//!
//! fn main() {
//!     let _ = Unbound::builder().build();
//! }
//! ```
//!
//! A construction cycle does not compile (E0275): the shape revision 2 of the
//! injection graph removed, where the directory supplies the record profile
//! that the record host needs, so each needs the other built first.
//!
//! ```compile_fail,E0275
//! use std::sync::Arc;
//!
//! use glade_node::assembly::{
//!     CarrierTransport, RecordHost, RecordProfile, RecordProfilePort, Records,
//! };
//! use glade_node::iroh_carrier::IrohCarrier;
//! use shaku::Component;
//!
//! #[derive(Component)]
//! #[shaku(interface = RecordProfile)]
//! struct DirectoryAsProfile {
//!     #[shaku(inject)]
//!     host: Arc<dyn RecordHost>,
//! }
//!
//! impl RecordProfilePort for DirectoryAsProfile {
//!     fn share(&self) -> &str {
//!         "home"
//!     }
//!
//!     fn hosts(&self, glade_id: &str) -> bool {
//!         self.host.who_serves(glade_id, 0).is_ok()
//!     }
//! }
//!
//! shaku::module! {
//!     Cyclic {
//!         components = [DirectoryAsProfile, Records, CarrierTransport, IrohCarrier],
//!         providers = []
//!     }
//! }
//!
//! fn main() {
//!     let _ = Cyclic::builder().build();
//! }
//! ```
//!
//! An ambiguous role does not compile (E0119): two providers bound to the
//! peer role.
//!
//! ```compile_fail,E0119
//! use glade_node::assembly::PeerCarrier;
//! use glade_node::iroh_carrier::IrohCarrier;
//! use shaku::{Component, Module, ModuleBuildContext};
//!
//! struct SecondPeerAdapter;
//!
//! impl<M: Module> Component<M> for SecondPeerAdapter {
//!     type Interface = dyn PeerCarrier;
//!     type Parameters = ();
//!
//!     fn build(context: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn PeerCarrier> {
//!         <IrohCarrier as Component<M>>::build(context, None)
//!     }
//! }
//!
//! shaku::module! {
//!     TwoPeers {
//!         components = [IrohCarrier, SecondPeerAdapter],
//!         providers = []
//!     }
//! }
//!
//! fn main() {
//!     let _ = TwoPeers::builder().build();
//! }
//! ```
//!
//! Nor does asking for a carrier by its port type alone, naming no role
//! (E0277): a port type is not a binding; a role's occurrence is.
//!
//! ```compile_fail,E0277
//! use std::sync::Arc;
//!
//! use glade_carrier_api::CarrierPort;
//! use glade_node::assembly::PendingWebSocketAdapter;
//! use glade_node::iroh_carrier::IrohCarrier;
//! use shaku::Component;
//!
//! trait Relay: shaku::Interface {}
//!
//! #[derive(Component)]
//! #[shaku(interface = Relay)]
//! struct AnyCarrierRelay {
//!     #[shaku(inject)]
//!     carrier: Arc<dyn CarrierPort>,
//! }
//!
//! impl Relay for AnyCarrierRelay {}
//!
//! shaku::module! {
//!     ByPortType {
//!         components = [IrohCarrier, PendingWebSocketAdapter, AnyCarrierRelay],
//!         providers = []
//!     }
//! }
//!
//! fn main() {
//!     let _ = ByPortType::builder().build();
//! }
//! ```

use std::fmt;
use std::future::ready;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture,
};
use glade_clock_api::ClockPort;
use glade_grant_api::{Denial, GrantPort, Holder};
use glade_signer_api::SignerPort;
use glade_wire::generated::Op;
use shaku::{module, Component, HasComponent, Module, ModuleBuildContext};

use crate::appdecl::{register, AppDecl, Registered};
use crate::iroh_carrier::IrohCarrier;
use crate::peer::NodeIdentity;
use crate::registry::{
    MemStore, Record, Registry, RegistryApi, RegistryError, StoreApi, G_BINDINGS,
    G_BINDING_RETRACTIONS, G_CLAIMS, G_GRANTS, G_NODES, G_PRINCIPALS, G_REVOCATIONS, G_SERVICES,
    G_TRANSPORT_BINDINGS, G_TRANSPORT_REVOCATIONS, G_WORKSPACES, HOME,
};
use crate::signing::NodeSigner;
use crate::sysdir::{now_ms, Boot, Profile};
use crate::transport::EndpointKey;

/// The stderr line the assembled composition root prints before anything
/// else, so a reader, and a test, can tell which root started the node.
pub const ASSEMBLED_ROOT_LINE: &str =
    "glade-node: composition root assembled from NodeAssembly (GLADE_NODE_ASSEMBLED=1)";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

static REAL_PROVIDERS: AtomicUsize = AtomicUsize::new(0);

/// How many real providers `NodeAssembly` modules have constructed in this
/// process: the system clock, the command line, the record host as the
/// module builds it, the three pending adapters and the node signer. A test
/// composition overrides each of them, so it adds nothing here (DI-E01).
pub fn real_providers_constructed() -> usize {
    REAL_PROVIDERS.load(Ordering::SeqCst)
}

fn constructed() {
    REAL_PROVIDERS.fetch_add(1, Ordering::SeqCst);
}

// ---- configuration --------------------------------------------------------

/// What `glade-node`'s command line says: its flags, then the positional port
/// and app-data store directory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub profile: Option<Profile>,
    pub name: Option<String>,
    pub operator: Option<String>,
    pub apps: Vec<String>,
    pub peers: Vec<String>,
    pub positional: Vec<String>,
}

impl Settings {
    /// Parse the arguments after the program name exactly as the hand-written
    /// root does: an unknown `--profile` is no profile, a flag given no value
    /// reads as absent, and anything that is not a flag is positional.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Settings {
        let mut settings = Settings::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--profile" => settings.profile = args.next().and_then(|s| Profile::parse(&s)),
                "--name" => settings.name = args.next(),
                "--operator" => settings.operator = args.next(),
                "--app" => settings.apps.extend(args.next()),
                "--peer" => settings.peers.extend(args.next()),
                _ => settings.positional.push(arg),
            }
        }
        settings
    }

    /// Whether this start boots an instance: `--profile` or `--name` given.
    pub fn booted(&self) -> bool {
        self.profile.is_some() || self.name.is_some()
    }

    /// The port to listen on: the first positional, else 0 (the OS chooses).
    pub fn port(&self) -> u16 {
        self.positional
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// The app-data store directory, when the second positional names one.
    pub fn store_dir(&self) -> Option<&str> {
        self.positional.get(1).map(String::as_str)
    }
}

/// The configuration binding's port. Node-local: plan Step 4.5's `ConfigPort`
/// (relay mode, bind address) grows from it.
pub trait ConfigPort: Send + Sync {
    fn settings(&self) -> &Settings;
}

// ---- the node-local ports -------------------------------------------------

/// `directory_profile_binding`'s port: what the directory profile hosts, which
/// the record host applies to every op it is handed. Pure. Node-local.
pub trait RecordProfilePort: Send + Sync {
    /// The share every record of this profile lives on.
    fn share(&self) -> &str;

    /// Whether `glade_id` is one of this profile's record streams.
    fn hosts(&self, glade_id: &str) -> bool;
}

/// `directory_host_binding`'s port: the record host the directory facade reads
/// and writes. Node-local: it names the node's own record types.
pub trait RecordHostPort: Send + Sync {
    /// Append `record` as `origin`'s next op and persist the fold. A
    /// byte-identical record already held is not appended again: `Ok(false)`,
    /// the exact-retry rule.
    fn append(&self, record: Record, origin: &str) -> Result<bool, HostError>;

    /// Register an app file's declarations under `origin`
    /// (`appdecl::register`), then persist the fold.
    fn register(&self, decl: &AppDecl, origin: &str) -> Result<Registered, HostError>;

    /// Take in an op carried from elsewhere (a peer, the disk): refused unless
    /// the profile hosts its share and stream, then verified as it lands. An
    /// op already held, byte for byte, is a duplicate: `Ok`, saving nothing.
    fn ingest(&self, op: Op) -> Result<(), HostError>;

    /// The node serving `share` at the reader's instant `now_ms`; lease expiry
    /// is judged at read time, never folded.
    fn who_serves(&self, share: &str, now_ms: i64) -> Result<Option<String>, HostError>;
}

/// `record_transport_binding`'s port: how the record host reaches a peer.
/// Node-local.
pub trait TransportPort: Send + Sync {
    /// Push `records`, each an encoded op, to `peer` over one link, best
    /// effort: a frame per record, then the link closes. Resolves with how many
    /// frames the carrier took, which is not an acknowledgement.
    fn push<'a>(
        &'a self,
        peer: &'a CarrierAddr,
        records: &'a [Vec<u8>],
    ) -> PortFuture<'a, Result<usize, CarrierError>>;
}

/// Why the record host refused.
#[derive(Debug)]
pub enum HostError {
    /// The host holds no instance: none was lent (the legacy form boots none),
    /// or the Server has adopted it.
    NotOpen,
    /// An op outside the profile: another share, or a stream it does not host.
    OutOfScope { share: String, glade_id: String },
    /// The registry's verify-as-ingest refused the record.
    Rejected(RegistryError),
    /// Persisting the fold failed.
    Io(io::Error),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::NotOpen => write!(f, "the record host holds no instance"),
            HostError::OutOfScope { share, glade_id } => {
                write!(f, "{share}/{glade_id} is outside the directory profile")
            }
            HostError::Rejected(e) => write!(f, "the registry refused the record: {e:?}"),
            HostError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HostError {}

/// A failed save, as `Registry::accept` reports it.
impl From<io::Error> for HostError {
    fn from(e: io::Error) -> HostError {
        HostError::Io(e)
    }
}

/// As the hand-written root reports the same failures: a refused registration
/// as `InvalidData` with the registry's `Debug` text, a failed write as the
/// I/O error itself.
impl From<HostError> for io::Error {
    fn from(e: HostError) -> io::Error {
        match e {
            HostError::Io(e) => e,
            HostError::Rejected(e) => io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")),
            other => io::Error::other(other.to_string()),
        }
    }
}

// ---- the facades: one Shaku interface per binding ------------------------

/// `clock_binding`'s interface.
pub trait Clock: ClockPort + shaku::Interface {}
impl<T: ClockPort + 'static> Clock for T {}

/// `peer_carrier_binding`'s interface: the carrier port in the peer role.
pub trait PeerCarrier: CarrierPort + shaku::Interface {}
impl<T: CarrierPort + 'static> PeerCarrier for T {}

/// `client_carrier_binding`'s interface: the same port in the client role.
pub trait ClientCarrier: CarrierPort + shaku::Interface {}
impl<T: CarrierPort + 'static> ClientCarrier for T {}

/// `directory_profile_binding`'s interface.
pub trait RecordProfile: RecordProfilePort + shaku::Interface {}
impl<T: RecordProfilePort + 'static> RecordProfile for T {}

/// The grant binding's interface.
pub trait Grants: GrantPort + shaku::Interface {}
impl<T: GrantPort + 'static> Grants for T {}

/// The signer binding's interface.
pub trait Signer: SignerPort + shaku::Interface {}
impl<T: SignerPort + 'static> Signer for T {}

/// The configuration binding's interface.
pub trait Config: ConfigPort + shaku::Interface {}
impl<T: ConfigPort + 'static> Config for T {}

/// `directory_host_binding`'s interface. Beside the port, it hands back the
/// profile and the record transport it was built with, as ports.
pub trait RecordHost: RecordHostPort + shaku::Interface {
    fn profile(&self) -> Arc<dyn RecordProfilePort>;
    fn transport(&self) -> Arc<dyn TransportPort>;
}

/// `record_transport_binding`'s interface. Beside the port, it hands back the
/// carrier occurrence it rides.
pub trait RecordTransport: TransportPort + shaku::Interface {
    fn carrier(&self) -> Arc<dyn CarrierPort>;
}

// ---- providers --------------------------------------------------------------

/// The system clock: wall-clock epoch milliseconds, as `sysdir::now_ms` reads
/// them.
pub struct SystemClock;

impl ClockPort for SystemClock {
    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

impl<M: Module> Component<M> for SystemClock {
    type Interface = dyn Clock;
    type Parameters = ();

    fn build(_: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Clock> {
        constructed();
        Box::new(SystemClock)
    }
}

/// The command line: the settings the composition root parsed, handed in as
/// this component's parameters. It reads no argument list and no environment.
pub struct CommandLine(Settings);

impl ConfigPort for CommandLine {
    fn settings(&self) -> &Settings {
        &self.0
    }
}

impl<M: Module> Component<M> for CommandLine {
    type Interface = dyn Config;
    type Parameters = Settings;

    fn build(_: &mut ModuleBuildContext<M>, settings: Settings) -> Box<dyn Config> {
        constructed();
        Box::new(CommandLine(settings))
    }
}

/// The directory profile on its own: the home share and its eleven record
/// streams. Pure, with nothing injected, which is what breaks the
/// Directory-Records constructor cycle
/// (`dev-docs/arch1/InjectionGraphRefinement.md:35-40`): rules, then records,
/// then the directory.
#[derive(Component)]
#[shaku(interface = RecordProfile)]
pub struct DirectoryRules;

const DIRECTORY_STREAMS: [&str; 11] = [
    G_NODES,
    G_WORKSPACES,
    G_CLAIMS,
    G_GRANTS,
    G_REVOCATIONS,
    G_BINDINGS,
    G_BINDING_RETRACTIONS,
    G_SERVICES,
    G_PRINCIPALS,
    G_TRANSPORT_BINDINGS,
    G_TRANSPORT_REVOCATIONS,
];

impl RecordProfilePort for DirectoryRules {
    fn share(&self) -> &str {
        HOME
    }

    fn hosts(&self, glade_id: &str) -> bool {
        DIRECTORY_STREAMS.contains(&glade_id)
    }
}

/// The instance a composition root lends the record host: filled after
/// `boot`, and taken back out, by value, when the Server adopts it.
pub type InstanceSlot = Arc<Mutex<Option<Boot>>>;

enum Held {
    /// The booted instance, lent through the composition root's slot.
    Instance(InstanceSlot),
    /// The node's own `Registry`, held here over an engine: `MemStore`, or
    /// one a test composition hands in.
    Memory(Mutex<(Registry, Box<dyn StoreApi + Send>)>),
}

/// `directory_host_binding`'s provider, the record host: the node's `Registry`
/// fold over a `StoreApi` engine, scoped by the directory profile. Built by
/// the module it is the real provider, over the instance slot it is given
/// (empty unless the root lends one); [`Records::in_memory`] is the test
/// composition's.
pub struct Records {
    profile: Arc<dyn RecordProfile>,
    transport: Arc<dyn RecordTransport>,
    held: Held,
}

impl Records {
    /// The in-memory store and registry a test composition binds: the node's
    /// own `Registry` over `MemStore`, so its fold is the real fold. Never
    /// built by the module.
    pub fn in_memory(
        profile: Arc<dyn RecordProfile>,
        transport: Arc<dyn RecordTransport>,
    ) -> Records {
        let store: Box<dyn StoreApi + Send> = Box::new(MemStore::default());
        let held = Held::Memory(Mutex::new((Registry::new(), store)));
        Records {
            profile,
            transport,
            held,
        }
    }

    /// The record host over `store`, an engine a test composition can read
    /// back and may have written before (plan Steps 3.4 and 4.4): the fold is
    /// loaded from it through verify-as-ingest, as boot loads records.json, so
    /// a host built again over the same engine resumes from what it accepted.
    /// A record the load quarantines stays out of the fold, as at boot. Never
    /// built by the module.
    pub fn over(
        profile: Arc<dyn RecordProfile>,
        transport: Arc<dyn RecordTransport>,
        store: Box<dyn StoreApi + Send>,
    ) -> io::Result<Records> {
        let (registry, _quarantined) = Registry::from_snapshot(&store.load()?);
        let held = Held::Memory(Mutex::new((registry, store)));
        Ok(Records {
            profile,
            transport,
            held,
        })
    }

    /// Run `f` over the fold and the engine that persists it.
    fn with<T>(
        &self,
        f: impl FnOnce(&mut Registry, &mut dyn StoreApi) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        match &self.held {
            Held::Instance(slot) => {
                let mut instance = lock(slot);
                let boot = instance.as_mut().ok_or(HostError::NotOpen)?;
                f(&mut boot.registry, &mut boot.store)
            }
            Held::Memory(memory) => {
                let mut memory = lock(memory);
                let (registry, store) = &mut *memory;
                f(registry, store.as_mut())
            }
        }
    }
}

// Every write goes through `Registry::accept` (slice profile SP-L1): staged,
// saved, and only then the fold, so a refused or unsaved write leaves nothing
// behind and its retry is judged against what was accepted.
impl RecordHostPort for Records {
    fn append(&self, record: Record, origin: &str) -> Result<bool, HostError> {
        self.with(|registry, store| {
            if registry.contains(record.glade_id(), &record.encode()) {
                return Ok(false);
            }
            registry.accept(store, |staged| {
                staged.append(record, origin).map_err(HostError::Rejected)
            })?;
            Ok(true)
        })
    }

    fn register(&self, decl: &AppDecl, origin: &str) -> Result<Registered, HostError> {
        self.with(|registry, store| {
            registry.accept(store, |staged| {
                register(decl, staged, origin).map_err(HostError::Rejected)
            })
        })
    }

    fn ingest(&self, op: Op) -> Result<(), HostError> {
        if op.share != self.profile.share() || !self.profile.hosts(&op.glade_id) {
            let (share, glade_id) = (op.share, op.glade_id);
            return Err(HostError::OutOfScope { share, glade_id });
        }
        self.with(|registry, store| {
            registry.accept(store, |staged| {
                staged.ingest(op).map(drop).map_err(HostError::Rejected)
            })
        })
    }

    fn who_serves(&self, share: &str, now_ms: i64) -> Result<Option<String>, HostError> {
        self.with(|registry, _| Ok(registry.who_serves(share, now_ms)))
    }
}

impl RecordHost for Records {
    fn profile(&self) -> Arc<dyn RecordProfilePort> {
        self.profile.clone()
    }

    fn transport(&self) -> Arc<dyn TransportPort> {
        self.transport.clone()
    }
}

impl<M> Component<M> for Records
where
    M: Module + HasComponent<dyn RecordProfile> + HasComponent<dyn RecordTransport>,
{
    type Interface = dyn RecordHost;
    type Parameters = InstanceSlot;

    fn build(context: &mut ModuleBuildContext<M>, slot: InstanceSlot) -> Box<dyn RecordHost> {
        constructed();
        let profile = <M as HasComponent<dyn RecordProfile>>::build_component(context);
        let transport = <M as HasComponent<dyn RecordTransport>>::build_component(context);
        Box::new(Records {
            profile,
            transport,
            held: Held::Instance(slot),
        })
    }
}

/// `record_transport_binding`'s provider: the record transport as a view over
/// the peer carrier's occurrence, never a second transport.
#[derive(Component)]
#[shaku(interface = RecordTransport)]
pub struct CarrierTransport {
    #[shaku(inject)]
    carrier: Arc<dyn PeerCarrier>,
}

impl TransportPort for CarrierTransport {
    fn push<'a>(
        &'a self,
        peer: &'a CarrierAddr,
        records: &'a [Vec<u8>],
    ) -> PortFuture<'a, Result<usize, CarrierError>> {
        Box::pin(async move {
            let link = self.carrier.dial(peer).await?;
            let mut taken = 0;
            for record in records {
                if let Err(e) = link.send(record).await {
                    link.close().await;
                    return Err(e);
                }
                taken += 1;
            }
            link.close().await;
            Ok(taken)
        })
    }
}

impl RecordTransport for CarrierTransport {
    fn carrier(&self) -> Arc<dyn CarrierPort> {
        self.carrier.clone()
    }
}

/// A carrier role whose adapter is not built yet, failing closed: `bind` is
/// refused, so nothing dials or accepts through it. Before a bind the contract
/// answers `Closed` to `dial` and `Ok(None)` to `accept`, and so does this.
pub struct PendingCarrier {
    adapter: &'static str,
}

impl CarrierPort for PendingCarrier {
    fn bind(&self, _config: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>> {
        let adapter = self.adapter;
        let why = format!("the {adapter} CarrierPort adapter is not built yet (plan Phase 4)");
        Box::pin(ready(Err(CarrierError::Transport(why))))
    }

    fn dial<'a>(
        &'a self,
        _peer: &'a CarrierAddr,
    ) -> PortFuture<'a, Result<Box<dyn CarrierLink>, CarrierError>> {
        Box::pin(ready(Err(CarrierError::Closed)))
    }

    fn accept(&self) -> PortFuture<'_, Result<Option<Box<dyn CarrierLink>>, CarrierError>> {
        Box::pin(ready(Ok(None)))
    }

    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(ready(()))
    }
}

/// `peer_carrier_binding`'s registration: the iroh adapter (plan Step 4.2c,
/// `iroh_carrier.rs`), bound with the endpoint key the composition root lends
/// as this component's parameters. Neither root lends one yet, so the peer
/// role refuses to bind and fails closed: the node's peer transport is still
/// the `PeerEndpoint` the root binds and `Server::enable_mesh` runs.
impl<M: Module> Component<M> for IrohCarrier {
    type Interface = dyn PeerCarrier;
    type Parameters = Option<EndpointKey>;

    fn build(_: &mut ModuleBuildContext<M>, key: Option<EndpointKey>) -> Box<dyn PeerCarrier> {
        constructed();
        Box::new(IrohCarrier::new(key))
    }
}

/// `client_carrier_binding`'s registration: the WebSocket adapter is pending
/// (plan Phase 4), so the client role fails closed. Client sessions still
/// arrive through the TCP listener the root binds and `Server::run` serves.
pub struct PendingWebSocketAdapter;

impl<M: Module> Component<M> for PendingWebSocketAdapter {
    type Interface = dyn ClientCarrier;
    type Parameters = ();

    fn build(_: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn ClientCarrier> {
        constructed();
        Box::new(PendingCarrier {
            adapter: "WebSocket",
        })
    }
}

/// The grant binding's registration: no grant adapter over the node's fold
/// exists before plan Step 4.3, so every check is `Unavailable` and fails
/// closed (GR-003).
pub struct PendingGrantFold;

impl GrantPort for PendingGrantFold {
    fn check(&self, _holder: &Holder, _verb: &str, _share: &str) -> Result<(), Denial> {
        Err(Denial::Unavailable)
    }
}

impl<M: Module> Component<M> for PendingGrantFold {
    type Interface = dyn Grants;
    type Parameters = ();

    fn build(_: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Grants> {
        constructed();
        Box::new(PendingGrantFold)
    }
}

/// The signer binding's registration: the Ed25519 signer over the node key
/// (plan Step 4.1a, `signing.rs`), the key being the identity the composition
/// root lends as this component's parameters. Lent none, as in the legacy
/// form, it holds no key and refuses both ways (SI-003), and its id is all
/// zeros. No consumer resolves it before plan Step 4.1b.
impl<M: Module> Component<M> for NodeSigner {
    type Interface = dyn Signer;
    type Parameters = Option<NodeIdentity>;

    fn build(_: &mut ModuleBuildContext<M>, identity: Option<NodeIdentity>) -> Box<dyn Signer> {
        constructed();
        Box::new(NodeSigner::new(identity))
    }
}

// ---- participants -----------------------------------------------------------

/// The directory facade: reads and writes the directory through the record
/// host, at the injected clock.
pub trait Directory: shaku::Interface {
    /// The clock this directory reads.
    fn clock(&self) -> Arc<dyn ClockPort>;

    /// The record host this directory reads and writes.
    fn host(&self) -> Arc<dyn RecordHostPort>;

    /// Which node serves `share` now, by the injected clock.
    fn serves(&self, share: &str) -> Result<Option<String>, HostError>;

    /// Register an app file's declarations under `origin`.
    fn register(&self, decl: &AppDecl, origin: &str) -> Result<Registered, HostError>;
}

#[derive(Component)]
#[shaku(interface = Directory)]
pub struct DirectoryFacade {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
    #[shaku(inject)]
    host: Arc<dyn RecordHost>,
}

impl Directory for DirectoryFacade {
    fn clock(&self) -> Arc<dyn ClockPort> {
        self.clock.clone()
    }

    fn host(&self) -> Arc<dyn RecordHostPort> {
        self.host.clone()
    }

    fn serves(&self, share: &str) -> Result<Option<String>, HostError> {
        self.host.who_serves(share, self.clock.now_ms())
    }

    fn register(&self, decl: &AppDecl, origin: &str) -> Result<Registered, HostError> {
        self.host.register(decl, origin)
    }
}

/// An admission decision, stamped with the instant it was made at: the
/// evidence frontier a re-evaluation compares against
/// (`dev-docs/arch1/RuntimeAndAssurance.md:54-60`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub at_ms: i64,
    pub outcome: Result<(), Denial>,
}

/// Admission: whether a holder may use a verb on a share, as the grant fold
/// answers at the injected clock's instant. No serve path consults it before
/// plan Step 4.3.
pub trait Admission: shaku::Interface {
    /// The clock this admission reads.
    fn clock(&self) -> Arc<dyn ClockPort>;

    /// The grant fold this admission consults.
    fn grants(&self) -> Arc<dyn GrantPort>;

    /// Decide now.
    fn admit(&self, holder: &Holder, verb: &str, share: &str) -> Decision;
}

#[derive(Component)]
#[shaku(interface = Admission)]
pub struct GrantAdmission {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
    #[shaku(inject)]
    grants: Arc<dyn Grants>,
}

impl Admission for GrantAdmission {
    fn clock(&self) -> Arc<dyn ClockPort> {
        self.clock.clone()
    }

    fn grants(&self) -> Arc<dyn GrantPort> {
        self.grants.clone()
    }

    fn admit(&self, holder: &Holder, verb: &str, share: &str) -> Decision {
        let at_ms = self.clock.now_ms();
        let outcome = self.grants.check(holder, verb, share);
        Decision { at_ms, outcome }
    }
}

/// Sessions: the consumer of both carrier roles, each through its own binding.
/// The Server still runs sessions over its own transports; they move here in
/// plan Phase 4.
pub trait Sessions: shaku::Interface {
    /// The peer role's carrier.
    fn peer(&self) -> Arc<dyn CarrierPort>;

    /// The client role's carrier.
    fn client(&self) -> Arc<dyn CarrierPort>;
}

#[derive(Component)]
#[shaku(interface = Sessions)]
pub struct RoleSessions {
    #[shaku(inject)]
    peer: Arc<dyn PeerCarrier>,
    #[shaku(inject)]
    client: Arc<dyn ClientCarrier>,
}

impl Sessions for RoleSessions {
    fn peer(&self) -> Arc<dyn CarrierPort> {
        self.peer.clone()
    }

    fn client(&self) -> Arc<dyn CarrierPort> {
        self.client.clone()
    }
}

// One built `NodeAssembly` is one node scope: every binding in it is one
// occurrence. `#[lazy]` marks the bindings no consumer on the assembled path
// resolves before plan Phase 4. (A plain comment: `module!` parses a
// visibility first and takes no attribute before it.)
module! {
    pub NodeAssembly {
        components = [
            CommandLine,
            SystemClock,
            DirectoryRules,
            IrohCarrier,
            CarrierTransport,
            Records,
            DirectoryFacade,
            #[lazy] PendingWebSocketAdapter,
            #[lazy] RoleSessions,
            #[lazy] PendingGrantFold,
            #[lazy] GrantAdmission,
            #[lazy] NodeSigner
        ],
        providers = []
    }
}
