//! The fast composition: Shaku assembly over the witness ports, with no
//! runtime and no socket reachable.
//!
//! This crate is a harness, never a production installation. It holds the
//! bridge of `AsyncWitnessPlan.md` §4.3 and the one module definition that
//! DI-E01, DI-E02 and DI-E03 share, so the criteria are decided against the
//! witness's own declared ports rather than against synthetic traits.
//!
//! **The bridge, and why it is unavoidable.** `shaku::Interface` is a trait
//! alias expanding to `trait Interface: Any + Send + Sync {}` plus a blanket
//! impl over every `T: Any + Send + Sync`. That impl has no `?Sized`, so a
//! foreign trait object such as `dyn CarrierPort` can never acquire
//! `Interface`, no matter which supertraits the port declares (the standing
//! proof is `di-eval/examples/foreign_port_shaku.rs`, E0277). The way through
//! is an assembly-local facade: a trait that names both the port and
//! `shaku::Interface`, a blanket impl over every port implementation, and
//! stable trait upcasting back to the port at the use site. The facade lives
//! here, in the assembly crate. `async-witness-ports` never names a framework
//! and keeps its single `glade-wire` dependency.
//!
//! **Hidden I/O is excluded mechanically, not by inspection.** This crate
//! declares `async-witness-ports` and `shaku` and nothing else, so a provider
//! that opens a socket or a file cannot compile into this target at all; the
//! architecture gate fails closed on any other manifest entry, where no `#[cfg]`
//! can reach around it. The stand-in providers below therefore *count* their
//! construction and *panic* in every port method, which is the strongest form
//! of "the real provider was never reached" this target can express.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use async_witness_ports::{
    CarriedFrame, CarrierError, CarrierPort, ClockPort, FrameType, PortFuture, StoreError,
    StorePort,
};
use shaku::{Component, Module, ModuleBuildContext, module};

/// Drive an already-complete port future to its value, with no runtime.
///
/// The witness fakes never yield: both of `FakeCarrier`'s futures are
/// `std::future::ready`, so one poll under a noop waker resolves them. This is
/// the same discipline `glade-lifecycle-api`'s conformance suite uses to drive
/// `ManagedResource` without a runtime. A fake that yields here is a defect in
/// the fake, not a scheduling problem to solve by adding tokio to this crate.
pub fn resolve_now<T>(mut future: PortFuture<'_, T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => {
            panic!("the fast composition has no runtime: a witness fake must never yield")
        }
    }
}

/// The assembly-only facade over `ClockPort`. Declared here so the port crate
/// stays framework-free; the blanket impl makes every clock implementation a
/// `Clock` without any of them knowing it.
pub trait Clock: ClockPort + shaku::Interface {}
impl<T: ClockPort> Clock for T {}

/// The assembly-only facade over `CarrierPort`.
pub trait Carrier: CarrierPort + shaku::Interface {}
impl<T: CarrierPort> Carrier for T {}

/// The assembly-only facade over `StorePort`.
pub trait Store: StorePort + shaku::Interface {}
impl<T: StorePort> Store for T {}

/// A caller that stamps a frame with the injected clock and hands it to the
/// injected carrier. Its own signatures return the **ports**, not the facades:
/// a consumer of a consumer never learns that a container assembled it.
pub trait Session: shaku::Interface {
    fn clock(&self) -> Arc<dyn ClockPort>;
    fn carrier(&self) -> Arc<dyn CarrierPort>;

    /// Send one frame carrying the current instant, and return that instant.
    fn send_stamped(&self, frame: FrameType) -> Result<i64, CarrierError>;
}

/// A caller that appends to the injected store, again stamping with the
/// injected clock.
pub trait Journal: shaku::Interface {
    fn clock(&self) -> Arc<dyn ClockPort>;
    fn store(&self) -> Arc<dyn StorePort>;

    /// Append one body and return the sequence number the store gave it.
    fn record(&self, share: &str, body: &[u8]) -> Result<i64, StoreError>;
}

/// A consumer of consumers: it holds two assembled services **and** a port of
/// its own, so DI-E01's "every relevant caller" includes a transitive one.
pub trait Supervisor: shaku::Interface {
    fn session(&self) -> Arc<dyn Session>;
    fn journal(&self) -> Arc<dyn Journal>;
    fn carrier(&self) -> Arc<dyn CarrierPort>;
}

#[derive(Component)]
#[shaku(interface = Session)]
pub struct PeerSession {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
    #[shaku(inject)]
    carrier: Arc<dyn Carrier>,
}

impl Session for PeerSession {
    fn clock(&self) -> Arc<dyn ClockPort> {
        self.clock.clone()
    }

    fn carrier(&self) -> Arc<dyn CarrierPort> {
        self.carrier.clone()
    }

    fn send_stamped(&self, frame: FrameType) -> Result<i64, CarrierError> {
        let now_ms = self.clock.now_ms();
        let body = now_ms.to_be_bytes();
        resolve_now(self.carrier.send(frame, &body))?;
        Ok(now_ms)
    }
}

#[derive(Component)]
#[shaku(interface = Journal)]
pub struct ShareJournal {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
    #[shaku(inject)]
    store: Arc<dyn Store>,
}

impl Journal for ShareJournal {
    fn clock(&self) -> Arc<dyn ClockPort> {
        self.clock.clone()
    }

    fn store(&self) -> Arc<dyn StorePort> {
        self.store.clone()
    }

    fn record(&self, share: &str, body: &[u8]) -> Result<i64, StoreError> {
        self.store.append(share, body)
    }
}

