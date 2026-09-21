//! Step 3.3's positive control: the counter `tests/shaku_assembly.rs` reads as
//! zero is **not vacuous**.
//!
//! A registered component that is never built and a registered component that
//! does not exist read the same way from a counter, so the zero has to be paid
//! for. This file builds the same module with **nothing overridden** and shows
//! the registration constructing itself, then shows that the thing it
//! constructs would be useless — every port method panics, because a
//! synchronous `build` cannot bind a socket.
//!
//! It is a test binary of its own because `BINDING_CARRIER_BUILDS` is
//! process-wide: the file that asserts zero must never share a process with the
//! file that deliberately drives it above zero. Phase 1 split
//! `fast/tests/di_e01_eager_construction.rs` out for exactly this reason.

use std::sync::Arc;

use async_witness_ports::{CarrierPort, FrameType};
use async_witness_real::shaku_bridge::{
    BindingCarrier, Carrier, PeerSession, RealComposition, binding_carrier_builds, resolve_carrier,
};
use shaku::HasComponent;

/// One ordered test, so the readings cannot interleave.
#[test]
fn the_registration_builds_itself_when_nothing_overrides_it() {
    assert_eq!(binding_carrier_builds(), 0);

    // No override. `LinkSession` injects `dyn Carrier`, so Shaku reaches the
    // registered build function — which is the one Step 3.3 must keep out.
    let module = RealComposition::builder().build();
    assert_eq!(
        binding_carrier_builds(),
        1,
        "the registration was expected to construct itself"
    );

    // Within one module the occurrence is shared, so resolving does not build
    // a second one.
    let session: Arc<dyn PeerSession> = module.resolve();
    let facade: Arc<dyn Carrier> = module.resolve();
    let port: Arc<dyn CarrierPort> = facade;
    assert!(Arc::ptr_eq(&session.carrier(), &port));
    assert!(Arc::ptr_eq(&resolve_carrier(&module), &port));
    assert_eq!(binding_carrier_builds(), 1);

    // A second, independently built module gets its own occurrence: the count
    // is per module, so "zero" in Step 3.3 means no module built one at all.
    let other = RealComposition::builder().build();
    assert_eq!(binding_carrier_builds(), 2);
    assert!(!Arc::ptr_eq(&resolve_carrier(&other), &port));
}

/// What the registered stand-in would be if it were ever reached: nothing
/// usable. A synchronous `build` cannot await a UDP bind, so the provider a
/// production composition would register for `dyn Carrier` has no endpoint and
/// says so loudly rather than returning a plausible-looking failure.
///
/// This constructs the type directly rather than through a module, so it
/// leaves the build counter alone.
#[test]
#[should_panic(expected = "BindingCarrier::send would need an endpoint it never bound")]
fn the_stand_in_provider_cannot_be_used() {
    // `send` panics before it can return a future, so nothing is ever awaited
    // and this test needs no runtime.
    let never_returned = BindingCarrier.send(FrameType::Ops, b"unreachable");
    drop(never_returned);
}
