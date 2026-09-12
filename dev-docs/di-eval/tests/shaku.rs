#![cfg(feature = "shaku")]
use di_eval_ports::{Clock as ClockPort, FakeClock, Reader as ReaderPort};
use shaku::{Component, HasComponent, HasProvider, module};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

// Assembly-only facades: foreign dyn port traits do not implement Shaku's
// Interface, even with Any + Send + Sync. Keep that dependency out of ports.
trait Clock: ClockPort + shaku::Interface {}
impl<T: ClockPort> Clock for T {}
trait Reader: ReaderPort + shaku::Interface {}
impl<T: ReaderPort> Reader for T {}

#[derive(Component)]
#[shaku(interface = Clock)]
struct DefaultClock;
impl ClockPort for DefaultClock {
    fn now(&self) -> u64 {
        0
    }
}

#[derive(Component)]
#[shaku(interface = Reader)]
struct ReadClock {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
}
impl ReaderPort for ReadClock {
    fn clock(&self) -> Arc<dyn ClockPort> {
        self.clock.clone()
    }
}
module! { TestModule { components = [DefaultClock, ReadClock], providers = [] } }

#[test]
fn fake_override_is_shared_transitively_without_changing_port_crate() {
    let fake = FakeClock::new(17);
    let module = TestModule::builder()
        .with_component_override::<dyn Clock>(Box::new(fake.clone()))
        .build();
    let clock: Arc<dyn Clock> = module.resolve();
    let clock: Arc<dyn ClockPort> = clock;
    let reader: Arc<dyn Reader> = module.resolve();
    assert!(Arc::ptr_eq(&clock, &reader.clock()));
    assert_eq!(reader.clock().now(), 17);
    fake.set(31);
    assert_eq!(reader.clock().now(), 31);
    let again: Arc<dyn Reader> = module.resolve();
    assert!(Arc::ptr_eq(&reader, &again));
}

#[test]
fn independently_built_test_modules_do_not_share_fakes() {
    let a = TestModule::builder()
        .with_component_override::<dyn Clock>(Box::new(FakeClock::new(1)))
        .build();
    let b = TestModule::builder()
        .with_component_override::<dyn Clock>(Box::new(FakeClock::new(2)))
        .build();
    let x: Arc<dyn Clock> = a.resolve();
    let y: Arc<dyn Clock> = b.resolve();
    assert!(!Arc::ptr_eq(&x, &y));
    assert_eq!((x.now(), y.now()), (1, 2));
}

#[test]
fn escaped_arc_outlives_module_drop() {
    let module = TestModule::builder().build();
    let reader: Arc<dyn Reader> = module.resolve();
    drop(module);
    assert_eq!(reader.clock().now(), 0);
}

trait Unused: shaku::Interface {
    fn value(&self) -> usize;
}
#[derive(Component)]
#[shaku(interface = Unused)]
struct UnusedImpl;
impl Unused for UnusedImpl {
    fn value(&self) -> usize {
        0
    }
}
module! { BroadModule { components=[DefaultClock,ReadClock,UnusedImpl], providers=[] } }
#[test]
fn overriding_a_service_does_not_prune_other_registered_components() {
    let constructions = Arc::new(AtomicUsize::new(0));
    let seen = constructions.clone();
    let module = BroadModule::builder()
        .with_component_override::<dyn Clock>(Box::new(FakeClock::new(8)))
        .with_component_override_fn::<dyn Unused>(Box::new(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::new(UnusedImpl)
        }))
        .build();
    let reader: Arc<dyn Reader> = module.resolve();
    assert_eq!(reader.clock().now(), 8);
    assert_eq!(constructions.load(Ordering::SeqCst), 1);
    let unused: Arc<dyn Unused> = module.resolve();
    assert_eq!(unused.value(), 0);
}

