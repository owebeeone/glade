//! Plan Step 3.3's done-when, second half: the witness's partial-order release
//! test (`dev-docs/async-witness/real/tests/release_order.rs`) reproduced on
//! the node's own plan, `glade_node::lifecycle::node_plan`.
//!
//! The order under test is `arch1/InjectionGraphRefinement.md:42-45`, at the
//! glade-wz root: "Records drains before storage/peer-carrier release;
//! Sessions drains before peer/client-carrier release. These are partial
//! ordering constraints, not a total serial shutdown algorithm. Independent
//! cleanup may still run concurrently."
//!
//! Most of it is asserted on the declaration, through `Plan::inspect()`, which
//! is pure: no body runs, so nothing binds, boots or writes. One test also
//! drives the plan through sdax-testkit's scripted driver, which simulates a
//! start and a stop with no body run and checks every step against sdax's own
//! invariants. It then asserts the same order on the simulated trace, where
//! the static tests read it from the declaration.

use std::sync::{Arc, Mutex};

use glade_node::assembly::{real_providers_constructed, Settings};
use glade_node::lifecycle::{node, node_plan, Console, NodeStart};
use sdax::{Request, Script};
use sdax_testkit::eol::{is_cleanup_end, is_cleanup_start};
use sdax_testkit::invariants::check_plan;
use sdax_testkit::ScriptedDriver;

/// The four constraints of the partial order: in each pair the first drains
/// before the second is released.
const DRAINS_BEFORE: [(&str, &str); 4] = [
    (node::RECORDS, node::STORAGE),
    (node::RECORDS, node::PEER_CARRIER),
    (node::SESSIONS, node::PEER_CARRIER),
    (node::SESSIONS, node::CLIENT_CARRIER),
];

/// A console that keeps what it is told, so a test can say nothing was.
#[derive(Default)]
struct Kept(Mutex<Vec<String>>);

impl Console for Kept {
    fn out(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_owned());
    }

    fn err(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_owned());
    }
}

/// A booted start: the shape with every node in use.
fn booted(console: Arc<Kept>) -> NodeStart {
    let args = ["--profile", "local", "--name", "order", "0"];
    let settings = Settings::from_args(args.map(String::from));
    NodeStart::from_settings(settings, Vec::new(), console)
}

/// The architecture's constraints hold as `before`: each drainer's cleanup
/// ends before the resource it drains into begins its own.
#[test]
fn records_and_sessions_drain_before_what_they_use_is_released() {
    let order = node_plan().inspect().release_order();
    for (drains, released) in DRAINS_BEFORE {
        assert!(
            order.before(drains, released),
            "{drains} must drain before {released} is released"
        );
    }
}

/// The same claim from the other side: nothing licenses releasing a carrier
/// or the storage first. A one-directional assertion would pass against an
/// order that made every pair mutually "before".
#[test]
fn nothing_is_released_before_what_drains_into_it() {
    let order = node_plan().inspect().release_order();
    for (drains, released) in DRAINS_BEFORE {
        assert!(
            !order.before(released, drains),
            "{released} before {drains}"
        );
        assert!(!order.unordered(drains, released), "{drains} ‖ {released}");
    }
}

/// "Independent cleanup may still run concurrently": the two drainers share
/// no `needs` path, and neither do the two carriers, so each pair must come
/// back unordered, not merely ordered some arbitrary way.
#[test]
fn independent_cleanup_may_run_concurrently() {
    let order = node_plan().inspect().release_order();
    assert!(order.unordered(node::RECORDS, node::SESSIONS));
    assert!(order.unordered(node::PEER_CARRIER, node::CLIENT_CARRIER));
    assert!(order.unordered(node::RECORDS, node::CLIENT_CARRIER));
    let unordered: Vec<String> = order
        .unordered_pairs()
        .iter()
        .map(|(a, b)| format!("{a}|{b}"))
        .collect();
    for pair in [
        format!("{}|{}", node::PEER_CARRIER, node::CLIENT_CARRIER),
        format!("{}|{}", node::RECORDS, node::SESSIONS),
    ] {
        assert!(unordered.contains(&pair), "{pair} missing: {unordered:?}");
    }
}

