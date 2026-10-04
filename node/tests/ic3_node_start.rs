//! IC3-C1 consumers of the actual Settings -> NodeStart composition boundary.
use glade_node::assembly::Settings;
use glade_node::lifecycle::{Console, NodeStart};
use std::sync::Arc;
struct Silent;
impl Console for Silent {
    fn out(&self, line: &str) {
        panic!("unexpected output: {line}");
    }
    fn err(&self, line: &str) {
        panic!("unexpected error: {line}");
    }
}
#[test]
fn explicit_independent_configuration_is_not_a_legacy_store_positional() {
    let settings = Settings::from_args(
        ["--independent-config", "/tmp/owned-ic3-profile", "0"].map(String::from),
    );
    assert_eq!(settings.positional, vec!["0"]);
    assert_eq!(settings.store_dir(), None);
    assert!(!settings.booted());
}
#[test]
fn independent_relative_configuration_refuses_before_node_start() {
    let settings = Settings::from_args(["--independent-config", "relative"].map(String::from));
    let result = NodeStart::from_settings(settings, Vec::new(), Arc::new(Silent));
    assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::InvalidInput));
}
#[test]
fn absent_independent_configuration_preserves_legacy_positionals() {
    let settings = Settings::from_args(["0", "/tmp/legacy-store"].map(String::from));
    assert_eq!(settings.positional, vec!["0", "/tmp/legacy-store"]);
    assert_eq!(settings.store_dir(), Some("/tmp/legacy-store"));
}
#[test]
fn missing_independent_configuration_value_cannot_fall_back_to_legacy() {
    let settings =
        Settings::from_args(["0", "/tmp/legacy-store", "--independent-config"].map(String::from));
    let result = NodeStart::from_settings(settings, Vec::new(), Arc::new(Silent));
    assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::InvalidInput));
}

#[test]
fn independent_opt_in_refuses_mixed_or_ambiguous_start_forms() {
    for extra in [
        vec!["0", "/tmp/legacy"],
        vec!["junk"],
        vec!["--config", "/tmp/net"],
        vec!["--app", "/tmp/app"],
        vec!["--peer", "peer"],
        vec!["--operator", "op"],
        vec!["--enforce-client-grants"],
        vec!["--recovery-out", "/tmp/key"],
        vec!["--independent-config", "/tmp/other"],
    ] {
        let mut args = vec![
            "--independent-config".to_string(),
            "/tmp/owned-ic3".to_string(),
        ];
        args.extend(extra.iter().map(|word| word.to_string()));
        let result =
            NodeStart::from_settings(Settings::from_args(args), Vec::new(), Arc::new(Silent));
        assert!(
            matches!(result,Err(error) if error.kind()==std::io::ErrorKind::InvalidInput),
            "mixed independent form {extra:?} cannot silently discard provisioning"
        );
    }
}
#[test]
fn actual_usage_names_explicit_inactive_independent_start_form() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_glade-node"))
        .env_clear()
        .output()
        .unwrap();
    assert!(!output.status.success());
    let usage = String::from_utf8(output.stderr).unwrap();
    assert!(
        usage.contains("--independent-config <absolute-profile> [PORT]"),
        "operator usage must name the actual opt-in form: {usage}"
    );
    assert!(
        usage.contains("disposable"),
        "the opt-in is qualification, not activation of existing stores"
    );
}

