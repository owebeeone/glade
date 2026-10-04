//! Default production composition refuses; explicit fresh provisioning qualifies real providers.
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
fn production_composition_recovers_a_genuinely_persistent_validated_image() {
    let node = NodeAssembly::builder().build();
    let (profile, source, roots) = real_config();
    let scratch = roots.data.parent().unwrap().to_owned();
    let mut adapters = node.independent_configured(profile, source, roots).unwrap();
    let mut session = adapters
        .recovery
        .open_replica(real_open())
        .expect("IC3B assembled open must supply its session");
    assert!(
        matches!(session.load(), LoadResult::Validated(_)),
        "IC3B qualified recovery not implemented"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(scratch).unwrap();
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

mod ic3_auth_fixture;
fn real_open() -> ReplicaOpen {
    let fixture = ic3_auth_fixture::Fixture::new();
    let intent: glade_crdt_recovery_codec::CreationIntent =
        glade_crdt_recovery_codec::decode_profile(
            fixture.identity.intent.value(),
            &ic3_auth_fixture::limits(),
        )
        .unwrap();
    ReplicaOpen {
        instance: fixture.descriptor.instance.clone(),
        descriptor: fixture.descriptor,
        identity: fixture.identity,
        owner: Owner {
            store: StoreIncarnation([70; 32]),
            generation: 1,
        },
        plan_namespace: [71; 32],
        invocation_namespace: [72; 32],
        limits: intent.limits,
        expected_floor: [0; 32],
    }
}

struct RealSource(glade_node::independent::TrustedCut);
impl glade_node::independent::PolicyClock for RealSource {
    fn current(&mut self) -> Result<glade_node::independent::TrustedCut, Fault> {
        Ok(self.0.clone())
    }
}
fn real_config() -> (
    glade_node::independent::EvidenceConfig,
    RealSource,
    glade_node::independent::DiskRootConfig,
) {
    let fixture = ic3_auth_fixture::Fixture::new();
    let scratch = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("glade-ic3-b2-assembly-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir(&scratch).unwrap();
    let profile = glade_node::independent::EvidenceConfig {
        descriptor: fixture.descriptor,
        identity: fixture.identity,
        trusted_root: glade_node::signing::public_key(&ic3_auth_fixture::ROOT),
        node_seed: ic3_auth_fixture::NODE,
        limits: real_open().limits.decode,
        certificates: vec![fixture.certificate],
        session_capacity: 16,
    };
    assert_eq!(fixture.query.mode, Mode::Historical);
    (
        profile,
        RealSource(glade_node::independent::TrustedCut {
            policy: fixture.policy,
            time: Some(TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            }),
            time_floor_ms: 10,
        }),
        glade_node::independent::DiskRootConfig {
            data: scratch.join("data"),
            trusted_floor: scratch.join("trusted"),
            registration: [73; 32],
            provision: true,
        },
    )
}
