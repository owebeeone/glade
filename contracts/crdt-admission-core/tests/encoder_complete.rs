//! Exact private production encoder exercised without exposing a new public API.
#[path = "../src/encoder.rs"]
mod encoder;
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use support::*;
fn base() -> (State, Descriptor, CommitPlan) {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut p = staged_plan(&d, c.clone());
    p.qualified_forks.push(ForkEvidence {
        first: c.clone(),
        second: c,
    });
    (initial(d.clone()), d, p)
}
fn changed(mutator: impl FnOnce(&mut State, &mut Descriptor, &mut CommitPlan)) {
    let (s, d, p) = base();
    let expected = encoder::encode(&s, &d, &p);
    assert_eq!(expected, encoder::encode(&s, &d, &p));
    let (mut s, mut d, mut p) = base();
    mutator(&mut s, &mut d, &mut p);
    assert_ne!(expected, encoder::encode(&s, &d, &p));
}
#[path = "encoder_complete/candidate.rs"]
mod candidate;
#[path = "encoder_complete/descriptor.rs"]
mod descriptor;
#[path = "encoder_complete/facts.rs"]
mod facts;
#[path = "encoder_complete/fork.rs"]
mod fork;
#[path = "encoder_complete/plan.rs"]
mod plan;
#[path = "encoder_complete/seal.rs"]
mod seal;
#[path = "encoder_complete/seal_scope.rs"]
mod seal_scope;
#[path = "encoder_complete/stage.rs"]
mod stage;
#[path = "encoder_complete/verify.rs"]
mod verify;
