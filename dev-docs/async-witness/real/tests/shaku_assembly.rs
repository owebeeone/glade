//! Step 3.3 — Shaku assembles over the **already-acquired** handle, from inside
//! an sdax step that `.needs` it.
//!
//! This is the seam `AsyncWitnessPlan.md` §4.2 describes and §4.3 bridges:
//!
//! ```text
//! sdax-rs acquires  ->  Shaku assembles over the acquired handles  ->  sdax-rs releases, in reverse
//! ```
//!
//! Shaku cannot do the first part — `build()` is synchronous and
//! `with_component_override` takes an already-constructed value — so the order
//! is forced rather than chosen. What this file has to show is that the forced
//! order actually works against a live socket: the module resolves the port the
//! **engine** acquired, it never constructs one of its own, and it is torn down
//! before `PeerEndpoint::close(self)` is awaited.
//!
//! **The facade traits are declared here, in `real`.** Step 3.3 cannot import
//! Phase 1's bridge: `architecture-policy.json` does not let `async-witness-real`
//! depend on `async-witness-fast`, and widening an allowlist to make a check
//! pass is forbidden (plan §7, and the Phase 1/2 update, item 1). "Depends on
//! 1.1" means the pattern, not the crate — and a facade is local to one
//! assembly anyway.
//!
//! **The construction counter is process-wide**, so this file must never share
//! a process with one that drives it above zero. The positive control that
//! shows the counter is not vacuous is a test binary of its own,
//! `tests/shaku_registration.rs`, exactly as Phase 1 split
//! `di_e01_eager_construction.rs` out.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_witness_real::peer_plan::{self, PeerEvent, PeerHarness, node, peer_plan};
use async_witness_real::shaku_bridge::binding_carrier_builds;
use sdax::prelude::*;
use sdax_tokio::{PlanStart, TokioRuntime};

const RELEASE_BOUND: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(5);

fn runtime() -> Arc<TokioRuntime> {
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()))
}

fn shown<Out>(report: &Report<Out>) -> String {
    format!("{report}")
}

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

/// Step 3.3's observable result: the module is built inside the run, resolves
/// the engine's own carrier, puts a frame across it, and builds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_module_assembles_over_the_acquired_handle_and_builds_no_endpoint() {
    let harness = PeerHarness::new().with_shaku_module();
    let report = peer_plan(&harness).start(runtime(), ()).await;

    assert!(report.is_clean(), "{}", shown(&report));
    assert!(
        harness.position(&PeerEvent::Ran(node::MODULE)).is_some(),
        "{:?}",
        harness.events()
    );

    // The port Shaku injected is the port the engine acquired: a frame sent
    // through the resolved `Arc<dyn CarrierPort>` came out of the *other*
    // endpoint's link. A module holding a provider of its own could not do
    // that, because no second endpoint was ever bound.
    let received = harness.module_received();
    assert_eq!(received.len(), 1, "{received:?}");
    assert_eq!(
        peer_plan::join(&received[0]),
        Ok(peer_plan::module_frame()),
        "the frame Shaku's injected port sent is the frame the served link read"
    );

    // And the registered stand-in — the component a production composition
    // would bind for `dyn Carrier`, which would bind a socket — never ran.
    assert_eq!(
        binding_carrier_builds(),
        0,
        "the module constructed a carrier of its own"
    );
}

/// The plan's Step 3.3 column, verbatim: "The module step's release is ordered
/// before the endpoint's, asserted by `release_order().before(...)`."
///
/// Static: `Plan::inspect()` runs no body, so this is a statement about the
/// declaration rather than about one lucky run.
#[test]
fn the_module_step_is_ordered_before_every_endpoint() {
    let plan = peer_plan(&PeerHarness::new().with_shaku_module());
    let order = plan.inspect().release_order();

    assert!(order.before(node::MODULE, node::ACCEPTOR));
    assert!(order.before(node::MODULE, node::DIALER));
    assert!(order.before(node::MODULE, node::SERVED));
    assert!(order.before(node::MODULE, node::DIALED));

    // The other direction, so the claim is not true of every pair: nothing
    // licenses closing an endpoint before the module that borrowed its link.
    assert!(!order.before(node::ACCEPTOR, node::MODULE));
    assert!(!order.unordered(node::MODULE, node::ACCEPTOR));

    // The module runs after the exchange rather than beside it, so the two
    // never contend for the same stream.
    assert!(order.before(node::MODULE, node::EXCHANGE));
}

