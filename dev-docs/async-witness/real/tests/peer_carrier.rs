//! Step 3.1 — the witness's `CarrierPort` over the node's **real**
//! `PeerEndpoint`, acquired inside `cx.hold(...)` so the engine owns the
//! cleanup in the same poll that the bind completes.
//!
//! Phase 2 decided AR-08 over fakes so that a lifecycle failure could never be
//! mistaken for an injector failure (`AsyncWitnessPlan.md` §8.1). This file
//! swaps the fakes for the thing itself: two witness nodes bind localhost QUIC
//! endpoints, one dials the other, both complete the node<->node HELLO seam,
//! and one real glade `Frame` crosses the witness's own port — all of it driven
//! by an sdax plan rather than by the test body.
//!
//! **What makes it the real port.** `glade_node::iroh_carrier::PeerEndpoint` is
//! the node's own type, unmodified; `bind_with` awaits a real UDP socket bind
//! (`iroh_carrier.rs:86`), `dial` awaits a QUIC connect plus `hello_dial`
//! (`:108`), `accept` awaits an inbound connection plus `hello_accept`
//! (`:118`). The bytes on the wire are the node's own `[u32 LE len][tag][CBOR
//! body]` framing from `peer.rs:36-53`, so a `Frame` the node encoded is a
//! `Frame` the witness's port delivers.
//!
//! **What it is not.** Nothing here asserts that the socket is released; that
//! is Step 3.2, and it needs its own evidence. Nothing here involves Shaku.

use std::sync::Arc;

use async_witness_real::peer_plan::{self, PeerEvent, PeerHarness, node, peer_plan};
use glade_node::frame::Frame;
use sdax::prelude::*;
use sdax_tokio::{PlanStart, TokioRuntime};

fn runtime() -> Arc<TokioRuntime> {
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()))
}

/// Render a report the way a failing assertion should show it.
fn shown<Out>(report: &Report<Out>) -> String {
    format!("{report}")
}

/// The whole of Step 3.1's observable result, in one run: two endpoints bound,
/// a dial, a HELLO on both sides, and one `Frame` across the port.
#[tokio::test(flavor = "multi_thread")]
async fn two_witness_nodes_bind_dial_hello_and_exchange_one_frame() {
    let harness = PeerHarness::new();
    let report = peer_plan(&harness).start(runtime(), ()).await;

    assert!(report.is_clean(), "{}", shown(&report));
    assert_eq!(report.outcome, Outcome::Ok);
    assert_eq!(report.output.as_deref(), Some(&1));

    // Both endpoints really bound: a port was recorded from `addr()` inside the
    // run, before anything released.
    let ports = harness.ports();
    assert_eq!(ports.len(), 2, "{ports:?}");
    assert!(ports.values().all(|port| *port != 0), "{ports:?}");

    // The HELLO seam completed on BOTH sides and each learned the other's glade
    // node_id — the dialer's link reports the acceptor's, and the acceptor's
    // link reports the dialer's.
    assert_eq!(
        harness.learned_peer(node::DIALED),
        Some(peer_plan::acceptor_identity().node_id),
        "the dialer learns the acceptor's node_id over iroh"
    );
    assert_eq!(
        harness.learned_peer(node::SERVED),
        Some(peer_plan::dialer_identity().node_id),
        "the acceptor learns the dialer's node_id over iroh"
    );

    // One real glade `Frame`, sent through `CarrierPort::send` on the dialed
    // link and taken back out of `CarrierPort::recv` on the served one.
    let received = harness.received();
    assert_eq!(received.len(), 1, "{received:?}");
    assert_eq!(
        peer_plan::join(&received[0]),
        Ok(peer_plan::exchange_frame()),
        "the frame that arrived is the frame that was sent"
    );
    assert!(matches!(peer_plan::exchange_frame(), Frame::Ops(_)));
}

/// AR-08's first clause against the real endpoint, statically: the link is the
/// endpoint's child, so its cleanup finishes before the endpoint's begins. The
/// same assertion Phase 2 made over fakes, now over the declaration that binds
/// a socket. `Plan::inspect()` runs no body, so this costs no I/O at all.
#[test]
fn the_link_is_released_before_the_endpoint_that_produced_it() {
    let plan = peer_plan(&PeerHarness::new());
    let order = plan.inspect().release_order();

    assert!(order.before(node::SERVED, node::ACCEPTOR));
    assert!(order.before(node::DIALED, node::DIALER));
    assert!(
        order.before(node::DIALED, node::ACCEPTOR),
        "the dialer's link needs the acceptor's address, so it is its child too"
    );
    assert!(order.before(node::EXCHANGE, node::SERVED));
    assert!(order.before(node::EXCHANGE, node::DIALED));
    assert!(
        order.before(node::EXCHANGE, node::ACCEPTOR),
        "the order is the transitive closure of the reverse `needs` DAG"
    );

    // The same claim from the other side: nothing licenses releasing an
    // endpoint first, and the pair is genuinely ordered rather than unordered.
    assert!(!order.before(node::ACCEPTOR, node::SERVED));
    assert!(!order.unordered(node::SERVED, node::ACCEPTOR));
    assert!(!order.unordered(node::DIALED, node::DIALER));
}

/// The edges are exactly the ones the plan declares. An implicit edge would
/// make the ordering above true for the wrong reason.
#[test]
fn the_declared_edges_are_the_only_edges() {
    let plan = peer_plan(&PeerHarness::new());
    let view = plan.inspect();

    let mut edges: Vec<String> = view
        .edges
        .iter()
        .map(|e| format!("{}->{}", e.from.leaf(), e.to.leaf()))
        .collect();
    edges.sort();
    assert_eq!(
        edges,
        vec![
            format!("{}->{}", node::DIALED, node::ACCEPTOR),
            format!("{}->{}", node::DIALED, node::DIALER),
            format!("{}->{}", node::EXCHANGE, node::DIALED),
            format!("{}->{}", node::EXCHANGE, node::SERVED),
            format!("{}->{}", node::SERVED, node::ACCEPTOR),
        ]
    );

    let mut names: Vec<&str> = view.nodes.iter().map(|n| n.path.leaf()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            node::ACCEPTOR,
            node::DIALED,
            node::DIALER,
            node::EXCHANGE,
            node::SERVED,
        ]
    );
}

/// The release graph really ran, in the declared order, on the real endpoint:
/// a link's release completed before the endpoint's did, for both endpoints.
#[tokio::test(flavor = "multi_thread")]
async fn the_run_releases_in_the_order_it_declared() {
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
        harness.released_before(node::DIALED, node::DIALER),
        Some(true),
        "{:?}",
        harness.events()
    );
    assert!(
        harness.position(&PeerEvent::Ran(node::EXCHANGE)).is_some(),
        "{:?}",
        harness.events()
    );
}
