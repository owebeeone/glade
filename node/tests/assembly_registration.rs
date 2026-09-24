//! The positive control for `tests/assembly`: the construction counter that
//! binary reads as zero is paid for. Built with nothing overridden,
//! `NodeAssembly` constructs its real providers: the eager ones when it is
//! built, the `#[lazy]` ones when first resolved, and none twice. And a real
//! provider built without the handle the composition root acquires refuses:
//! it never acquires one itself (no constructor fallback to real I/O,
//! `arch1/RuntimeAndAssurance.md:73-76`).
//!
//! A test binary of its own, because the counter is process-wide: the binary
//! that asserts zero must never share a process with this one. One test, so
//! the readings cannot interleave.

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use glade_carrier_api::{CarrierAddr, CarrierConfig, CarrierError, CarrierPort};
use glade_grant_api::{Denial, Holder};
use glade_node::assembly::{
    real_providers_constructed, Admission, ClientCarrier, Directory, Grants, HostError,
    NodeAssembly, PeerCarrier, RecordHost, RecordHostPort, Sessions, Signer,
};
use glade_node::registry::{Record, HOME};
use glade_node::sysdata::NodeRecord;
use glade_signer_api::{Purpose, SignError};
use shaku::HasComponent;

/// The pending carriers answer at once; one poll is enough.
fn now<T>(future: impl Future<Output = T>) -> T {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("a pending carrier waited"),
    }
}

#[test]
fn an_assembly_with_nothing_overridden_builds_its_real_providers_and_they_refuse() {
    assert_eq!(real_providers_constructed(), 0);

    // Eager: the command line, the system clock, the pending iroh adapter
    // (which the record transport rides) and the record host.
    let node = NodeAssembly::builder().build();
    assert_eq!(real_providers_constructed(), 4);

    // Lazy: each is built on its first resolution, and only then.
    let sessions: Arc<dyn Sessions> = node.resolve();
    assert_eq!(
        real_providers_constructed(),
        5,
        "the pending websocket adapter"
    );
    let admission: Arc<dyn Admission> = node.resolve();
    assert_eq!(real_providers_constructed(), 6, "the pending grant fold");
    let signer: Arc<dyn Signer> = node.resolve();
    assert_eq!(real_providers_constructed(), 7, "the pending node signer");

    // One scope, one occurrence: resolving again builds nothing more.
    let _: Arc<dyn Directory> = node.resolve();
    let _: Arc<dyn PeerCarrier> = node.resolve();
    let _: Arc<dyn ClientCarrier> = node.resolve();
    let _: Arc<dyn Grants> = node.resolve();
    let _: Arc<dyn Signer> = node.resolve();
    assert_eq!(real_providers_constructed(), 7);

    // No instance was lent, so the record host refuses. It does not boot one.
    let host: Arc<dyn RecordHost> = node.resolve();
    let host: Arc<dyn RecordHostPort> = host;
    assert!(matches!(host.who_serves(HOME, 0), Err(HostError::NotOpen)));
    let record = Record::Node(NodeRecord::default());
    assert!(matches!(host.append(record, "n1"), Err(HostError::NotOpen)));

    // The pending providers fail closed.
    let peer: Arc<dyn CarrierPort> = sessions.peer();
    let config = CarrierConfig {
        local: CarrierAddr("127.0.0.1:0".into()),
        max_frame_bytes: NonZeroUsize::new(64).unwrap(),
    };
    assert!(matches!(
        now(peer.bind(config)),
        Err(CarrierError::Transport(_))
    ));
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
