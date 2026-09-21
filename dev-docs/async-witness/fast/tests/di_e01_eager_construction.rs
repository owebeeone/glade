//! **DI-E01, caveat 2** — `AsyncWitnessPlan.md` §8.4: "Shaku's default module
//! constructs registered components even when another is overridden; `#[lazy]`
//! or a separate module defers it — already recorded at
//! `DependencyInjectionEvaluation.md:41`. Using either is a caveat."
//!
//! A caveat has to be written down **with its evidence** or it is a failure, so
//! this file produces the evidence rather than citing the evaluation. It is a
//! test binary of its own because the construction counters are process-wide:
//! the file that asserts zero must never share a process with the file that
//! deliberately drives them above zero.
//!
//! What it shows, in one ordered test so the readings cannot interleave:
//!
//! - an **overridden** registration is not constructed — the override is
//!   consulted before the registered build function, so selecting a fake is
//!   enough to keep a real provider out;
//! - an **unreferenced** registration *is* constructed at `build()` anyway,
//!   even though nothing in the module injects its interface;
//! - registering it `#[lazy]` defers that construction to first resolution.
//!
//! The consequence for the witness is narrow and worth stating plainly: DI-E01
//! passes here because every port a test binds is overridden, not because Shaku
//! prunes what a composition does not use. A composition that registers a real
//! provider it does not override gets it built.

use std::sync::Arc;

use async_witness_fast::{
    Carrier, Clock, EagerComposition, LazyComposition, Store, directory_store_builds,
    endpoint_carrier_builds, wall_clock_builds,
};
use async_witness_ports::{FakeCarrier, FakeClock};
use shaku::HasComponent;

#[test]
fn a_registered_stand_in_is_built_at_module_build_unless_it_is_lazy() {
    assert_eq!(directory_store_builds(), 0);

    // Nothing in either composition injects `dyn Store`: the journal is not
    // registered there, so the store is a pure bystander.
    let eager = EagerComposition::builder()
        .with_component_override::<dyn Clock>(Box::new(FakeClock::new(1)))
        .with_component_override::<dyn Carrier>(Box::new(FakeCarrier::new()))
        .build();
    assert_eq!(
        directory_store_builds(),
        1,
        "an unreferenced registration was expected to be built eagerly"
    );
    // The two that *were* overridden stayed unbuilt, which is the half of
    // Shaku's behaviour DI-E01 relies on.
    assert_eq!((wall_clock_builds(), endpoint_carrier_builds()), (0, 0));
    drop(eager);

    let lazy = LazyComposition::builder()
        .with_component_override::<dyn Clock>(Box::new(FakeClock::new(1)))
        .with_component_override::<dyn Carrier>(Box::new(FakeCarrier::new()))
        .build();
    assert_eq!(
        directory_store_builds(),
        1,
        "#[lazy] was expected to defer construction past build()"
    );

    let _deferred: Arc<dyn Store> = lazy.resolve();
    assert_eq!(directory_store_builds(), 2);
    assert_eq!((wall_clock_builds(), endpoint_carrier_builds()), (0, 0));
}