mod ic3_auth_fixture;
use glade_crdt_recovery_codec::*;
use glade_node::independent::{DiskSession, PolicyClock, SessionRequest};
use ic3_auth_fixture::*;
mod ic3_disk_fixture;
use sdax_tokio::{PlanStart, TokioRuntime};
use std::sync::Mutex;
#[derive(Default)]
struct Lines(Mutex<Vec<String>>);
impl Console for Lines {
    fn out(&self, line: &str) {
        self.0.lock().unwrap().push(line.into());
    }
    fn err(&self, line: &str) {
        self.0.lock().unwrap().push(format!("error {line}"));
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn configured_actual_node_plan_recovers_before_listening_without_legacy_store() {
    let f = Fixture::new();
    let root = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("ic3-c-node-start-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let fields = profile_fields(&f, &root, true);
    let path = root.join("owned.profile");
    std::fs::write(&path, encode(&fields, &limits()).unwrap().value()).unwrap();
    let settings = Settings::from_args(vec![
        "--independent-config".into(),
        path.display().to_string(),
        "0".into(),
    ]);
    let lines = Arc::new(Lines::default());
    let start = NodeStart::from_settings(settings, Vec::new(), lines.clone()).unwrap();
    let runtime = Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()));
    let mut run = glade_node::lifecycle::node_plan().start(runtime, start);
    let ready = tokio::time::timeout(std::time::Duration::from_secs(5), run.ready()).await;
    run.handle().shutdown();
    let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .expect("bounded stop");
    assert_eq!(
        ready.ok(),
        Some(Ok(())),
        "{:?}: {report}",
        lines.0.lock().unwrap()
    );
    assert!(
        lines
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("independent ready ")),
        "recovery must precede listening"
    );
    assert!(
        lines
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("independent peer ")),
        "same node scope must bind the actual peer carrier"
    );
    assert!(root.join("trusted/replica.floor").is_file());
    assert!(!root.join("data/home").exists());
    assert!(report.is_clean(), "{report}");
    assert_eq!(f.query.instance, f.descriptor.instance);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ambiguous_or_misbound_peer_provisioning_refuses_before_data_open() {
    let f = ic3_disk_fixture::disk_profile();
    let remote = glade_node::signing::public_key(&[34; 32]);
    let endpoint = glade_node::signing::public_key(&[45; 32]);
    let peer = (
        remote,
        (endpoint, format!("{}@127.0.0.1:1", hex(&endpoint))),
    );
    for (name, peers) in [
        ("duplicate-node", vec![peer.clone(), peer.clone()]),
        (
            "shared-endpoint",
            vec![peer.clone(), ([99; 32], peer.1.clone())],
        ),
        (
            "misbound-address",
            vec![(
                remote,
                (endpoint, format!("{}@127.0.0.1:1", hex(&[99; 32]))),
            )],
        ),
        (
            "self-node",
            vec![(glade_node::signing::public_key(&NODE), peer.1.clone())],
        ),
    ] {
        let root = std::fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("ic3-c-provision-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("owned.profile");
        let mut fields = profile_fields(&f, &root, true);
        fields[10] = encode(&peers, &limits()).unwrap();
        std::fs::write(&path, encode(&fields, &limits()).unwrap().value()).unwrap();
        let lines = Arc::new(Lines::default());
        let start = NodeStart::from_settings(
            Settings::from_args(vec![
                "--independent-config".into(),
                path.display().to_string(),
                "0".into(),
            ]),
            Vec::new(),
            lines.clone(),
        )
        .unwrap();
        let mut run = glade_node::lifecycle::node_plan().start(
            Arc::new(TokioRuntime::new(tokio::runtime::Handle::current())),
            start,
        );
        let ready = run.ready().await;
        run.handle().shutdown();
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .unwrap();
        assert!(
            ready.is_err(),
            "ambiguous {name} provisioning must be refused: {report}"
        );
        assert!(!root.join("trusted/replica.floor").exists());
        assert!(!lines
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("listening ")));
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn profile_fields(f: &Fixture, root: &std::path::Path, fresh: bool) -> Vec<Bytes> {
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    let request = ReplicaOpen {
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
        expected_floor: [0; 32],
    };
    vec![
        Bytes::new(b"glade/ic3/node-qualification/v1".to_vec()),
        encode(&request, &limits()).unwrap(),
        encode(&f.policy, &limits()).unwrap(),
        encode(&vec![f.certificate.clone()], &limits()).unwrap(),
        Bytes::new(NODE.to_vec()),
        Bytes::new([44; 32].to_vec()),
        encode(
            &vec![
                root.join("data").display().to_string(),
                root.join("trusted").display().to_string(),
            ],
            &limits(),
        )
        .unwrap(),
        Bytes::new([73; 32].to_vec()),
        encode(&fresh, &limits()).unwrap(),
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
    ]
}

#[test]
fn actual_binary_explicit_opt_in_reaches_same_recovery_route_without_environment_switch() {
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Command, Stdio};
    let f = Fixture::new();
    let root = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("ic3-c-binary-start-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let profile = root.join("owned.profile");
    std::fs::write(
        &profile,
        encode(&profile_fields(&f, &root, true), &limits())
            .unwrap()
            .value(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_glade-node"))
        .env_clear()
        .arg("--independent-config")
        .arg(&profile)
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut seen = vec![];
    let mut listening = false;
    while let Ok(line) = receive.recv_timeout(std::time::Duration::from_secs(5)) {
        listening = line.starts_with("listening ");
        seen.push(line);
        if listening {
            break;
        }
    }
    let _ = child.kill();
    child.wait().unwrap();
    let mut errors = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    drop(receive);
    reader.join().unwrap();
    assert!(
        listening,
        "actual binary did not route independent config: {seen:?}; {errors}"
    );
    assert!(seen
        .iter()
        .any(|line| line.starts_with("independent ready ")));
    assert!(root.join("trusted/replica.floor").is_file());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn listening_dispatches_fresh_signed_local_append_to_same_independent_disk_session() {
    let f = ic3_disk_fixture::disk_profile();
    let root = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("ic3-c-local-start-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let path = root.join("owned.profile");
    std::fs::write(
        &path,
        encode(&profile_fields(&f, &root, true), &limits())
            .unwrap()
            .value(),
    )
    .unwrap();
    let lines = Arc::new(Lines::default());
    let start = NodeStart::from_settings(
        Settings::from_args(vec![
            "--independent-config".into(),
            path.display().to_string(),
            "0".into(),
        ]),
        Vec::new(),
        lines.clone(),
    )
    .unwrap();
    let mut run = glade_node::lifecycle::node_plan().start(
        Arc::new(TokioRuntime::new(tokio::runtime::Handle::current())),
        start,
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), run.ready())
            .await
            .unwrap()
            .is_ok()
    );
    let port: u16 = lines
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|line| line.strip_prefix("listening "))
        .unwrap()
        .parse()
        .unwrap();
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(2), local_append(port, &f)).await;
    run.handle().shutdown();
    let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .unwrap();
    if result.as_ref().is_ok_and(|value| value.is_err()) {
        let mut session = reopen(&f, &root);
        use glade_crdt_recovery_api::ReplicaRecoverySession;
        if let glade_crdt_recovery_api::LoadResult::Validated(recovered) = session.load() {
            let state = &recovered.image.state.instances[&f.descriptor.instance];
            println!(
                "local custody counts accepted={} candidates={} security={} pending={:?}",
                state.accepted.len(),
                state.candidates.len(),
                state.security_evidence.len(),
                state.pending
            );
            println!("attempt count {}", recovered.image.recovery.attempts.len());
            println!(
                "pending native effects: queries {:?}, prepares {:?}, commits {:?}",
                recovered.image.state.queries.keys(),
                recovered.image.state.preparing.keys(),
                recovered.image.state.commits.keys()
            );
        }
    }
    let accepted = result
        .expect("Listening did not dispatch independent signed ingress")
        .unwrap_or_else(|_| panic!("{:?}", lines.0.lock().unwrap()));
    assert_eq!(accepted.kind, RecordKind::Accepted);
    let receipt = accepted
        .receipt
        .expect("genuine original application receipt");
    assert_eq!(receipt.operation, f.query.operation.canonical().digest());
    assert_eq!(receipt.storage, StorageClass::LocalProcessRestart);
    assert!(accepted.candidate.evidence.admission.is_some());
    let mut restored = reopen(&f, &root);
    use glade_crdt_recovery_api::ReplicaRecoverySession;
    let request = restored.reconstruction_request().unwrap();
    let glade_crdt_recovery_api::ReconstructResult::Cut(cut) = restored.reconstruct_cut(request)
    else {
        panic!("retained local cut unavailable");
    };
    assert!(cut.complete_local,"guarded original local intent needs native admission/custody proof, not an unresolved obligation");
    assert_eq!(cut.kernel.eligible, vec![receipt.operation]);
    assert!(matches!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    ));

    assert!(report.is_clean(), "{report}");
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test(flavor = "multi_thread")]
async fn signed_local_capacity_refusal_retains_real_outcome_without_receipt() {
    let f = Fixture::new();
    let root = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("ic3-c-local-capacity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let path = root.join("owned.profile");
    std::fs::write(
        &path,
        encode(&profile_fields(&f, &root, true), &limits())
            .unwrap()
            .value(),
    )
    .unwrap();
    let lines = Arc::new(Lines::default());
    let start = NodeStart::from_settings(
        Settings::from_args(vec![
            "--independent-config".into(),
            path.display().to_string(),
            "0".into(),
        ]),
        Vec::new(),
        lines.clone(),
    )
    .unwrap();
    let mut run = glade_node::lifecycle::node_plan().start(
        Arc::new(TokioRuntime::new(tokio::runtime::Handle::current())),
        start,
    );
    assert!(run.ready().await.is_ok());
    let port: u16 = lines
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|line| line.strip_prefix("listening "))
        .unwrap()
        .parse()
        .unwrap();
    assert!(local_append(port, &f).await.is_err());
    run.handle().shutdown();
    assert!(tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .unwrap()
        .is_clean());
    assert!(lines
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.contains("local outcomes:") && line.contains("Capacity")));
    let mut restored = reopen(&f, &root);
    use glade_crdt_recovery_api::ReplicaRecoverySession;
    let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
        panic!("capacity custody unavailable");
    };
    assert!(value.image.state.instances[&f.descriptor.instance]
        .accepted
        .is_empty());
    assert!(value.image.recovery.attempts.is_empty());
    assert_eq!(value.image.obligations.len(), 1);
    assert_eq!(
        value.image.obligations[0].classification,
        Classification::Missing
    );
    assert!(matches!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_listener_refuses_foreign_revoked_expired_and_unqualified_node_authority() {
    for mode in [
        "foreign-writer",
        "revoked-writer",
        "expired-permit",
        "no-holder",
        "no-admitter",
        "wrong-node",
        "wrong-domain",
        "bad-signature",
        "wrong-role",
    ] {
        let mut f = ic3_disk_fixture::disk_profile_named(mode);
        let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
        match mode {
            "revoked-writer" => {
                policy
                    .revoked
                    .push(glade_node::signing::public_key(&WRITER));
            }
            "expired-permit" => {}
            "no-holder" => {
                policy.replica_holders.clear();
            }
            "no-admitter" => {
                policy.admitting_nodes.clear();
            }
            _ => {}
        }
        f.policy = signed(&ROOT, &policy);
        if mode == "expired-permit" {
            let wrapper: PermitWrapper =
                decode_profile(f.query.evidence.permit.value(), &limits()).unwrap();
            let mut permit: PermitBody =
                decode_signed_body(&wrapper.signed_permit, &limits()).unwrap();
            permit.window.not_after_ms = 19;
            let signed_permit = signed(&ROOT, &permit);
            let mut proof: OperationProofBody =
                decode_signed_body(&wrapper.writer_proof, &limits()).unwrap();
            proof.signed_permit = envelope(&signed_permit).digest();
            f.query.evidence.permit = encode_profile(
                &PermitWrapper {
                    signed_permit,
                    writer_proof: signed(&WRITER, &proof),
                },
                &limits(),
            )
            .unwrap();
        }
        let root = std::fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("ic3-c-authority-{mode}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("owned.profile");
        std::fs::write(
            &path,
            encode(&profile_fields(&f, &root, true), &limits())
                .unwrap()
                .value(),
        )
        .unwrap();
        let lines = Arc::new(Lines::default());
        let start = NodeStart::from_settings(
            Settings::from_args(vec![
                "--independent-config".into(),
                path.display().to_string(),
                "0".into(),
            ]),
            Vec::new(),
            lines.clone(),
        )
        .unwrap();
        let mut run = glade_node::lifecycle::node_plan().start(
            Arc::new(TokioRuntime::new(tokio::runtime::Handle::current())),
            start,
        );
        let ready = run.ready().await;
        if mode == "no-admitter" {
            assert!(
                ready.is_err(),
                "a node absent from current admitting policy refuses before Listening"
            );
            assert!(!lines
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.starts_with("listening ")));
            run.handle().shutdown();
            let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
                .await
                .unwrap();
            assert!(!report.is_clean());
            std::fs::remove_dir_all(root).unwrap();
            continue;
        }
        assert!(ready.is_ok(), "{mode}: {:?}", lines.0.lock().unwrap());
        let port: u16 = lines
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|line| line.strip_prefix("listening "))
            .unwrap()
            .parse()
            .unwrap();
        let writer = if mode == "foreign-writer" {
            [99; 32]
        } else {
            WRITER
        };
        let node = if mode == "wrong-node" {
            [99; 32]
        } else {
            glade_node::signing::public_key(&NODE)
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if matches!(mode, "wrong-domain" | "bad-signature" | "wrong-role") {
                let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
                let body = LocalChallengeBody {
                    instance: f.descriptor.instance.clone(),
                    identity: f.identity.clone(),
                    resource: f.descriptor.resource.clone(),
                    node,
                    requester: "alice".into(),
                    writer_key: glade_node::signing::public_key(&WRITER),
                    operation: f.query.operation.canonical().digest(),
                    role: if mode == "wrong-role" {
                        "dialer".into()
                    } else {
                        "local-append-request".into()
                    },
                    channel: [81; 32],
                    session_nonce: [82; 32],
                    challenge_nonce: [83; 32],
                    window: Window {
                        not_before_ms: 0,
                        not_after_ms: 1000,
                    },
                };
                let mut record = signed(&WRITER, &body);
                if mode == "wrong-domain" {
                    record.purpose = PERMIT_DOMAIN.into();
                }
                if mode == "bad-signature" {
                    record.signature = Bytes::new(vec![0; 64]);
                }
                let raw = envelope(&record);
                stream.write_u32_le(raw.value().len() as u32).await?;
                stream.write_all(raw.value()).await?;
                return stream.read_u32_le().await;
            }
            let (mut stream, bundle) = local_connection_for(port, &f, &writer, node).await?;
            let raw = encode_profile(&bundle, &limits()).unwrap();
            stream.write_u32_le(raw.value().len() as u32).await?;
            stream.write_all(raw.value()).await?;
            stream.read_u32_le().await
        })
        .await
        .expect("bounded authority refusal");
        assert!(
            result.is_err(),
            "{mode} cannot produce an application receipt"
        );
        run.handle().shutdown();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .unwrap()
            .is_clean());
        let mut restored = reopen(&f, &root);
        use glade_crdt_recovery_api::ReplicaRecoverySession;
        let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
            panic!("denied authority custody {mode}");
        };
        assert!(value.image.state.instances[&f.descriptor.instance]
            .accepted
            .is_empty());
        assert!(value.image.recovery.attempts.is_empty());
        assert!(matches!(
            ReplicaRecoverySession::close(&mut restored),
            CloseResult::Closed
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn actual_nodes_automatically_exchange_committed_history_without_client_ferry() {
    automatic_exchange_case(true).await;
}
#[tokio::test(flavor = "multi_thread")]
async fn tls_configured_peer_without_current_hold_permission_receives_no_history() {
    automatic_exchange_case(false).await;
}
async fn automatic_exchange_case(authorized: bool) {
    let mut f = ic3_disk_fixture::disk_profile();
    let second_seed = [34; 32];
    let second_key = glade_node::signing::public_key(&second_seed);
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    if authorized {
        policy.replica_holders.push(second_key);
    }
    policy.admitting_nodes.push(second_key);
    f.policy = signed(&ROOT, &policy);
    f.query.evidence = Fixture::proofs(
        &f.descriptor.instance,
        &f.identity,
        &f.descriptor,
        &f.certificate,
        &f.policy,
        &f.query.operation,
        Some((glade_node::signing::public_key(&NODE), &NODE)),
    );
    let base = std::fs::canonicalize("/tmp").unwrap().join(format!(
        "ic3-c-auto-{}-{}",
        authorized,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let a = base.join("a");
    let b = base.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ports = [free_port(), free_port()];
    let mut profiles = [profile_fields(&f, &a, true), profile_fields(&f, &b, true)];
    profiles[1][4] = Bytes::new(second_seed.to_vec());
    profiles[1][5] = Bytes::new([45; 32].to_vec());
    profiles[1][1] = encode(
        &{
            let mut request: ReplicaOpen = decode(profiles[1][1].value(), &limits()).unwrap();
            request.owner.store = StoreIncarnation([74; 32]);
            request.plan_namespace = [75; 32];
            request.invocation_namespace = [76; 32];
            request
        },
        &limits(),
    )
    .unwrap();
    profiles[1][7] = Bytes::new([77; 32].to_vec());
    // The refused case keeps the shared profile's 1000 ms: it settles no idle
    // cut and shuts down with rounds in flight, whose drain the deadline also
    // bounds, under the 5 s shutdown bound below.
    if authorized {
        for profile in &mut profiles {
            profile[11] = encode(&(50_u64, EXCHANGE_DEADLINE_MS), &limits()).unwrap();
        }
    }
    let keys = [glade_node::signing::public_key(&NODE), second_key];
    let endpoints = [
        glade_node::signing::public_key(&[44; 32]),
        glade_node::signing::public_key(&[45; 32]),
    ];
    for (index, profile) in profiles.iter_mut().enumerate() {
        let other = 1 - index;
        profile[10] = encode(
            &vec![(
                keys[other],
                (
                    endpoints[other],
                    format!("{}@127.0.0.1:{}", hex(&endpoints[other]), ports[other]),
                ),
            )],
            &limits(),
        )
        .unwrap();
    }
    let lines = [Arc::new(Lines::default()), Arc::new(Lines::default())];
    let paths = [a.join("owned.profile"), b.join("owned.profile")];
    let mut runs = Vec::new();
    for index in 0..2 {
        std::fs::write(
            &paths[index],
            encode(&profiles[index], &limits()).unwrap().value(),
        )
        .unwrap();
        let start = NodeStart::from_settings(
            Settings::from_args(vec![
                "--independent-config".into(),
                paths[index].display().to_string(),
                ports[index].to_string(),
            ]),
            Vec::new(),
            lines[index].clone(),
        )
        .unwrap();
        let mut run = glade_node::lifecycle::node_plan().start(
            Arc::new(TokioRuntime::new(tokio::runtime::Handle::current())),
            start,
        );
        assert!(run.ready().await.is_ok());
        runs.push(run);
    }
    let original = local_append(ports[0], &f).await.unwrap();
    let exchanged = if authorized {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if lines[1]
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.starts_with("independent exchanged "))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    } else {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        !lines[1]
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("independent exchanged "))
    };
    let mut later = None;
    if authorized {
        let previous = wait_idle(
            || {
                lines
                    .iter()
                    .map(|owned| owned.0.lock().unwrap().clone())
                    .collect()
            },
            None,
            EXCHANGE_SETTLE,
        )
        .await;
        let higher = usize::from(keys[1] > keys[0]);
        let mut next = Fixture {
            descriptor: f.descriptor.clone(),
            identity: f.identity.clone(),
            certificate: f.certificate.clone(),
            policy: f.policy.clone(),
            query: f.query.clone(),
        };
        let mut op = next.query.operation.inner().clone();
        op.seq = 1;
        op.lamport = 1;
        op.prev = Some(f.query.operation.canonical().digest().to_vec());
        op.payload = br#"{"after":"a","atom_id":"b","kind":"insert","text":"B"}"#.to_vec();
        next.query.operation = Operation::new(op);
        next.query.evidence = proof(&next, &WRITER, &keys);
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, bundle) =
            local_connection_for(ports[higher], &next, &WRITER, keys[higher])
                .await
                .unwrap();
        let raw = encode_profile(&bundle, &limits()).unwrap();
        stream.write_u32_le(raw.value().len() as u32).await.unwrap();
        stream.write_all(raw.value()).await.unwrap();
        let len = stream.read_u32_le().await.unwrap() as usize;
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes).await.unwrap();
        later = Some(decode_profile::<TransferBundle>(&bytes, &limits()).unwrap());
        wait_idle(
            || {
                lines
                    .iter()
                    .map(|owned| owned.0.lock().unwrap().clone())
                    .collect()
            },
            Some(&previous),
            EXCHANGE_SETTLE,
        )
        .await;
    }
    for run in &runs {
        run.handle().shutdown();
    }
    for run in runs {
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .unwrap();
        assert!(report.is_clean(), "{report}");
    }
    assert!(
        exchanged,
        "actual peer runtime did not automatically transfer history: {:?} {:?}",
        lines[0].0.lock().unwrap(),
        lines[1].0.lock().unwrap()
    );
    let mut restored_a = reopen(&f, &a);
    let cut_request = restored_a.reconstruction_request().unwrap();
    let glade_crdt_recovery_api::ReconstructResult::Cut(cut_a) =
        restored_a.reconstruct_cut(cut_request)
    else {
        panic!("first-node cut unavailable");
    };
    assert!(
        cut_a.complete_local,
        "normal duplex choreography must not manufacture permanent receive loss"
    );
    assert!(matches!(
        ReplicaRecoverySession::close(&mut restored_a),
        CloseResult::Closed
    ));
    // Reopen the second node with its exact independent ownership/provisioning.
    let mut request: ReplicaOpen = decode(profiles[1][1].value(), &limits()).unwrap();
    request.expected_floor =
        Bytes::new(std::fs::read(b.join("trusted/replica.floor")).unwrap()).digest();
    use glade_crdt_recovery_api::{ReplicaRecoveryHost, ReplicaRecoverySession};
    use glade_node::independent::*;
    let mut host = glade_node::assembly::NodeAssembly::builder()
        .build()
        .independent_configured(
            EvidenceConfig {
                descriptor: f.descriptor.clone(),
                identity: f.identity.clone(),
                trusted_root: f.identity.root,
                node_seed: second_seed,
                limits: request.limits.decode.clone(),
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
                data: b.join("data"),
                trusted_floor: b.join("trusted"),
                registration: [77; 32],
                provision: false,
            },
        )
        .unwrap()
        .recovery;
    let mut restored = host.open_replica(request).unwrap();
    let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
        panic!("exchanged state unavailable");
    };
    let stored = &value.image.state.instances[&f.descriptor.instance].accepted;
    if authorized {
        assert_eq!(stored.len(), 2);
        assert!(stored
            .iter()
            .any(|item| item.candidate == original.candidate
                && Some(item.receipt.clone()) == original.receipt));
        let later = later.as_ref().unwrap();
        assert!(
            stored.iter().any(|item| item.candidate == later.candidate
                && Some(item.receipt.clone()) == later.receipt),
            "post-success acceptor-only edit must reach unchanged peer"
        );
    } else {
        assert!(stored.is_empty());
        assert!(value.image.observations.is_empty());
        assert!(value.authoritative_floor.guards.is_empty());
        assert!(value.image.state.instances[&f.descriptor.instance]
            .candidates
            .is_empty());
        assert!(value.image.state.instances[&f.descriptor.instance]
            .security_evidence
            .is_empty());
    }
    let request = restored.reconstruction_request().unwrap();
    let glade_crdt_recovery_api::ReconstructResult::Cut(cut) = restored.reconstruct_cut(request)
    else {
        panic!("exchanged cut unavailable");
    };
    assert!(cut.complete_local);
    let mut eligible = cut.kernel.eligible.clone();
    eligible.sort();
    let mut expected = if authorized {
        vec![
            f.query.operation.canonical().digest(),
            later
                .as_ref()
                .unwrap()
                .candidate
                .operation
                .canonical()
                .digest(),
        ]
    } else {
        vec![]
    };
    expected.sort();
    assert_eq!(eligible, expected);
    assert!(matches!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    ));
    std::fs::remove_dir_all(base).unwrap();
}
fn proof(f: &Fixture, writer: &Digest, nodes: &[Digest]) -> EvidenceSet {
    let certificate = envelope(&f.certificate);
    let policy = envelope(&f.policy);
    let permit = signed(
        &ROOT,
        &PermitBody {
            instance: f.descriptor.instance.clone(),
            identity: f.identity.clone(),
            resource: f.descriptor.resource.clone(),
            certificate: certificate.digest(),
            requester: "alice".into(),
            writer_key: glade_node::signing::public_key(writer),
            canonical_origin: f.query.operation.inner().origin.clone(),
            action: ResourceAction::Append,
            admitting_nodes: nodes.to_vec(),
            policy: policy.digest(),
            policy_generation: 0,
            window: Window {
                not_before_ms: 10,
                not_after_ms: 100,
            },
            first_seq: 0,
            last_seq: 8,
        },
    );
    let operation = signed(
        writer,
        &OperationProofBody {
            instance: f.descriptor.instance.clone(),
            identity: f.identity.clone(),
            resource: f.descriptor.resource.clone(),
            operation: f.query.operation.canonical().clone(),
            certificate: certificate.digest(),
            signed_permit: envelope(&permit).digest(),
            requester: "alice".into(),
        },
    );
    EvidenceSet {
        certificate,
        policy,
        permit: encode_profile(
            &PermitWrapper {
                signed_permit: permit,
                writer_proof: operation,
            },
            &limits(),
        )
        .unwrap(),
        admission: None,
    }
}
/// The automatic-exchange witness's scheduling deadline, as the other
/// two-node witnesses use: every dial, send and guarded receive is bounded by
/// it, and a started receive that times out is permanent loss by design,
/// after which no complete idle cut can settle; under load one step can take
/// over a second. It also bounds the busy backoff: when both nodes dial at
/// once both answer busy and the larger key waits one deadline (`serve`).
const EXCHANGE_DEADLINE_MS: u64 = 5000;
/// Settling covers the rounds themselves (8 s) plus one such backoff.
const EXCHANGE_SETTLE: std::time::Duration =
    std::time::Duration::from_millis(8000 + EXCHANGE_DEADLINE_MS);
