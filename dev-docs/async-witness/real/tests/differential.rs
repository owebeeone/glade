//! Step 3.4 — **the differential**: the same sdax plan, run with and without
//! the Shaku module step.
//!
//! This is the step `AsyncWitnessPlan.md` §8.1 reserves for separating the two
//! questions the witness could otherwise confuse:
//!
//! > If the identical sdax plan is `is_clean()` and frees the port **without**
//! > the Shaku module step, and is not clean or does not free the port **with**
//! > it, the fault is the bridge's and `selection_reopened` is recorded. If
//! > both runs fail the same way, the fault is not Shaku's.
//!
//! So the two runs must differ in **one** thing. [`observe`] takes a single
//! `bool`, and that bool reaches exactly one place: `PeerHarness::with_shaku_module`,
//! which adds one node to the plan. Everything else — the plan, the bodies, the
//! budgets, the runtime, the order of observations, the code path in this file
//! — is shared. `tests/shaku_assembly.rs` asserts the structural half of that
//! claim statically: with the module the plan has exactly one more node and
//! exactly three more edges, and every edge of the plain plan survives.
//!
//! A divergence here is not a bug to fix. It is the answer to `async_witness`,
//! and the plan says to report it rather than repair it.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_witness_real::peer_plan::{PeerEvent, PeerHarness, node, peer_plan};
use async_witness_real::shaku_bridge::binding_carrier_builds;
use sdax::prelude::*;
use sdax_tokio::{PlanStart, TokioRuntime};

const RELEASE_BOUND: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(5);
const WAIT: Duration = Duration::from_secs(10);

async fn wait_until_free(port: u16) -> Option<Duration> {
    let started = Instant::now();
    loop {
        if UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return Some(started.elapsed());
        }
        if started.elapsed() >= RELEASE_BOUND {
            return None;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Everything about one run that the two sides are compared on.
///
/// Deliberately **not** the wall-clock timings, and deliberately not the
/// sequence in which releases completed. AR-08's third clause is "concurrent
/// independent cleanup can progress", and the plan means it: `Served` and
/// `Dialed` share no `needs` path, and neither do `Acceptor` and `Dialer`, so
/// `release_order()` reports both pairs `unordered` and the engine is free to
/// finish them in either order. It observably does, run to run, in the same
/// configuration. A differential that compared the sequence would report that
/// freedom as a divergence — so this compares the **partial** order the plan
/// actually declares, which is what AR-08 asks about.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    clean: bool,
    outcome: Outcome,
    faults: usize,
    cleanup_failures: usize,
    ambiguous: usize,
    incomplete: Vec<String>,
    output: Option<usize>,
    shutdown: Result<(), usize>,
    tracked_after: usize,
    /// Which endpoints bound a port, and whether each port came back.
    ports_freed: BTreeMap<&'static str, bool>,
    /// Which releases completed, as a set.
    released: BTreeSet<&'static str>,
    /// The ordered pairs the declaration does constrain, each answered from
    /// the run rather than from the declaration. Keyed by the pair, because
    /// one child can have two parents.
    child_before_parent: BTreeMap<(&'static str, &'static str), Option<bool>>,
    /// How many times the registered stand-in provider was constructed.
    provider_builds: usize,
}

/// One run, with the parts that are compared and the parts that are only
/// recorded kept apart.
struct Run {
    observed: Observed,
    /// The report's own `Display`.
    shown: String,
    /// The order releases actually completed in. Recorded, never compared.
    sequence: Vec<&'static str>,
}

/// One run. The `bool` is the **only** difference between the two sides.
async fn observe(with_module: bool) -> Run {
    let harness = if with_module {
        PeerHarness::new().with_shaku_module()
    } else {
        PeerHarness::new()
    };
    let rt = Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()));
    let report = peer_plan(&harness).start(rt.clone(), ()).await;

    let shown = format!("{report}");
    let shutdown = rt.shutdown(WAIT).await;
    let tracked_after = rt.tracked();

    let mut ports_freed = BTreeMap::new();
    for (endpoint, port) in harness.ports() {
        ports_freed.insert(endpoint, wait_until_free(port).await.is_some());
    }

    let sequence: Vec<&'static str> = harness
        .events()
        .iter()
        .filter_map(|event| match event {
            PeerEvent::Released(name) => Some(*name),
            _ => None,
        })
        .collect();

    let mut child_before_parent = BTreeMap::new();
    for (child, parent) in [
        (node::SERVED, node::ACCEPTOR),
        (node::DIALED, node::ACCEPTOR),
        (node::DIALED, node::DIALER),
    ] {
        child_before_parent.insert((child, parent), harness.released_before(child, parent));
    }

    let observed = Observed {
        clean: report.is_clean(),
        outcome: report.outcome,
        faults: report.faults.len(),
        cleanup_failures: report.cleanup_failures.len(),
        ambiguous: report.ambiguous.len(),
        incomplete: report
            .incomplete
            .iter()
            .map(|record| record.node.leaf().to_owned())
            .collect(),
        output: report.output.as_deref().copied(),
        shutdown,
        tracked_after,
        ports_freed,
        released: sequence.iter().copied().collect(),
        child_before_parent,
        provider_builds: binding_carrier_builds(),
    };
    Run {
        observed,
        shown,
        sequence,
    }
}

