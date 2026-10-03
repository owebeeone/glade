//! Explicit trusted test inputs and volatile Records consumer. Not crypto/durability.
use glade_crdt_admission_core::*;
use glade_wire::generated::{Head, Op, Shape};
use std::collections::BTreeMap;

pub fn descriptor(name: &str) -> Descriptor {
    let canonical = Bytes::new(name.as_bytes().to_vec());
    Descriptor {
        instance: Instance {
            descriptor: canonical.digest(),
            incarnation: name.into(),
        },
        canonical,
        creation_root: [1; 32],
        resource: name.into(),
        share: name.into(),
        glade_id: "document".into(),
        key: vec![],
        engine: "crdt.oracle/v1".into(),
        payload_profile: "text_crdt.profile/v1".into(),
        admission_profile: "glade.crdt.independent-admission/v1".into(),
        authorization_profile: "bounded-offline-permit/v1".into(),
        mode: AdmissionMode::Independent,
        bounds: Bounds {
            max_operation_bytes: 4096,
            max_origins: 8,
            max_refs: 8,
            edit_bytes: 65536,
            recovery_bytes: 8192,
            evidence_bytes: 8192,
            pending_bytes: 8192,
            pending_items: 16,
        },
    }
}
pub fn initial(d: Descriptor) -> State {
    let instance = d.instance.clone();
    let value = InstanceState {
        descriptor: d,
        revision: 0,
        storage: StorageClass::VolatileTest,
        accepted: vec![],
        candidates: vec![],
        security_evidence: vec![],
        qualified_forks: vec![],
        canonical_origins: BTreeMap::new(),
        heads: BTreeMap::new(),
        pending: vec![],
        quarantine: BTreeMap::new(),
        policy: Bytes::new(b"granted-test-cut".to_vec()),
        time_floor_ms: 10,
        policy_time: TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        },
        used: Charges {
            edit: 0,
            recovery: 0,
            evidence: 0,
            pending: 0,
        },
    };
    State {
        instances: BTreeMap::from([(instance, value)]),
        ..State::default()
    }
}
pub fn insert(atom: &str, after: Option<&str>, text: &str) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"kind":"insert","atom_id":atom,"after":after,"text":text}),
    )
    .unwrap()
}
pub fn candidate(
    d: &Descriptor,
    origin: &str,
    seq: i64,
    prev: Option<Vec<u8>>,
    refs: Vec<Head>,
    payload: Vec<u8>,
) -> Candidate {
    Candidate {
        instance: d.instance.clone(),
        operation: Operation::new(Op {
            share: d.share.clone(),
            glade_id: d.glade_id.clone(),
            key: d.key.clone(),
            origin: origin.into(),
            seq,
            prev,
            lamport: seq + 1,
            refs,
            shape: Shape::Crdt,
            payload,
        }),
        evidence: EvidenceSet {
            certificate: Bytes::new(format!("certificate:{origin}").into_bytes()),
            permit: Bytes::new(b"finite-test-permit".to_vec()),
            policy: Bytes::new(b"granted-test-cut".to_vec()),
            admission: Some(Bytes::new(
                format!("test-admission:{origin}:{seq}").into_bytes(),
            )),
        },
    }
}
pub fn facts(c: &Candidate) -> Facts {
    Facts {
        certificate: OriginCertificate {
            instance: c.instance.clone(),
            canonical_origin: c.operation.inner().origin.clone(),
            epoch: if c.operation.inner().origin == "writer-e1" {
                "epoch-1".into()
            } else {
                "epoch-0".into()
            },
            display_name: "repeated-display-name".into(),
            writer_key: [2; 32],
            requester: "alice".into(),
            canonical: c.evidence.certificate.clone(),
        },
        descriptor_valid: true,
        payload_valid: true,
        signatures_valid: true,
        issuance_unique: true,
        permit_window: Window {
            not_before_ms: 1,
            not_after_ms: 100,
        },
        permit_first_seq: 0,
        permit_last_seq: 100,
        current_allowed: true,
        historical_admitter_authorized: true,
        historical_policy_allowed: true,
        historical_time: Some(TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        }),
    }
}
pub fn accepted(c: Candidate) -> Accepted {
    let receipt = Receipt {
        instance: c.instance.clone(),
        operation: c.operation.canonical().digest(),
        admission: c.evidence.admission.as_ref().unwrap().digest(),
        stable_id: c.operation.canonical().digest(),
        storage: StorageClass::VolatileTest,
    };
    Accepted {
        facts: facts(&c),
        candidate: c,
        receipt,
    }
}
pub fn seed(state: &mut State, c: Candidate) {
    let value = state.instances.get_mut(&c.instance).unwrap();
    let a = accepted(c);
    let op = a.candidate.operation.inner();
    value
        .canonical_origins
        .insert(op.origin.clone(), a.facts.certificate.clone());
    value.heads.insert(
        op.origin.clone(),
        (op.seq, a.candidate.operation.canonical().digest()),
    );
    value.accepted.push(a);
}
/// Compiled downstream host executes the public effects, never private kernel helpers.
/// Test evidence is intentionally trusted; commit replies attest VOLATILE test retention.
pub fn drive(mut state: State, first: Event, supplied: Facts) -> Transition {
    let mut known = BTreeMap::new();
    for instance in state.instances.values() {
        for record in &instance.accepted {
            known.insert(
                (
                    record.candidate.instance.clone(),
                    record.candidate.operation.canonical().digest(),
                ),
                record.facts.clone(),
            );
        }
        for record in &instance.candidates {
            known.insert(
                (
                    record.instance.clone(),
                    record.operation.canonical().digest(),
                ),
                facts(record),
            );
        }
    }
    match &first {
        Event::SubmitLocal(c) | Event::OfferReplica(c) => {
            known.insert(
                (c.instance.clone(), c.operation.canonical().digest()),
                supplied,
            );
        }
        Event::OfferFork(pair) => {
            known
                .entry((
                    pair.first.instance.clone(),
                    pair.first.operation.canonical().digest(),
                ))
                .or_insert_with(|| facts(&pair.first));
            known.insert(
                (
                    pair.second.instance.clone(),
                    pair.second.operation.canonical().digest(),
                ),
                supplied,
            );
        }
        _ => {}
    }
    let mut pending = vec![first];
    let mut observed = Vec::new();
    for _ in 0..32 {
        let Some(event) = pending.pop() else {
            return Transition {
                state,
                effects: observed,
            };
        };
        let result = step(&state, event);
        state = result.state;
        for effect in result.effects {
            match &effect {
                Effect::Verify(query) => {
                    pending.push(Event::EvidenceResolved(Box::new(EvidenceReply {
                        query: query.clone(),
                        verdict: known
                            .get(&(query.instance.clone(), query.operation.canonical().digest()))
                            .map(|f| Verdict::Verified(Box::new(f.clone())))
                            .unwrap_or(Verdict::Unavailable),
                    })))
                }
                Effect::Seal(query) => {
                    pending.push(Event::EvidenceResolved(Box::new(EvidenceReply {
                        query: query.clone(),
                        verdict: Verdict::Sealed(Bytes::new(b"test-sealed-admission".to_vec())),
                    })))
                }
                Effect::Commit(plan) => pending.push(Event::CommitResolved(CommitReply {
                    plan_id: plan.id,
                    instance: plan.instance.clone(),
                    batch_digest: plan.batch.digest(),
                    result: CommitResult::Committed {
                        revision: plan.expected_revision + 1,
                        storage: StorageClass::VolatileTest,
                    },
                })),
                _ => {}
            }
            observed.push(effect);
        }
    }
    panic!("ICD-017 bounded host failed to finish");
}
pub fn cut(state: &State, instance: Instance) -> ReadCut {
    let transition = step(state, Event::Read(instance));
    transition
        .effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Report(Outcome::Read(cut)) => Some(cut),
            _ => None,
        })
        .expect("ICD-011 expected explicit local read cut")
}

