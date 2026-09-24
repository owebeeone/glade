//! Plan Step 3.4: the eight journeys of the build entry's step 2
//! (`dev-docs/GladeBuildEntry.md:52-56`), each a consumer test of `NodeAssembly`
//! in a test composition, AR-05 the criterion. The design, with what each
//! journey drives, what it asserts, what its fakes do not prove and the Phase 4
//! provider that replaces them, is `glade/dev-docs/GladeNodeAssembly.md`,
//! "Journeys (plan Step 3.4)".
//!
//! Part (i), `delivery.rs`: publish, exact retry, lost acknowledgement. Part
//! (ii), `leases.rs` and `admission.rs`: renewal, expiry, partial lookup, wrong
//! scope, unknown or denied authority. Plan Step 4.4, `restart.rs`: a restart
//! in the middle of a round, and a retry after a known failure. Nothing here
//! opens a file or a socket, starts a runtime or sleeps: `fakes::run` polls
//! every future, and the fake clock and each test's own steps are the whole
//! schedule. `tests/durable` runs the same journeys over records.json.

// Step 3.2's fakes, shared with `tests/assembly`: this binary uses some of them.
#[allow(dead_code)]
#[path = "../assembly/fakes.rs"]
mod fakes;

// The harness every journey shares, over the engine this binary supplies.
mod node;

// Part (i): the fakes the journeys add, and three journeys.
mod delivery;
mod faults;

// Part (ii): five journeys.
mod admission;
mod leases;

// Plan Step 4.4: restart and retry.
mod restart;

pub use faults::VolatileStore as Engine;
pub use node::{pair, TestNode, T0, WS};

/// The engine a test node persists through in the fast loop: the last
/// snapshot, in memory, and nothing on disk.
pub fn engine(_name: &str) -> Engine {
    Engine::default()
}
