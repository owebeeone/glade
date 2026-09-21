//! Step 2.3 — a concrete `ManagedResource` whose cleanup is an sdax run, put
//! through `glade-lifecycle-api`'s own LC-001..LC-006 suite **unchanged**.
//!
//! This is the seam `AsyncWitnessPlan.md` §3.5 describes: `ManagedResource`
//! takes `&mut self`, is `Send` and not `Sync`, and returns `impl Future`, so
//! it is not dyn-compatible and can never be a Shaku component interface. The
//! answer is not to change it — §8.4 says a contract change to please a
//! container is never a caveat — but to let sdax own the `&mut`, `Send`-only
//! handle while Shaku (Phase 3) injects the shared `Sync` views. Nothing in
//! this file names Shaku, and **no Glade contract is edited**: the suite at
//! `glade/contracts/lifecycle-api/src/conformance.rs` is used exactly as it
//! stands, and every clause below is its wording, not the witness's.
//!
//! The adapter lives in this test target rather than in `src/`, because
//! `glade-lifecycle-api` is a **dev**-dependency of `async-witness-real`
//! (`real/Cargo.toml:36`, and `dev:glade-lifecycle-api` in
//! `architecture-policy.json`). Promoting it to a normal dependency would be a
//! manifest and policy change, which Phase 0 closed; the adapter is a harness
//! object and a test target is where it belongs.
//!
//! # What sdax does and does not give the contract
//!
//! One thing had to be decided rather than read off: **sdax's release graph
//! runs exactly once per run.** There is no API to re-run a completed run's
//! cleanup, and no retry attribute reaches a release body. `ManagedResource`
//! meanwhile requires that "repeated shutdown MUST retry unfinished cleanup
//! without repeating completed irreversible cleanup"
//! (`lifecycle-api/src/lib.rs:39-41`). The adapter therefore owns the retry and
//! sdax owns each attempt: one run while obligations remain, and a fresh run
//! over **only** what the ledger still shows outstanding. Completed cleanup is
//! not in the retry's plan at all, so "without repeating" holds by
//! construction rather than by a flag. Every attempt is ordered by sdax from
//! the same `needs` chain, so the reverse order of Step 2.1 survives a retry.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glade_lifecycle_api::{
    Failure, ManagedResource, Mode, Phase, Shutdown, ShutdownReport, conformance,
};
use sdax::{Acquire, Cx, Error, Key, Plan, Policy, Release, ServingPhase, Start};
use sdax_tokio::{PlanStart, Running, TokioRuntime};

// ------------------------------------------------------------------ fixture

/// The error an obligation's first release attempts return. LC-002 asserts
/// this exact text through `ShutdownReport`, so it is the fixture's wording
/// and not a witness invention.
#[derive(Debug)]
struct FixtureFailure;

impl fmt::Display for FixtureFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fixture failure")
    }
}

impl std::error::Error for FixtureFailure {}

/// One obligation the adapter already owns, and how its cleanup behaves.
#[derive(Clone, Copy, Debug)]
struct Obligation {
    name: &'static str,
    /// Release attempts that fail before one succeeds.
    failures: usize,
    /// Work a `Drain` executes and a `Cancel` skips.
    drain: Duration,
}

impl Obligation {
    const fn new(name: &'static str) -> Self {
        Obligation {
            name,
            failures: 0,
            drain: Duration::ZERO,
        }
    }

    const fn failing_once(mut self) -> Self {
        self.failures = 1;
        self
    }

    const fn draining(mut self, drain: Duration) -> Self {
        self.drain = drain;
        self
    }
}

/// Long enough that a zero budget cannot finish it and a one-second budget
/// comfortably can. LC-004 needs outstanding work to be genuinely outstanding.
const DRAIN: Duration = Duration::from_millis(20);

/// The AR-08 shape again, under the names LC-002 and LC-004 require: a parent
/// `socket` and the `work` that depends on it, so cleanup runs work-first.
fn witness_pair() -> Vec<Obligation> {
    vec![
        Obligation::new("socket"),
        Obligation::new("work").draining(DRAIN),
    ]
}

/// LC-002's shape: the same pair, with the parent's first close failing.
fn failing_socket() -> Vec<Obligation> {
    vec![
        Obligation::new("socket").failing_once(),
        Obligation::new("work"),
    ]
}

/// LC-004's shape: one obligation, named `work`, that a zero budget cannot
/// discharge. LC-004 asserts `remaining == ["work"]` exactly, so nothing else
/// may be outstanding at that moment.
fn outstanding_work() -> Vec<Obligation> {
    vec![Obligation::new("work").draining(DRAIN)]
}

