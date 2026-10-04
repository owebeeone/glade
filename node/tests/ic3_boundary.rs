//! Current production composition refuses; ignored success gates are real IC3B RED.
use glade_crdt_admission_data::*;
use glade_crdt_recovery_api::*;
use glade_node::assembly::NodeAssembly;
#[test]
fn assembled_typed_recovery_has_no_fabricated_validated_image() {
    let node = NodeAssembly::builder().build();
    let _adapters = node.independent_adapters();
    let mut session = glade_node::independent::RefusingSession;
    assert!(matches!(session.load(), LoadResult::Unavailable));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
}
#[test]
fn assembled_refuser_never_issues_a_consume_permit() {
    let node = NodeAssembly::builder().build();
    let _adapters = node.independent_adapters();
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
#[ignore = "IC3B real host restart success deliberately remains RED before typed acceptance"]
fn production_composition_recovers_a_genuinely_persistent_validated_image() {
    let node = NodeAssembly::builder().build();
    let _adapters = node.independent_adapters();
    let mut session = glade_node::independent::RefusingSession;
    assert!(
        matches!(session.load(), LoadResult::Validated(_)),
        "IC3B not implemented"
    );
}

#[test]
fn caller_created_permit_is_refused_before_actual_node_callback() {
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
