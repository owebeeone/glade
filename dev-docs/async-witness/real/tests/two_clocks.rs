//! Step 3.6 — **the two clocks**, and what makes "they agree" falsifiable.
//!
//! `AsyncWitnessPlan.md` §4.4: "There are two clocks and the witness must not
//! conflate them." sdax has its own injected `Clock` on which every backoff,
//! `within` deadline and shutdown budget is measured; Glade's clock port is a
//! separate, Glade-owned thing. "Using sdax's `Clock` as Glade's clock port
//! would put a framework type in a contract crate and fail DI-E04 by
//! construction."
//!
//! **Why the engine does not run on `sdax_testkit::FakeClock`.** The plan's
//! update from Phases 1 and 2, item 2: that clock's `sleep` answers `Pending`
//! without registering a waker (`crates/sdax-testkit/src/clock.rs:59-71` at the
//! pinned rev), so a task sleeping on it on a multi-thread runtime never wakes.
//! The update offers two ways out — a scaled real clock, as sdax's own
//! multi-thread conformance suite uses
//! (`crates/sdax-tokio/tests/conformance/multi_thread.rs:157`), or a clock that
//! records wakers and wakes them when advanced. This file takes the **second**,
//! because a scaled clock still measures wall time: "advancing both clocks"
//! would become "sleeping and hoping", and the agreement it observed would be
//! as good as the machine's load. [`WitnessClock`] moves only when a test moves
//! it, so every assertion below is about the engine and not about the
//! scheduler.
//!
//! **What "they agree" is about.** Not two counters that happen to match: the
//! engine times a real budget on its clock, and the **port** reads
//! `ClockPort::now_ms` from the contract crate's `FakeClock` on either side of
//! that wait. The two clocks start at different origins on purpose — the engine
//! at its own zero, the port at a wall-clock-shaped instant — so what they have
//! to agree about is the *interval*, which is the only thing two clocks can
//! honestly agree about.

use std::sync::Arc;
use std::time::Duration;

use async_witness_ports::{ClockPort, FakeClock};
use async_witness_real::two_clocks::{ClockHarness, ENGINE_BUDGET, WitnessClock, two_clock_plan};
use sdax::host::Clock;
use sdax_tokio::{PlanStart, TokioRuntime};

/// The port clock's origin: a wall-clock-shaped instant, so that a test that
/// confused an instant with an interval could not accidentally pass.
const PORT_ORIGIN_MS: i64 = 1_700_000_000_000;

/// How many equal steps the budget is advanced in.
const TICKS: u32 = 10;

/// How long to wait for tasks that have already been asked to finish.
const WAIT: Duration = Duration::from_secs(10);

fn budget_ms() -> i64 {
    i64::try_from(ENGINE_BUDGET.as_millis()).expect("the budget fits in an i64 of milliseconds")
}

fn runtime(engine: &Arc<WitnessClock>) -> Arc<TokioRuntime> {
    let clock: Arc<dyn Clock> = engine.clone();
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()).with_clock(clock))
}

/// **The agreement.** The engine's budget expires exactly where the port's
/// clock says it should — not one tick early, not one tick late.
///
/// The budget is advanced in ten equal steps on both clocks at once. Nine of
/// them wake nothing, because the deadline the **engine** registered has not
/// been reached; the tenth wakes exactly one sleeper. Then the port's own
/// reading of the same wait, taken inside the body on either side of it, is
/// exactly the budget. Two independent clocks, one deadline, one story.
#[tokio::test(flavor = "multi_thread")]
async fn the_engine_deadline_falls_where_the_port_clock_says_it_should() {
    let port = FakeClock::new(PORT_ORIGIN_MS);
    let engine = WitnessClock::new();
    let harness = ClockHarness::new(Arc::new(port.clone()));
    let rt = runtime(&engine);

    // `Running::poll` is what launches the engine (`sdax-tokio/src/running.rs:339`),
    // so the ticking has to be a second branch of the same await rather than
    // something that happens before it. Awaiting `running` first would hang:
    // nothing would ever register a deadline to advance past.
    let ticking = async {
        // Wait for the ENGINE's registration rather than for an announcement of
        // the body's own: the deadline is fixed when `Clock::sleep` is called
        // and the waker exists one poll later, so waiting for that is what
        // removes the race with the first tick.
        engine.first_sleeper().await;
        assert_eq!(
            engine.asked_for(),
            vec![ENGINE_BUDGET],
            "the only thing waiting on the engine's clock must be the body's budget"
        );

        let tick = ENGINE_BUDGET / TICKS;
        let tick_ms = budget_ms() / i64::from(TICKS);
        for step in 1..=TICKS {
            port.advance(tick_ms);
            let woken = engine.advance(tick);
            let expected = if step == TICKS { 1 } else { 0 };
            assert_eq!(
                woken, expected,
                "at tick {step} of {TICKS}: the engine's deadline is reached on the last tick and no other"
            );
        }
    };

    let (report, ()) = tokio::join!(two_clock_plan(&harness).start(rt.clone(), ()), ticking);
    assert!(report.is_clean(), "{report}");
    assert_eq!(rt.shutdown(WAIT).await, Ok(()));
    assert_eq!(rt.tracked(), 0);

    assert!(
        harness.timed_out(),
        "the engine's own timeout is what ended the wait, not the body"
    );
    assert_eq!(harness.entered(), Some(PORT_ORIGIN_MS));
    assert_eq!(harness.fired(), Some(PORT_ORIGIN_MS + budget_ms()));
    assert_eq!(
        report.output.as_deref().copied(),
        Some(budget_ms()),
        "the run's export is the PORT's measure of the ENGINE's budget"
    );
    assert_eq!(engine.elapsed(), ENGINE_BUDGET);
}