#[derive(Component)]
#[shaku(interface = Supervisor)]
pub struct NodeSupervisor {
    #[shaku(inject)]
    session: Arc<dyn Session>,
    #[shaku(inject)]
    journal: Arc<dyn Journal>,
    #[shaku(inject)]
    carrier: Arc<dyn Carrier>,
}

impl Supervisor for NodeSupervisor {
    fn session(&self) -> Arc<dyn Session> {
        self.session.clone()
    }

    fn journal(&self) -> Arc<dyn Journal> {
        self.journal.clone()
    }

    fn carrier(&self) -> Arc<dyn CarrierPort> {
        self.carrier.clone()
    }
}

static WALL_CLOCK_BUILDS: AtomicUsize = AtomicUsize::new(0);
static ENDPOINT_CARRIER_BUILDS: AtomicUsize = AtomicUsize::new(0);
static DIRECTORY_STORE_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// How many times the three stand-in real providers were constructed in this
/// process. DI-E01 asserts it is zero for a composition that overrides all
/// three; a non-zero reading means a real provider was built despite a fake
/// being selected.
pub fn real_provider_builds() -> usize {
    wall_clock_builds() + endpoint_carrier_builds() + directory_store_builds()
}

pub fn wall_clock_builds() -> usize {
    WALL_CLOCK_BUILDS.load(Ordering::SeqCst)
}

pub fn endpoint_carrier_builds() -> usize {
    ENDPOINT_CARRIER_BUILDS.load(Ordering::SeqCst)
}

pub fn directory_store_builds() -> usize {
    DIRECTORY_STORE_BUILDS.load(Ordering::SeqCst)
}

/// Every stand-in provider method ends here. Reaching one means an injected
/// fake did not reach a caller that a test believed it had covered.
fn provider_was_reached(what: &str) -> ! {
    panic!("the fast composition reached a stand-in real provider: {what}");
}

/// The provider a production composition would bind for `ClockPort`: it would
/// read the machine's wall clock, as `node/src/sysdir.rs:206` does today.
///
/// It cannot do that here, and that is the mechanical part of DI-E01: this
/// crate has no dependency through which any real provider could read a clock,
/// open a socket or touch a file. What remains observable is construction,
/// which is counted, and use, which panics.
pub struct WallClock;

impl ClockPort for WallClock {
    fn now_ms(&self) -> i64 {
        provider_was_reached("WallClock::now_ms would read the machine's wall clock")
    }
}

impl<M: Module> Component<M> for WallClock {
    type Interface = dyn Clock;
    type Parameters = ();

    fn build(_: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Clock> {
        WALL_CLOCK_BUILDS.fetch_add(1, Ordering::SeqCst);
        Box::new(WallClock)
    }
}

/// The provider a production composition would bind for `CarrierPort`: in
/// Phase 3 it is the node's own `PeerEndpoint`, whose acquisition awaits a real
/// UDP socket bind.
pub struct EndpointCarrier;

impl CarrierPort for EndpointCarrier {
    fn send<'a>(
        &'a self,
        _frame: FrameType,
        _body: &'a [u8],
    ) -> PortFuture<'a, Result<(), CarrierError>> {
        provider_was_reached("EndpointCarrier::send would write to a bound socket")
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<CarriedFrame>, CarrierError>> {
        provider_was_reached("EndpointCarrier::recv would read from a bound socket")
    }
}

impl<M: Module> Component<M> for EndpointCarrier {
    type Interface = dyn Carrier;
    type Parameters = ();

    fn build(_: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Carrier> {
        ENDPOINT_CARRIER_BUILDS.fetch_add(1, Ordering::SeqCst);
        Box::new(EndpointCarrier)
    }
}

/// The provider a production composition would bind for `StorePort`: the
/// node's `Store`, which is synchronous blocking file I/O.
pub struct DirectoryStore;

impl StorePort for DirectoryStore {
    fn append(&self, _share: &str, _body: &[u8]) -> Result<i64, StoreError> {
        provider_was_reached("DirectoryStore::append would write to a share directory")
    }

    fn scan(&self, _share: &str, _from_seq: i64) -> Vec<Vec<u8>> {
        provider_was_reached("DirectoryStore::scan would read a share directory")
    }
}

impl<M: Module> Component<M> for DirectoryStore {
    type Interface = dyn Store;
    type Parameters = ();

    fn build(_: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Store> {
        DIRECTORY_STORE_BUILDS.fetch_add(1, Ordering::SeqCst);
        Box::new(DirectoryStore)
    }
}

// The one composition Phase 1 shares: three consumers over three ports, with a
// stand-in real provider registered as the default binding for each port so
// that a test composition has something to override. The comment is ordinary
// rather than a doc comment because `module!` parses a visibility first and
// accepts no attributes before it.
module! {
    pub FastComposition {
        components = [
            PeerSession,
            ShareJournal,
            NodeSupervisor,
            WallClock,
            EndpointCarrier,
            DirectoryStore
        ],
        providers = []
    }
}

// The pair below exists only to produce the evidence for caveat 2 of
// AsyncWitnessPlan.md §8.4, which must be recorded with its evidence or it
// counts as a failure. Neither registers the journal, so NOTHING in either
// injects `dyn Store`: the store is an unreferenced registration, and the only
// difference between the two modules is `#[lazy]`.
module! {
    pub EagerComposition {
        components = [PeerSession, WallClock, EndpointCarrier, DirectoryStore],
        providers = []
    }
}

module! {
    pub LazyComposition {
        components = [PeerSession, WallClock, EndpointCarrier, #[lazy] DirectoryStore],
        providers = []
    }
}