async fn wait_idle(
    read: impl Fn() -> Vec<Vec<String>>,
    previous: Option<&str>,
    settle: std::time::Duration,
) -> String {
    tokio::time::timeout(settle, async {
        loop {
            let lines = read();
            let latest: Vec<_> = lines
                .iter()
                .map(|owned| {
                    owned
                        .iter()
                        .rev()
                        .find_map(|line| line.strip_prefix("independent idle "))
                        .map(str::to_owned)
                })
                .collect();
            if latest.len() == 2
                && latest[0]
                    .as_ref()
                    .is_some_and(|line| line.ends_with(" complete=true"))
                && latest[0] == latest[1]
                && latest[0].as_deref() != previous
            {
                return latest[0].clone().unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("matching actual full-cut settled idle snapshots before shutdown")
}
/// The first witness port; every port handed out lies below 32768, under
/// every default OS ephemeral range (Linux 32768, macOS 49152).
const FIRST_PORT: u16 = 20000;
/// Ports in each test process's own block; 32 blocks end below 32768.
const BLOCK_PORTS: u16 = 384;
const BLOCKS: u32 = 32;
const _: () = assert!(FIRST_PORT as u32 + BLOCKS * BLOCK_PORTS as u32 <= 32768);
/// The next offset into this test process's block.
static NEXT_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
/// A loopback port for a node's listener and carrier (TCP and UDP), free when
/// handed out. A port released by `bind(0)` was unsafe: the node binds it
/// some time later, and meanwhile any socket's own `bind(0)`, a parallel
/// witness's carrier for one, could take it ("Address already in use"). No
/// `bind(0)` hands out these: they are below the ephemeral ranges, drawn from
/// this process's own block (by process id) and never repeated in it. The
/// library witnesses' `actual_binary::port` allocates the same way.
fn free_port() -> u16 {
    let block = FIRST_PORT + u16::try_from(std::process::id() % BLOCKS).unwrap() * BLOCK_PORTS;
    loop {
        let offset = NEXT_PORT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(
            offset < BLOCK_PORTS,
            "this test process used all {BLOCK_PORTS} witness ports of its block"
        );
        let port = block + offset;
        let at = (std::net::Ipv4Addr::LOCALHOST, port);
        if std::net::TcpListener::bind(at).is_ok() && std::net::UdpSocket::bind(at).is_ok() {
            return port;
        }
    }
}
/// A witness's port is one no `bind(0)` hands out, in any process: below
/// every default OS ephemeral range (Linux 32768, macOS 49152), and never the
/// same twice in this process.
#[test]
fn witness_ports_are_below_the_ephemeral_ranges_and_distinct() {
    let ports: Vec<u16> = (0..8).map(|_| free_port()).collect();
    for port in &ports {
        assert!(
            *port < 32768,
            "{port} is in an OS ephemeral range: {ports:?}"
        );
    }
    let mut distinct = ports.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), ports.len(), "{ports:?}");
}
fn hex(key: &Digest) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}
#[tokio::test(flavor = "multi_thread")]
async fn guarded_local_receive_is_drained_on_cooperative_stop_and_retains_honest_loss() {
    local_receive_refusal("drain", None).await;
}
#[tokio::test(flavor = "multi_thread")]
async fn oversized_local_header_refuses_before_body_and_retains_guard_loss() {
    local_receive_refusal("oversized", Some(100_001)).await;
}
async fn local_receive_refusal(name: &str, size: Option<u32>) {
    use tokio::io::AsyncWriteExt;
    let f = ic3_disk_fixture::disk_profile();
    let root = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("ic3-c-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let path = root.join("owned.profile");
    std::fs::write(
        &path,
        encode(&profile_fields(&f, &root, true), &limits())
            .unwrap()
            .value(),
    )
    .unwrap();
    let lines = Arc::new(Lines::default());
    let start = NodeStart::from_settings(
        Settings::from_args(vec![
            "--independent-config".into(),
            path.display().to_string(),
            "0".into(),
        ]),
        Vec::new(),
        lines.clone(),
    )
    .unwrap();
    let mut run = glade_node::lifecycle::node_plan().start(
        Arc::new(TokioRuntime::new(tokio::runtime::Handle::current())),
        start,
    );
    assert!(run.ready().await.is_ok());
    let port: u16 = lines
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|line| line.strip_prefix("listening "))
        .unwrap()
        .parse()
        .unwrap();
    let (mut stream, _) = local_connection(port, &f).await.unwrap();
    if let Some(size) = size {
        stream.write_u32_le(size).await.unwrap();
    }
    // The server must durably announce readiness at the actual owned receive.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if lines
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.starts_with("independent local receive "))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("no exact durable receive boundary");
    if size.is_some() {
        use tokio::io::AsyncReadExt;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(500), stream.read_u8())
                .await
                .is_ok(),
            "over-limit header must refuse before waiting for its body"
        );
    }
    run.handle().shutdown();
    let report = tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .unwrap();
    assert!(
        report.is_clean(),
        "actual native receive task was not drained: {report}"
    );
    let mut restored = reopen(&f, &root);
    use glade_crdt_recovery_api::ReplicaRecoverySession;
    let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
        panic!("retained drained loss unavailable");
    };
    assert!(value.image.state.instances[&f.descriptor.instance]
        .accepted
        .is_empty());
    assert_eq!(value.image.permanent_loss.len(), 1);
    assert!(value
        .authoritative_floor
        .guards
        .values()
        .all(|status| !matches!(status, GuardStatus::Active(_))));
    let request = restored.reconstruction_request().unwrap();
    let glade_crdt_recovery_api::ReconstructResult::Cut(cut) = restored.reconstruct_cut(request)
    else {
        panic!("loss cut unavailable");
    };
    assert!(!cut.complete_local);
    assert!(cut.permanent_loss);
    assert!(matches!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    ));
    std::fs::remove_dir_all(root).unwrap();
}
async fn local_connection(
    port: u16,
    f: &Fixture,
) -> Result<(tokio::net::TcpStream, TransferBundle), std::io::Error> {
    local_connection_for(port, f, &WRITER, glade_node::signing::public_key(&NODE)).await
}
async fn local_connection_for(
    port: u16,
    f: &Fixture,
    writer: &Digest,
    node: Digest,
) -> Result<(tokio::net::TcpStream, TransferBundle), std::io::Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let request = LocalChallengeBody {
        instance: f.descriptor.instance.clone(),
        identity: f.identity.clone(),
        resource: f.descriptor.resource.clone(),
        node,
        requester: "alice".into(),
        writer_key: glade_node::signing::public_key(writer),
        operation: f.query.operation.canonical().digest(),
        role: "local-append-request".into(),
        channel: [81; 32],
        session_nonce: [82; 32],
        challenge_nonce: [83; 32],
        window: Window {
            not_before_ms: 0,
            not_after_ms: 1000,
        },
    };
    let raw = envelope(&signed(writer, &request));
    stream.write_u32_le(raw.value().len() as u32).await?;
    stream.write_all(raw.value()).await?;
    let length = stream.read_u32_le().await? as usize;
    assert!(length <= limits().bytes);
    let mut raw = vec![0; length];
    stream.read_exact(&mut raw).await?;
    let challenge: SignedRecord = decode_profile(&raw, &limits()).unwrap();
    let body: LocalChallengeBody = decode_signed_body(&challenge, &limits()).unwrap();
    assert_eq!(body.operation, request.operation);
    assert_eq!(
        glade_node::signing::verify_in(
            &body.node,
            CHALLENGE_DOMAIN.as_bytes(),
            challenge.body.value(),
            challenge.signature.value()
        ),
        glade_signer_api::SignatureStatus::Valid
    );
    let response = LocalResponseBody {
        instance: body.instance.clone(),
        node: body.node,
        requester: body.requester,
        writer_key: body.writer_key,
        signed_challenge: envelope(&challenge).digest(),
        operation: body.operation,
        channel: body.channel,
        session_nonce: body.session_nonce,
        challenge_nonce: body.challenge_nonce,
    };
    let raw = envelope(&signed(writer, &response));
    stream.write_u32_le(raw.value().len() as u32).await?;
    stream.write_all(raw.value()).await?;
    let mut evidence = f.query.evidence.clone();
    evidence.admission = None;
    let bundle = TransferBundle {
        instance: f.descriptor.instance.clone(),
        candidate: Candidate {
            instance: f.descriptor.instance.clone(),
            operation: f.query.operation.clone(),
            evidence,
        },
        kind: RecordKind::Candidate,
        receipt: None,
        fork: None,
    };
    Ok((stream, bundle))
}
async fn local_append(port: u16, f: &Fixture) -> Result<TransferBundle, std::io::Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut stream, bundle) = local_connection(port, f).await?;
    let raw = encode_profile(&bundle, &limits()).unwrap();
    stream.write_u32_le(raw.value().len() as u32).await?;
    stream.write_all(raw.value()).await?;
    let length = stream.read_u32_le().await? as usize;
    assert!(length <= limits().bytes);
    let mut raw = vec![0; length];
    stream.read_exact(&mut raw).await?;
    Ok(decode_profile(&raw, &limits()).unwrap())
}

