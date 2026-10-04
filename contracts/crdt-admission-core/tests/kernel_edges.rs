//! IC-2 RED-first checks through the unchanged public event/effect boundary.
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use support::*;

fn sealed_plan(mut state: State, mut p: CommitPlan) -> Transition {
    let q = p.acceptances.remove(0);
    let seal = q.seal_query.unwrap();
    state.queries.insert(
        seal.id,
        PendingVerification {
            query: seal.clone(),
            candidate: q.candidate,
            verified: Some(q.facts),
            verified_query: Some(q.verified_query),
        },
    );
    state.next_effect_id = 7;
    step(
        &state,
        Event::EvidenceResolved(Box::new(EvidenceReply {
            query: seal,
            verdict: Verdict::Sealed(q.sealed_admission),
        })),
    )
}
fn emitted(t: &Transition) -> &CommitPlan {
    t.effects
        .iter()
        .find_map(|e| {
            if let Effect::Prepare { plan, .. } = e {
                Some(plan)
            } else {
                None
            }
        })
        .expect("complete immutable plan emission")
}
#[path = "kernel_edges/admission.rs"]
mod admission;
#[path = "kernel_edges/encoder.rs"]
mod encoder;
#[path = "kernel_edges/frontier.rs"]
mod frontier;
#[path = "kernel_edges/storage.rs"]
mod storage;
