use glade_crdt_recovery_codec::*;
#[path = "../../crdt-admission-core/src/encoder.rs"]
mod production_encoder;
fn descriptor() -> Descriptor {
    Descriptor {
        instance: Instance {
            descriptor: [1; 32],
            incarnation: "i".into(),
        },
        canonical: Bytes::new(vec![1]),
        creation_root: [2; 32],
        resource: "r".into(),
        share: "s".into(),
        glade_id: "g".into(),
        key: vec![9],
        engine: "crdt".into(),
        payload_profile: "text".into(),
        admission_profile: "independent".into(),
        authorization_profile: "bounded".into(),
        mode: AdmissionMode::Independent,
        bounds: Bounds {
            max_operation_bytes: 4096,
            max_origins: 4,
            max_refs: 8,
            edit_bytes: 8192,
            recovery_bytes: 8192,
            evidence_bytes: 8192,
            pending_bytes: 8192,
            pending_items: 16,
        },
    }
}
fn plan(d: &Descriptor) -> CommitPlan {
    CommitPlan {
        id: u64::MAX,
        instance: d.instance.clone(),
        expected_revision: 7,
        local_precondition: Some(LocalPrecondition {
            policy: [3; 32],
            time: TimeInterval {
                earliest_ms: 11,
                latest_ms: 12,
            },
        }),
        batch: Bytes::new(vec![]),
        kind: CommitKind::AcceptedBatch,
        records: vec![],
        acceptances: vec![],
        qualified_forks: vec![],
        charges: Charges {
            edit: 1,
            recovery: 2,
            evidence: 3,
            pending: 4,
        },
    }
}
fn limits() -> DecodeLimits {
    DecodeLimits {
        bytes: 65536,
        items: 4096,
        depth: 32,
        name_bytes: 128,
    }
}
#[test]
fn unchanged_private_encoder_has_a_complete_bounded_decoder() {
    let d = descriptor();
    let state = State {
        plan_namespace: [4; 32],
        invocation_namespace: [5; 32],
        next_effect_id: u64::MAX,
        storage_owner: Some(Owner {
            store: StoreIncarnation([6; 32]),
            generation: 9,
        }),
        ..State::default()
    };
    let mut p = plan(&d);
    let bytes = production_encoder::encode(&state, &d, &p);
    p.batch = bytes.clone();
    let decoded = decode_batch(bytes.value(), &limits()).expect("IC2 batch decoder");
    assert_eq!(decoded.plan, p);
    assert_eq!(decoded.descriptor, d);
    assert_eq!(decoded.owner, state.storage_owner);
    assert_eq!(decoded.next_effect_id, u64::MAX);
    assert_eq!(
        production_encoder::encode(&state, &decoded.descriptor, &decoded.plan),
        bytes
    );
}
fn candidate(d: &Descriptor) -> Candidate {
    Candidate {
        instance: d.instance.clone(),
        operation: Operation::new(glade_wire::generated::Op {
            share: d.share.clone(),
            glade_id: d.glade_id.clone(),
            key: d.key.clone(),
            origin: "o".into(),
            seq: 0,
            prev: None,
            lamport: 1,
            refs: vec![],
            shape: glade_wire::generated::Shape::Crdt,
            payload: vec![1],
        }),
        evidence: EvidenceSet {
            certificate: Bytes::new(vec![7; 1024]),
            permit: Bytes::new(vec![8]),
            policy: Bytes::new(vec![9]),
            admission: None,
        },
    }
}
#[test]
fn interned_blob_references_cannot_amplify_owned_decoding_past_budget() {
    let d = descriptor();
    let state = State::default();
    let mut p = plan(&d);
    p.records = vec![candidate(&d); 64];
    let bytes = production_encoder::encode(&state, &d, &p);
    assert!(bytes.value().len() < 8192);
    let mut l = limits();
    l.bytes = 8192;
    assert_eq!(decode_batch(bytes.value(), &l), Err(Error::Capacity));
}