pub fn staged_plan(d: &Descriptor, c: Candidate) -> CommitPlan {
    let verified = facts(&c);
    let verify = Query {
        id: 5,
        instance: d.instance.clone(),
        descriptor: d.clone(),
        operation: c.operation.clone(),
        evidence: c.evidence.clone(),
        mode: Mode::Local {
            current_policy: c.evidence.policy.digest(),
            time: TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            },
        },
        verified_facts: None,
        validated_under: None,
    };
    let seal = Query {
        id: 6,
        mode: Mode::AdmissionSeal,
        verified_facts: Some(Box::new(verified.clone())),
        validated_under: Some(verify.mode.clone()),
        ..verify.clone()
    };
    let stored = accepted(c.clone());
    CommitPlan {
        id: 7,
        instance: d.instance.clone(),
        expected_revision: 0,
        local_precondition: Some(LocalPrecondition {
            policy: c.evidence.policy.digest(),
            time: TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            },
        }),
        batch: Bytes::new(b"exact-test-batch".to_vec()),
        kind: CommitKind::AcceptedBatch,
        records: vec![c.clone()],
        acceptances: vec![StagedAcceptance {
            candidate: c,
            facts: verified,
            verified_query: verify,
            seal_query: Some(seal),
            sealed_admission: stored
                .candidate
                .evidence
                .admission
                .as_ref()
                .unwrap()
                .clone(),
            receipt: stored.receipt,
        }],
        charges: Charges {
            edit: 50,
            recovery: 30,
            evidence: 20,
            pending: 0,
        },
    }
}
