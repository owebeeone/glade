//! **This example MUST FAIL to compile: E0277.**
//!
//! DI-E03's third half — "ambiguous role … rejected before useful startup", and
//! the positive form of it: *selecting a role requires the key*.
//!
//! Two occurrences serve one interface here, the shape
//! `InjectionGraphRefinement.md` gives the carrier when a peer recipe and a
//! client recipe want the same contract. Registering them `#[keyed]` makes the
//! module implement `HasComponentMap<Role, dyn Clock>` and **not**
//! `HasComponent<dyn Clock>`: `shaku_derive`'s module expansion filters
//! multibound components out of the `HasComponent` impls it generates. So the
//! last line — asking for the interface without saying which role — has no
//! trait to call, and the ambiguity is a compile error rather than a silent
//! last-registration-wins.
//!
//! There is no string key and no service locator in reach: `resolve` is generic
//! in the interface type and `resolve_map` is generic in the key type, so a
//! name cannot be smuggled in as data. The keyed half that *does* compile is
//! asserted in `tests/keyed_roles.rs`, including that a missing key is an
//! absent map entry rather than a panic.
//!
//! ```sh
//! cargo check --locked --offline --features negative --example ambiguous_role
//! ```

use std::sync::Arc;

use async_witness_fast::Clock;
use async_witness_ports::ClockPort;
use shaku::{Component, HasComponent, Keyed, module};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Role {
    Peer,
    Client,
}

#[derive(Component)]
#[shaku(interface = Clock)]
struct PeerClock;

impl ClockPort for PeerClock {
    fn now_ms(&self) -> i64 {
        1
    }
}

impl Keyed for PeerClock {
    type KeyType = Role;
    const KEY: Role = Role::Peer;
}

#[derive(Component)]
#[shaku(interface = Clock)]
struct ClientClock;

impl ClockPort for ClientClock {
    fn now_ms(&self) -> i64 {
        2
    }
}

impl Keyed for ClientClock {
    type KeyType = Role;
    const KEY: Role = Role::Client;
}

module! {
    Ambiguous {
        components = [
            #[keyed(dyn Clock, Role)] PeerClock,
            #[keyed(dyn Clock, Role)] ClientClock
        ],
        providers = []
    }
}

fn main() {
    let module = Ambiguous::builder().build();
    // Two call sites, because they fail differently and the plan predicted only
    // the second. Asking the natural way reports **E0599**: with the interface
    // multibound there is no `HasComponent` impl at all, so `resolve` is not a
    // method on this module. Naming the bound explicitly reports the **E0277**
    // the plan expected, which is the same fact said in the other direction.
    let _unqualified: Arc<dyn Clock> = module.resolve();
    let _explicit: Arc<dyn Clock> = <Ambiguous as HasComponent<dyn Clock>>::resolve(&module);
}
