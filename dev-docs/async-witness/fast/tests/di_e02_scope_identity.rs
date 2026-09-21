//! **DI-E02** — "Equal scoped binding → shared identity, independent test
//! scope → isolation, also during concurrent first access"
//! (`DependencyInjectionEvaluation.md:134`).
//!
//! Three separable claims:
//!
//! 1. **Shared identity within one scope.** Every resolution of an interface
//!    from one built module is the same occurrence, and so is every injected
//!    copy a consumer holds.
//! 2. **Isolation across scopes.** Two independently built modules share
//!    nothing, by pointer and by behaviour: a frame sent through one scope's
//!    carrier is invisible to the other's, and advancing one scope's clock
//!    leaves the other where it was.
//! 3. **One construction under a concurrent first access.** A `#[lazy]`
//!    component's first resolution is raced by two threads at a barrier; the
//!    construction counter must read exactly 1 and both threads must hold the
//!    same occurrence. Shaku's `thread_safe` feature backs a lazy slot with
//!    `std::sync::OnceLock` (`shaku-0.6.3/src/lib.rs:41-50`), so this is
//!    witnessable rather than hopeless — the evaluation recorded Dill failing
//!    the same probe.
//!
//! Two threads and a barrier are a probe, not a proposed Glade threading
//! architecture. A passing race is evidence that this construction is atomic,
//! not a proof that every interleaving was explored.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use async_witness_fast::{
    Carrier, Clock, FastComposition, Journal, LazyComposition, Session, Store, Supervisor,
    resolve_now,
};
use async_witness_ports::{
    CarrierPort, ClockPort, FakeCarrier, FakeClock, FakeStore, FrameType, StorePort,
};
use shaku::HasComponent;

fn composition(clock: &FakeClock, carrier: &FakeCarrier, store: &FakeStore) -> FastComposition {
    FastComposition::builder()
        .with_component_override::<dyn Clock>(Box::new(clock.clone()))
        .with_component_override::<dyn Carrier>(Box::new(carrier.clone()))
        .with_component_override::<dyn Store>(Box::new(store.clone()))
        .build()
}

// `HasComponent` is generic in the interface rather than the method, so each
// resolution names its facade in a binding and upcasts to the port on return —
// the use site's view, which is the one the comparisons below are about.
fn clock_of(module: &FastComposition) -> Arc<dyn ClockPort> {
    let facade: Arc<dyn Clock> = module.resolve();
    facade
}

fn carrier_of(module: &FastComposition) -> Arc<dyn CarrierPort> {
    let facade: Arc<dyn Carrier> = module.resolve();
    facade
}

fn store_of(module: &FastComposition) -> Arc<dyn StorePort> {
    let facade: Arc<dyn Store> = module.resolve();
    facade
}

#[test]
fn one_scope_hands_out_one_occurrence_of_everything() {
    let module = composition(&FakeClock::new(2), &FakeCarrier::new(), &FakeStore::new());

    let clock_first: Arc<dyn Clock> = module.resolve();
    let clock_again: Arc<dyn Clock> = module.resolve();
    assert!(Arc::ptr_eq(&clock_first, &clock_again));

    let carrier_first: Arc<dyn Carrier> = module.resolve();
    let carrier_again: Arc<dyn Carrier> = module.resolve();
    assert!(Arc::ptr_eq(&carrier_first, &carrier_again));

    let store_first: Arc<dyn Store> = module.resolve();
    let store_again: Arc<dyn Store> = module.resolve();
    assert!(Arc::ptr_eq(&store_first, &store_again));

    let session: Arc<dyn Session> = module.resolve();
    let session_again: Arc<dyn Session> = module.resolve();
    assert!(Arc::ptr_eq(&session, &session_again));

    let journal: Arc<dyn Journal> = module.resolve();
    let supervisor: Arc<dyn Supervisor> = module.resolve();
    assert!(Arc::ptr_eq(&session, &supervisor.session()));
    assert!(Arc::ptr_eq(&journal, &supervisor.journal()));
}

#[test]
fn two_independently_built_scopes_share_nothing() {
    let (clock_a, carrier_a, store_a) = (FakeClock::new(10), FakeCarrier::new(), FakeStore::new());
    let (clock_b, carrier_b, store_b) = (FakeClock::new(20), FakeCarrier::new(), FakeStore::new());
    let a = composition(&clock_a, &carrier_a, &store_a);
    let b = composition(&clock_b, &carrier_b, &store_b);

    let (clock_in_a, clock_in_b) = (clock_of(&a), clock_of(&b));
    assert!(!Arc::ptr_eq(&clock_in_a, &clock_in_b));

    let (carrier_in_a, carrier_in_b) = (carrier_of(&a), carrier_of(&b));
    assert!(!Arc::ptr_eq(&carrier_in_a, &carrier_in_b));

    let (store_in_a, store_in_b) = (store_of(&a), store_of(&b));
    assert!(!Arc::ptr_eq(&store_in_a, &store_in_b));

    let session_in_a: Arc<dyn Session> = a.resolve();
    let session_in_b: Arc<dyn Session> = b.resolve();
    assert!(!Arc::ptr_eq(&session_in_a, &session_in_b));

    // Isolation by behaviour, which is what a test composition actually needs:
    // one scope's traffic, time and appends stay inside it.
    assert_eq!(session_in_a.send_stamped(FrameType::Ops), Ok(10));
    assert_eq!(carrier_a.sent().len(), 1);
    assert!(carrier_b.sent().is_empty());

    clock_a.advance(5);
    assert_eq!(clock_in_a.now_ms(), 15);
    assert_eq!(clock_in_b.now_ms(), 20);

    let journal_in_a: Arc<dyn Journal> = a.resolve();
    assert_eq!(journal_in_a.record("share-a", b"one"), Ok(1));
    assert!(store_b.scan("share-a", 1).is_empty());
    assert_eq!(resolve_now(carrier_in_b.recv()), Ok(None));
}

#[test]
fn a_concurrent_first_access_of_a_lazy_component_constructs_once() {
    let constructions = Arc::new(AtomicUsize::new(0));
    let counted = constructions.clone();
    let module = LazyComposition::builder()
        .with_component_override::<dyn Clock>(Box::new(FakeClock::new(0)))
        .with_component_override::<dyn Carrier>(Box::new(FakeCarrier::new()))
        .with_component_override_fn::<dyn Store>(Box::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::new(FakeStore::new())
        }))
        .build();
    // Deferred, so the race below really is the first access.
    assert_eq!(constructions.load(Ordering::SeqCst), 0);

    let gate = Barrier::new(2);
    let (first, second) = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            gate.wait();
            let store: Arc<dyn Store> = module.resolve();
            store
        });
        let right = scope.spawn(|| {
            gate.wait();
            let store: Arc<dyn Store> = module.resolve();
            store
        });
        (
            left.join().expect("left racer"),
            right.join().expect("right racer"),
        )
    });

    assert_eq!(
        constructions.load(Ordering::SeqCst),
        1,
        "a lazy component was constructed more than once under a concurrent first access"
    );
    assert!(Arc::ptr_eq(&first, &second));
}
