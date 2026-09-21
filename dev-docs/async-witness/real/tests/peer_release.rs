//! Step 3.2 — **AR-08 for real**: the endpoint is released last, and the
//! socket is actually free afterwards.
//!
//! §8.3 sets the bar: "the recorded UDP port can be re-bound". §8.4 caveat 3
//! sets what that decides — an `Arc` escape "is a caveat **only if** the
//! witness's own composition demonstrably prevents it … If the port cannot be
//! re-bound, it is not a caveat; it is a DI-E01 and AR-08 failure."
//!
//! **A proof that cannot fail is not evidence**, so this file contains both
//! directions. The ordinary composition frees both ports. A variant in which
//! one clone of the acceptor's endpoint deliberately escapes leaves that port
//! bound at the whole bound, and frees it the moment the clone is dropped —
//! and the sharp part, worth stating plainly: **that run is still
//! `is_clean()`**. A leaked handle is invisible to the report. Only the socket
//! tells the truth, which is why the plan chose this port.
//!
//! **Why the wait is bounded rather than immediate** (plan §6, the update from
//! Phase 0): iroh gives no signal for the socket being released. Its driver
//! task ends a few milliseconds after `close` resolves and the last handle
//! drops — the node measured 6 to 10 ms with its whole suite running in
//! parallel. An immediate bind that fails is not a leak; a bind that still
//! fails at the bound is.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_witness_real::peer_plan::{PeerHarness, node, peer_plan};
use sdax::prelude::*;
use sdax_tokio::{PlanStart, TokioRuntime};

/// The bound the node's own tests use (`iroh_carrier.rs:221-233`). Two seconds
/// against a release measured in single-digit milliseconds.
const RELEASE_BOUND: Duration = Duration::from_secs(2);

/// How often to ask, also the node's figure.
const POLL: Duration = Duration::from_millis(5);

/// How long to wait for tasks that have already been asked to finish.
const WAIT: Duration = Duration::from_secs(10);

fn runtime() -> Arc<TokioRuntime> {
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()))
}

fn shown<Out>(report: &Report<Out>) -> String {
    format!("{report}")
}

/// Wait for `port` to become bindable, bounded, and say how long it took.
/// `None` means it was still bound at the bound.
///
/// This is the witness's own copy of the node's `port_is_freed` helper, which
/// lives in a private `#[cfg(test)]` module and cannot be imported.
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

/// The helper itself can answer "still bound", and takes the whole bound to say
/// so. Without this, a `Some(..)` everywhere else could mean the check is
/// vacuous rather than that the port is free.
#[tokio::test(flavor = "multi_thread")]
async fn the_release_check_can_answer_still_bound() {
    let held = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a free port");
    let port = held.local_addr().expect("a bound address").port();

    let started = Instant::now();
    assert_eq!(
        wait_until_free(port).await,
        None,
        "a port this test is holding must never read as free"
    );
    assert!(
        started.elapsed() >= RELEASE_BOUND,
        "the bound is what makes a negative answer meaningful: {:?}",
        started.elapsed()
    );

    drop(held);
    assert!(
        wait_until_free(port).await.is_some(),
        "and it answers free once the holder is gone"
    );
}

/// AR-08's observable result for Step 3.2. Both endpoints bound a real UDP
/// socket, the run was clean, and both ports re-bind: **no clone escaped**.
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_run_frees_every_port_it_bound() {
    let harness = PeerHarness::new();
    let rt = runtime();
    let report = peer_plan(&harness).start(rt.clone(), ()).await;

    assert!(report.is_clean(), "{}", shown(&report));
    assert!(report.incomplete.is_empty(), "{}", shown(&report));

    // §8.3, read the way the plan's update item 3 requires: `tracked()` is read
    // *together with* an empty `incomplete`, never on its own, because an
    // expired budget aborts the task as well as listing it.
    assert_eq!(rt.shutdown(WAIT).await, Ok(()));
    assert_eq!(rt.tracked(), 0);

    let ports = harness.ports();
    assert_eq!(ports.len(), 2, "{ports:?}");
    for (endpoint, port) in &ports {
        let freed = wait_until_free(*port).await.unwrap_or_else(|| {
            panic!("{endpoint} port {port} was still bound at {RELEASE_BOUND:?}")
        });
        println!("{endpoint} port {port} was free after {freed:?}");
    }
}

/// The same claim as a fact about the run rather than about the socket: the
/// endpoints' releases really ran, and really ran last.
#[tokio::test(flavor = "multi_thread")]
async fn the_endpoints_are_the_last_things_released() {
    let harness = PeerHarness::new();
    let report = peer_plan(&harness).start(runtime(), ()).await;
    assert!(report.is_clean(), "{}", shown(&report));

    assert_eq!(
        harness.released_before(node::SERVED, node::ACCEPTOR),
        Some(true),
        "{:?}",
        harness.events()
    );
    assert_eq!(
        harness.released_before(node::DIALED, node::ACCEPTOR),
        Some(true),
        "{:?}",
        harness.events()
    );
    assert_eq!(
        harness.released_before(node::DIALED, node::DIALER),
        Some(true),
        "{:?}",
        harness.events()
    );
}

/// **The falsification.** One clone of the acceptor's endpoint escapes the
/// composition from inside the acquire body that bound it. Everything else is
/// identical, and the difference is visible only in the socket.
#[tokio::test(flavor = "multi_thread")]
async fn an_escaped_clone_keeps_the_port_bound_until_it_is_dropped() {
    let harness = PeerHarness::new().let_a_clone_escape();
    let rt = runtime();
    let report = peer_plan(&harness).start(rt.clone(), ()).await;

    // The sharp part: a leaked handle is invisible to the report. Everything
    // sdax can see is clean, and the runtime is genuinely empty.
    assert!(report.is_clean(), "{}", shown(&report));
    assert_eq!(rt.shutdown(WAIT).await, Ok(()));
    assert_eq!(rt.tracked(), 0);
    assert!(harness.holds_an_escaped_clone());

    let acceptor_port = harness
        .port(node::ACCEPTOR)
        .expect("a recorded acceptor port");
    let dialer_port = harness.port(node::DIALER).expect("a recorded dialer port");

    let started = Instant::now();
    assert_eq!(
        wait_until_free(acceptor_port).await,
        None,
        "an escaped clone must keep port {acceptor_port} bound for the whole bound"
    );
    println!(
        "escaped clone held port {acceptor_port} for {:?}",
        started.elapsed()
    );

    // The leak is exactly as wide as the clone: the endpoint nobody cloned is
    // released normally, in the same run.
    let freed = wait_until_free(dialer_port)
        .await
        .expect("the dialer's port is not the one that leaked");
    println!("dialer port {dialer_port} was free after {freed:?}");

    // And it is the clone, not something else about the variant: dropping it
    // frees the port.
    assert!(harness.drop_escaped_clone());
    let freed = wait_until_free(acceptor_port)
        .await
        .expect("the port is free once every clone is gone");
    println!("port {acceptor_port} was free {freed:?} after the clone was dropped");
}
