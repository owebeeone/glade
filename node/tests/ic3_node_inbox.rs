//! Selected authenticated inbox survives OS termination before native dispatch.
mod ic3_auth_fixture;
mod ic3_disk_fixture;
use glade_crdt_recovery_api::{
    LoadResult, PersistResult, ReplicaRecoveryHost, ReplicaRecoverySession,
};
use glade_crdt_recovery_codec::*;
use glade_node::independent::{
    DiskRootConfig, DiskSession, EvidenceConfig, PolicyClock, SessionRequest, TrustedCut,
};
use ic3_auth_fixture::*;
struct Source(TrustedCut);
impl PolicyClock for Source {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        Ok(self.0.clone())
    }
}
fn fixture() -> Fixture {
    let mut f = ic3_disk_fixture::disk_profile();
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    let key = glade_node::signing::public_key(&[34; 32]);
    policy.admitting_nodes.push(key);
    policy.replica_holders.push(key);
    f.policy = signed(&ROOT, &policy);
    f.query.evidence = Fixture::proofs(
        &f.query.instance,
        &f.identity,
        &f.descriptor,
        &f.certificate,
        &f.policy,
        &f.query.operation,
        None,
    );
    f
}
fn request(f: &Fixture) -> ReplicaOpen {
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    ReplicaOpen {
        instance: f.query.instance.clone(),
        descriptor: f.descriptor.clone(),
        identity: f.identity.clone(),
        owner: Owner {
            store: StoreIncarnation([74; 32]),
            generation: 1,
        },
        plan_namespace: [75; 32],
        invocation_namespace: [76; 32],
        limits: intent.limits,
        expected_floor: [0; 32],
    }
}
fn host(
    f: &Fixture,
    root: &std::path::Path,
    seed: Digest,
    fresh: bool,
    registration: Digest,
) -> glade_node::independent::DiskHost<Source> {
    let request = request(f);
    glade_node::assembly::NodeAssembly::builder()
        .build()
        .independent_configured(
            EvidenceConfig {
                descriptor: f.descriptor.clone(),
                identity: f.identity.clone(),
                trusted_root: f.identity.root,
                node_seed: seed,
                limits: request.limits.decode,
                certificates: vec![f.certificate.clone()],
                session_capacity: request.limits.guard_history,
            },
            Source(TrustedCut {
                policy: f.policy.clone(),
                time: Some(TimeInterval {
                    earliest_ms: 20,
                    latest_ms: 21,
                }),
                time_floor_ms: 10,
            }),
            DiskRootConfig {
                data: root.join("data"),
                trusted_floor: root.join("trusted"),
                registration,
                provision: fresh,
            },
        )
        .unwrap()
        .recovery
}
async fn retain(
    session: &mut DiskSession<Source>,
    f: &Fixture,
    role: &str,
    raw: Bytes,
    expected: Vec<RecordRef>,
) {
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("owned exact image");
    };
    let floor = loaded.authoritative_floor;
    let guard = ReceiveGuard {
        id: GuardId {
            store: floor.owner.store.clone(),
            instance: f.query.instance.clone(),
            number: floor.guard_high_water,
        },
        owner: floor.owner,
        lease: floor.lease,
        source: glade_node::signing::public_key(&NODE),
        channel: [99; 32],
        role: role.into(),
        base_generation: floor.selected.generation,
        base_digest: floor.selected.digest,
        max_bytes: 100000,
        max_items: 128,
        deadline_ms: 1000,
    };
    let permit = session.begin_ingress(guard.clone()).unwrap();
    let bytes = raw.clone();
    let (permit, result) = session
        .receive_ingress(permit, || async move { Ok(bytes) })
        .unwrap()
        .await;
    assert_eq!(result, Ok(raw.clone()));
    let observation = Observation {
        id: guard.id.number + 1,
        instance: f.query.instance.clone(),
        source: guard.source,
        channel: guard.channel,
        guard: guard.id,
        bytes: raw,
        expected,
    };
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.watermark = observation.id;
    for reference in &observation.expected {
        checkpoint.image.obligations.push(Obligation {
            observation: observation.id,
            expected: reference.clone(),
            classification: Classification::Missing,
        });
    }
    checkpoint.image.observations.push(observation.clone());
    assert!(matches!(
        session.settle_ingress(permit, observation, checkpoint),
        PersistResult::Committed { .. }
    ));
}
#[tokio::test]
async fn selected_inbox_preparer_child() {
    let Ok(base) = std::env::var("IC3_SELECTED_INBOX_ROOT") else {
        return;
    };
    let base = std::path::PathBuf::from(base);
    let f = fixture();
    let source = base.join("source");
    let destination = base.join("recipient");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    let mut source_request = request(&f);
    source_request.owner.store = StoreIncarnation([70; 32]);
    source_request.plan_namespace = [71; 32];
    source_request.invocation_namespace = [72; 32];
    let mut sender = host(&f, &source, NODE, true, [73; 32])
        .open_replica(source_request)
        .unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut sender, 92);
    sender
        .submit_local(
            Candidate {
                instance: f.query.instance.clone(),
                operation: f.query.operation.clone(),
                evidence: f.query.evidence.clone(),
            },
            [90; 32],
            [91; 32],
        )
        .unwrap();
    let LoadResult::Validated(loaded) = sender.load() else {
        panic!("genuine original receipt");
    };
    let accepted = &loaded.image.state.instances[&f.query.instance].accepted[0];
    let bundle = TransferBundle {
        instance: f.query.instance.clone(),
        candidate: accepted.candidate.clone(),
        kind: RecordKind::Accepted,
        receipt: Some(accepted.receipt.clone()),
        fork: None,
    };
    let raw = encode_profile(&bundle, &limits()).unwrap();
    let reference = RecordRef {
        kind: RecordKind::Accepted,
        origin: bundle.candidate.operation.inner().origin.clone(),
        seq: 0,
        operation: bundle.candidate.operation.canonical().digest(),
        bundle: raw.digest(),
        receipt: bundle.receipt.clone(),
    };
    let inventory = Inventory {
        instance: f.query.instance.clone(),
        source: glade_node::signing::public_key(&NODE),
        number: 0,
        entries: vec![reference.clone()],
        total_bytes: raw.value().len(),
    };
    let header = signed(
        &NODE,
        &InventoryHeaderBody {
            instance: f.query.instance.clone(),
            identity: f.identity.clone(),
            source: inventory.source,
            target: glade_node::signing::public_key(&[34; 32]),
            channel: [99; 32],
            nonce: [98; 32],
            number: 0,
            pages: 1,
            entries: 1,
            total_bytes: inventory.total_bytes,
            manifest: encode_profile(&inventory, &limits()).unwrap().digest(),
        },
    );
    let mut recipient = host(&f, &destination, [34; 32], true, [77; 32])
        .open_replica(request(&f))
        .unwrap();
    retain(
        &mut recipient,
        &f,
        "inventory",
        encode(&(header, inventory), &limits()).unwrap(),
        vec![reference.clone()],
    )
    .await;
    retain(&mut recipient, &f, "bundle", raw, vec![reference]).await;
    let LoadResult::Validated(loaded) = recipient.load() else {
        panic!("selected inbox");
    };
    assert!(loaded.image.state.instances[&f.query.instance]
        .accepted
        .is_empty());
    assert_eq!(loaded.image.observations.len(), 2);
    std::fs::write(
        base.join("original.bundle"),
        encode_profile(&bundle, &limits()).unwrap().value(),
    )
    .unwrap();
    println!("IC3 inbox selected");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn actual_node_restart_replays_selected_remote_inbox_with_peer_absent() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let base = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("ic3-c-node-inbox-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir(&base).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("IC3_SELECTED_INBOX_ROOT", &base)
        .args(["--exact", "selected_inbox_preparer_child", "--nocapture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    loop {
        if receive
            .recv_timeout(std::time::Duration::from_secs(8))
            .unwrap()
            .starts_with("IC3 inbox selected")
        {
            break;
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    drop(receive);
    reader.join().unwrap();
    let f = fixture();
    let root = base.join("recipient");
    let request = request(&f);
    let profile = root.join("owned.profile");
    let fields = vec![
        Bytes::new(b"glade/ic3/node-qualification/v1".to_vec()),
        encode(&request, &limits()).unwrap(),
        encode(&f.policy, &limits()).unwrap(),
        encode(&vec![f.certificate.clone()], &limits()).unwrap(),
        Bytes::new(vec![34; 32]),
        Bytes::new(vec![45; 32]),
        encode(
            &vec![
                root.join("data").display().to_string(),
                root.join("trusted").display().to_string(),
            ],
            &limits(),
        )
        .unwrap(),
        Bytes::new(vec![77; 32]),
        encode(&false, &limits()).unwrap(),
        encode(
            &(
                Some(TimeInterval {
                    earliest_ms: 20,
                    latest_ms: 21,
                }),
                10_u64,
            ),
            &limits(),
        )
        .unwrap(),
        encode(&Vec::<(Digest, (Digest, String))>::new(), &limits()).unwrap(),
        encode(&(50_u64, 1000_u64), &limits()).unwrap(),
    ];
    std::fs::write(&profile, encode(&fields, &limits()).unwrap().value()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_glade-node"))
        .env_clear()
        .args(["--independent-config", profile.to_str().unwrap(), "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    loop {
        if receive
            .recv_timeout(std::time::Duration::from_secs(8))
            .unwrap()
            .starts_with("listening ")
        {
            break;
        }
    }
    let signal = Command::new("/bin/kill")
        .env_clear()
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    let status = child.wait().unwrap();
    drop(receive);
    reader.join().unwrap();
    assert!(status.success());
    let foreign = ic3_disk_fixture::disk_profile_named("foreign-inbox-declaration");
    assert!(
        host(&foreign, &root, [34; 32], false, [77; 32])
            .open_replica(self::request(&foreign))
            .is_err(),
        "selected inbox cannot be reopened under a different signed declaration"
    );
    let mut restored_request = request.clone();
    restored_request.expected_floor =
        Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut restored = host(&f, &root, [34; 32], false, [77; 32])
        .open_replica(restored_request)
        .unwrap();
    let LoadResult::Validated(loaded) = restored.load() else {
        panic!("replayed actual node");
    };
    let accepted = &loaded.image.state.instances[&f.query.instance].accepted;
    assert_eq!(accepted.len(),1,"actual node startup must dispatch its selected inbox before Listening without the source peer");
    let original: TransferBundle = decode_profile(
        &std::fs::read(base.join("original.bundle")).unwrap(),
        &limits(),
    )
    .unwrap();
    assert_eq!(accepted[0].candidate, original.candidate);
    assert_eq!(Some(accepted[0].receipt.clone()), original.receipt);
    assert!(loaded
        .image
        .obligations
        .iter()
        .all(|item| matches!(item.classification, Classification::Retained(_))));
    assert_eq!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    );
    drop(restored);
    std::fs::remove_dir_all(base).unwrap();
}
