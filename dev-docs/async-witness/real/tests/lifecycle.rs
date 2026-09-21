//! Step 2.2 — run the plan Step 2.1 inspected: drain to completion, then
//! cancel, then expire the budget.
//!
//! `AsyncWitnessPlan.md` §8.3 sets the bar and it is deliberately higher than
//! "it returned Ok": "a clean run reports `is_clean()`, never merely
//! `outcome == Ok`; an expired budget lists the unfinished resource in
//! `report.incomplete`; a cancelled run still executes the release graph".
//! Each of those is a separate test below, because each can fail on its own.
//!
//! Every test uses `#[tokio::test(flavor = "multi_thread")]`: `TokioRuntime::new`
//! **panics** on a `current_thread` handle (`sdax-tokio/src/lib.rs:114-125`,
//! plan §9.3), and the panic message is right — a dropped `Running` hands its
//! release graph to a drainer, and a drainer is a task.

use std::sync::Arc;
use std::time::Duration;

use async_witness_real::{Event, Harness, node, witness_plan};
use sdax::prelude::*;
use sdax_tokio::{PlanStart, TokioRuntime};

/// The adapter over this test's own runtime. Multi-threaded by construction,
/// so `new` is the right constructor and the drainer has a thread.
fn runtime() -> Arc<TokioRuntime> {
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()))
}

