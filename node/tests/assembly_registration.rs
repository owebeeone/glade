//! The positive control for `tests/assembly`: the construction observer that
//! binary's test nodes read as empty is paid for. Built with nothing
//! overridden but its observer, a recorder of this test's own,
//! `NodeAssembly` constructs its real providers, and each tells the recorder:
//! the eager ones when it is built, the `#[lazy]` ones when first resolved,
//! and none twice. And a real provider built without the handle the
//! composition root acquires refuses: it never acquires one itself (no
//! constructor fallback to real I/O, `arch1/RuntimeAndAssurance.md:73-76`).

// Step 3.2's fakes, shared with `tests/assembly`: this binary uses the recorder.
#[allow(dead_code)]
#[path = "assembly/fakes.rs"]
mod fakes;

use std::any::type_name;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use glade_carrier_api::{CarrierAddr, CarrierConfig, CarrierError, CarrierPort};
use glade_grant_api::{Denial, Holder};
use glade_node::assembly::{
    Admission, ClientCarrier, CommandLine, Constructions, Directory, Grants, HostError,
    NodeAssembly, PeerCarrier, PendingWebSocketAdapter, RecordHost, RecordHostPort, Records,
    Sessions, Signer, SystemClock,
};
use glade_node::grants::PolicyView;
use glade_node::iroh_carrier::IrohCarrier;
use glade_node::registry::{Record, HOME};
use glade_node::signing::NodeSigner;
use glade_node::sysdata::NodeRecord;
use glade_signer_api::{Purpose, SignError};
use shaku::HasComponent;

use fakes::{real_providers_constructed, Recorder};

/// The carriers, lent nothing, answer at once; one poll is enough.
fn now<T>(future: impl Future<Output = T>) -> T {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("a carrier waited"),
    }
}

#[test]
fn an_assembly_with_nothing_overridden_builds_its_real_providers_and_they_refuse() {
    let node = NodeAssembly::builder()
        .with_component_override::<dyn Constructions>(Box::new(Recorder::default()))
        .build();

    // Eager, in the module's order: the command line, the system clock, the
    // iroh adapter (which the record transport rides) and the record host.
    let mut built = vec![
        type_name::<CommandLine>(),
        type_name::<SystemClock>(),
        type_name::<IrohCarrier>(),
        type_name::<Records>(),
    ];
    assert_eq!(real_providers_constructed(&node), built);

    // Lazy: each is built on its first resolution, and only then.
    let sessions: Arc<dyn Sessions> = node.resolve();
    built.push(type_name::<PendingWebSocketAdapter>());
    assert_eq!(real_providers_constructed(&node), built, "the websocket");
    let admission: Arc<dyn Admission> = node.resolve();
    built.push(type_name::<PolicyView>());
    assert_eq!(real_providers_constructed(&node), built, "the grant fold");
    let signer: Arc<dyn Signer> = node.resolve();
    built.push(type_name::<NodeSigner>());
    assert_eq!(real_providers_constructed(&node), built, "the node signer");

    // One scope, one occurrence: resolving again builds nothing more.
    let _: Arc<dyn Directory> = node.resolve();
    let _: Arc<dyn PeerCarrier> = node.resolve();
    let _: Arc<dyn ClientCarrier> = node.resolve();
    let _: Arc<dyn Grants> = node.resolve();
    let _: Arc<dyn Signer> = node.resolve();
    assert_eq!(real_providers_constructed(&node), built);

    // No instance was lent, so the record host refuses. It does not boot one.
    let host: Arc<dyn RecordHost> = node.resolve();
    let host: Arc<dyn RecordHostPort> = host;
    assert!(matches!(host.who_serves(HOME, 0), Err(HostError::NotOpen)));
    let record = Record::Node(NodeRecord::default());
    assert!(matches!(host.append(record, "n1"), Err(HostError::NotOpen)));

    // The providers fail closed: the iroh adapter, lent no endpoint key, the
    // pending ones, and the signer, lent no key.
    let peer: Arc<dyn CarrierPort> = sessions.peer();
    let config = CarrierConfig {
        local: CarrierAddr("127.0.0.1:0".into()),
        max_frame_bytes: NonZeroUsize::new(64).unwrap(),
    };
    let unkeyed = "the iroh adapter was lent no endpoint key";
    let bound = now(peer.bind(config));
    assert_eq!(bound, Err(CarrierError::Transport(unkeyed.into())));
    let dialed = now(peer.dial(&CarrierAddr("x".into())));
    assert!(matches!(dialed, Err(CarrierError::Closed)));
    assert!(matches!(now(sessions.client().accept()), Ok(None)));
    let holder = Holder::Principal("alice".into());
    assert_eq!(
        admission.admit(&holder, "read", "ws").outcome,
        Err(Denial::Unavailable)
    );
    let grants: Arc<dyn Grants> = node.resolve();
    assert_eq!(
        grants.check(&holder, "read", "ws"),
        Err(Denial::Unavailable)
    );
    assert_eq!(
        signer.sign(Purpose::OriginOp, b"op"),
        Err(SignError::Unavailable)
    );
}
