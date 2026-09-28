use std::future::ready;
use std::io;
use std::sync::{Arc, Mutex};

use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture,
};
use glade_clock_api::ClockPort;
use glade_wire::generated::Op;
use shaku::{Component, HasComponent, Module, ModuleBuildContext};

use crate::appdecl::{register, AppDecl, Registered};
use crate::envelope;
use crate::grants::PolicyView;
use crate::iroh_carrier::IrohCarrier;
use crate::peer::NodeIdentity;
use crate::registry::{MemStore, Record, Registry, RegistryApi, StoreApi, HOME};
use crate::signing::NodeSigner;
use crate::sysdir::{now_ms, Boot};

use super::{
    constructed, lock, ClientCarrier, Clock, Config, ConfigPort, Constructions, Grants, HostError,
    PeerCarrier, RecordHost, RecordHostPort, RecordProfile, RecordProfilePort, RecordTransport,
    Settings, Signer, TransportPort,
};

// ---- providers --------------------------------------------------------------

/// The system clock: wall-clock epoch milliseconds, as `sysdir::now_ms` reads
/// them.
pub struct SystemClock;

impl ClockPort for SystemClock {
    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

impl<M: Module + HasComponent<dyn Constructions>> Component<M> for SystemClock {
    type Interface = dyn Clock;
    type Parameters = ();

    fn build(context: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Clock> {
        constructed::<M, SystemClock>(context);
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

impl<M: Module + HasComponent<dyn Constructions>> Component<M> for CommandLine {
    type Interface = dyn Config;
    type Parameters = Settings;

    fn build(context: &mut ModuleBuildContext<M>, settings: Settings) -> Box<dyn Config> {
        constructed::<M, CommandLine>(context);
        Box::new(CommandLine(settings))
    }
}

/// The directory profile on its own: the home share and its thirteen record
/// streams, the ones whose kinds a signed `home` record is checked against
/// (`envelope.rs`, plan Step 4.1b). Pure, with nothing injected, which is
/// what breaks the Directory-Records constructor cycle
/// (`dev-docs/arch1/InjectionGraphRefinement.md:35-40`): rules, then records,
/// then the directory.
#[derive(Component)]
#[shaku(interface = RecordProfile)]
pub struct DirectoryRules;

impl RecordProfilePort for DirectoryRules {
    fn share(&self) -> &str {
        HOME
    }

    fn hosts(&self, glade_id: &str) -> bool {
        envelope::directory_stream(glade_id)
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
    M: Module
        + HasComponent<dyn RecordProfile>
        + HasComponent<dyn RecordTransport>
        + HasComponent<dyn Constructions>,
{
    type Interface = dyn RecordHost;
    type Parameters = InstanceSlot;

    fn build(context: &mut ModuleBuildContext<M>, slot: InstanceSlot) -> Box<dyn RecordHost> {
        constructed::<M, Records>(context);
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
/// `iroh_carrier.rs`), the node's one, lent in this component's parameters
/// (plan Step 4.5b, part 3): the assembled root lends the adapter it built
/// from the booted instance, which its `PeerCarrier` binds and the mesh runs
/// on, and the module holds a clone of it. Lent none, as in the legacy form,
/// it is an adapter lent nothing, which refuses to bind and fails closed.
impl<M: Module + HasComponent<dyn Constructions>> Component<M> for IrohCarrier {
    type Interface = dyn PeerCarrier;
    type Parameters = Option<IrohCarrier>;

    fn build(
        context: &mut ModuleBuildContext<M>,
        lent: Option<IrohCarrier>,
    ) -> Box<dyn PeerCarrier> {
        constructed::<M, IrohCarrier>(context);
        Box::new(lent.unwrap_or_else(|| IrohCarrier::new(None)))
    }
}

/// `client_carrier_binding`'s registration: the WebSocket adapter is pending
/// (plan Phase 4), so the client role fails closed. Client sessions still
/// arrive through the TCP listener the root binds and `Server::run` serves.
pub struct PendingWebSocketAdapter;

impl<M: Module + HasComponent<dyn Constructions>> Component<M> for PendingWebSocketAdapter {
    type Interface = dyn ClientCarrier;
    type Parameters = ();

    fn build(context: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn ClientCarrier> {
        constructed::<M, PendingWebSocketAdapter>(context);
        Box::new(PendingCarrier {
            adapter: "WebSocket",
        })
    }
}

/// The grant binding's registration: the adapter over the node's grant fold
/// (plan Step 4.3, `grants.rs`), built holding no fold, so every check is
/// `Unavailable` and fails closed (GR-003). The module registers the app files
/// and is dropped before the node serves; the served node checks the view its
/// `Server` holds, which adopting the instance fills.
impl<M: Module + HasComponent<dyn Constructions>> Component<M> for PolicyView {
    type Interface = dyn Grants;
    type Parameters = ();

    fn build(context: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Grants> {
        constructed::<M, PolicyView>(context);
        Box::new(PolicyView::unavailable())
    }
}

/// The signer binding's registration: the Ed25519 signer over the node key
/// (plan Step 4.1a, `signing.rs`), the key being the identity the composition
/// root lends as this component's parameters. Lent none, as in the legacy
/// form, it holds no key and refuses both ways (SI-003), and its id is all
/// zeros. No consumer resolves it yet: plan Step 4.1b signs and checks
/// `home` records with the node's own functions, and D9's known set (4.1b's
/// part 2) is to be its first.
impl<M: Module + HasComponent<dyn Constructions>> Component<M> for NodeSigner {
    type Interface = dyn Signer;
    type Parameters = Option<NodeIdentity>;

    fn build(
        context: &mut ModuleBuildContext<M>,
        identity: Option<NodeIdentity>,
    ) -> Box<dyn Signer> {
        constructed::<M, NodeSigner>(context);
        Box::new(NodeSigner::new(identity))
    }
}
