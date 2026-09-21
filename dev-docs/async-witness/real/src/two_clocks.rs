//! Step 3.6 — **the two clocks**: one the engine measures its budget on, one
//! the port reads, and neither of them in `async-witness-ports`.
//!
//! `AsyncWitnessPlan.md` §4.4 states the separation this module exists to keep:
//! sdax has its own injected `Clock` (`sdax/src/contracts.rs:72-78`) on which
//! "backoff waits, `within` deadlines, `stop_within` and the shutdown budget
//! are all measured", and Glade's clock port is a separate, Glade-owned thing.
//! "Using sdax's `Clock` as Glade's clock port would put a framework type in a
//! contract crate and fail DI-E04 by construction."
//!
//! So there are two, and they meet in one body: [`two_clock_plan`] declares a
//! step that asks the **engine** to time a budget and reads
//! `ClockPort::now_ms` from the **contract crate's** `FakeClock` on either side
//! of the wait. The step's export is the port's own measure of the engine's
//! budget, which is what `tests/two_clocks.rs` compares.
//!
//! # Which engine clock, and why this one
//!
//! Not `sdax_testkit::FakeClock`. The plan's update from Phases 1 and 2, item
//! 2: its `sleep` answers `Pending` without registering a waker
//! (`crates/sdax-testkit/src/clock.rs:59-71` at the pinned rev), so a task
//! sleeping on it on a multi-thread runtime never wakes unless something else
//! happens to poll it — and every witness test is
//! `#[tokio::test(flavor = "multi_thread")]`, because `TokioRuntime::new`
//! panics on a current-thread handle.
//!
//! The update offers two ways out: a scaled real clock, as sdax's own
//! multi-thread conformance suite uses
//! (`crates/sdax-tokio/tests/conformance/multi_thread.rs:157`), or a clock that
//! records wakers and wakes them when advanced. [`WitnessClock`] is the
//! **second**. A scaled clock still measures wall time, so "advance both
//! clocks and see whether they agree" would become "sleep and hope", and the
//! agreement observed would be a fact about the machine's load rather than
//! about the engine. `WitnessClock` moves only when a test moves it, and it
//! reports how many registered deadlines each advance crossed — which is what
//! lets a test say *exactly* where the engine's budget expired instead of
//! saying that it eventually did.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use async_witness_ports::ClockPort;
use sdax::host::{BoxFuture, Clock, Time};
use sdax::prelude::*;
use tokio::sync::Notify;

/// The node name, in one place, so a test and a plan cannot disagree about a
/// spelling.
pub mod node {
    /// The one step: it asks the engine to time a budget and reads the port's
    /// clock on either side of the wait.
    pub const WAITING: &str = "Waiting";
}

/// The budget the **engine** times, on the engine's own clock.
///
/// Half a second of engine time that costs no wall time at all: nothing
/// advances a [`WitnessClock`] but a test. It divides evenly into ten equal
/// ticks, which is how `tests/two_clocks.rs` advances it.
pub const ENGINE_BUDGET: Duration = Duration::from_millis(500);

/// The run's shutdown budget, also engine time. Generous on purpose: plan §9.3
/// forbids shortening a drain to make a test quick. There is nothing to drain
/// here — the plan holds no resource and binds no socket, which is the
/// fakes-or-real boundary the plan draws — so it is never reached.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(30);

/// The port side: the contract crate's clock, and what the body read from it.
///
/// The harness holds the port as `Arc<dyn ClockPort>` — the framework-free
/// trait — so the body can only do to it what any injected consumer could.
#[derive(Clone)]
pub struct ClockHarness {
    port: Arc<dyn ClockPort>,
    entered: Arc<Mutex<Option<i64>>>,
    fired: Arc<Mutex<Option<i64>>>,
    timed_out: Arc<AtomicBool>,
}

