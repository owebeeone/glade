//! Step 2.4 — ownership honesty: nothing the engine spawned is still out there
//! when it says it is done, and nothing is quietly abandoned when it is not.
//!
//! AR-08's last clause is "no invisible detached work"
//! (`arch1/RuntimeAndAssurance.md:126`), and §3.4 of the plan measures the
//! distance to it: the node today has "five nested untracked spawns per peer
//! link, none cancellable, none drained". `TokioRuntime` registers every spawn
//! in a `TaskTracker` (`sdax-tokio/src/lib.rs:77`), so `tracked()` is a direct
//! answer to "is anything of mine still running?" and `shutdown(budget)`
//! answers `Err(n)` rather than a silent success.
//!
//! The clause about drop is the contract's own, at
//! `lifecycle-api/src/lib.rs:44-45`: "Dropping a polled shutdown leaves
//! ownership and its progress ledger recoverable for another shutdown attempt;
//! drop is not cleanup." `tests/managed_resource.rs` shows that at the
//! `ManagedResource` level (LC-005). This file shows the sdax level underneath
//! it: a dropped `Running` leaves **one tracked drainer**, and the release
//! graph still runs.

use std::sync::Arc;
use std::time::Duration;

use async_witness_real::{Event, Harness, node, witness_plan};
use sdax::Outcome;
use sdax_tokio::{PlanStart, TokioRuntime};

/// Generous on purpose. This budget bounds waiting for tasks that have already
/// been asked to finish, and the fakes finish in microseconds; a short one
/// here would only make an `Err` mean "the machine was busy".
const WAIT: Duration = Duration::from_secs(10);

fn runtime() -> Arc<TokioRuntime> {
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()))
}

/// §8.3's clause, verbatim: "`TokioRuntime::tracked()` is 0 afterwards."
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_run_leaves_no_tracked_task() {
    let harness = Harness::new();
    let rt = runtime();
    let report = witness_plan(&harness).start(rt.clone(), ()).await;
    assert!(report.is_clean(), "{report}");

    assert_eq!(rt.shutdown(WAIT).await, Ok(()));
    assert_eq!(rt.tracked(), 0);
}

/// The counter is not vacuously zero: while the run is in flight there really
/// are tasks registered, and they really are the engine's.
#[tokio::test(flavor = "multi_thread")]
async fn tracked_counts_the_run_while_it_is_still_in_flight() {
    let harness = Harness::new();
    harness.block_exchange();
    let rt = runtime();
    let mut running = witness_plan(&harness).start(rt.clone(), ());

    // `start` is lazy — nothing is spawned until the first poll
    // (`sdax/docs/Running.md`) — so drive it until the blocked body is live.
    tokio::select! {
        _ = &mut running => panic!("the exchange blocks, so the run cannot end here"),
        () = harness.exchange_started() => {}
    }
    assert!(rt.tracked() > 0, "an in-flight run owns at least one task");

    running.cancel();
    let report = running.await;
    assert_eq!(report.outcome, Outcome::Cancelled);
    assert_eq!(rt.shutdown(WAIT).await, Ok(()));
    assert_eq!(rt.tracked(), 0);
}

/// "Dropping a live `Running` cancels it and leaves **one tracked drainer** to
/// finish the release graph inside the shutdown budget"
/// (`sdax-tokio/src/lib.rs:30-34`). Both halves matter: the drainer is
/// *tracked*, so it is not invisible detached work, and it really does run the
/// release graph, so drop is not a silent leak either.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_run_still_drains_through_a_tracked_drainer() {
    let harness = Harness::new();
    harness.block_exchange();
    let rt = runtime();
    let mut running = witness_plan(&harness).start(rt.clone(), ());

    tokio::select! {
        _ = &mut running => panic!("the exchange blocks, so the run cannot end here"),
        () = harness.exchange_started() => {}
    }
    drop(running);

    // The drainer is a task, and this is what waiting for it looks like.
    assert_eq!(rt.shutdown(WAIT).await, Ok(()));
    assert_eq!(rt.tracked(), 0);

    assert_eq!(
        harness.released_before(node::LINK, node::ENDPOINT),
        Some(true),
        "a dropped run still releases, and still in reverse order: {:?}",
        harness.events()
    );
    assert!(harness.position(&Event::Released(node::JOURNAL)).is_some());
}

/// Where an abandoned obligation shows up, and where it does not.
///
/// This was written expecting `Err(n)`, and the expectation was wrong in a way
/// worth recording. `abandon` (`sdax/src/host/engine/cleanup.rs:310-333`)
/// pushes an `Effect::Abort` for the node **and** a record into `incomplete`,
/// and only excludes `BlockingStep` and `Template`, "a blocking body cannot be
/// aborted (T7)". An async release body therefore ends: the abandonment is
/// visible in `report.incomplete`, and the runtime is left genuinely empty.
///
/// So `tracked() == 0` is not weaker evidence than `Err(n)` would be — the two
/// answer different questions. `Err(n)` is reserved for work nothing can stop,
/// and the plan already names where Glade has some: §3.3 records that the
/// node's `Store` is synchronous blocking file I/O held behind a
/// `tokio::sync::Mutex` and locked from async code (`server.rs:83`). A
/// `blocking_step` over it is the shape that would answer `Err(n)`, and Phase 3
/// should expect that rather than be surprised by it.
///
/// The run's own budget is short here for the same reason as in
/// `tests/lifecycle.rs`: this release never returns at any budget.
#[tokio::test(flavor = "multi_thread")]
async fn an_abandoned_obligation_is_reported_and_leaves_nothing_running() {
    let harness = Harness::new().with_shutdown_budget(Duration::from_millis(200));
    harness.hang_endpoint_release();
    let rt = runtime();
    let report = witness_plan(&harness).start(rt.clone(), ()).await;

    assert_eq!(report.incomplete.len(), 1, "{report}");
    assert_eq!(report.incomplete[0].node.leaf(), node::ENDPOINT);

    assert_eq!(
        rt.shutdown(WAIT).await,
        Ok(()),
        "an aborted body leaves no task, so the wait is not what expires"
    );
    assert_eq!(rt.tracked(), 0);

    // The obligation is genuinely not discharged: the abandonment is a fact
    // about the report, not about a task still trying.
    assert!(
        harness
            .position(&Event::ReleaseStalled(node::ENDPOINT))
            .is_some()
    );
    assert!(harness.position(&Event::Released(node::ENDPOINT)).is_none());
}
