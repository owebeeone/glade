//! **DI-E01** — "One fake clock/store/carrier selection reaches every relevant
//! caller; real-provider construction and hidden I/O are rejected in the fast
//! composition" (`DependencyInjectionEvaluation.md:133`).
//!
//! Three claims, each asserted separately so the evidence cannot be read as
//! more than it is:
//!
//! 1. **One selection, every caller.** `Arc::ptr_eq` across the module's own
//!    resolution and every declared consumer, direct and transitive. Pointer
//!    identity alone would be a weak claim, so each port is also *used* — the
//!    instant every consumer reads moves when the one fake advances, and the
//!    frames the session sends land in the one fake carrier's record.
//! 2. **No real provider is constructed.** Each port's registered stand-in
//!    counts its own construction; the counter is read after everything has
//!    been resolved.
//! 3. **No hidden I/O.** The manifest half is mechanical and outside this file:
//!    the crate declares `async-witness-ports` and `shaku`, the architecture
//!    gate is an allowlist that fails closed on anything else, and no `#[cfg]`
//!    can reach around a manifest. The half a manifest cannot cover — `std::net`
//!    needs no dependency entry — is asserted here over the crate's source.
//!
//! **Scope, stated because the plan requires it.** The node has no clock trait
//! and its store is synchronous with no port trait, so the clock and the store
//! are the witness's own ports and nothing here claims otherwise. Only the
//! carrier has a real Glade provider behind it, and that provider is Phase 3's
//! subject, not this file's.

use std::sync::Arc;

use async_witness_fast::{
    Carrier, Clock, FastComposition, Journal, Session, Store, Supervisor, real_provider_builds,
    resolve_now,
};
use async_witness_ports::{
    CarrierPort, ClockPort, FakeCarrier, FakeClock, FakeStore, FrameType, StorePort,
};
use shaku::HasComponent;

/// The one selection under test: every interface that has a stand-in real
/// provider registered is overridden by a fake.
fn composition(clock: &FakeClock, carrier: &FakeCarrier, store: &FakeStore) -> FastComposition {
    FastComposition::builder()
        .with_component_override::<dyn Clock>(Box::new(clock.clone()))
        .with_component_override::<dyn Carrier>(Box::new(carrier.clone()))
        .with_component_override::<dyn Store>(Box::new(store.clone()))
        .build()
}

fn consumers(
    module: &FastComposition,
) -> (Arc<dyn Session>, Arc<dyn Journal>, Arc<dyn Supervisor>) {
    (module.resolve(), module.resolve(), module.resolve())
}

#[test]
fn one_clock_selection_reaches_every_declared_caller() {
    let clock = FakeClock::new(1_000);
    let module = composition(&clock, &FakeCarrier::new(), &FakeStore::new());
    let (session, journal, supervisor) = consumers(&module);

    let facade: Arc<dyn Clock> = module.resolve();
    let selected: Arc<dyn ClockPort> = facade;
    for (caller, reached) in [
        ("session", session.clock()),
        ("journal", journal.clock()),
        ("supervisor.session", supervisor.session().clock()),
        ("supervisor.journal", supervisor.journal().clock()),
    ] {
        assert!(
            Arc::ptr_eq(&selected, &reached),
            "the clock selection did not reach {caller}"
        );
    }

    // Identity is not the whole claim: one instance means one instant.
    clock.advance(45);
    assert_eq!(session.clock().now_ms(), 1_045);
    assert_eq!(supervisor.journal().clock().now_ms(), 1_045);
    assert_eq!(real_provider_builds(), 0);
}

