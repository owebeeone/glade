//! Plan Step 4.4: the journeys over the node's real store. The eight journeys
//! of Step 3.4 and Step 4.4's restart and retry run here unchanged, from
//! `tests/journeys` by `#[path]`, with each test node persisting through
//! records.json (`BlobStore`) in a temp directory of its own instead of the
//! fast loop's in-memory engine; `adapter.rs` holds the tests the contracts
//! README requires of a real persistence adapter. This binary writes files,
//! so it stays out of the fast loop (`--test journeys --test assembly`). The
//! design is `glade/dev-docs/GladeNodeAssembly.md`, "Durable store and
//! restart (plan Step 4.4)".

// Step 3.2's fakes and Step 3.4's, shared with `tests/journeys`: this binary
// uses some of each.
#[allow(dead_code)]
#[path = "../assembly/fakes.rs"]
mod fakes;
#[allow(dead_code)]
#[path = "../journeys/faults.rs"]
mod faults;

// The harness, and the journeys it runs, exactly as the fast loop has them.
#[path = "../journeys/admission.rs"]
mod admission;
#[path = "../journeys/delivery.rs"]
mod delivery;
#[path = "../journeys/leases.rs"]
mod leases;
#[path = "../journeys/node.rs"]
mod node;
#[path = "../journeys/restart.rs"]
mod restart;

// This binary's engine, and the adapter tests on it.
mod adapter;
mod disk;

pub use disk::DiskStore as Engine;
pub use node::{pair, TestNode, T0, WS};

/// The engine a test node persists through here: records.json, in a fresh
/// temp directory named after the node.
pub fn engine(name: &str) -> Engine {
    Engine::fresh(name)
}
