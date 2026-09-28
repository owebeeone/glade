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
//! | grant | [`Grants`] | `GrantPort` | [`PolicyView`], the node's fold (plan Step 4.3) |
//! | signer | [`Signer`] | `SignerPort` | [`NodeSigner`], Ed25519 (plan Step 4.1a) |
//! | configuration | [`Config`] | [`ConfigPort`] | [`CommandLine`] |
//! | construction observer | [`Constructions`] | none: told of each real provider built | [`Unobserved`] |
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
//! neither its clock nor its record host bound. The modules after it bind the
//! construction observer, [`Unobserved`], which every real provider needs, so
//! each fails only for the reason it shows.
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
//!     CarrierTransport, RecordHost, RecordProfile, RecordProfilePort, Records, Unobserved,
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
//!         components = [DirectoryAsProfile, Records, CarrierTransport, IrohCarrier, Unobserved],
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
//! use glade_node::assembly::{Constructions, PeerCarrier, Unobserved};
//! use glade_node::iroh_carrier::IrohCarrier;
//! use shaku::{Component, HasComponent, Module, ModuleBuildContext};
//!
//! struct SecondPeerAdapter;
//!
//! impl<M: Module + HasComponent<dyn Constructions>> Component<M> for SecondPeerAdapter {
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
//!         components = [IrohCarrier, SecondPeerAdapter, Unobserved],
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
//! use glade_node::assembly::{PendingWebSocketAdapter, Unobserved};
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
//!         components = [IrohCarrier, PendingWebSocketAdapter, AnyCarrierRelay, Unobserved],
//!         providers = []
//!     }
//! }
//!
//! fn main() {
//!     let _ = ByPortType::builder().build();
//! }
//! ```

use std::any::type_name;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use glade_carrier_api::CarrierPort;
use glade_clock_api::ClockPort;
use glade_grant_api::GrantPort;
use glade_signer_api::SignerPort;
use shaku::{module, Component, HasComponent, Module, ModuleBuildContext};

use crate::grants::PolicyView;
use crate::iroh_carrier::IrohCarrier;
use crate::signing::NodeSigner;

mod config;
mod participants;
mod ports;
mod providers;

pub use config::{ConfigPort, Settings};
pub use participants::{
    Admission, Decision, Directory, DirectoryFacade, GrantAdmission, RoleSessions, Sessions,
};
pub use ports::{
    HostError, LinkNotes, PathSeen, RecordHostPort, RecordProfilePort, RelayState, TransportPort,
};
pub use providers::{
    CarrierTransport, CommandLine, DirectoryRules, InstanceSlot, PendingCarrier,
    PendingWebSocketAdapter, Records, SystemClock,
};

/// The stderr line the assembled composition root prints before anything
/// else, so a reader, and a test, can tell which root started the node.
pub const ASSEMBLED_ROOT_LINE: &str =
    "glade-node: composition root assembled from NodeAssembly (GLADE_NODE_ASSEMBLED=1)";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- construction -----------------------------------------------------------

/// The construction observer binding's interface. Each real provider tells
/// it, with its type's name, as the module constructs it: the system clock,
/// the command line, the record host as the module builds it, the iroh
/// carrier, the pending WebSocket adapter, the grant fold and the node signer.
/// The module binds [`Unobserved`]. A test composition binds a recorder of its
/// own, which sees only its own scope, and overrides every real provider, so
/// its recorder sees none (DI-E01).
pub trait Constructions: shaku::Interface {
    /// The real provider `provider`, by its type's name, was constructed.
    fn constructed(&self, provider: &'static str);
}

/// The construction observer the module binds: it keeps nothing.
#[derive(Component)]
#[shaku(interface = Constructions)]
pub struct Unobserved;

impl Constructions for Unobserved {
    fn constructed(&self, _provider: &'static str) {}
}

/// Tell the scope's construction observer that the real provider `P` was
/// constructed.
fn constructed<M, P>(context: &mut ModuleBuildContext<M>)
where
    M: Module + HasComponent<dyn Constructions>,
{
    let observer = <M as HasComponent<dyn Constructions>>::build_component(context);
    observer.constructed(type_name::<P>());
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

// One built `NodeAssembly` is one node scope: every binding in it is one
// occurrence. `#[lazy]` marks the bindings no consumer on the assembled path
// resolves before plan Phase 4. (A plain comment: `module!` parses a
// visibility first and takes no attribute before it.)
module! {
    pub NodeAssembly {
        components = [
            Unobserved,
            CommandLine,
            SystemClock,
            DirectoryRules,
            IrohCarrier,
            CarrierTransport,
            Records,
            DirectoryFacade,
            #[lazy] PendingWebSocketAdapter,
            #[lazy] RoleSessions,
            #[lazy] PolicyView,
            #[lazy] GrantAdmission,
            #[lazy] NodeSigner
        ],
        providers = []
    }
}
