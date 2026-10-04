//! Review remediation regressions through the unchanged public kernel and port.
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use glade_wire::generated::Head;
use std::collections::BTreeMap;
use support::*;

fn verify(t: &Transition) -> Query {
    t.effects
        .iter()
        .find_map(|e| {
            if let Effect::Verify(q) = e {
                Some(q.clone())
            } else {
                None
            }
        })
        .expect("issued verification")
}
fn verified(s: &State, q: Query, c: &Candidate) -> Transition {
    step(
        s,
        Event::EvidenceResolved(Box::new(EvidenceReply {
            query: q,
            verdict: Verdict::Verified(Box::new(facts(c))),
        })),
    )
}
fn prepare(t: &Transition) -> (CommitPlan, PrepareRequest) {
    t.effects
        .iter()
        .find_map(|e| {
            if let Effect::Prepare { plan, request } = e {
                Some((plan.clone(), request.clone()))
            } else {
                None
            }
        })
        .expect("issued immutable prepare")
}
fn staged(s: &State, c: &Candidate, local: bool, f: Facts) -> Transition {
    let offered = step(
        s,
        if local {
            Event::SubmitLocal(Box::new(c.clone()))
        } else {
            Event::OfferReplica(Box::new(c.clone()))
        },
    );
    let t = step(
        &offered.state,
        Event::EvidenceResolved(Box::new(EvidenceReply {
            query: verify(&offered),
            verdict: Verdict::Verified(Box::new(f)),
        })),
    );
    if let Some(q) = t.effects.iter().find_map(|e| {
        if let Effect::Seal(q) = e {
            Some(q.clone())
        } else {
            None
        }
    }) {
        step(
            &t.state,
            Event::EvidenceResolved(Box::new(EvidenceReply {
                query: q,
                verdict: Verdict::Sealed(c.evidence.admission.clone().unwrap()),
            })),
        )
    } else {
        t
    }
}
fn refs(names: &[&str]) -> Vec<Head> {
    names
        .iter()
        .map(|n| Head {
            origin: (*n).into(),
            seq: 0,
            hash: None,
        })
        .collect()
}
fn no_new_work(t: &Transition) {
    assert!(!t.effects.iter().any(|e| matches!(
        e,
        Effect::Verify(_) | Effect::Seal(_) | Effect::Prepare { .. } | Effect::Begin(_)
    )));
}
#[path = "remediation1/exhaustion.rs"]
mod exhaustion;
#[path = "remediation1/frontier.rs"]
mod frontier;
#[path = "remediation1/integrity.rs"]
mod integrity;
#[path = "remediation1/pending.rs"]
mod pending;
#[path = "remediation1/terminal.rs"]
mod terminal;