struct Fresh;
impl<M: shaku::Module + HasComponent<dyn Clock>> shaku::Provider<M> for Fresh {
    type Interface = di_eval_ports::ClockHolder;
    fn provide(module: &M) -> Result<Box<Self::Interface>, Box<dyn std::error::Error>> {
        let clock: Arc<dyn Clock> = module.resolve();
        Ok(Box::new(di_eval_ports::ClockHolder(clock)))
    }
}
module! { ProviderModule { components=[DefaultClock], providers=[Fresh] } }
#[test]
fn fresh_providers_share_module_clock_but_not_holder_identity() {
    let m = ProviderModule::builder().build();
    let a: Box<di_eval_ports::ClockHolder> = m.provide().unwrap();
    let b: Box<di_eval_ports::ClockHolder> = m.provide().unwrap();
    assert!(!std::ptr::eq(a.as_ref(), b.as_ref()));
    assert!(Arc::ptr_eq(&a.0, &b.0));
}

#[test]
fn provider_failure_is_returned() {
    let m = ProviderModule::builder()
        .with_provider_override::<di_eval_ports::ClockHolder>(Box::new(|_| {
            Err(std::io::Error::other("fixture failure").into())
        }))
        .build();
    let outcome: Result<Box<di_eval_ports::ClockHolder>, _> = m.provide();
    assert_eq!(outcome.err().unwrap().to_string(), "fixture failure");
}

trait ClockScope: HasComponent<dyn Clock> {}
module! { ClockScopeImpl: ClockScope { components=[DefaultClock], providers=[] } }
module! { Child { components=[ReadClock], providers=[], use dyn ClockScope { components=[dyn Clock], providers=[] } } }
#[test]
fn child_modules_share_parent_provider_not_child_components() {
    let parent = Arc::new(
        ClockScopeImpl::builder()
            .with_component_override::<dyn Clock>(Box::new(FakeClock::new(42)))
            .build(),
    );
    let a = Child::builder(parent.clone()).build();
    let b = Child::builder(parent).build();
    let x: Arc<dyn Reader> = a.resolve();
    let y: Arc<dyn Reader> = b.resolve();
    assert!(!Arc::ptr_eq(&x, &y));
    assert!(Arc::ptr_eq(&x.clock(), &y.clock()));
}

// Derive-based constructor cycles fail compilation; see examples/cycle_shaku.rs.

module! { LazyBroad { components=[DefaultClock,ReadClock,#[lazy] UnusedImpl], providers=[] } }

#[test]
fn lazy_registration_defers_unused_construction_until_resolution() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let module = LazyBroad::builder()
        .with_component_override_fn::<dyn Unused>(Box::new(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::new(UnusedImpl)
        }))
        .build();
    let reader: Arc<dyn Reader> = module.resolve();
    assert_eq!(reader.clock().now(), 0);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let unused: Arc<dyn Unused> = module.resolve();
    assert_eq!(unused.value(), 0);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Role {
    Peer,
    Client,
    Missing,
}
impl shaku::Keyed for DefaultClock {
    type KeyType = Role;
    const KEY: Role = Role::Peer;
}
#[derive(Component)]
#[shaku(interface = Clock)]
struct OtherClock;
impl ClockPort for OtherClock {
    fn now(&self) -> u64 {
        9
    }
}
impl shaku::Keyed for OtherClock {
    type KeyType = Role;
    const KEY: Role = Role::Client;
}
module! {
    Qualified {
        components=[#[keyed(dyn Clock, Role)] DefaultClock, #[keyed(dyn Clock, Role)] OtherClock],
        providers=[]
    }
}
#[test]
fn keyed_multibinding_distinguishes_two_roles_with_one_contract() {
    use shaku::HasComponentMap;
    let module = Qualified::builder().build();
    let values = module.resolve_map();
    assert_eq!(values.get(&Role::Peer).unwrap().now(), 0);
    assert_eq!(values.get(&Role::Client).unwrap().now(), 9);
    assert!(!values.contains_key(&Role::Missing));
}
