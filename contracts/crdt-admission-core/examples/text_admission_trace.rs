//! Actual consumer of the refusing kernel, emits observed operations only.
#[allow(dead_code)]
#[path = "../tests/support/mod.rs"]
mod support;
use glade_crdt_admission_core::*;
use glade_wire::generated::Head;
use serde_json::{Value, json};
use support::*;

fn projection_trace(state: &State, instance: &Instance) -> Value {
    let result = step(state, Event::Read(instance.clone()));
    let read = result.effects.iter().find_map(|e| match e {
        Effect::Report(Outcome::Read(c)) => Some(c),
        _ => None,
    });
    let eligible = read.map(|c| c.eligible.clone()).unwrap_or_default();
    let ops: Vec<Value> = state.instances[instance].accepted.iter()
        .filter(|a| eligible.contains(&a.candidate.operation.canonical().digest()))
        .map(|a| {
            let op = a.candidate.operation.inner();
            json!({ "origin":op.origin, "seq":op.seq.to_string(),
                "refs":op.refs.iter().map(|r| json!({"origin":r.origin,"seq":r.seq.to_string()})).collect::<Vec<_>>(),
                "payload":op.payload, "digest":a.candidate.operation.canonical().digest() })
        }).collect();
    json!({"read_available":read.is_some(), "operations":ops,
        "security_evidence":state.instances[instance].security_evidence.len(),
        "quarantine":state.instances[instance].quarantine})
}
fn main() {
    let d = descriptor("x");
    let a = candidate(
        &d,
        "writer-e0",
        0,
        None,
        vec![],
        insert("atom-a", None, "A"),
    );
    let mut traces = Vec::new();
    for reverse in [false, true] {
        let mut state = initial(d.clone());
        let b = candidate(
            &d,
            "writer-e1",
            0,
            None,
            vec![Head {
                origin: "writer-e0".into(),
                seq: 0,
                hash: None,
            }],
            insert("atom-b", Some("atom-a"), "B"),
        );
        for op in if reverse {
            vec![b.clone(), a.clone()]
        } else {
            vec![a.clone(), b.clone()]
        } {
            let mut verified = facts(&op);
            if op.operation.inner().origin == "writer-e1" {
                verified.certificate.epoch = "epoch-1".into();
            }
            state = drive(state, Event::OfferReplica(Box::new(op.clone())), verified).state;
        }
        traces.push(json!({"scenario":"AB", "reverse":reverse, "result":projection_trace(&state,&d.instance)}));
        let dependent = candidate(
            &d,
            "dependent",
            0,
            None,
            vec![Head {
                origin: "writer-e0".into(),
                seq: 0,
                hash: None,
            }],
            insert("atom-d", Some("atom-a"), "D"),
        );
        let mut rival = candidate(
            &d,
            "writer-e0",
            0,
            None,
            vec![],
            insert("atom-x", None, "X"),
        );
        rival.evidence.admission = None;
        let mut denied = facts(&rival);
        denied.current_allowed = false;
        denied.historical_policy_allowed = false;
        denied.historical_time = None;
        let mut state = initial(d.clone());
        if reverse {
            state = drive(
                state,
                Event::OfferReplica(Box::new(rival.clone())),
                denied.clone(),
            )
            .state;
        }
        for op in [a.clone(), dependent] {
            state = drive(state, Event::OfferReplica(Box::new(op.clone())), facts(&op)).state;
        }
        if !reverse {
            state = drive(state, Event::OfferReplica(Box::new(rival)), denied).state;
        }
        traces.push(json!({"scenario":"AD", "reverse":reverse, "result":projection_trace(&state,&d.instance)}));
    }
    for reverse in [false, true] {
        let mut state = initial(d.clone());
        seed(&mut state, a.clone());
        let branch = candidate(
            &d,
            "writer-e0",
            1,
            Some(a.operation.canonical().digest().to_vec()),
            vec![],
            insert("atom-b", Some("atom-a"), "B"),
        );
        let rival = candidate(
            &d,
            "writer-e0",
            1,
            Some(a.operation.canonical().digest().to_vec()),
            vec![],
            insert("atom-x", Some("atom-a"), "X"),
        );
        let (first, second) = if reverse {
            (rival, branch)
        } else {
            (branch, rival)
        };
        seed(&mut state, first.clone());
        let dependent = candidate(
            &d,
            "dependent",
            0,
            None,
            vec![Head {
                origin: "writer-e0".into(),
                seq: 1,
                hash: Some(first.operation.canonical().digest().to_vec()),
            }],
            insert(
                "atom-d",
                Some(if reverse { "atom-x" } else { "atom-b" }),
                "D",
            ),
        );
        seed(&mut state, dependent);
        state = drive(
            state,
            Event::OfferFork(Box::new(ForkEvidence {
                first,
                second: second.clone(),
            })),
            facts(&second),
        )
        .state;
        traces.push(json!({"scenario":"A", "reverse":reverse, "result":projection_trace(&state,&d.instance)}));
    }
    let y = descriptor("y");
    let mut isolated = initial(d.clone());
    isolated.instances.extend(initial(y.clone()).instances);
    seed(&mut isolated, a.clone());
    let allowed_y = candidate(
        &y,
        "writer-e0",
        0,
        None,
        vec![],
        insert("atom-y", None, "Y"),
    );
    isolated = drive(
        isolated,
        Event::SubmitLocal(Box::new(allowed_y.clone())),
        facts(&allowed_y),
    )
    .state;
    let forbidden_y = candidate(
        &y,
        "denied-writer",
        0,
        None,
        vec![],
        insert("atom-z", None, "Z"),
    );
    let mut denied = facts(&forbidden_y);
    denied.current_allowed = false;
    let refused = drive(isolated, Event::SubmitLocal(Box::new(forbidden_y)), denied);
    traces.push(json!({"scenario":"isolation", "x_instance":format!("{:?}",d.instance),
        "y_instance":format!("{:?}",y.instance),
        "denied":refused.effects.contains(&Effect::Report(Outcome::Denied)),
        "x":projection_trace(&refused.state,&d.instance),"y":projection_trace(&refused.state,&y.instance)}));
    // Same canonical corpus as the design's concurrent_siblings ABC witness.
    let seed_a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut left = initial(d.clone());
    let mut right = initial(d.clone());
    seed(&mut left, seed_a.clone());
    seed(&mut right, seed_a);
    let b = candidate(
        &d,
        "b",
        0,
        None,
        vec![Head {
            origin: "a".into(),
            seq: 0,
            hash: None,
        }],
        insert("b", Some("a"), "B"),
    );
    let c = candidate(
        &d,
        "c",
        0,
        None,
        vec![Head {
            origin: "a".into(),
            seq: 0,
            hash: None,
        }],
        insert("c", Some("a"), "C"),
    );
    let left_local = drive(left, Event::SubmitLocal(Box::new(b.clone())), facts(&b));
    let right_local = drive(right, Event::SubmitLocal(Box::new(c.clone())), facts(&c));
    let receipts = [
        left_local
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_)))),
        right_local
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_)))),
    ];
    let left = drive(
        left_local.state,
        Event::OfferReplica(Box::new(c.clone())),
        facts(&c),
    );
    let right = drive(
        right_local.state,
        Event::OfferReplica(Box::new(b.clone())),
        facts(&b),
    );
    traces.push(json!({"scenario":"ABC", "local_receipts":receipts,
        "left":projection_trace(&left.state,&d.instance), "right":projection_trace(&right.state,&d.instance)}));
    println!(
        "{}",
        json!({"trust":"explicit test inputs", "storage":"volatile Records model", "traces":traces})
    );
}