impl ClockHarness {
    /// A harness over one port clock.
    pub fn new(port: Arc<dyn ClockPort>) -> ClockHarness {
        ClockHarness {
            port,
            entered: Arc::new(Mutex::new(None)),
            fired: Arc::new(Mutex::new(None)),
            timed_out: Arc::new(AtomicBool::new(false)),
        }
    }

    /// What the port's clock read when the body began waiting.
    pub fn entered(&self) -> Option<i64> {
        *self.entered.lock().expect("clock harness entered lock")
    }

    /// What the port's clock read when the engine's budget expired.
    pub fn fired(&self) -> Option<i64> {
        *self.fired.lock().expect("clock harness fired lock")
    }

    /// Whether the engine's own timeout is what ended the wait. The body's
    /// inner future never completes, so anything else would be a surprise
    /// worth failing on.
    pub fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::SeqCst)
    }

    fn record_entered(&self, at: i64) {
        *self.entered.lock().expect("clock harness entered lock") = Some(at);
    }

    fn record_fired(&self, at: i64, timed_out: bool) {
        *self.fired.lock().expect("clock harness fired lock") = Some(at);
        self.timed_out.store(timed_out, Ordering::SeqCst);
    }
}

/// The plan: one step, two clocks, no socket.
///
/// The export is `fired - entered` in **port** milliseconds — the port's own
/// measure of an interval the **engine** decided the length of. Two clocks with
/// different origins cannot agree about an instant; an interval is the only
/// thing they can honestly be compared on.
pub fn two_clock_plan(harness: &ClockHarness) -> Plan<i64> {
    let mut p = Plan::builder("AsyncWitnessTwoClocks");

    let waiting = {
        let harness = harness.clone();
        p.step(node::WAITING).run(move |cx: Cx<Run>, ()| {
            let harness = harness.clone();
            async move {
                let entered = harness.port.now_ms();
                harness.record_entered(entered);
                // The ENGINE times this, on the engine's clock
                // (`sdax/src/cx.rs:373-386` polls `clock.sleep(d)`). The inner
                // future never completes, so the deadline is the only way out.
                let outcome = cx
                    .timeout(ENGINE_BUDGET, std::future::pending::<()>())
                    .await;
                let fired = harness.port.now_ms();
                harness.record_fired(fired, outcome.is_err());
                Ok(fired - entered)
            }
        })
    };

    p.export(waiting)
        .build(
            Policy::FailFast,
            Shutdown::within(SHUTDOWN_BUDGET),
            Mode::Finite,
        )
        .expect("the two-clock plan is valid by construction")
}

/// One sleeper waiting on the engine's clock.
struct Sleeper {
    /// When it is due, in nanoseconds on this clock.
    deadline: u64,
    /// The waker of the task that last polled it, once it has been polled.
    waker: Option<Waker>,
}

/// Everything the clock and its sleep futures share.
struct ClockInner {
    state: Mutex<ClockState>,
    /// Fires the first time a sleeper stores a waker — that is, once something
    /// is genuinely waiting and its deadline is already fixed.
    registered: Notify,
}

#[derive(Default)]
struct ClockState {
    nanos: u64,
    next_id: u64,
    sleepers: BTreeMap<u64, Sleeper>,
    asked_for: Vec<Duration>,
}

/// A [`Clock`] for the engine that moves only when a test moves it, and that
/// **wakes** what is waiting on it.
///
/// This is the difference from `sdax_testkit::FakeClock`, which is otherwise
/// the obvious choice: that one's `sleep` answers `Pending` and registers no
/// waker, so on a multi-thread runtime nothing ever re-polls the sleeper. Here
/// the sleep future stores the polling task's waker, and [`advance`] wakes
/// every waker whose deadline has passed.
///
/// [`advance`]: WitnessClock::advance
pub struct WitnessClock {
    inner: Arc<ClockInner>,
}

impl WitnessClock {
    /// A clock at its origin. `Arc` because `TokioRuntime::with_clock` takes an
    /// `Arc<dyn Clock>` and a test needs a handle of its own to advance.
    pub fn new() -> Arc<WitnessClock> {
        Arc::new(WitnessClock {
            inner: Arc::new(ClockInner {
                state: Mutex::new(ClockState::default()),
                registered: Notify::new(),
            }),
        })
    }

