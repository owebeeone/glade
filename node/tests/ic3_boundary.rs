//! Current production composition refuses; ignored success gates are real IC3B RED.
use glade_crdt_admission_data::*;
use glade_crdt_evidence_api::*;
use glade_crdt_recovery_api::*;
use glade_node::assembly::NodeAssembly;
#[test]
fn standalone_refuser_has_no_fabricated_validated_image() {
    let mut session = glade_node::independent::RefusingSession;
    assert!(matches!(session.load(), LoadResult::Unavailable));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
}
#[test]
fn standalone_refuser_never_issues_a_consume_permit() {
    let mut session = glade_node::independent::RefusingSession;
    let guard = ReceiveGuard {
        id: GuardId {
            store: StoreIncarnation([1; 32]),
            instance: Instance {
                descriptor: [2; 32],
                incarnation: "i".into(),
            },
            number: 1,
        },
        owner: Owner {
            store: StoreIncarnation([1; 32]),
            generation: 1,
        },
        lease: 1,
        source: [3; 32],
        channel: [4; 32],
        role: "inventory".into(),
        base_generation: 0,
        base_digest: [0; 32],
        max_bytes: 64,
        max_items: 8,
        deadline_ms: 100,
    };
    assert!(matches!(
        session.begin_ingress(guard),
        Err(Fault::Unavailable)
    ));
}
#[test]
#[ignore = "IC3B assembled open and validated recovery success remain RED; physical restart is deferred"]
fn production_composition_recovers_a_genuinely_persistent_validated_image() {
    let node = NodeAssembly::builder().build();
    let mut adapters = node.independent_adapters();
    let mut session = adapters
        .recovery
        .open_replica(open_request())
        .expect("IC3B assembled open must supply its session");
    assert!(
        matches!(session.load(), LoadResult::Validated(_)),
        "IC3B qualified recovery not implemented"
    );
}

#[test]
fn standalone_refuser_rejects_caller_created_permit_before_callback() {
    let mut issuer = IngressAuthority::new(1);
    let guard = ReceiveGuard {
        id: GuardId {
            store: StoreIncarnation([1; 32]),
            instance: Instance {
                descriptor: [2; 32],
                incarnation: "i".into(),
            },
            number: 1,
        },
        owner: Owner {
            store: StoreIncarnation([1; 32]),
            generation: 1,
        },
        lease: 1,
        source: [3; 32],
        channel: [4; 32],
        role: "inventory".into(),
        base_generation: 0,
        base_digest: [0; 32],
        max_bytes: 64,
        max_items: 8,
        deadline_ms: 100,
    };
    let permit = issuer.issue(guard).unwrap();
    let mut session = glade_node::independent::RefusingSession;
    let result = session.receive_ingress(permit, || -> std::future::Ready<Result<Bytes, Fault>> {
        panic!("caller-created permit cannot start node receive");
    });
    assert!(matches!(result, Err(Fault::Unavailable)));
}

