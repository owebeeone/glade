//! Step 2.1 — AR-08's reverse order, asserted **statically**, from the Glade
//! side, with no runtime and no body ever run.
//!
//! `AsyncWitnessPlan.md` §9.3 records why this is asserted here rather than
//! taken from sdax-rs's own checker: `sdax-testkit`'s release-order invariants
//! INV-5 and INV-6 have no negative fixture, because the core derives the order
//! *from* the edge set, so "the two computations cannot disagree unless the
//! core's derivation regresses" (`sdax-testkit/src/invariants.rs:27-32`). They
//! are regression guards, not independent falsification. These assertions are
//! the witness's own, written against the public `Plan::inspect()` surface.
//!
//! The clause under test, verbatim (`arch1/RuntimeAndAssurance.md:126`):
//! "Parent resource remains owned while child cleanup is pending … Concurrent
//! independent cleanup can progress."

use async_witness_real::{Harness, node, witness_plan};

/// AR-08, first clause. `before(a, b)` is "a's obligation finishes before b's
/// starts" (`sdax/src/view.rs:477`), so the child naming the parent in `needs`
/// is what keeps the parent owned while the child's cleanup is pending.
#[test]
fn child_cleanup_finishes_before_its_parent_begins() {
    let plan = witness_plan(&Harness::new());
    let order = plan.inspect().release_order();

    assert!(order.before(node::LINK, node::ENDPOINT));
    assert!(order.before(node::EXCHANGE, node::LINK));
    assert!(
        order.before(node::EXCHANGE, node::ENDPOINT),
        "the order is the transitive closure of the reverse `needs` DAG"
    );
}

/// The same claim from the other side: nothing licenses releasing the parent
/// first. A one-directional assertion would pass against an order that made
/// every pair mutually "before".
#[test]
fn the_parent_is_never_released_before_its_child() {
    let plan = witness_plan(&Harness::new());
    let order = plan.inspect().release_order();

    assert!(!order.before(node::ENDPOINT, node::LINK));
    assert!(!order.before(node::LINK, node::EXCHANGE));
    assert!(!order.unordered(node::LINK, node::ENDPOINT));
}

/// AR-08, third clause: "Concurrent independent cleanup can progress." The
/// order is a **partial** order, and `InjectionGraphRefinement.md:41-45` says
/// the architecture means it that way — "partial ordering constraints, not a
/// total serial shutdown algorithm". Two branches that share no `needs` path
/// must come back `unordered`, not merely ordered some arbitrary way.
#[test]
fn two_independent_branches_may_release_concurrently() {
    let plan = witness_plan(&Harness::new());
    let order = plan.inspect().release_order();

    assert!(order.unordered(node::ENDPOINT, node::JOURNAL));
    assert!(order.unordered(node::LINK, node::JOURNAL));
    assert!(order.unordered(node::EXCHANGE, node::RECORD));
    assert!(order.before(node::RECORD, node::JOURNAL));

    let unordered: Vec<String> = order
        .unordered_pairs()
        .iter()
        .map(|(a, b)| format!("{a}|{b}"))
        .collect();
    assert!(
        unordered.contains(&format!("{}|{}", node::ENDPOINT, node::JOURNAL)),
        "unordered_pairs disagrees with unordered(): {unordered:?}"
    );
}

/// The edges are exactly the three the plan declares. An implicit edge the
/// author did not write would make the order above true for the wrong reason.
#[test]
fn the_declared_edges_are_the_only_edges() {
    let plan = witness_plan(&Harness::new());
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
            format!("{}->{}", node::EXCHANGE, node::LINK),
            format!("{}->{}", node::LINK, node::ENDPOINT),
            format!("{}->{}", node::RECORD, node::JOURNAL),
        ]
    );

    let mut names: Vec<&str> = view.nodes.iter().map(|n| n.path.leaf()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            node::ENDPOINT,
            node::EXCHANGE,
            node::JOURNAL,
            node::LINK,
            node::RECORD,
        ]
    );
}

/// `inspect()` is pure (`sdax/docs/Inspect.md`), so this whole step needs no
/// tokio runtime at all. That is worth asserting rather than assuming: if a
/// body had run, the order above would be a fact about a *run*, not about the
/// declaration, and Step 2.2 would have nothing left to show.
#[test]
fn asserting_the_order_runs_no_body() {
    let harness = Harness::new();
    let plan = witness_plan(&harness);
    let order = plan.inspect().release_order();

    assert!(order.before(node::LINK, node::ENDPOINT));
    assert_eq!(harness.events(), Vec::new());
    assert_eq!(harness.acquisitions(), 0);
}