/// The live state of one obligation, shared between the adapter and whichever
/// run is currently trying to discharge it. This is the "progress ledger" the
/// contract names at `lifecycle-api/src/lib.rs:44-45`: what survives a dropped
/// shutdown future and lets the next attempt pick up where it stopped.
struct Ledger {
    name: &'static str,
    failures_left: AtomicUsize,
    drain: Duration,
    released: AtomicBool,
}

/// What the whole fixture shares: the per-obligation ledgers, how much work a
/// drain actually did, the release order, and whether this shutdown is a
/// cancel.
struct Fixture {
    ledgers: Vec<Arc<Ledger>>,
    drained: AtomicUsize,
    cancelling: AtomicBool,
    order: Mutex<Vec<&'static str>>,
}

impl Fixture {
    fn new(obligations: &[Obligation]) -> Arc<Self> {
        Arc::new(Fixture {
            ledgers: obligations
                .iter()
                .map(|o| {
                    Arc::new(Ledger {
                        name: o.name,
                        failures_left: AtomicUsize::new(o.failures),
                        drain: o.drain,
                        released: AtomicBool::new(false),
                    })
                })
                .collect(),
            drained: AtomicUsize::new(0),
            cancelling: AtomicBool::new(false),
            order: Mutex::new(Vec::new()),
        })
    }

    /// The obligations still outstanding, in declaration order.
    fn outstanding(&self) -> Vec<Arc<Ledger>> {
        self.ledgers
            .iter()
            .filter(|l| !l.released.load(Ordering::SeqCst))
            .cloned()
            .collect()
    }
}

/// The engine-owned value. The adapter acquired these resources; the run is
/// handed them to release. `cx.hold_value` is exactly the documented call for
/// "a value you already own" (`sdax/docs/Cleanup.md`); `cx.hold` is for an
/// acquisition that is itself the external effect, which is Step 3.1's job.
///
/// The release body reads its ledger out of this value rather than out of a
/// capture of its own, so what it discharges is what the engine registered and
/// handed back. That also lets [`release`] be a plain `fn` item rather than a
/// closure over the fixture, which keeps the body one function rather than one
/// per declaration site.
struct Owned {
    ledger: Arc<Ledger>,
    fixture: Arc<Fixture>,
}

/// Build the cleanup plan for one attempt over `outstanding`, in declaration
/// order, each obligation `needs`ing the one before it, so sdax derives the
/// reverse release order from the typed edges exactly as in Step 2.1.
///
/// `Mode::Resident` with a `Keeper` service, because a `Finite` plan of pure
/// resources settles and releases immediately: the adapter would be born
/// `Closed` and `shutdown` would have nothing to do.
fn cleanup_plan(fixture: &Arc<Fixture>, outstanding: &[Arc<Ledger>]) -> Plan {
    let mut p = Plan::builder("ManagedResource");
    let mut previous: Option<Key<Owned>> = None;

    for ledger in outstanding {
        let held = Owned {
            ledger: ledger.clone(),
            fixture: fixture.clone(),
        };
        // The two branches differ only in the dependency the acquire body is
        // handed: `()` for the first obligation, the parent's `Arc<Owned>` for
        // every later one. That edge is the whole point — it is what sdax
        // reverses to order the cleanup.
        let key = match previous {
            None => {
                let held = Arc::new(held);
                p.resource(ledger.name)
                    .acquire(move |cx: Cx<Acquire>, ()| {
                        let held = held.clone();
                        async move { Ok(cx.hold_value(held)) }
                    })
                    .release(release)
            }
            Some(previous) => {
                let held = Arc::new(held);
                p.resource(ledger.name)
                    .needs(previous)
                    .acquire(move |cx: Cx<Acquire>, _parent: Arc<Owned>| {
                        let held = held.clone();
                        async move { Ok(cx.hold_value(held)) }
                    })
                    .release(release)
            }
        };
        previous = Some(key);
    }

    let last = previous.expect("a cleanup plan is only built for outstanding obligations");
    p.service("Keeper")
        .needs(last)
        .stop_within(Duration::from_secs(5))
        .initialize(|_cx: Cx<Start>, _last: Arc<Owned>| async move { Ok(()) })
        .serve(|cx: Cx<ServingPhase>, _handle: Arc<()>| async move {
            cx.stop().await;
            Ok(())
        });

    p.build(
        Policy::FailFast,
        sdax::Shutdown::within(Duration::from_secs(10)),
        sdax::Mode::Resident,
    )
    .expect("the adapter's cleanup plan is valid by construction")
}

