#![cfg(feature = "dill")]
use di_eval_ports::{Clock, FakeClock};
use dill::{Catalog, Component, InjectionError};
use std::sync::Arc;

#[dill::component]
#[dill::interface(dyn Clock)]
#[dill::scope(dill::Singleton)]
struct DefaultClock;
impl Clock for DefaultClock {
    fn now(&self) -> u64 {
        0
    }
}
#[dill::component]
#[dill::interface(dyn Clock)]
#[dill::scope(dill::Singleton)]
struct OtherClock;
impl Clock for OtherClock {
    fn now(&self) -> u64 {
        9
    }
}
#[dill::component]
#[dill::scope(dill::Singleton)]
struct ReadClock {
    clock: Arc<dyn Clock>,
}
#[dill::component]
struct Fresh {
    clock: Arc<dyn Clock>,
}
#[dill::component]
#[dill::scope(dill::Transaction)]
struct Scoped {
    clock: Arc<dyn Clock>,
}

// The shared contract/fixture crate does not import Dill. Only assembly does.
#[dill::component]
#[dill::interface(dyn Clock)]
#[dill::scope(dill::Singleton)]
struct FakeProvider {
    inner: Arc<FakeClock>,
}
impl Clock for FakeProvider {
    fn now(&self) -> u64 {
        self.inner.now()
    }
}

fn fake_catalog(value: u64) -> Catalog {
    let mut b = Catalog::builder();
    b.add_builder(ReadClock::builder().with_clock(Arc::new(FakeClock::new(value))));
    b.validate().unwrap();
    b.build()
}
#[test]
fn builder_override_injects_fake_and_keeps_singleton_identity() {
    let c = fake_catalog(17);
    let a = c.get_one::<ReadClock>().unwrap();
    let b = c.get_one::<ReadClock>().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(a.clock.now(), 17);
}
#[test]
fn registration_once_is_shared_transitively() {
    let fake = Arc::new(FakeClock::new(11));
    let c = Catalog::builder()
        .add::<ReadClock>()
        .add::<Fresh>()
        .add_builder(FakeProvider::builder().with_inner(fake.clone()))
        .build();
    let a = c.get_one::<ReadClock>().unwrap();
    let b = c.get_one::<Fresh>().unwrap();
    assert!(Arc::ptr_eq(&a.clock, &b.clock));
    fake.set(99);
    assert_eq!(a.clock.now(), 99);
}
#[test]
fn independent_test_catalogs_do_not_share_fakes() {
    let a = fake_catalog(1);
    let b = fake_catalog(2);
    let x = a.get_one::<ReadClock>().unwrap();
    let y = b.get_one::<ReadClock>().unwrap();
    assert!(!Arc::ptr_eq(&x, &y));
    assert_eq!((x.clock.now(), y.clock.now()), (1, 2));
}
#[test]
fn missing_binding_is_reported_by_validation_and_resolution() {
    let mut b = Catalog::builder();
    b.add::<ReadClock>();
    assert!(b.validate().is_err());
    assert!(matches!(
        b.build().get_one::<ReadClock>(),
        Err(InjectionError::Unregistered(_))
    ));
}
#[test]
fn ambiguous_binding_is_not_rejected_by_validate_but_resolution_rejects_it() {
    let mut b = Catalog::builder();
    b.add::<DefaultClock>()
        .add::<OtherClock>()
        .add::<ReadClock>();
    assert!(b.validate().is_ok());
    assert!(matches!(
        b.build().get_one::<ReadClock>(),
        Err(InjectionError::Ambiguous(_))
    ));
}
#[test]
fn child_catalog_does_not_shadow_parent_registration() {
    let parent = Catalog::builder().add::<DefaultClock>().build();
    let child = parent.builder_chained().add::<OtherClock>().build();
    assert!(matches!(
        child.get_one::<dyn Clock>(),
        Err(InjectionError::Ambiguous(_))
    ));
    assert_eq!(parent.get_one::<dyn Clock>().unwrap().now(), 0);
}
#[test]
fn transaction_cache_reuses_inside_scope_and_isolates_siblings() {
    let root = Catalog::builder()
        .add::<DefaultClock>()
        .add::<Scoped>()
        .build();
    let a = root
        .builder_chained()
        .add_value(dill::TransactionCache::new())
        .build();
    let b = root
        .builder_chained()
        .add_value(dill::TransactionCache::new())
        .build();
    let x = a.get_one::<Scoped>().unwrap();
    let again = a.get_one::<Scoped>().unwrap();
    let y = b.get_one::<Scoped>().unwrap();
    assert!(Arc::ptr_eq(&x, &again));
    assert!(!Arc::ptr_eq(&x, &y));
    assert!(Arc::ptr_eq(&x.clock, &y.clock));
}
#[test]
fn transaction_without_cache_fails_at_resolution() {
    let root = Catalog::builder()
        .add::<DefaultClock>()
        .add::<Scoped>()
        .build();
    assert!(matches!(
        root.get_one::<Scoped>(),
        Err(InjectionError::Unregistered(_))
    ));
}
#[dill::component]
struct Short;
#[allow(dead_code)] // Deliberately invalid fixture: resolution must not be attempted.
#[dill::component]
#[dill::scope(dill::Singleton)]
struct Long {
    short: Arc<Short>,
}
#[test]
fn validation_rejects_short_lived_dependency_in_singleton() {
    let mut b = Catalog::builder();
    b.add::<Short>().add::<Long>();
    assert!(
        b.validate()
            .unwrap_err()
            .errors
            .iter()
            .any(|e| matches!(e, InjectionError::ScopeInversion(_)))
    );
}
#[allow(dead_code)] // Deliberately cyclic fixture; validate only, never resolve.
#[dill::component]
#[dill::scope(dill::Singleton)]
struct A {
    b: Arc<B>,
}
#[allow(dead_code)]
#[dill::component]
#[dill::scope(dill::Singleton)]
struct B {
    a: Arc<A>,
}
#[test]
fn validation_does_not_detect_constructor_cycle_do_not_resolve_it() {
    let mut b = Catalog::builder();
    b.add::<A>().add::<B>();
    assert!(b.validate().is_ok());
    // Deliberately do not call get: singleton locks can deadlock on recursion.
}
#[test]
fn escaped_arc_outlives_catalog_drop() {
    let c = fake_catalog(5);
    let value = c.get_one::<ReadClock>().unwrap();
    drop(c);
    assert_eq!(value.clock.now(), 5);
}
#[test]
fn transient_objects_are_fresh_but_share_the_clock() {
    let c = Catalog::builder()
        .add::<DefaultClock>()
        .add::<Fresh>()
        .build();
    let a = c.get_one::<Fresh>().unwrap();
    let b = c.get_one::<Fresh>().unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
    assert!(Arc::ptr_eq(&a.clock, &b.clock));
}

