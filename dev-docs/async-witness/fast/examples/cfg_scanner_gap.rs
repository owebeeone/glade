//! **This fixture is expected to COMPILE, and it is deliberately written in a
//! shape the owner's standing rules forbid. Do not copy it into real code.**
//!
//! It is the negative fixture `AsyncWitnessPlan.md` §4.5 asks Step 1.3 to file:
//! the architecture checker "skips items under `#[cfg]`/`#[cfg_attr]`
//! (`architecture-check/src/lib.rs:97-102`, `fn conditional`), so `ARCH-003`'s
//! trait-method checks can be silently weakened by a conditional attribute".
//! The limitation is real, so it is demonstrated rather than described.
//!
//! Every item below is compiled — `main` calls all of them — while the scanner
//! treats each one as absent. `fn conditional` tests only whether an attribute
//! path is `cfg` or `cfg_attr`; it never evaluates the condition, so
//! `#[cfg(all())]`, which is always true, is skipped exactly like
//! `#[cfg(any())]`, which is never true.
//!
//! Two directions, and only one of them is safe:
//!
//! - **Fails closed.** A method the policy *requires* disappears from the
//!   scanner's index, so `ARCH-003 … must expose required methods …
//!   unconditionally` fires. The gate reports a problem that is really a
//!   reporting problem, but it does not pass.
//! - **Silent.** Anything the policy does *not* name is simply unexamined. A
//!   `cfg_attr` carrying an ordinary, non-conditional attribute hides a whole
//!   module — its traits, its public types and its impls — from the scanner
//!   while the compiler compiles all of it. This is the gap, and the same
//!   `!conditional(&m.attrs)` guard sits in front of the checker's own
//!   `#[path]` refusal (`architecture-check/src/lib.rs:211-214`, "`#[path]`
//!   modules need explicit checker support; cannot silently skip them"), so
//!   `#[cfg_attr(…, path = "…")]` defeats the one guard written to stop silent
//!   skipping.
//!
//! **What this does NOT weaken.** The dependency half of the gate is read from
//! `cargo metadata`, not from source, so no `#[cfg]` can reach it: DI-E01's and
//! DI-E04's manifest claims stand. The gap is confined to `ARCH-003`'s trait
//! half and to whatever a conditional module hides from the other source rules.
//!
//! Reproduce both halves:
//!
//! ```sh
//! cargo check --locked --offline --features negative --example cfg_scanner_gap
//! # then, for the scanner half, copy this file as the src/lib.rs of a scratch
//! # package classified `"role": "contract"` with
//! # `"traits": {"PartlyScannedPort": ["seen", "unseen"]}` and run
//! # glade-discover/tools/architecture-check over it. The README records what
//! # it printed.
//! ```

/// A port with one ordinary method and one behind a bare `#[cfg]` — the shape
/// the standing rule forbids because a later edit can silently reassociate the
/// condition with the next declaration. The scanner indexes `seen` and not
/// `unseen`, though both are compiled.
pub trait PartlyScannedPort {
    fn seen(&self) -> i64;

    #[cfg(all())]
    fn unseen(&self) -> i64;
}

pub struct Fixture;

impl PartlyScannedPort for Fixture {
    fn seen(&self) -> i64 {
        1
    }

    #[cfg(all())]
    fn unseen(&self) -> i64 {
        2
    }
}

// `allow(dead_code)` is an ordinary attribute and nothing here is conditional
// compilation at all — but wrapping it in `cfg_attr` is enough to make the
// scanner skip the entire module, because it matches on the attribute's path
// rather than on what the attribute does.
#[cfg_attr(all(), allow(dead_code))]
mod invisible_to_the_scanner {
    pub trait EscapedPort {
        fn escape(&self) -> i64;
    }

    pub struct Escape;

    impl EscapedPort for Escape {
        fn escape(&self) -> i64 {
            3
        }
    }
}

fn main() {
    use invisible_to_the_scanner::{Escape, EscapedPort};

    // Every one of these resolves: the compiler sees what the scanner did not.
    assert_eq!(Fixture.seen(), 1);
    assert_eq!(Fixture.unseen(), 2);
    assert_eq!(Escape.escape(), 3);
    println!("compiled: seen, unseen and an entire module the scanner skipped");
}