/// One obligation's release: do the drain's work unless this is a cancel, then
/// either fail the attempt or discharge the obligation in the ledger.
async fn release(_cx: Cx<Release>, owned: Arc<Owned>) -> Result<(), Error> {
    let ledger = &owned.ledger;
    let fixture = &owned.fixture;
    // `Mode::Cancel` "requests cooperative cancellation" and is not a drain
    // (`lifecycle-api/src/lib.rs:33-35`): the outstanding work is abandoned,
    // not executed. LC-006 checks that it really was not.
    if !fixture.cancelling.load(Ordering::SeqCst) && !ledger.drain.is_zero() {
        tokio::time::sleep(ledger.drain).await;
        fixture.drained.fetch_add(1, Ordering::SeqCst);
    }
    if ledger.failures_left.load(Ordering::SeqCst) > 0 {
        ledger.failures_left.fetch_sub(1, Ordering::SeqCst);
        return Err(Error::from(FixtureFailure));
    }
    ledger.released.store(true, Ordering::SeqCst);
    fixture
        .order
        .lock()
        .expect("release order lock")
        .push(ledger.name);
    Ok(())
}

// ------------------------------------------------------------------ adapter

/// A `ManagedResource` whose cleanup is an sdax run.
///
/// `Phase` comes from the adapter's own state machine and not from
/// `Outcome`: a `Mode::Cancel` shutdown ends its run `Outcome::Cancelled` and
/// is nonetheless a complete, successful close, because every obligation was
/// discharged. What decides `Closed` is the ledger plus `cleanup_failures`,
/// never the outcome alone — the same refusal §8.3 makes about `is_clean()`.
struct SdaxManagedResource {
    runtime: Arc<TokioRuntime>,
    fixture: Arc<Fixture>,
    phase: Phase,
    /// The attempt currently in flight. `Some` means ownership is still with a
    /// live run, which is what makes a dropped shutdown future recoverable.
    running: Option<Running<()>>,
    /// What the last completed attempt's cleanup failures were.
    failures: Vec<Failure>,
    /// The negative fixture of `rejects_false_cleanup_success`: claim `Open`
    /// after a clean close, so the suite has something to reject.
    lie: bool,
}

impl SdaxManagedResource {
    /// Acquire the obligations and reach steady state, so the adapter really
    /// is `Open` before anything asks it to close.
    async fn start(runtime: Arc<TokioRuntime>, obligations: &[Obligation]) -> Self {
        let fixture = Fixture::new(obligations);
        let mut adapter = SdaxManagedResource {
            runtime,
            fixture,
            phase: Phase::Open,
            running: None,
            failures: Vec::new(),
            lie: false,
        };
        adapter.begin_attempt().await;
        adapter
    }

    /// The same adapter, lying about its phase after a clean close.
    fn that_lies(mut self) -> Self {
        self.lie = true;
        self
    }

    /// Start one attempt over whatever is still outstanding and wait until it
    /// holds them.
    async fn begin_attempt(&mut self) {
        let outstanding = self.fixture.outstanding();
        if outstanding.is_empty() {
            return;
        }
        let plan = cleanup_plan(&self.fixture, &outstanding);
        let mut running = plan.start(self.runtime.clone(), ());
        running
            .ready()
            .await
            .expect("the adapter's obligations are already owned, so acquisition cannot fail");
        self.running = Some(running);
    }

    /// Obligations the ledger still shows outstanding.
    fn remaining(&self) -> Vec<String> {
        self.fixture
            .outstanding()
            .iter()
            .map(|l| l.name.to_string())
            .collect()
    }

    /// The order the obligations were actually discharged in.
    fn released(&self) -> Vec<&'static str> {
        self.fixture
            .order
            .lock()
            .expect("release order lock")
            .clone()
    }

    /// How many drains executed their work.
    fn drained(&self) -> usize {
        self.fixture.drained.load(Ordering::SeqCst)
    }
}

impl ManagedResource for SdaxManagedResource {
    fn phase(&self) -> Phase {
        self.phase
    }