#[test]
fn parent_singleton_can_capture_first_child_dependency_and_leak_to_sibling() {
    let root = Catalog::builder().add::<ReadClock>().build();
    let mut first = root.builder_chained();
    first.add_builder(FakeProvider::builder().with_inner(Arc::new(FakeClock::new(1))));
    assert!(first.validate().is_ok());
    let mut second = root.builder_chained();
    second.add_builder(FakeProvider::builder().with_inner(Arc::new(FakeClock::new(2))));
    assert!(second.validate().is_ok());
    let a = first.build().get_one::<ReadClock>().unwrap();
    let b = second.build().get_one::<ReadClock>().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!((a.clock.now(), b.clock.now()), (1, 1));
}

#[test]
fn transaction_cache_can_construct_twice_during_concurrent_first_resolution() {
    use dill::Scope;
    use std::{sync::mpsc, thread, time::Duration};

    // Probe the actual built-in scope, without production I/O or sleeps. Every
    // wait is bounded, including if a future version serializes construction.
    let scope = Arc::new(dill::Transaction::new());
    let catalog = Catalog::builder()
        .add_value(dill::TransactionCache::new())
        .build();
    let (entered_tx, entered_rx) = mpsc::channel();
    let mut releases = Vec::new();
    let mut workers = Vec::new();
    for value in 0..2usize {
        let scope = scope.clone();
        let catalog = catalog.clone();
        let entered = entered_tx.clone();
        let (release_tx, release_rx) = mpsc::channel();
        releases.push(release_tx);
        workers.push(thread::spawn(move || {
            scope
                .get_or_create(&catalog, || {
                    entered.send(()).unwrap();
                    let _ = release_rx.recv_timeout(Duration::from_secs(2));
                    Ok(Arc::new(value))
                })
                .unwrap()
        }));
    }
    let both_entered = entered_rx.recv_timeout(Duration::from_secs(2)).is_ok()
        && entered_rx.recv_timeout(Duration::from_secs(2)).is_ok();
    for release in releases {
        let _ = release.send(());
    }
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(
        both_entered,
        "Characterized duplicate construction no longer reproduced"
    );
    assert!(!Arc::ptr_eq(&results[0], &results[1]));
}