/// One run in which the engine is advanced by its whole budget in one step
/// while the port clock is advanced by `port_advance_ms`. Returns what the port
/// made of the wait, and whether the engine's timeout is what ended it.
async fn observe(port_advance_ms: i64) -> (Option<i64>, bool) {
    let port = FakeClock::new(PORT_ORIGIN_MS);
    let engine = WitnessClock::new();
    let harness = ClockHarness::new(Arc::new(port.clone()));
    let rt = runtime(&engine);

    let ticking = async {
        engine.first_sleeper().await;
        port.advance(port_advance_ms);
        assert_eq!(engine.advance(ENGINE_BUDGET), 1);
    };
    let (report, ()) = tokio::join!(two_clock_plan(&harness).start(rt.clone(), ()), ticking);
    assert!(report.is_clean(), "{report}");
    assert_eq!(rt.shutdown(WAIT).await, Ok(()));
    assert_eq!(rt.tracked(), 0);
    (report.output.as_deref().copied(), harness.timed_out())
}

/// **The falsification.** The same code path, with the two clocks advanced
/// apart, produces values the agreement above would fail on. Without this, "the
/// two clocks agree" could be a sentence about a test that cannot disagree.
#[tokio::test(flavor = "multi_thread")]
async fn advancing_the_two_clocks_apart_makes_them_disagree() {
    // In lockstep they agree, and this is the value the agreement test asserts.
    assert_eq!(observe(budget_ms()).await, (Some(budget_ms()), true));

    // A frozen port clock: the engine's budget expired in full and the port saw
    // no time pass at all.
    assert_eq!(observe(0).await, (Some(0), true));

    // A port clock running at twice the rate: the same expiry, twice the
    // elapsed. The engine did not change its mind about when to fire.
    assert_eq!(
        observe(2 * budget_ms()).await,
        (Some(2 * budget_ms()), true)
    );
}

/// **Neither clock leaks into the contract crate.** The engine's clock is a
/// type of *this* crate implementing a *framework* trait; the port's clock is a
/// type of the contract crate implementing the contract crate's own trait. They
/// are two types from two crates and neither is reachable from the other.
///
/// The load-bearing enforcement is not here — it is `architecture-policy.json`,
/// which does not let `async-witness-ports` name `sdax`, and `check.sh`'s
/// `cargo tree --invert sdax`, which finds no contract-role package. This test
/// records the shape that gate is protecting. Static; no runtime.
#[test]
fn the_two_clocks_are_two_types_from_two_crates() {
    let engine_type = std::any::type_name::<WitnessClock>();
    let port_type = std::any::type_name::<FakeClock>();
    assert!(
        engine_type.starts_with("async_witness_real::"),
        "{engine_type}"
    );
    assert!(
        port_type.starts_with("async_witness_ports::"),
        "{port_type}"
    );
    assert_ne!(engine_type, port_type);

    // Each satisfies its own contract and has no idea about the other's: the
    // port clock reads an instant in milliseconds and cannot sleep; the engine
    // clock sleeps and has no `now_ms`.
    let engine: Arc<dyn Clock> = WitnessClock::new();
    let port: Arc<dyn ClockPort> = Arc::new(FakeClock::new(PORT_ORIGIN_MS));
    assert_eq!(port.now_ms(), PORT_ORIGIN_MS);
    assert_eq!(engine.now().as_nanos(), 0);
}
