//! Step 1.1 — the bridge, over the witness's own ports rather than a synthetic
//! one: an assembly-only facade trait carrying Shaku's `Interface` as a
//! supertrait, a blanket impl over every port implementation, and trait
//! upcasting back to the port at the use site.
//!
//! **What this proves.** Shaku can inject the witness's Glade-shaped ports
//! while `async-witness-ports` keeps its single `glade-wire` dependency and
//! never names a framework. The facade is unavoidable rather than stylistic:
//! `shaku::Interface`'s blanket impl has no `?Sized`, so a foreign trait object
//! such as `dyn CarrierPort` can never acquire `Interface` on its own
//! (`AsyncWitnessPlan.md` §3.5, `di-eval/examples/foreign_port_shaku.rs`).
//!
//! **What it does not prove.** Nothing here touches a real provider, a runtime
//! or a socket; the carrier is a recording fake whose futures are already
//! complete. The real async port is Phase 3's work, and DI-E01's "every
//! relevant caller" claim is Step 1.2's, not this file's.

use std::sync::Arc;

use async_witness_fast::{
    Carrier, Clock, FastComposition, Journal, Session, Store, Supervisor, resolve_now,
};
use async_witness_ports::{
    CarrierPort, ClockPort, FakeCarrier, FakeClock, FakeStore, FrameType, StorePort,
};
use shaku::HasComponent;

/// One composition with all three ports bound to the supplied fakes. Every
/// interface the module registers a stand-in real provider for is overridden
/// here, so nothing in the returned module can reach a provider that would do
/// I/O if this crate could compile one.
fn composition(clock: &FakeClock, carrier: &FakeCarrier, store: &FakeStore) -> FastComposition {
    FastComposition::builder()
        .with_component_override::<dyn Clock>(Box::new(clock.clone()))
        .with_component_override::<dyn Carrier>(Box::new(carrier.clone()))
        .with_component_override::<dyn Store>(Box::new(store.clone()))
        .build()
}

#[test]
fn the_module_resolves_the_facade_and_the_use_site_receives_the_port() {
    let carrier = FakeCarrier::new();
    let module = composition(&FakeClock::new(7), &carrier, &FakeStore::new());

    let facade: Arc<dyn Carrier> = module.resolve();
    // The one line the whole bridge exists for: stable trait upcasting gives
    // the caller back the framework-free port, so no consumer signature, and no
    // crate the consumer depends on, has to name Shaku.
    let port: Arc<dyn CarrierPort> = facade;

    assert_eq!(resolve_now(port.send(FrameType::NodeHello, b"hi")), Ok(()));
    assert_eq!(carrier.sent(), vec![(FrameType::NodeHello, b"hi".to_vec())]);
}

#[test]
fn the_clock_and_store_cross_the_same_bridge() {
    let clock = FakeClock::new(11);
    let store = FakeStore::new();
    let module = composition(&clock, &FakeCarrier::new(), &store);

    // `HasComponent` is generic in the interface, not the method, so the
    // facade is named by the binding's type and upcast on the next line.
    let clock_facade: Arc<dyn Clock> = module.resolve();
    let clock_port: Arc<dyn ClockPort> = clock_facade;
    let store_facade: Arc<dyn Store> = module.resolve();
    let store_port: Arc<dyn StorePort> = store_facade;

    assert_eq!(clock_port.now_ms(), 11);
    clock.advance(4);
    assert_eq!(clock_port.now_ms(), 15);
    assert_eq!(store_port.append("share-a", b"body"), Ok(1));
    assert_eq!(store_port.scan("share-a", 1), vec![b"body".to_vec()]);
}

#[test]
fn an_injected_consumer_hands_its_caller_the_port_not_the_facade() {
    let carrier = FakeCarrier::new();
    let module = composition(&FakeClock::new(1_000), &carrier, &FakeStore::new());

    let session: Arc<dyn Session> = module.resolve();
    // `Session::carrier` is declared to return `Arc<dyn CarrierPort>`: the
    // consumer's own public signature stays framework-free even though the
    // field Shaku filled is an `Arc<dyn Carrier>`.
    let port: Arc<dyn CarrierPort> = session.carrier();
    assert_eq!(resolve_now(port.recv()), Ok(None));

    assert_eq!(session.send_stamped(FrameType::Ops), Ok(1_000));
    assert_eq!(
        carrier.sent(),
        vec![(FrameType::Ops, 1_000_i64.to_be_bytes().to_vec())]
    );
}

#[test]
fn a_transitive_consumer_is_assembled_through_the_same_facades() {
    let clock = FakeClock::new(5);
    let store = FakeStore::new();
    let module = composition(&clock, &FakeCarrier::new(), &store);

    let supervisor: Arc<dyn Supervisor> = module.resolve();
    let journal: Arc<dyn Journal> = supervisor.journal();

    assert_eq!(journal.record("share-a", b"one"), Ok(1));
    assert_eq!(supervisor.session().clock().now_ms(), 5);
    assert_eq!(store.scan("share-a", 1), vec![b"one".to_vec()]);
}
