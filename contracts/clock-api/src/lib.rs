//! The clock the node assembly injects: one wall-clock instant source that every
//! consumer in a scope shares. Not a timer, sleep, deadline or budget.
//!
//! Adopted from the async witness's `ClockPort`
//! (`glade/dev-docs/async-witness/ports/src/lib.rs`) without its `Any`
//! supertrait: an injector's facade asks `'static` of the implementation
//! instead (`dev-docs/arch1/AsyncWitnessResult.md` §5, caveat 4).

/// Wall-clock milliseconds since the Unix epoch, UTC: the unit
/// `RegistryApi::who_serves` takes and `sysdir::now_ms()` reads today. The port
/// supplies the instant; lease expiry and every read-time policy stay the caller's.
///
/// Every handle to one clock MUST read one instant source: a change is seen
/// through all of them, never through a per-handle copy. That is what lets a test
/// composition substitute one clock for every consumer (the clock binding of
/// `arch1/InjectionGraphRefinement.md`); a sibling scope gets its own clock.
///
/// Wall time is not monotonic: it may step backwards, so a caller MUST NOT measure
/// elapsed time as the difference of two reads. `now_ms` never fails, blocks or
/// sleeps, and has no effect.
///
/// ```compile_fail
/// use glade_clock_api::ClockPort;
/// struct Missing;
/// impl ClockPort for Missing {}
/// ```
///
/// It bridges onto an injector's facade with `'static` asked of the
/// implementation. `Interface` is `shaku::Interface` as Shaku defines it, and
/// `is_interface` is the bound a Shaku component's interface must meet:
///
/// ```
/// use glade_clock_api::ClockPort;
/// use std::any::Any;
/// trait Interface: Any + Send + Sync {}
/// impl<T: Any + Send + Sync> Interface for T {}
/// trait Clock: ClockPort + Interface {}
/// impl<T: ClockPort + 'static> Clock for T {}
/// fn is_interface<I: Interface + ?Sized>() {}
/// is_interface::<dyn Clock>();
/// fn build<T: ClockPort + 'static>(provider: T) -> Box<dyn Clock> { Box::new(provider) }
/// fn read(clock: &dyn Clock) -> i64 { clock.now_ms() }
/// ```
///
/// The witness's form does not compile, because the port names no `Any`:
///
/// ```compile_fail,E0310
/// # use glade_clock_api::ClockPort;
/// # trait Interface: std::any::Any + Send + Sync {}
/// # impl<T: std::any::Any + Send + Sync> Interface for T {}
/// trait Clock: ClockPort + Interface {}
/// impl<T: ClockPort> Clock for T {}
/// ```
pub trait ClockPort: Send + Sync {
    fn now_ms(&self) -> i64;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Probes over handles the fixture supplies; its controls are explicit.
    use crate::ClockPort;

    /// CL-001. Requires a clock the fixture controls: `first` and `second` are
    /// two handles to it and `set` moves it. Both read every instant set,
    /// including a backward step.
    pub fn one_instant(first: &dyn ClockPort, second: &dyn ClockPort, set: &dyn Fn(i64)) {
        for instant in [1_700_000_000_000, 1_700_000_060_000, 1_699_999_999_999] {
            set(instant);
            assert_eq!(
                first.now_ms(),
                instant,
                "CL-001 a handle reads the instant set"
            );
            assert_eq!(
                second.now_ms(),
                instant,
                "CL-001 every handle reads one instant"
            );
        }
    }

    /// CL-002. Epoch milliseconds: a read lies between two reads of an
    /// independent epoch-millisecond `reference` the fixture trusts.
    pub fn epoch_millis(clock: &dyn ClockPort, reference: &dyn Fn() -> i64) {
        let before = reference();
        let read = clock.now_ms();
        let after = reference();
        assert!(
            before <= read && read <= after,
            "CL-002 {read} is not epoch milliseconds within [{before}, {after}]"
        );
    }
}