    /// `async fn` rather than a hand-written `impl Future`, which is also what
    /// the contract's own fixture does (`tests/public_contract.rs:15`). The
    /// body does nothing until polled, so an unpolled shutdown leaves the phase
    /// `Open` (LC-003).
    async fn shutdown(&mut self, request: Shutdown) -> ShutdownReport {
        if self.phase == Phase::Closed {
            return ShutdownReport {
                remaining: Vec::new(),
                failures: Vec::new(),
            };
        }
        // "Shutdown MUST stop admitting new work when first polled."
        self.phase = Phase::Closing;
        self.failures.clear();
        self.fixture
            .cancelling
            .store(request.mode == Mode::Cancel, Ordering::SeqCst);

        if self.running.is_none() {
            self.begin_attempt().await;
        }
        if let Some(running) = self.running.as_mut() {
            match request.mode {
                Mode::Drain => running.shutdown(),
                Mode::Cancel => running.cancel(),
            }
            // The caller's budget bounds this call, not the run: an expired
            // budget leaves the run draining and ownership recoverable, which
            // is what LC-004 and LC-005 are about.
            if let Ok(report) = tokio::time::timeout(request.budget, running).await {
                self.failures = report
                    .cleanup_failures
                    .iter()
                    .map(|fault| Failure {
                        resource: fault.node.leaf().to_string(),
                        reason: fault.kind.to_string(),
                    })
                    .collect();
                self.running = None;
            }
        }

        let report = ShutdownReport {
            remaining: self.remaining(),
            failures: self.failures.clone(),
        };
        if report.remaining.is_empty() && report.failures.is_empty() {
            // "Closed requires no remaining owned work/resources; otherwise
            // phase is Closing."
            self.phase = if self.lie { Phase::Open } else { Phase::Closed };
        }
        report
    }
}

// -------------------------------------------------------------------- suite

fn runtime() -> Arc<TokioRuntime> {
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()))
}

async fn adapter(obligations: Vec<Obligation>) -> SdaxManagedResource {
    SdaxManagedResource::start(runtime(), &obligations).await
}

#[tokio::test(flavor = "multi_thread")]
async fn lc_001_shutdown_is_terminal_and_idempotent() {
    conformance::closed(&mut adapter(witness_pair()).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lc_002_cleanup_failure_is_retryable_and_visible() {
    conformance::failure(&mut adapter(failing_socket()).await).await;
}

/// LC-003 has no helper in `conformance.rs`; this is the contract's own test
/// (`lifecycle-api/tests/public_contract.rs:78-84`) against the adapter.
#[tokio::test(flavor = "multi_thread")]
async fn lc_003_unpolled_shutdown_is_lazy() {
    let mut resource = adapter(witness_pair()).await;
    let never_polled = resource.shutdown(conformance::request());
    drop(never_polled);
    assert_eq!(resource.phase(), Phase::Open);
}

#[tokio::test(flavor = "multi_thread")]
async fn lc_004_zero_budget_reports_unfinished_work() {
    conformance::budget(&mut adapter(outstanding_work()).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lc_005_pending_drop_retains_ownership() {
    let mut resource = adapter(witness_pair()).await;
    conformance::poll_then_drop(&mut resource);
    conformance::closed(&mut resource).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lc_006_cancel_does_not_drain_work() {
    let mut resource = adapter(witness_pair()).await;
    conformance::cancel(&mut resource).await;
    assert_eq!(
        resource.drained(),
        0,
        "a cancel requested cooperative cancellation, not a drain"
    );
}

/// The suite's negative still fires. `rejects_false_cleanup_success` is the
/// guard that LC-001 is not passing because the assertions are toothless; an
/// adapter that closes cleanly and then claims `Open` must be rejected.
#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "assertion `left == right` failed")]
async fn rejects_false_cleanup_success() {
    let mut resource = adapter(witness_pair()).await.that_lies();
    conformance::closed(&mut resource).await;
}

/// The reverse order of Step 2.1 is still what the adapter's own plan does:
/// the obligation that `needs` the other is discharged first.
#[tokio::test(flavor = "multi_thread")]
async fn the_adapters_cleanup_is_still_reverse_order() {
    let mut resource = adapter(witness_pair()).await;
    conformance::closed(&mut resource).await;
    assert_eq!(resource.released(), vec!["work", "socket"]);
}

/// A retry re-attempts only what remains. LC-002 already requires the retry to
/// succeed; this says what it must **not** do — repeat the cleanup that was
/// already completed, which for an irreversible one would be a second effect.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_does_not_repeat_completed_cleanup() {
    let mut resource = adapter(failing_socket()).await;
    conformance::failure(&mut resource).await;
    assert_eq!(
        resource.released(),
        vec!["work", "socket"],
        "`work` was discharged once, by the first attempt"
    );
}