/// The pairs the comparison above leaves out really are unordered in the
/// declaration, so leaving them out is reading the plan rather than excusing a
/// difference. Static; no body runs.
#[test]
fn the_uncompared_pairs_are_the_ones_the_plan_declares_unordered() {
    let order = peer_plan(&PeerHarness::new()).inspect().release_order();

    assert!(order.unordered(node::SERVED, node::DIALED));
    assert!(order.unordered(node::ACCEPTOR, node::DIALER));

    // Everything the differential *does* compare is genuinely ordered.
    assert!(order.before(node::SERVED, node::ACCEPTOR));
    assert!(order.before(node::DIALED, node::ACCEPTOR));
    assert!(order.before(node::DIALED, node::DIALER));
}

/// The plan's Step 3.4 column, verbatim: "Both runs are `is_clean()` and both
/// free the port. A divergence is the answer to `async_witness`."
#[tokio::test(flavor = "multi_thread")]
async fn both_runs_are_clean_and_both_free_every_port() {
    let plain = observe(false).await;
    let assembled = observe(true).await;

    println!("--- without the Shaku module step ---\n{}", plain.shown);
    println!(
        "{:#?}\nreleases completed: {:?}",
        plain.observed, plain.sequence
    );
    println!("--- with the Shaku module step ---\n{}", assembled.shown);
    println!(
        "{:#?}\nreleases completed: {:?}",
        assembled.observed, assembled.sequence
    );

    assert!(plain.observed.clean, "without Shaku: {}", plain.shown);
    assert!(
        assembled.observed.clean,
        "WITH Shaku and not without it — by §8.1 that is the bridge's fault: {}",
        assembled.shown
    );

    assert_eq!(
        plain.observed.ports_freed.len(),
        2,
        "{:?}",
        plain.observed.ports_freed
    );
    assert!(plain.observed.ports_freed.values().all(|freed| *freed));
    assert!(
        assembled.observed.ports_freed.values().all(|freed| *freed),
        "WITH Shaku the port did not come back and without it it did — by §8.1 that is the bridge's fault: {:?}",
        assembled.observed.ports_freed
    );
}

/// The differential proper: every observable the two runs share compares
/// equal. A field that differed would be named by the assertion, which is the
/// form §8.1 asks the evidence to take.
#[tokio::test(flavor = "multi_thread")]
async fn the_two_runs_diverge_in_nothing_that_is_observable() {
    let plain = observe(false).await;
    let assembled = observe(true).await;

    assert_eq!(
        plain.observed, assembled.observed,
        "the two runs diverged.\nwithout:\n{}\nwith:\n{}",
        plain.shown, assembled.shown
    );

    // And the comparison is not vacuous: these are the values it compared.
    let seen = &plain.observed;
    assert_eq!(seen.outcome, Outcome::Ok);
    assert_eq!(seen.output, Some(1));
    assert_eq!(seen.shutdown, Ok(()));
    assert_eq!(seen.tracked_after, 0);
    assert!(seen.incomplete.is_empty());
    assert_eq!(seen.provider_builds, 0);
    assert_eq!(
        seen.released,
        [node::ACCEPTOR, node::DIALED, node::DIALER, node::SERVED]
            .into_iter()
            .collect(),
        "every resource released"
    );
    assert_eq!(seen.child_before_parent.len(), 3);
    assert!(
        seen.child_before_parent
            .values()
            .all(|ordered| *ordered == Some(true)),
        "{:?}",
        seen.child_before_parent
    );

    // The last two releases are the two endpoints, in whichever order the
    // engine got to them: parents outlive dependents, and only that.
    let tail: BTreeSet<&str> = plain.sequence[2..].iter().copied().collect();
    assert_eq!(
        tail,
        [node::ACCEPTOR, node::DIALER].into_iter().collect(),
        "{:?}",
        plain.sequence
    );
}

/// Three A/B pairs, so the answer is not one lucky pair. Each pair binds four
/// real endpoints; the whole test is a few hundred milliseconds.
#[tokio::test(flavor = "multi_thread")]
async fn the_answer_repeats() {
    let mut sequences: BTreeSet<Vec<&'static str>> = BTreeSet::new();
    for round in 1..=3 {
        let plain = observe(false).await;
        let assembled = observe(true).await;

        assert!(
            plain.observed.clean,
            "round {round} without Shaku: {}",
            plain.shown
        );
        assert!(
            assembled.observed.clean,
            "round {round} with Shaku: {}",
            assembled.shown
        );
        assert!(
            plain.observed.ports_freed.values().all(|freed| *freed),
            "round {round} without Shaku: {:?}",
            plain.observed.ports_freed
        );
        assert!(
            assembled.observed.ports_freed.values().all(|freed| *freed),
            "round {round} with Shaku: {:?}",
            assembled.observed.ports_freed
        );
        assert_eq!(plain.observed, assembled.observed, "round {round} diverged");

        sequences.insert(plain.sequence);
        sequences.insert(assembled.sequence);
    }

    // Recorded, not asserted. AR-08's third clause is visible here: the
    // sequences vary between runs of the *same* configuration, because two
    // pairs of releases are genuinely unordered and genuinely concurrent.
    println!("release sequences observed over six runs: {sequences:?}");
}

/// The order the two sides are run in does not decide the answer. Without
/// this, a difference could be an artefact of which run went first — a port
/// still settling, a warm allocator, a scheduler that had already spun up.
#[tokio::test(flavor = "multi_thread")]
async fn the_answer_does_not_depend_on_which_side_runs_first() {
    let assembled_first = observe(true).await.observed;
    let plain_second = observe(false).await.observed;
    assert_eq!(assembled_first, plain_second);

    let plain_first = observe(false).await.observed;
    let assembled_second = observe(true).await.observed;
    assert_eq!(plain_first, assembled_second);

    assert_eq!(assembled_first, assembled_second);
}