/// The module step is an addition, not a rearrangement: with it, the plan has
/// exactly one more node and exactly three more edges, and every edge the
/// plain plan declares is still declared.
#[test]
fn the_module_step_adds_a_node_and_changes_no_other_edge() {
    let plain = edges_and_nodes(&PeerHarness::new());
    let assembled = edges_and_nodes(&PeerHarness::new().with_shaku_module());

    assert_eq!(assembled.0.len(), plain.0.len() + 1);
    assert!(plain.0.iter().all(|name| assembled.0.contains(name)));
    assert!(assembled.0.contains(&node::MODULE.to_owned()));

    let added: Vec<&String> = assembled
        .1
        .iter()
        .filter(|e| !plain.1.contains(e))
        .collect();
    assert_eq!(
        added,
        vec![
            &format!("{}->{}", node::MODULE, node::DIALED),
            &format!("{}->{}", node::MODULE, node::EXCHANGE),
            &format!("{}->{}", node::MODULE, node::SERVED),
        ]
    );
    assert!(plain.1.iter().all(|edge| assembled.1.contains(edge)));
}

fn edges_and_nodes(harness: &PeerHarness) -> (Vec<String>, Vec<String>) {
    let plan = peer_plan(harness);
    let view = plan.inspect();
    let mut nodes: Vec<String> = view
        .nodes
        .iter()
        .map(|n| n.path.leaf().to_owned())
        .collect();
    let mut edges: Vec<String> = view
        .edges
        .iter()
        .map(|e| format!("{}->{}", e.from.leaf(), e.to.leaf()))
        .collect();
    nodes.sort();
    edges.sort();
    (nodes, edges)
}

/// The sharp risk the plan names at this seam (§4.2): "The Shaku module holds
/// `Arc` clones of things derived from the endpoint, and an escaped clone keeps
/// the UDP socket bound."
///
/// It does not, here, and the reason is worth stating: the provider gives its
/// `SendStream`, `RecvStream` and `Connection` up **by value** on release, so a
/// module that outlives the step holds a carrier that owns nothing. This is
/// what makes §8.4 caveat 3 a caveat — an `Arc` escape is possible in
/// principle, and this composition demonstrably prevents it from holding a
/// socket. Contrast `tests/peer_release.rs`, where an escaped `PeerEndpoint`
/// clone — a handle the engine never owned — does keep the port bound.
#[tokio::test(flavor = "multi_thread")]
async fn a_module_that_outlives_the_step_holds_no_socket() {
    let harness = PeerHarness::new().with_shaku_module().keep_the_module();
    let report = peer_plan(&harness).start(runtime(), ()).await;

    assert!(report.is_clean(), "{}", shown(&report));
    assert!(
        harness.holds_the_module(),
        "the fixture is only meaningful while the module is still alive"
    );
    assert_eq!(binding_carrier_builds(), 0);

    for (endpoint, port) in &harness.ports() {
        let freed = wait_until_free(*port).await.unwrap_or_else(|| {
            panic!(
                "{endpoint} port {port} was still bound at {RELEASE_BOUND:?} with the module alive"
            )
        });
        println!("{endpoint} port {port} free after {freed:?} with the module still held");
    }

    // The module really is still resolvable, so nothing was quietly emptied
    // out from under the assertion above.
    assert!(harness.resolve_kept_module().is_some());
}