struct Source(glade_node::independent::TrustedCut);
impl glade_node::independent::PolicyClock for Source {
    fn current(&mut self) -> Result<glade_node::independent::TrustedCut, Fault> {
        Ok(self.0.clone())
    }
}
fn reopen(f: &Fixture, root: &std::path::Path) -> glade_node::independent::DiskSession<Source> {
    use glade_crdt_recovery_api::ReplicaRecoveryHost;
    use glade_node::independent::*;
    let fields = profile_fields(f, root, false);
    let mut request: ReplicaOpen = decode(fields[1].value(), &limits()).unwrap();
    request.expected_floor =
        Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let source = Source(TrustedCut {
        policy: f.policy.clone(),
        time: Some(TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        }),
        time_floor_ms: 10,
    });
    let mut host = glade_node::assembly::NodeAssembly::builder()
        .build()
        .independent_configured(
            EvidenceConfig {
                descriptor: f.descriptor.clone(),
                identity: f.identity.clone(),
                trusted_root: f.identity.root,
                node_seed: NODE,
                limits: request.limits.decode.clone(),
                certificates: vec![f.certificate.clone()],
                session_capacity: request.limits.guard_history,
            },
            source,
            DiskRootConfig {
                data: root.join("data"),
                trusted_floor: root.join("trusted"),
                registration: [73; 32],
                provision: false,
            },
        )
        .unwrap()
        .recovery;
    host.open_replica(request).unwrap()
}