/// Render a report the way a failing assertion should show it: the `Display`
/// impl lists every record, so a failure says *what* was unclean.
fn shown<Out>(report: &Report<Out>) -> String {
    format!("{report}")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clean_drain_reports_is_clean_and_not_merely_ok() {
    let harness = Harness::new();
    let report = witness_plan(&harness).start(runtime(), ()).await;

    assert!(report.is_clean(), "{}", shown(&report));
    assert_eq!(report.outcome, Outcome::Ok);
    assert!(report.faults.is_empty());
    assert!(report.cleanup_failures.is_empty());
    assert!(report.incomplete.is_empty());
    assert!(report.ambiguous.is_empty());

    // One frame through the witness's own `CarrierPort`, exported as the run's
    // completed output.
    assert_eq!(report.output.as_deref(), Some(&1));
    assert_eq!(harness.acquisitions(), 3);
}

/// AR-08's first clause, now as a fact about the run rather than about the
/// declaration. `Event::Released(ENDPOINT)` is only recorded when
/// `FakeEndpoint::close` succeeded, and it refuses while a link is live, so its
/// presence *is* the proof that the child's cleanup finished first.
#[tokio::test(flavor = "multi_thread")]
async fn the_parent_is_released_after_its_child_and_only_then() {
    let harness = Harness::new();
    let report = witness_plan(&harness).start(runtime(), ()).await;
    assert!(report.is_clean(), "{}", shown(&report));

    assert_eq!(
        harness.released_before(node::LINK, node::ENDPOINT),
        Some(true),
        "{:?}",
        harness.events()
    );
    assert!(harness.position(&Event::Released(node::JOURNAL)).is_some());
}

/// The plan's §5 update asks whether the pinned rev's new required-output rule
/// changes what `report.is_clean()` means. It does not: `is_clean()`
/// (`sdax/src/report.rs:356-362`) is byte-identical to the rev the plan was
/// written from, and `into_required_output` (`required_output.rs:59-67`) is a
/// **consumer** of it. This test pins both halves of that answer.
#[tokio::test(flavor = "multi_thread")]
async fn required_output_consumes_is_clean_and_does_not_redefine_it() {
    // Half one: a clean run with an export hands the output back.
    let harness = Harness::new();
    let report = witness_plan(&harness).start(runtime(), ()).await;
    assert!(report.is_clean());
    assert_eq!(*report.into_required_output().expect("completed output"), 1);

    // Half two: `is_clean()` does not require an output, so the new rule adds
    // a distinction ABOVE it rather than tightening it. A clean run with no
    // export is still clean and is still `MissingOutput`.
    let report = plan_without_an_export().start(runtime(), ()).await;
    assert!(report.is_clean(), "{}", shown(&report));
    assert!(report.output.is_none());
    match report.into_required_output() {
        Err(RequiredOutputError::MissingOutput(report)) => {
            assert!(report.is_clean(), "the report survives the error");
        }
        Err(RequiredOutputError::Failed(report)) => {
            panic!("a clean run must not be Failed: {}", shown(&report));
        }
        Ok(_) => panic!("no output was exported, so none may be fabricated"),
    }
}

/// A plan that exports nothing, for the second half above. Nothing in Phase 2
/// needs it otherwise.
fn plan_without_an_export() -> Plan<()> {
    let mut p = Plan::builder("NoExport");
    p.resource("Nothing")
        .acquire(|cx: Cx<Acquire>, ()| async move { Ok(cx.hold_value(())) })
        .release(|_cx: Cx<Release>, _value: Arc<()>| async move { Ok(()) });
    p.build(
        Policy::FailFast,
        Shutdown::within(Duration::from_secs(10)),
        Mode::Finite,
    )
    .expect("valid")
}

/// `Running::cancel()` — "in-flight bodies are interrupted, the release graph
/// still runs (INV-7)" (`sdax-tokio/src/running.rs:262`). AR-08 says the same
/// thing from the Glade side: shutdown "drains or cooperatively cancels
/// children, then releases the resources they use".
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_run_still_executes_the_release_graph() {
    let harness = Harness::new();
    harness.block_exchange();
    let running = witness_plan(&harness).start(runtime(), ());

    let handle = running.handle();
    let waiting = harness.clone();
    let cancel_once_it_is_really_in_flight = async move {
        // Cancelling a body that has not started would witness nothing.
        waiting.exchange_started().await;
        handle.cancel();
    };
    let (report, ()) = tokio::join!(running, cancel_once_it_is_really_in_flight);

    assert_eq!(report.outcome, Outcome::Cancelled);
    assert!(
        !report.is_clean(),
        "a cancelled run is not a clean run: {}",
        shown(&report)
    );
    assert!(
        report.cleanup_failures.is_empty(),
        "cancellation must not break cleanup: {}",
        shown(&report)
    );
    assert!(
        report.incomplete.is_empty(),
        "the release graph had its full budget: {}",
        shown(&report)
    );

    // The whole release graph ran, in order, even though the run was
    // interrupted mid-step.
    assert_eq!(
        harness.released_before(node::LINK, node::ENDPOINT),
        Some(true),
        "{:?}",
        harness.events()
    );
    assert!(harness.position(&Event::Released(node::JOURNAL)).is_some());
    assert!(
        harness.position(&Event::Ran(node::EXCHANGE)).is_none(),
        "the cancelled body did not complete"
    );
}

/// The honest half. The endpoint's release never returns, so the budget must
/// expire and the report must **name** the obligation it abandoned:
/// `Report::incomplete` is "obligations the shutdown budget abandoned"
/// (`sdax/src/report.rs:333`).
///
/// The budget is short here and nowhere else. Plan §9.3 forbids shortening a
/// drain "to make a test quick — that turns a drain into an abandonment"; this
/// fixture's obligation can never be discharged, so it is abandoned at *every*
/// budget and the short one only spends less wall clock saying so.
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_budget_names_the_resource_it_abandoned() {
    let harness = Harness::new().with_shutdown_budget(Duration::from_millis(200));
    harness.hang_endpoint_release();
    let report = witness_plan(&harness).start(runtime(), ()).await;

    assert_eq!(report.incomplete.len(), 1, "{}", shown(&report));
    assert_eq!(report.incomplete[0].node.leaf(), node::ENDPOINT);

    // This is the sharp one, and the reason §8.3 refuses `outcome == Ok` as
    // evidence: the run ended Ok and still abandoned a resource.
    assert_eq!(report.outcome, Outcome::Ok);
    assert!(
        !report.is_clean(),
        "an abandoned obligation is never silent: {}",
        shown(&report)
    );

    // The child still released: an abandoned parent does not abandon the
    // cleanup that was already ordered before it.
    assert!(harness.position(&Event::Released(node::LINK)).is_some());
    assert!(
        harness
            .position(&Event::ReleaseStalled(node::ENDPOINT))
            .is_some()
    );
    assert!(
        harness.position(&Event::Released(node::ENDPOINT)).is_none(),
        "a release that never returned must not be recorded as done"
    );
}
