//! Real signed provisioning and physical host controls; never MemoryHost/Facts success.
mod ic3_auth_fixture;
use glade_crdt_recovery_api::*;
use glade_crdt_recovery_codec::*;
use glade_node::independent::*;
use ic3_auth_fixture::*;
#[derive(Clone)]
struct Source(TrustedCut);
impl PolicyClock for Source {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        Ok(self.0.clone())
    }
}
fn evidence(f: &Fixture) -> GenuineEvidence<Source> {
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    GenuineEvidence::new(
        EvidenceConfig {
            descriptor: f.descriptor.clone(),
            identity: f.identity.clone(),
            trusted_root: glade_node::signing::public_key(&ROOT),
            node_seed: NODE,
            limits: intent.limits.decode,
            certificates: vec![f.certificate.clone()],
            session_capacity: 16,
        },
        Source(TrustedCut {
            policy: f.policy.clone(),
            time: Some(TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            }),
            time_floor_ms: 10,
        }),
    )
    .unwrap()
}
fn request(f: &Fixture, expected_floor: Digest) -> ReplicaOpen {
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    ReplicaOpen {
        instance: f.descriptor.instance.clone(),
        descriptor: f.descriptor.clone(),
        identity: f.identity.clone(),
        owner: Owner {
            store: StoreIncarnation([70; 32]),
            generation: 1,
        },
        plan_namespace: [71; 32],
        invocation_namespace: [72; 32],
        limits: intent.limits,
        expected_floor,
    }
}
fn root(name: &str) -> std::path::PathBuf {
    let root = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("glade-ic3-b2-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    root
}
fn config(root: &std::path::Path, provision: bool) -> DiskRootConfig {
    DiskRootConfig {
        data: root.join("data"),
        trusted_floor: root.join("trusted"),
        registration: [73; 32],
        provision,
    }
}
#[test]
fn genuine_fresh_replica_opens_owned_validated_genesis_and_reopens_same_identity() {
    let f = Fixture::new();
    let root = root("genesis");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host
        .open_replica(request(&f, [0; 32]))
        .expect("genuine provisioned disk session");
    let LoadResult::Validated(first) = session.load() else {
        panic!("owned complete genesis must validate")
    };
    assert_eq!(first.image.identity, f.identity);
    assert_eq!(first.image.instance, f.descriptor.instance);
    assert_eq!(
        first.image.state.storage_owner,
        Some(request(&f, [0; 32]).owner)
    );
    assert_eq!(
        first.image.state.instances[&f.descriptor.instance].storage,
        StorageClass::LocalProcessRestart
    );
    assert_eq!(
        first.image.state.instances[&f.descriptor.instance].revision,
        0
    );
    assert_ne!(first.authoritative_floor.selected.digest, [0; 32]);
    assert!(first.image.recovery.attempts.is_empty());
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let floor = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host
        .open_replica(request(&f, floor))
        .expect("same registered roots/owner/namespaces");
    let LoadResult::Validated(second) = session.load() else {
        panic!("validated reopen")
    };
    assert_eq!(second.image.instance, first.image.instance);
    assert_eq!(second.image.state, first.image.state);
    assert!(second.authoritative_floor.lease > first.authoritative_floor.lease);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
fn guard(recovery: &ReplicaRecovery) -> ReceiveGuard {
    let floor = &recovery.authoritative_floor;
    ReceiveGuard {
        id: GuardId {
            store: floor.owner.store.clone(),
            instance: recovery.image.instance.clone(),
            number: floor.guard_high_water,
        },
        owner: floor.owner.clone(),
        lease: floor.lease,
        source: glade_node::signing::public_key(&NODE),
        channel: [77; 32],
        role: "local-qualification".into(),
        base_generation: floor.selected.generation,
        base_digest: floor.selected.digest,
        max_bytes: 8192,
        max_items: 16,
        deadline_ms: 1000,
    }
}
#[tokio::test]
async fn actual_owned_begin_precedes_factory_and_active_guard_suppresses_complete_cut() {
    let f = Fixture::new();
    let root = root("guard");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(before) = session.load() else {
        panic!("genesis")
    };
    let incoming = guard(&before);
    let permit = session
        .begin_ingress(incoming.clone())
        .expect("durable pre-consumption guard");
    let after = std::fs::read(root.join("trusted/replica.floor")).unwrap();
    assert_ne!(
        Bytes::new(after).digest(),
        request(&f, [0; 32]).expected_floor
    );
    let LoadResult::Validated(retained) = session.load() else {
        panic!("guard-leading-image is validated but incomplete")
    };
    assert_eq!(
        retained.authoritative_floor.guards.get(&incoming.id),
        Some(&GuardStatus::Active(incoming.clone()))
    );
    assert_eq!(
        retained.image.generation, before.image.generation,
        "begin is a floor-only transition"
    );
    let cut = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
        panic!("combined cut")
    };
    assert!(!cut.complete_local);
    assert_eq!(cut.guards, vec![incoming.clone()]);
    let mut foreign = IngressAuthority::new(1);
    let wrong = foreign.issue(incoming.clone()).unwrap();
    let mut consumed = false;
    let denied = session.receive_ingress(wrong, || {
        consumed = true;
        async { Ok(Bytes::new(vec![1])) }
    });
    assert!(matches!(denied, Err(Fault::Ownership)));
    assert!(!consumed);
    let receive = session
        .receive_ingress(permit, || async { Ok(Bytes::new(vec![1])) })
        .unwrap();
    let (permit, bytes) = receive.await;
    assert_eq!(bytes.unwrap().value(), &[1]);
    assert!(
        matches!(
            session.abandon_ingress(permit),
            PersistResult::Refused(Fault::Ownership)
        ),
        "started receive can never masquerade as untouched abandon"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let cut = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
        panic!("restart combined cut")
    };
    assert!(!cut.complete_local);
    assert_eq!(cut.guards, vec![incoming]);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
fn empty_inventory(f: &Fixture, guard: &ReceiveGuard) -> Bytes {
    let inventory = Inventory {
        instance: f.descriptor.instance.clone(),
        source: guard.source,
        number: 1,
        entries: vec![],
        total_bytes: 0,
    };
    let manifest = encode_profile(&inventory, &limits()).unwrap().digest();
    let header = signed(
        &NODE,
        &InventoryHeaderBody {
            instance: inventory.instance.clone(),
            identity: f.identity.clone(),
            source: inventory.source,
            target: glade_node::signing::public_key(&NODE),
            channel: guard.channel,
            nonce: [78; 32],
            number: inventory.number,
            pages: 0,
            entries: 0,
            total_bytes: 0,
            manifest,
        },
    );
    encode(&(header, inventory), &limits()).unwrap()
}
#[tokio::test]
async fn finite_authenticated_empty_round_settles_exact_observation_and_never_started_abandons() {
    let f = Fixture::new();
    let root = root("settled");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("loaded")
    };
    let incoming = guard(&loaded);
    let bytes = empty_inventory(&f, &incoming);
    let permit = session.begin_ingress(incoming.clone()).unwrap();
    let receive = session
        .receive_ingress(permit, || async { Ok(bytes.clone()) })
        .unwrap();
    let (permit, _) = receive.await;
    let observation = Observation {
        id: 1,
        instance: incoming.id.instance.clone(),
        source: incoming.source,
        channel: incoming.channel,
        guard: incoming.id.clone(),
        bytes: bytes.clone(),
        expected: vec![],
    };
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.observations.push(observation.clone());
    checkpoint.image.watermark = 1;
    assert!(
        matches!(
            session.settle_ingress(permit, observation, checkpoint),
            PersistResult::Committed { .. }
        ),
        "exact drained empty snapshot has finite normal completion"
    );
    let cut = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
        panic!("settled cut")
    };
    assert!(cut.complete_local);
    assert!(cut.guards.is_empty());
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("retained observation")
    };
    assert_eq!(loaded.image.observations.len(), 1);
    let incoming2 = guard(&loaded);
    let permit = session.begin_ingress(incoming2.clone()).unwrap();
    assert!(
        matches!(
            session.abandon_ingress(permit),
            PersistResult::Committed { .. }
        ),
        "issuer-proven never-started guard may retire durably"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let LoadResult::Validated(reopened) = session.load() else {
        panic!("reopened exact empty observation")
    };
    assert_eq!(reopened.image.observations[0].bytes, bytes);
    assert!(matches!(
        reopened.authoritative_floor.guards.get(&incoming2.id),
        Some(GuardStatus::Abandoned { .. })
    ));
    let cut = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
        panic!("reopened cut")
    };
    assert!(cut.complete_local);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn genuine_challenge_response_custody_and_consumption_survive_owned_reopen() {
    let f = Fixture::new();
    let root = root("auth-custody");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let challenge_request = SessionRequest {
        requester: "alice".into(),
        writer_key: glade_node::signing::public_key(&WRITER),
        operation: f.query.operation.canonical().digest(),
        channel: [79; 32],
        session_nonce: [80; 32],
        challenge_nonce: [81; 32],
        window: Window {
            not_before_ms: 10,
            not_after_ms: 100,
        },
    };
    let challenge = session
        .issue_challenge(challenge_request.clone())
        .expect("genuine challenge persists before response");
    let body: LocalChallengeBody = decode_signed_body(&challenge, &limits()).unwrap();
    let response = signed(
        &WRITER,
        &LocalResponseBody {
            instance: body.instance,
            node: body.node,
            requester: body.requester,
            writer_key: body.writer_key,
            signed_challenge: envelope(&challenge).digest(),
            operation: body.operation,
            channel: body.channel,
            session_nonce: body.session_nonce,
            challenge_nonce: body.challenge_nonce,
        },
    );
    session
        .authenticate(response.clone(), [79; 32], [80; 32])
        .expect("genuine possession consumption persists before reply");
    let LoadResult::Validated(before) = session.load() else {
        panic!("signed session custody")
    };
    assert_eq!(before.image.sessions.len(), 1);
    assert!(before.image.sessions[0].consumed);
    assert_eq!(before.image.sessions[0].response, Some(response.clone()));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    assert!(
        matches!(
            session.authenticate(response, [79; 32], [80; 32]),
            Err(Fault::Ownership)
        ),
        "persisted consumption cannot be reset by restart"
    );
    assert!(
        matches!(
            session.issue_challenge(challenge_request),
            Err(Fault::Integrity)
        ),
        "old challenge nonce stays reserved"
    );
    let LoadResult::Validated(after) = session.load() else {
        panic!("exact signed custody after reopen")
    };
    assert_eq!(after.image.sessions, before.image.sessions);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

mod ic3_disk_fixture;

mod ic3_disk_native;
