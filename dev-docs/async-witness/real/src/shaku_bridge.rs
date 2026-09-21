//! Step 3.3 — the bridge, declared **in this crate**, over the real port.
//!
//! `AsyncWitnessPlan.md` §4.3 gives the mechanism: a facade trait in the
//! assembly crate carrying `shaku::Interface` as a supertrait, a blanket impl
//! over every port implementation, and stable trait upcasting back to the port
//! at the use site. `async-witness-ports` never names a framework.
//!
//! **Why it is declared here rather than imported.** The Phase 1/2 update to
//! the plan, item 1: `architecture-policy.json` does not let
//! `async-witness-real` depend on `async-witness-fast`, and §7 forbids widening
//! an allowlist to make a check pass. "Depends on 1.1" means the pattern. A
//! facade is local to one assembly, so declaring a second one is the design
//! working rather than duplication.
//!
//! **Why a facade is unavoidable at all.** `shaku::Interface` is a trait alias
//! expanding to `trait Interface: Any + Send + Sync {}` plus a blanket impl
//! over every `T: Any + Send + Sync`, and that impl has no `?Sized`, so a
//! foreign trait object such as `dyn CarrierPort` can never acquire `Interface`
//! however its supertraits are written (§3.5; the standing proof is
//! `di-eval/examples/foreign_port_shaku.rs`, E0277).
//!
//! # What the module is *not* allowed to do
//!
//! Construct an endpoint. Shaku is synchronous: `ComponentFn` is a plain
//! `FnOnce(&mut ModuleBuildContext<M>) -> Box<I>`, so a component that wanted a
//! bound socket would have to block a thread on one. [`BindingCarrier`] is the
//! registration a production composition would carry for `dyn Carrier`; it
//! counts its own construction and panics in every port method, so "the module
//! built its own provider" is a counter reading rather than a judgement.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_witness_ports::{CarriedFrame, CarrierError, CarrierPort, FrameType, PortFuture};
use shaku::{Component, HasComponent, Module, ModuleBuildContext, module};

/// The assembly-only facade over `CarrierPort`, and the one line of the bridge
/// that matters: every carrier implementation becomes a `Carrier` without
/// knowing it, and none of them names Shaku.
pub trait Carrier: CarrierPort + shaku::Interface {}
impl<T: CarrierPort> Carrier for T {}

/// A consumer assembled by the container. Its own signature hands back the
/// **port**, so a caller of a caller never learns that a container was
/// involved — the upcast happens here, once.
pub trait PeerSession: shaku::Interface {
    /// The carrier this session was injected with, as the framework-free port.
    fn carrier(&self) -> Arc<dyn CarrierPort>;
}

#[derive(Component)]
#[shaku(interface = PeerSession)]
pub struct LinkSession {
    #[shaku(inject)]
    carrier: Arc<dyn Carrier>,
}

impl PeerSession for LinkSession {
    fn carrier(&self) -> Arc<dyn CarrierPort> {
        // `Arc<dyn Carrier>` -> `Arc<dyn CarrierPort>`: stable trait upcasting,
        // the whole reason the facade can stay inside the assembly.
        self.carrier.clone()
    }
}

static BINDING_CARRIER_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// How many times the registered stand-in provider was constructed in this
/// process. Step 3.3 asserts it is zero for a composition that overrides
/// `dyn Carrier` with an acquired handle.
pub fn binding_carrier_builds() -> usize {
    BINDING_CARRIER_BUILDS.load(Ordering::SeqCst)
}

/// The provider a production composition would register for `CarrierPort`: in
/// the node it is a `PeerEndpoint`, whose acquisition awaits a real UDP socket
/// bind and therefore cannot happen inside a synchronous `build`.
///
/// It is registered so that the witness's module has something to override, and
/// it panics in every port method so that "it was never reached" is enforced
/// rather than asserted.
pub struct BindingCarrier;

/// Every stand-in method ends here.
fn provider_was_reached(what: &str) -> ! {
    panic!("the assembled module reached the registered stand-in provider: {what}");
}

impl CarrierPort for BindingCarrier {
    fn send<'a>(
        &'a self,
        _frame: FrameType,
        _body: &'a [u8],
    ) -> PortFuture<'a, Result<(), CarrierError>> {
        provider_was_reached("BindingCarrier::send would need an endpoint it never bound");
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<CarriedFrame>, CarrierError>> {
        provider_was_reached("BindingCarrier::recv would need an endpoint it never bound");
    }
}

impl<M: Module> Component<M> for BindingCarrier {
    type Interface = dyn Carrier;
    type Parameters = ();

    fn build(_: &mut ModuleBuildContext<M>, _: ()) -> Box<dyn Carrier> {
        BINDING_CARRIER_BUILDS.fetch_add(1, Ordering::SeqCst);
        Box::new(BindingCarrier)
    }
}

// The composition Step 3.3 assembles inside the run. An ordinary comment, not a
// doc comment, because `module!` parses a visibility first and accepts no
// attributes before it.
module! {
    pub RealComposition {
        components = [LinkSession, BindingCarrier],
        providers = []
    }
}

/// Assemble over an **already-acquired** carrier.
///
/// The override is consulted before the registered build function, so
/// `BindingCarrier::build` never runs — Phase 1 measured that
/// (`fast/tests/di_e01_eager_construction.rs`) and Step 3.3 re-reads the
/// counter against the real port. `with_component_override_fn` on a `#[lazy]`
/// registration would also work and is not needed: there is nothing to defer
/// when the value already exists.
///
/// `AcquiredCarrier` (in `peer_carrier`) is what makes the plain override
/// possible at all: `with_component_override` takes `Box<I>`, and the engine's
/// value is an `Arc`, so the box is a delegating handle rather than a copy.
pub fn assemble(carrier: Arc<dyn CarrierPort>) -> RealComposition {
    RealComposition::builder()
        .with_component_override::<dyn Carrier>(Box::new(
            crate::peer_carrier::AcquiredCarrier::over(carrier),
        ))
        .build()
}

/// Resolve the assembled session and hand its caller the port.
pub fn resolve_carrier(module: &RealComposition) -> Arc<dyn CarrierPort> {
    let session: Arc<dyn PeerSession> = module.resolve();
    session.carrier()
}