#[cfg(unix)]
mod actual_processes {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    struct Process {
        child: Child,
        lines: Arc<Mutex<Vec<String>>>,
        readers: Vec<std::thread::JoinHandle<()>>,
    }
    impl Process {
        fn start(path: &std::path::Path, port: u16) -> Self {
            let mut child = Command::new(env!("CARGO_BIN_EXE_glade-node"))
                .env_clear()
                .arg("--independent-config")
                .arg(path)
                .arg(port.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let lines = Arc::new(Mutex::new(Vec::new()));
            let mut readers = vec![];
            let outputs: Vec<Box<dyn std::io::Read + Send>> = vec![
                Box::new(child.stdout.take().unwrap()),
                Box::new(child.stderr.take().unwrap()),
            ];
            for output in outputs {
                let seen = lines.clone();
                readers.push(std::thread::spawn(move || {
                    for line in BufReader::new(output).lines() {
                        match line {
                            Ok(line) => seen.lock().unwrap().push(line),
                            Err(_) => {
                                break;
                            }
                        }
                    }
                }));
            }
            Self {
                child,
                lines,
                readers,
            }
        }
        async fn wait_line(&mut self, prefix: &str) {
            let ready = tokio::time::timeout(std::time::Duration::from_secs(8), async {
                loop {
                    if self
                        .lines
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|line| line.starts_with(prefix))
                    {
                        break;
                    }
                    if let Some(status) = self.child.try_wait().unwrap() {
                        panic!("node exited {status}: {:?}", self.lines.lock().unwrap());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;
            assert!(
                ready.is_ok(),
                "missing {prefix}: {:?}",
                self.lines.lock().unwrap()
            );
        }
        fn kill(&mut self) {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
            for reader in self.readers.drain(..) {
                reader.join().unwrap();
            }
        }
        async fn stop(&mut self) {
            let status = Command::new("/bin/kill")
                .env_clear()
                .arg("-TERM")
                .arg(self.child.id().to_string())
                .status()
                .unwrap();
            assert!(status.success());
            let stopped = tokio::time::timeout(std::time::Duration::from_secs(8), async {
                loop {
                    if let Some(status) = self.child.try_wait().unwrap() {
                        break status;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            for reader in self.readers.drain(..) {
                reader.join().unwrap();
            }
            assert!(
                stopped.success(),
                "unclean actual node stop: {:?}",
                self.lines.lock().unwrap()
            );
        }
    }
    impl Drop for Process {
        fn drop(&mut self) {
            if self.child.try_wait().unwrap().is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            for reader in self.readers.drain(..) {
                let _ = reader.join();
            }
        }
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn actual_binary_lost_reply_then_sigkill_reopen_retries_original_receipt() {
        use tokio::io::AsyncWriteExt;
        let f = ic3_disk_fixture::disk_profile();
        let root = std::fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("ic3-c-lost-reply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("owned.profile");
        let mut profile = profile_fields(&f, &root, true);
        let port = free_port();
        std::fs::write(&path, encode(&profile, &limits()).unwrap().value()).unwrap();
        let mut process = Process::start(&path, port);
        process.wait_line("listening ").await;
        let (mut stream, bundle) = local_connection(port, &f).await.unwrap();
        let raw = encode_profile(&bundle, &limits()).unwrap();
        stream.write_u32_le(raw.value().len() as u32).await.unwrap();
        stream.write_all(raw.value()).await.unwrap();
        drop(stream);
        process.wait_line("independent local committed ").await;
        process.kill();
        let mut restored = reopen(&f, &root);
        use glade_crdt_recovery_api::ReplicaRecoverySession;
        let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
            panic!("lost reply commit unavailable");
        };
        let original = &value.image.state.instances[&f.descriptor.instance].accepted[0];
        let expected = TransferBundle {
            instance: f.descriptor.instance.clone(),
            candidate: original.candidate.clone(),
            kind: RecordKind::Accepted,
            receipt: Some(original.receipt.clone()),
            fork: None,
        };
        let next_attempt = value.authoritative_floor.next_attempt;
        assert!(matches!(
            ReplicaRecoverySession::close(&mut restored),
            CloseResult::Closed
        ));
        profile[8] = encode(&false, &limits()).unwrap();
        std::fs::write(&path, encode(&profile, &limits()).unwrap().value()).unwrap();
        process = Process::start(&path, port);
        process.wait_line("listening ").await;
        let retried = local_append(port, &f).await.unwrap();
        assert!(
            retried == expected,
            "lost ACK retry changed original evidence or receipt"
        );
        process.stop().await;
        let mut restored = reopen(&f, &root);
        let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
            panic!("exact retry unavailable");
        };
        assert_eq!(
            value.authoritative_floor.next_attempt, next_attempt,
            "exact retry must not remint attempt"
        );
        assert_eq!(
            value.image.state.instances[&f.descriptor.instance]
                .accepted
                .len(),
            1
        );
        assert!(matches!(
            ReplicaRecoverySession::close(&mut restored),
            CloseResult::Closed
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn actual_binary_receive_sigkill_reopen_preserves_orphan_and_refuses_false_complete() {
        let f = ic3_disk_fixture::disk_profile();
        let root = std::fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("ic3-c-receive-kill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("owned.profile");
        let mut profile = profile_fields(&f, &root, true);
        let port = free_port();
        std::fs::write(&path, encode(&profile, &limits()).unwrap().value()).unwrap();
        let mut process = Process::start(&path, port);
        process.wait_line("listening ").await;
        let (stream, _) = local_connection(port, &f).await.unwrap();
        process.wait_line("independent local receive ").await;
        process.kill();
        drop(stream);
        let mut restored = reopen(&f, &root);
        use glade_crdt_recovery_api::ReplicaRecoverySession;
        let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
            panic!("orphan retained image unavailable");
        };
        assert_eq!(
            value
                .authoritative_floor
                .guards
                .values()
                .filter(|status| matches!(status, GuardStatus::Active(_)))
                .count(),
            1
        );
        assert!(
            value.image.permanent_loss.is_empty(),
            "process death cannot manufacture drained loss"
        );
        let request = restored.reconstruction_request().unwrap();
        let glade_crdt_recovery_api::ReconstructResult::Cut(cut) =
            restored.reconstruct_cut(request)
        else {
            panic!("orphan cut unavailable");
        };
        assert!(!cut.complete_local);
        assert_eq!(cut.guards.len(), 1);
        assert!(!cut.permanent_loss);
        assert!(matches!(
            ReplicaRecoverySession::close(&mut restored),
            CloseResult::Pending
        ));
        drop(restored);
        profile[8] = encode(&false, &limits()).unwrap();
        std::fs::write(&path, encode(&profile, &limits()).unwrap().value()).unwrap();
        process = Process::start(&path, port);
        process.wait_line("listening ").await;
        assert!(
            local_append(port, &f).await.is_err(),
            "new local transport custody cannot replace orphan Active history"
        );
        process.kill();
        let mut restored = reopen(&f, &root);
        let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
            panic!("orphan after refused new receive unavailable");
        };
        assert!(value.image.state.instances[&f.descriptor.instance]
            .accepted
            .is_empty());
        assert!(value.image.permanent_loss.is_empty());
        assert_eq!(
            value
                .authoritative_floor
                .guards
                .values()
                .filter(|status| matches!(status, GuardStatus::Active(_)))
                .count(),
            1
        );
        drop(restored);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn corrupt_x_node_recovery_and_wrong_scope_do_not_block_independent_y() {
        let x = ic3_disk_fixture::disk_profile_named("instance-X");
        let y = ic3_disk_fixture::disk_profile_named("instance-Y");
        let base = std::fs::canonicalize("/tmp").unwrap().join(format!(
            "ic3-c-instance-independence-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let roots = [base.join("x"), base.join("y")];
        for root in &roots {
            std::fs::create_dir_all(root).unwrap();
        }
        let paths = [
            roots[0].join("owned.profile"),
            roots[1].join("owned.profile"),
        ];
        let ports = [free_port(), free_port()];
        let mut profiles = [
            profile_fields(&x, &roots[0], true),
            profile_fields(&y, &roots[1], true),
        ];
        let mut request: ReplicaOpen = decode(profiles[1][1].value(), &limits()).unwrap();
        request.owner.store = StoreIncarnation([79; 32]);
        request.plan_namespace = [80; 32];
        request.invocation_namespace = [81; 32];
        profiles[1][1] = encode(&request, &limits()).unwrap();
        profiles[1][7] = Bytes::new([82; 32].to_vec());
        for index in 0..2 {
            std::fs::write(
                &paths[index],
                encode(&profiles[index], &limits()).unwrap().value(),
            )
            .unwrap();
        }
        let mut a = Process::start(&paths[0], ports[0]);
        a.wait_line("listening ").await;
        let mut b = Process::start(&paths[1], ports[1]);
        b.wait_line("listening ").await;
        a.kill();
        let floor = std::fs::read(roots[0].join("trusted/replica.floor")).unwrap();
        for index in 0..2 {
            use std::io::{Seek, SeekFrom, Write};
            let mut slot = std::fs::OpenOptions::new()
                .write(true)
                .open(roots[0].join(format!("data/image.{index}")))
                .unwrap();
            slot.seek(SeekFrom::Start(0)).unwrap();
            slot.write_all(b"CORRUPT").unwrap();
            slot.sync_all().unwrap();
        }
        profiles[0][8] = encode(&false, &limits()).unwrap();
        std::fs::write(&paths[0], encode(&profiles[0], &limits()).unwrap().value()).unwrap();
        a = Process::start(&paths[0], ports[0]);
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(status) = a.child.try_wait().unwrap() {
                    break status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("corrupt actual node must refuse before serving");
        assert!(!status.success());
        for reader in a.readers.drain(..) {
            reader.join().unwrap();
        }
        assert!(!a
            .lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("listening ")));
        assert_eq!(
            std::fs::read(roots[0].join("trusted/replica.floor")).unwrap(),
            floor
        );
        assert!(
            local_connection(ports[1], &x).await.is_err(),
            "equal resource key bytes cannot substitute a different signed declaration/instance"
        );
        let receipt = local_append(ports[1], &y).await.unwrap();
        assert_eq!(receipt.kind, RecordKind::Accepted);
        assert_eq!(receipt.instance, y.descriptor.instance);
        b.stop().await;
        use glade_crdt_recovery_api::{ReplicaRecoveryHost, ReplicaRecoverySession};
        use glade_node::independent::*;
        request.expected_floor =
            Bytes::new(std::fs::read(roots[1].join("trusted/replica.floor")).unwrap()).digest();
        let mut host = glade_node::assembly::NodeAssembly::builder()
            .build()
            .independent_configured(
                EvidenceConfig {
                    descriptor: y.descriptor.clone(),
                    identity: y.identity.clone(),
                    trusted_root: y.identity.root,
                    node_seed: NODE,
                    limits: request.limits.decode.clone(),
                    certificates: vec![y.certificate.clone()],
                    session_capacity: request.limits.guard_history,
                },
                Source(TrustedCut {
                    policy: y.policy.clone(),
                    time: Some(TimeInterval {
                        earliest_ms: 20,
                        latest_ms: 21,
                    }),
                    time_floor_ms: 10,
                }),
                DiskRootConfig {
                    data: roots[1].join("data"),
                    trusted_floor: roots[1].join("trusted"),
                    registration: [82; 32],
                    provision: false,
                },
            )
            .unwrap()
            .recovery;
        let mut restored = host.open_replica(request).unwrap();
        ic3_disk_fixture::local_authorization(&y, &mut restored, 93);
        let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
            panic!("independent Y recovery");
        };
        assert_eq!(
            value.image.state.instances[&y.descriptor.instance]
                .accepted
                .len(),
            1
        );
        assert!(!value
            .image
            .state
            .instances
            .contains_key(&x.descriptor.instance));
        assert_eq!(
            ReplicaRecoverySession::close(&mut restored),
            CloseResult::Closed
        );
        std::fs::remove_dir_all(base).unwrap();
    }
    fn bytes_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn two_actual_binaries_partition_edit_kill_reopen_exact_retry_then_automatic_heal() {
        partition_case(false, false).await;
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn two_actual_binaries_preserve_same_slot_rivals_and_fork_closure_on_heal() {
        partition_case(true, true).await;
    }
    async fn partition_case(fork: bool, reverse: bool) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut first = ic3_disk_fixture::disk_profile();
        let second_node = [34; 32];
        let second_writer = if fork { WRITER } else { [23; 32] };
        let nodes = [
            glade_node::signing::public_key(&NODE),
            glade_node::signing::public_key(&second_node),
        ];
        let mut policy: PolicyBody = decode_signed_body(&first.policy, &limits()).unwrap();
        policy.admitting_nodes = nodes.to_vec();
        policy.replica_holders = nodes.to_vec();
        if !fork {
            policy.grants.push(ScopedGrant {
                requester: "alice".into(),
                key: glade_node::signing::public_key(&second_writer),
                action: ResourceAction::Append,
                resource: first.descriptor.resource.clone(),
            });
        }
        first.policy = signed(&ROOT, &policy);
        first.query.evidence = proof(&first, &WRITER, &nodes);
        let mut second = Fixture {
            descriptor: first.descriptor.clone(),
            identity: first.identity.clone(),
            certificate: first.certificate.clone(),
            policy: first.policy.clone(),
            query: first.query.clone(),
        };
        let mut cert: CertificateBody = decode_signed_body(&second.certificate, &limits()).unwrap();
        if !fork {
            cert.writer_key = glade_node::signing::public_key(&second_writer);
            cert.canonical_origin = "writer-second-e0".into();
            cert.issuance = 1;
            cert.display_name = "second".into();
        }
        second.certificate = signed(&ROOT, &cert);
        let mut op = second.query.operation.inner().clone();
        op.origin = cert.canonical_origin;
        op.payload = br#"{"after":null,"atom_id":"b","kind":"insert","text":"B"}"#.to_vec();
        second.query.operation = Operation::new(op);
        second.query.evidence = proof(&second, &second_writer, &nodes);
        let base = std::fs::canonicalize("/tmp").unwrap().join(format!(
            "ic3-c-process-{}-{}",
            if fork { "fork" } else { "heal" },
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let roots = [base.join("a"), base.join("b")];
        for root in &roots {
            std::fs::create_dir_all(root).unwrap();
        }
        let ports = [free_port(), free_port()];
        let unreachable = [free_port(), free_port()];
        let endpoints = [
            glade_node::signing::public_key(&[44; 32]),
            glade_node::signing::public_key(&[45; 32]),
        ];
        let mut profiles = [
            profile_fields(&first, &roots[0], true),
            profile_fields(&first, &roots[1], true),
        ];
        profiles[1][4] = Bytes::new(second_node.to_vec());
        profiles[1][5] = Bytes::new([45; 32].to_vec());
        profiles[1][7] = Bytes::new([77; 32].to_vec());
        let mut request: ReplicaOpen = decode(profiles[1][1].value(), &limits()).unwrap();
        request.owner.store = StoreIncarnation([74; 32]);
        request.plan_namespace = [75; 32];
        request.invocation_namespace = [76; 32];
        profiles[1][1] = encode(&request, &limits()).unwrap();
        let paths = [
            roots[0].join("owned.profile"),
            roots[1].join("owned.profile"),
        ];
        for index in 0..2 {
            profiles[index][3] = encode(
                &vec![first.certificate.clone(), second.certificate.clone()],
                &limits(),
            )
            .unwrap();
            let other = 1 - index;
            profiles[index][10] = encode(
                &vec![(
                    nodes[other],
                    (
                        endpoints[other],
                        format!(
                            "{}@127.0.0.1:{}",
                            hex(&endpoints[other]),
                            unreachable[other]
                        ),
                    ),
                )],
                &limits(),
            )
            .unwrap();
            std::fs::write(
                &paths[index],
                encode(&profiles[index], &limits()).unwrap().value(),
            )
            .unwrap();
        }
        let mut a = Process::start(&paths[0], ports[0]);
        a.wait_line("listening ").await;
        let mut b = Process::start(&paths[1], ports[1]);
        b.wait_line("listening ").await;
        // Only each writer's own node receives its own local edit. There is no
        // client replay of the other peer's operation anywhere in this test.
        let receipt_a = local_append(ports[0], &first).await.unwrap();
        let (mut stream, bundle) =
            local_connection_for(ports[1], &second, &second_writer, nodes[1])
                .await
                .unwrap();
        let raw = encode_profile(&bundle, &limits()).unwrap();
        stream.write_u32_le(raw.value().len() as u32).await.unwrap();
        stream.write_all(raw.value()).await.unwrap();
        let length = stream.read_u32_le().await.unwrap() as usize;
        let mut raw = vec![0; length];
        stream.read_exact(&mut raw).await.unwrap();
        let receipt_b: TransferBundle = decode_profile(&raw, &limits()).unwrap();
        assert_eq!(receipt_b.kind, RecordKind::Accepted);
        a.kill();
        profiles[0][8] = encode(&false, &limits()).unwrap();
        std::fs::write(&paths[0], encode(&profiles[0], &limits()).unwrap().value()).unwrap();
        a = Process::start(&paths[0], ports[0]);
        a.wait_line("listening ").await;
        let exact = local_append(ports[0], &first).await.unwrap();
        assert_eq!(
            exact, receipt_a,
            "process restart retry must return the original sealed admission/receipt"
        );
        a.stop().await;
        b.stop().await;
        for index in 0..2 {
            profiles[index][8] = encode(&false, &limits()).unwrap();
            let other = 1 - index;
            profiles[index][10] = encode(
                &vec![(
                    nodes[other],
                    (
                        endpoints[other],
                        format!("{}@127.0.0.1:{}", hex(&endpoints[other]), ports[other]),
                    ),
                )],
                &limits(),
            )
            .unwrap();
            std::fs::write(
                &paths[index],
                encode(&profiles[index], &limits()).unwrap().value(),
            )
            .unwrap();
        }
        if reverse {
            b = Process::start(&paths[1], ports[1]);
            b.wait_line("listening ").await;
            a = Process::start(&paths[0], ports[0]);
            a.wait_line("listening ").await;
        } else {
            a = Process::start(&paths[0], ports[0]);
            a.wait_line("listening ").await;
            b = Process::start(&paths[1], ports[1]);
            b.wait_line("listening ").await;
        }
        a.wait_line("independent exchanged ").await;
        b.wait_line("independent exchanged ").await;
        if fork {
            for process in [&a, &b] {
                tokio::time::timeout(std::time::Duration::from_secs(8), async {
                    loop {
                        if process
                            .lines
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|line| line.starts_with("independent exchanged "))
                            .count()
                            >= 2
                        {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("automatic transfer of both original fork proof orientations");
            }
        }
        wait_idle(
            || {
                vec![
                    a.lines.lock().unwrap().clone(),
                    b.lines.lock().unwrap().clone(),
                ]
            },
            None,
            std::time::Duration::from_secs(8),
        )
        .await;
        a.stop().await;
        b.stop().await;
        use glade_crdt_recovery_api::{ReplicaRecoveryHost, ReplicaRecoverySession};
        use glade_node::independent::*;
        let mut exact_sets = vec![];
        let mut fork_sets = vec![];
        for index in 0..2 {
            let mut request: ReplicaOpen = decode(profiles[index][1].value(), &limits()).unwrap();
            request.expected_floor =
                Bytes::new(std::fs::read(roots[index].join("trusted/replica.floor")).unwrap())
                    .digest();
            let mut host = glade_node::assembly::NodeAssembly::builder()
                .build()
                .independent_configured(
                    EvidenceConfig {
                        descriptor: first.descriptor.clone(),
                        identity: first.identity.clone(),
                        trusted_root: first.identity.root,
                        node_seed: if index == 0 { NODE } else { second_node },
                        limits: request.limits.decode.clone(),
                        certificates: vec![first.certificate.clone(), second.certificate.clone()],
                        session_capacity: request.limits.guard_history,
                    },
                    Source(TrustedCut {
                        policy: first.policy.clone(),
                        time: Some(TimeInterval {
                            earliest_ms: 20,
                            latest_ms: 21,
                        }),
                        time_floor_ms: 10,
                    }),
                    DiskRootConfig {
                        data: roots[index].join("data"),
                        trusted_floor: roots[index].join("trusted"),
                        registration: if index == 0 { [73; 32] } else { [77; 32] },
                        provision: false,
                    },
                )
                .unwrap()
                .recovery;
            let mut restored = host.open_replica(request).unwrap();
            let glade_crdt_recovery_api::LoadResult::Validated(value) = restored.load() else {
                panic!("process state unavailable");
            };
            let mut set = value.image.state.instances[&first.descriptor.instance]
                .accepted
                .clone();
            set.sort_by_key(|accepted| accepted.candidate.operation.canonical().digest());
            assert_eq!(set.len(), 2);
            assert!(set
                .iter()
                .any(|accepted| accepted.candidate == receipt_a.candidate
                    && Some(accepted.receipt.clone()) == receipt_a.receipt));
            assert!(set
                .iter()
                .any(|accepted| accepted.candidate == receipt_b.candidate
                    && Some(accepted.receipt.clone()) == receipt_b.receipt));
            let request = restored.reconstruction_request().unwrap();
            let glade_crdt_recovery_api::ReconstructResult::Cut(cut) =
                restored.reconstruct_cut(request)
            else {
                panic!("process cut unavailable");
            };
            assert!(cut.complete_local,"final actual-node cut index={index} native={} obligations_missing={} permanent_loss={} guards={:?}; a={:?}; b={:?}",cut.kernel.complete_local,value.image.obligations.iter().filter(|item|matches!(item.classification,Classification::Missing)).count(),value.image.permanent_loss.len(),value.authoritative_floor.guards.values().map(|guard|match guard{GuardStatus::Active(_)=>"Active",GuardStatus::Settled{..}=>"Settled",GuardStatus::Loss{..}=>"Loss",_=>"Other"}).collect::<Vec<_>>(),a.lines.lock().unwrap(),b.lines.lock().unwrap());
            assert_eq!(cut.kernel.eligible.len(), if fork { 0 } else { 2 });
            if fork {
                assert!(!cut.kernel.quarantine.is_empty());
                assert!(!value.image.state.instances[&first.descriptor.instance]
                    .qualified_forks
                    .is_empty());
            }
            let mut proofs = value.image.state.instances[&first.descriptor.instance]
                .qualified_forks
                .iter()
                .map(|proof| {
                    encode_profile(
                        &TransferBundle {
                            instance: first.descriptor.instance.clone(),
                            candidate: proof.first.clone(),
                            kind: RecordKind::Fork,
                            receipt: None,
                            fork: Some(proof.clone()),
                        },
                        &limits(),
                    )
                    .unwrap()
                    .value()
                    .to_vec()
                })
                .collect::<Vec<_>>();
            proofs.sort();
            fork_sets.push(proofs);
            let mut rows = String::new();
            for accepted in &set {
                let op = accepted.candidate.operation.inner();
                let refs = if op.refs.is_empty() {
                    "-".into()
                } else {
                    op.refs
                        .iter()
                        .map(|head| {
                            format!(
                                "{}:{}:{}",
                                bytes_hex(head.origin.as_bytes()),
                                head.seq,
                                head.hash
                                    .as_ref()
                                    .map_or("-".into(), |hash| bytes_hex(hash))
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };
                rows.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    bytes_hex(op.origin.as_bytes()),
                    op.seq,
                    op.lamport,
                    op.prev.as_ref().map_or("-".into(), |hash| bytes_hex(hash)),
                    refs,
                    bytes_hex(&op.payload),
                    usize::from(
                        cut.kernel
                            .eligible
                            .contains(&accepted.candidate.operation.canonical().digest())
                    )
                ));
            }
            let projection = std::path::PathBuf::from(format!(
                "/tmp/ic3-c-recovered-{}-projection-{}-{index}.tsv",
                if fork { "fork" } else { "heal" },
                std::process::id()
            ));
            std::fs::write(&projection, rows).unwrap();
            println!("actual recovered projection {}", projection.display());
            exact_sets.push(
                set.into_iter()
                    .map(|accepted| (accepted.candidate, accepted.receipt))
                    .collect::<Vec<_>>(),
            );
            assert!(matches!(
                ReplicaRecoverySession::close(&mut restored),
                CloseResult::Closed
            ));
        }
        assert!(exact_sets[0]==exact_sets[1],"exact operations, signed evidence and original receipts must converge; local/historical verification context remains distinct");
        assert!(
            fork_sets[0] == fork_sets[1],
            "full fork proof identities, not only quarantine outcomes, must converge"
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