#[test]
fn one_carrier_selection_reaches_every_declared_caller() {
    let carrier = FakeCarrier::new();
    let module = composition(&FakeClock::new(7), &carrier, &FakeStore::new());
    let (session, _, supervisor) = consumers(&module);

    let facade: Arc<dyn Carrier> = module.resolve();
    let selected: Arc<dyn CarrierPort> = facade;
    for (caller, reached) in [
        ("session", session.carrier()),
        ("supervisor", supervisor.carrier()),
        ("supervisor.session", supervisor.session().carrier()),
    ] {
        assert!(
            Arc::ptr_eq(&selected, &reached),
            "the carrier selection did not reach {caller}"
        );
    }

    // Every send, from whichever caller, lands in the one recording fake.
    assert_eq!(session.send_stamped(FrameType::NodeHello), Ok(7));
    assert_eq!(supervisor.session().send_stamped(FrameType::Ops), Ok(7));
    assert_eq!(
        resolve_now(supervisor.carrier().send(FrameType::Heads, b"z")),
        Ok(())
    );
    assert_eq!(
        carrier.sent(),
        vec![
            (FrameType::NodeHello, 7_i64.to_be_bytes().to_vec()),
            (FrameType::Ops, 7_i64.to_be_bytes().to_vec()),
            (FrameType::Heads, b"z".to_vec()),
        ]
    );
    assert_eq!(real_provider_builds(), 0);
}

#[test]
fn one_store_selection_reaches_every_declared_caller() {
    let store = FakeStore::new();
    let module = composition(&FakeClock::new(0), &FakeCarrier::new(), &store);
    let (_, journal, supervisor) = consumers(&module);

    let facade: Arc<dyn Store> = module.resolve();
    let selected: Arc<dyn StorePort> = facade;
    for (caller, reached) in [
        ("journal", journal.store()),
        ("supervisor.journal", supervisor.journal().store()),
    ] {
        assert!(
            Arc::ptr_eq(&selected, &reached),
            "the store selection did not reach {caller}"
        );
    }

    assert_eq!(journal.record("share-a", b"one"), Ok(1));
    assert_eq!(supervisor.journal().record("share-a", b"two"), Ok(2));
    assert_eq!(
        store.scan("share-a", 1),
        vec![b"one".to_vec(), b"two".to_vec()]
    );
    assert_eq!(real_provider_builds(), 0);
}

/// The consumers are shared, not re-created per resolution: the transitive
/// caller holds the very same services the module hands out.
#[test]
fn the_consumers_themselves_are_one_occurrence_each() {
    let module = composition(&FakeClock::new(0), &FakeCarrier::new(), &FakeStore::new());
    let (session, journal, supervisor) = consumers(&module);

    assert!(Arc::ptr_eq(&session, &supervisor.session()));
    assert!(Arc::ptr_eq(&journal, &supervisor.journal()));
    let again: Arc<dyn Supervisor> = module.resolve();
    assert!(Arc::ptr_eq(&supervisor, &again));
    assert_eq!(real_provider_builds(), 0);
}

/// Building and draining the whole composition constructs no stand-in real
/// provider at all — not the overridden clock, not the carrier, not the store.
#[test]
fn no_stand_in_real_provider_is_constructed_in_this_process() {
    let module = composition(&FakeClock::new(3), &FakeCarrier::new(), &FakeStore::new());
    let (session, journal, supervisor) = consumers(&module);
    let _ = (session.clock(), journal.store(), supervisor.carrier());

    // Every test in this binary overrides all three ports, so this reading is
    // process-wide and cannot be made true by test ordering. A stand-in that
    // *was* reached would have panicked in the method call above rather than
    // returning a value.
    assert_eq!(real_provider_builds(), 0);
}

/// The half of "no hidden I/O" that a manifest gate cannot cover.
///
/// `tokio` and `iroh` are excluded mechanically by the architecture gate's
/// allowlist, where no `#[cfg]` can reach around them. `std::net` is different:
/// it is in the standard library, so a blocking socket needs no manifest entry
/// at all. This asserts the source names none of it, at compile time through
/// `include_str!`, so the test itself does no file I/O (LBT-008).
///
/// It is textual, and it covers this crate's single source file. `tokio` and
/// `iroh` are deliberately **not** in the list: they are manifest-level facts
/// the gate already decides, and a crate that may not name them in a comment
/// could not explain why. Naming a socket API in a comment would trip this,
/// which is the intended failure — naming one in the fast composition is
/// exactly what should be reviewed.
#[test]
fn the_fast_composition_names_no_socket_api() {
    const SOURCE: &str = include_str!("../src/lib.rs");
    for forbidden in ["std::net", "TcpStream", "TcpListener", "UdpSocket"] {
        assert!(
            !SOURCE.contains(forbidden),
            "the fast composition's source names {forbidden}"
        );
    }
}