fn open_request() -> ReplicaOpen {
    let instance = Instance {
        descriptor: [2; 32],
        incarnation: "i".into(),
    };
    let descriptor = Descriptor {
        instance: instance.clone(),
        canonical: Bytes::new(vec![4]),
        creation_root: [1; 32],
        resource: "r".into(),
        share: "s".into(),
        glade_id: "g".into(),
        key: vec![1],
        engine: "crdt".into(),
        payload_profile: "text".into(),
        admission_profile: "independent".into(),
        authorization_profile: "ic3".into(),
        mode: AdmissionMode::Independent,
        bounds: Bounds {
            max_operation_bytes: 1024,
            max_origins: 4,
            max_refs: 8,
            edit_bytes: 4096,
            recovery_bytes: 4096,
            evidence_bytes: 4096,
            pending_bytes: 4096,
            pending_items: 8,
        },
    };
    let identity = IdentityBinding {
        root: [1; 32],
        intent: Bytes::new(vec![1]),
        zone: "z".into(),
        parameters: Bytes::new(vec![]),
        declaration: Bytes::new(vec![1]),
        declaration_version: "1".into(),
        schema: Bytes::new(vec![1]),
        schema_identity: "text".into(),
        schema_version: "1".into(),
        corpus_identity: "c".into(),
        corpus_version: "1".into(),
        corpus_digest: [3; 32],
        proof_profile: "ic3".into(),
        recovery_profile: "ic3".into(),
        retention_policy: "retain".into(),
        rejoin_policy: "never-reset".into(),
        initial_chain_base: -1,
        migration_generation: 0,
    };
    ReplicaOpen {
        instance,
        descriptor,
        identity,
        owner: Owner {
            store: StoreIncarnation([1; 32]),
            generation: 1,
        },
        plan_namespace: [4; 32],
        invocation_namespace: [5; 32],
        limits: ReplicaLimits {
            storage: Limits {
                instances: 1,
                attempts: 8,
                invocation_history: 16,
                max_batch_bytes: 4096,
                max_receipts: 4,
                max_windows: 4,
                max_name_bytes: 128,
                critical_bytes: 8192,
                critical_per_attempt: 1024,
            },
            decode: DecodeLimits {
                bytes: 8192,
                items: 512,
                depth: 32,
                name_bytes: 128,
            },
            image_bytes: 8192,
            guard_history: 8,
            inbox_items: 8,
            inbox_bytes: 4096,
            inventory_items: 8,
            inventory_bytes: 4096,
        },
        expected_floor: [6; 32],
    }
}
// The same consumer follows the selected actual host into its session.
fn probe_open<H: ReplicaRecoveryHost>(host: &mut H, downstream: &mut bool) -> Result<(), Fault> {
    let mut session = host.open_replica(open_request())?;
    *downstream = true;
    let _ = session.load();
    let _ = ReplicaRecoverySession::close(&mut session);
    Ok(())
}

struct WrongRecoverySelection;
impl ReplicaRecoveryHost for WrongRecoverySelection {
    type Session = glade_node::independent::RefusingSession;
    fn open_replica(&mut self, _: ReplicaOpen) -> Result<Self::Session, Fault> {
        Ok(glade_node::independent::RefusingSession)
    }
}
#[test]
fn consumer_detects_recovery_selection_mutant_instead_of_discarding_host() {
    let node = NodeAssembly::builder().build();
    let mut selected = node.independent_adapters();
    let mut downstream = false;
    assert_eq!(
        probe_open(&mut selected.recovery, &mut downstream),
        Err(Fault::Unavailable)
    );
    assert!(!downstream);
    let mut mutant = WrongRecoverySelection;
    assert_eq!(probe_open(&mut mutant, &mut downstream), Ok(()));
    assert!(downstream);
}

#[test]
fn actual_assembled_evidence_verify_seal_and_observe_refuse() {
    let node = NodeAssembly::builder().build();
    let mut selected = node.independent_adapters();
    let request = open_request();
    assert_eq!(
        selected
            .evidence
            .observe(&request.instance, &request.identity),
        Err(Fault::Unavailable)
    );
    let query = Query {
        id: 1,
        instance: request.instance,
        descriptor: request.descriptor,
        operation: Operation::new(glade_wire::generated::Op {
            share: "s".into(),
            glade_id: "g".into(),
            key: vec![1],
            origin: "o".into(),
            seq: 1,
            prev: None,
            lamport: 1,
            refs: vec![],
            shape: glade_wire::generated::Shape::Crdt,
            payload: vec![1],
        }),
        evidence: EvidenceSet {
            certificate: Bytes::new(vec![]),
            permit: Bytes::new(vec![]),
            policy: Bytes::new(vec![]),
            admission: None,
        },
        mode: Mode::Historical,
        verified_facts: None,
        validated_under: None,
    };
    let bound = BoundQuery {
        query: query.clone(),
        identity: request.identity,
    };
    let verified = selected.evidence.verify(bound.clone());
    assert_eq!(verified.query, query);
    assert_eq!(verified.verdict, Verdict::Unavailable);
    let sealed = selected.evidence.seal(bound);
    assert_eq!(sealed.query, query);
    assert_eq!(sealed.verdict, Verdict::Unavailable);
}