    /// Move the engine's clock forward, wake every sleeper whose deadline has
    /// passed, and return **how many registered deadlines this advance
    /// crossed**.
    ///
    /// That count is the observable a test needs: "nothing was due yet" and
    /// "exactly one thing came due here" are both statements about deadlines
    /// the *engine* chose, so a test can say where a budget expired rather than
    /// only that it eventually did. Wakers are woken after the lock is
    /// released, so a woken task never contends with the advance that woke it.
    pub fn advance(&self, d: Duration) -> usize {
        let mut crossed = 0;
        let mut woken = Vec::new();
        {
            let mut state = self.lock();
            let before = state.nanos;
            state.nanos = before.saturating_add(nanos_of(d));
            let now = state.nanos;
            for sleeper in state.sleepers.values_mut() {
                if sleeper.deadline > before && sleeper.deadline <= now {
                    crossed += 1;
                }
                if sleeper.deadline <= now
                    && let Some(waker) = sleeper.waker.take()
                {
                    woken.push(waker);
                }
            }
        }
        for waker in woken {
            waker.wake();
        }
        crossed
    }

    /// How far this clock has been advanced from its origin.
    pub fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.lock().nanos)
    }

    /// Every duration the engine has asked this clock to sleep for, in order.
    ///
    /// A test reads it to make sure the thing it is about to advance past is
    /// the deadline it means, and not some other wait the engine took out.
    pub fn asked_for(&self) -> Vec<Duration> {
        self.lock().asked_for.clone()
    }

    /// Resolves once something is waiting on this clock **and has stored its
    /// waker**.
    ///
    /// Waiting for this rather than for an announcement from the body is what
    /// removes the race with a test's first advance: the deadline is fixed when
    /// `Clock::sleep` is called, and the waker exists one poll later, so a test
    /// that advances after this returns can neither move the deadline nor lose
    /// the wake-up.
    pub async fn first_sleeper(&self) {
        loop {
            if self.lock().sleepers.values().any(|s| s.waker.is_some()) {
                return;
            }
            self.inner.registered.notified().await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, ClockState> {
        self.inner.state.lock().expect("witness clock lock")
    }
}

impl Clock for WitnessClock {
    fn now(&self) -> Time {
        Time::from_nanos(self.lock().nanos)
    }

    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        let id = {
            let mut state = self.lock();
            let id = state.next_id;
            state.next_id += 1;
            let deadline = state.nanos.saturating_add(nanos_of(d));
            state.asked_for.push(d);
            state.sleepers.insert(
                id,
                Sleeper {
                    deadline,
                    waker: None,
                },
            );
            id
        };
        Box::pin(Sleeping {
            id,
            inner: Arc::clone(&self.inner),
        })
    }
}

/// One `Clock::sleep` future. It owns nothing but its place in the clock's
/// table, and takes itself out of that table when it resolves or is dropped.
struct Sleeping {
    id: u64,
    inner: Arc<ClockInner>,
}

impl Future for Sleeping {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.inner.state.lock().expect("witness clock lock");
        let now = state.nanos;
        let Some(deadline) = state.sleepers.get(&self.id).map(|s| s.deadline) else {
            return Poll::Ready(());
        };
        if now >= deadline {
            state.sleepers.remove(&self.id);
            return Poll::Ready(());
        }
        let mut first = false;
        if let Some(sleeper) = state.sleepers.get_mut(&self.id) {
            first = sleeper.waker.is_none();
            sleeper.waker = Some(cx.waker().clone());
        }
        drop(state);
        if first {
            self.inner.registered.notify_one();
        }
        Poll::Pending
    }
}

impl Drop for Sleeping {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.sleepers.remove(&self.id);
        }
    }
}

fn nanos_of(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}
