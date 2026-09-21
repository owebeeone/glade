//! **This example MUST FAIL to compile: E0277.**
//!
//! DI-E03's first half — "Missing/ambiguous role … rejected before useful
//! startup". `PeerSession` is the witness's real consumer: it injects
//! `Arc<dyn Clock>` and `Arc<dyn Carrier>`. This module registers the consumer
//! and neither port, so the module type never acquires
//! `HasComponent<dyn Clock>` and the builder cannot be typed at all. The
//! rejection is a trait-bound failure at the composition, not a `None` at run
//! time and not a lookup that panics on the first request.
//!
//! Run it and read the diagnostic, not the exit code:
//!
//! ```sh
//! cargo check --locked --offline --features negative --example missing_binding
//! ```

use async_witness_fast::PeerSession;
use shaku::module;

module! {
    MissingPorts {
        components = [PeerSession],
        providers = []
    }
}

fn main() {
    let _ = MissingPorts::builder().build();
}
