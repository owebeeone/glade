//! **This example MUST FAIL to compile: E0275.**
//!
//! DI-E03's second half — "construction cycle rejected before useful startup".
//! Admission needs the quota to decide, the quota needs admission to decide,
//! and neither can be constructed first. Shaku rejects it while compiling the
//! module rather than deadlocking or overflowing the stack at `build()`.
//!
//! The diagnostic is a **caveat, recorded as one** (`AsyncWitnessPlan.md` §8.4,
//! from `DependencyInjectionEvaluation.md:39`): E0275 is a trait-bound
//! evaluation overflow that names the bound it gave up on, not an explanation
//! of the cycle. A reader is told the composition is impossible, not which two
//! services form the loop. DI-E03 asks only that the cycle be rejected before
//! useful startup, and it is.
//!
//! ```sh
//! cargo check --locked --offline --features negative --example construction_cycle
//! ```

use std::sync::Arc;

use shaku::{Component, module};

trait Admission: shaku::Interface {
    fn allowed(&self) -> bool;
}

trait Quota: shaku::Interface {
    fn remaining(&self) -> i64;
}

#[derive(Component)]
#[shaku(interface = Admission)]
struct AdmissionGate {
    #[shaku(inject)]
    quota: Arc<dyn Quota>,
}

impl Admission for AdmissionGate {
    fn allowed(&self) -> bool {
        self.quota.remaining() > 0
    }
}

#[derive(Component)]
#[shaku(interface = Quota)]
struct LeaseQuota {
    #[shaku(inject)]
    admission: Arc<dyn Admission>,
}

impl Quota for LeaseQuota {
    fn remaining(&self) -> i64 {
        if self.admission.allowed() { 1 } else { 0 }
    }
}

module! {
    Cyclic {
        components = [AdmissionGate, LeaseQuota],
        providers = []
    }
}

fn main() {
    let _ = Cyclic::builder().build();
}