/// The edges are exactly the ones the plan declares. An implicit edge the
/// author did not write would make the order above true for the wrong reason.
#[test]
fn the_declared_edges_are_the_only_edges() {
    let view = node_plan().inspect();
    let mut edges: Vec<String> = view
        .edges
        .iter()
        .map(|e| format!("{}->{}", e.from.leaf(), e.to.leaf()))
        .collect();
    edges.sort();
    let mut expected = vec![
        (node::ASSEMBLY, node::INSTANCE),
        (node::STORAGE, node::INSTANCE),
        (node::STORAGE, node::ASSEMBLY),
        (node::PEER_CARRIER, node::INSTANCE),
        (node::PEER_CARRIER, node::STORAGE),
        (node::CLIENT_CARRIER, node::STORAGE),
        (node::RECORDS, node::STORAGE),
        (node::RECORDS, node::PEER_CARRIER),
        (node::SESSIONS, node::STORAGE),
        (node::SESSIONS, node::PEER_CARRIER),
        (node::SESSIONS, node::CLIENT_CARRIER),
        (node::PEERS, node::STORAGE),
        (node::PEERS, node::SESSIONS),
        (node::WORKSPACES, node::ASSEMBLY),
        (node::WORKSPACES, node::STORAGE),
        (node::WORKSPACES, node::PEERS),
        (node::WORKSPACES, node::RECORDS),
        (node::LISTENING, node::CLIENT_CARRIER),
        (node::LISTENING, node::SESSIONS),
        (node::LISTENING, node::WORKSPACES),
    ]
    .into_iter()
    .map(|(from, to)| format!("{from}->{to}"))
    .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(edges, expected);

    let mut names: Vec<&str> = view.nodes.iter().map(|n| n.path.leaf()).collect();
    names.sort_unstable();
    let mut all = vec![
        node::INSTANCE,
        node::ASSEMBLY,
        node::STORAGE,
        node::PEER_CARRIER,
        node::CLIENT_CARRIER,
        node::RECORDS,
        node::SESSIONS,
        node::PEERS,
        node::WORKSPACES,
        node::LISTENING,
    ];
    all.sort_unstable();
    assert_eq!(names, all);
}

/// sdax-testkit's static checker agrees with the declaration (INV-1, INV-5,
/// INV-6). Its own docs say INV-5 and INV-6 have no negative fixture, because
/// the core derives the order from the edges: a regression guard, not an
/// independent falsification. The assertions above are the independent ones.
#[test]
fn sdax_testkit_finds_no_violation_in_the_declaration() {
    assert_eq!(check_plan(&node_plan().inspect()), Vec::new());
}

/// The order as a fact about a run rather than a declaration: a simulated
/// start, then a stop at one second, with no body run. Every step is checked
/// against sdax's invariants as it is fed, and each drainer's cleanup has
/// ended before the cleanup of what it drains into begins.
#[test]
fn a_simulated_stop_releases_in_that_order() {
    let console = Arc::new(Kept::default());
    let plan = node_plan();
    let script = Script::new().at(1.0, Request::Shutdown);
    let driven = ScriptedDriver::run_with_input(&plan, booted(console.clone()), &script)
        .expect("the plan simulates");
    assert_eq!(driven.problems(), None);
    assert!(driven.report.is_clean(), "{}", driven.report);
    let trace = driven.eol();
    for (drains, released) in DRAINS_BEFORE {
        let ended = trace
            .pos(is_cleanup_end, drains)
            .expect("a cleanup that ended");
        let began = trace
            .pos(is_cleanup_start, released)
            .expect("a cleanup that began");
        assert!(
            ended < began,
            "{drains} ended at {ended}, {released} began at {began}"
        );
    }
    assert_eq!(*console.0.lock().unwrap(), Vec::<String>::new());
}

/// Inspecting and simulating run no body: no provider was constructed and
/// nothing was said. If a body had run, the order above would be a fact about
/// that run and not about the declaration.
#[test]
fn asserting_the_order_runs_no_body() {
    let console = Arc::new(Kept::default());
    let plan = node_plan();
    let _ = plan.inspect().release_order();
    let script = Script::new().at(1.0, Request::Shutdown);
    let _ = ScriptedDriver::run_with_input(&plan, booted(console.clone()), &script);
    assert_eq!(real_providers_constructed(), 0);
    assert_eq!(*console.0.lock().unwrap(), Vec::<String>::new());
}
