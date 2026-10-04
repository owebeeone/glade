//! Provider-owned current cuts are explicit per-test values, never process globals.
use super::*;
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct MutableSource(Arc<Mutex<TrustedCut>>);
impl PolicyClock for MutableSource {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        Ok(self.0.lock().unwrap().clone())
    }
}
fn source(f: &Fixture, time: u64) -> MutableSource {
    MutableSource(Arc::new(Mutex::new(TrustedCut {
        policy: f.policy.clone(),
        time: Some(TimeInterval {
            earliest_ms: time,
            latest_ms: time + 1,
        }),
        time_floor_ms: 10,
    })))
}
fn provider(f: &Fixture, s: MutableSource) -> GenuineEvidence<MutableSource> {
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
        s,
    )
    .unwrap()
}
fn challenge(f: &Fixture) -> SessionRequest {
    SessionRequest {
        requester: "alice".into(),
        writer_key: glade_node::signing::public_key(&WRITER),
        operation: f.query.operation.canonical().digest(),
        channel: [90; 32],
        session_nonce: [91; 32],
        challenge_nonce: [92; 32],
        window: Window {
            not_before_ms: 10,
            not_after_ms: 100,
        },
    }
}
#[test]
fn denied_challenge_retains_advanced_time_before_reopen_can_reauthorize() {
    let f = Fixture::new();
    let root = root("r2-denied");
    let s = source(&f, 20);
    let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    s.0.lock().unwrap().time = Some(TimeInterval {
        earliest_ms: 1001,
        latest_ms: 1002,
    });
    assert!(session.issue_challenge(challenge(&f)).is_err());
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(provider(&f, source(&f, 20)), config(&root, false));
    if let Ok(mut session) = host.open_replica(request(&f, digest)) {
        assert!(
            session.issue_challenge(challenge(&f)).is_err(),
            "restart cannot forget time learned on the denied challenge"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn receive_only_holder_advances_trusted_time_and_converges_without_append_grants() {
    let mut f = Fixture::new();
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.grants.clear();
    f.policy = signed(&ROOT, &policy);
    let root = root("r2-receive-only");
    let s = source(&f, 20);
    let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    s.0.lock().unwrap().time = Some(TimeInterval {
        earliest_ms: 22,
        latest_ms: 23,
    });
    policy.generation = 1;
    policy.previous = Some(envelope(&f.policy).digest());
    s.0.lock().unwrap().policy = signed(&ROOT, &policy);
    let mut permit = None;
    for _ in 0..3 {
        let LoadResult::Validated(loaded) = session.load() else {
            panic!("stable current holder reload")
        };
        match session.begin_ingress(guard(&loaded)) {
            Ok(p) => {
                permit = Some(p);
                break;
            }
            Err(Fault::BindingMismatch | Fault::Unavailable) => {}
            Err(e) => panic!("valid receive-only cut {e:?}"),
        }
    }
    let permit = permit.expect(
        "stable trusted holder must converge within bounded reload/retry without append permission",
    );
    assert!(matches!(
        session.abandon_ingress(permit),
        PersistResult::Committed { .. }
    ));
    let LoadResult::Validated(advanced) = session.load() else {
        panic!("retained receive-only policy/time");
    };
    assert_eq!(advanced.authoritative_floor.issuance.policy_generation, 1);
    assert_eq!(advanced.authoritative_floor.issuance.time_floor_ms, 22);
    assert!(
        session.issue_challenge(challenge(&f)).is_err(),
        "receive permission does not borrow append permission"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(provider(&f, s), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let LoadResult::Validated(restored) = session.load() else {
        panic!("exact receive-only cut restored");
    };
    assert_eq!(restored.authoritative_floor.issuance.policy_generation, 1);
    assert_eq!(restored.authoritative_floor.issuance.time_floor_ms, 22);
    let permit = session.begin_ingress(guard(&restored)).unwrap();
    assert!(matches!(
        session.abandon_ingress(permit),
        PersistResult::Committed { .. }
    ));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone)]
struct FailingPublication {
    inner: MutableSource,
    armed: Arc<Mutex<bool>>,
    floor: std::path::PathBuf,
}
impl PolicyClock for FailingPublication {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        if *self.armed.lock().unwrap() {
            std::fs::create_dir_all(self.floor.join("replica.next")).unwrap();
        }
        self.inner.current()
    }
}
#[test]
fn learned_denial_with_failed_publication_keeps_full_and_narrow_reopen_unavailable() {
    let f = Fixture::new();
    let root = root("r2-publication-failure");
    let s = source(&f, 20);
    let armed = Arc::new(Mutex::new(false));
    let source = FailingPublication {
        inner: s.clone(),
        armed: armed.clone(),
        floor: root.join("trusted"),
    };
    let config_evidence = || EvidenceConfig {
        descriptor: f.descriptor.clone(),
        identity: f.identity.clone(),
        trusted_root: glade_node::signing::public_key(&ROOT),
        node_seed: NODE,
        limits: limits(),
        certificates: vec![f.certificate.clone()],
        session_capacity: 16,
    };
    let mut host = DiskHost::new(
        GenuineEvidence::new(config_evidence(), source).unwrap(),
        config(&root, true),
    );
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    s.0.lock().unwrap().time = Some(TimeInterval {
        earliest_ms: 1001,
        latest_ms: 1002,
    });
    *armed.lock().unwrap() = true;
    assert!(session.issue_challenge(challenge(&f)).is_err());
    assert!(
        matches!(session.load(), LoadResult::Unavailable),
        "unresolved learned observation has no usable complete cached image"
    );
    assert!(matches!(
        StorageAttemptSession::recover(&mut session),
        RecoveryResult::Unavailable
    ));
    drop(session);
    std::fs::remove_dir(root.join("trusted/replica.next")).unwrap();
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(
        provider(&f, super::observations::source(&f, 20)),
        config(&root, false),
    );
    assert!(
        host.open_replica(request(&f, digest)).is_err(),
        "unretained learned cut must not reset to older disk authority"
    );
    let mut host = DiskHost::new(
        provider(&f, super::observations::source(&f, 20)),
        config(&root, false),
    );
    let original = request(&f, digest);
    assert!(StorageAttemptHost::open(
        &mut host,
        OpenRequest {
            owner: original.owner,
            invocation_namespace: original.invocation_namespace,
            limits: original.limits.storage
        }
    )
    .is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn capacity_refusal_after_learning_keeps_marker_unavailable_and_survives_both_open_forms() {
    let f = ic3_disk_fixture::disk_profile_with(|limit| {
        limit.storage.critical_bytes = limit.image_bytes - 1;
    });
    let root = root("r2-critical-capacity");
    let s = source(&f, 20);
    let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    s.0.lock().unwrap().time = Some(TimeInterval {
        earliest_ms: 1001,
        latest_ms: 1002,
    });
    assert!(session.issue_challenge(challenge(&f)).is_err());
    assert!(
        matches!(session.load(), LoadResult::Unavailable),
        "learned cut refused before full image publication still blocks cached authority"
    );
    assert!(matches!(
        StorageAttemptSession::recover(&mut session),
        RecoveryResult::Unavailable
    ));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(provider(&f, source(&f, 20)), config(&root, false));
    assert!(host.open_replica(request(&f, digest)).is_err());
    let mut host = DiskHost::new(provider(&f, source(&f, 20)), config(&root, false));
    let original = request(&f, digest);
    assert!(StorageAttemptHost::open(
        &mut host,
        OpenRequest {
            owner: original.owner,
            invocation_namespace: original.invocation_namespace,
            limits: original.limits.storage
        }
    )
    .is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone)]
struct CountingSource {
    inner: MutableSource,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl PolicyClock for CountingSource {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.current()
    }
}
#[test]
fn failed_marker_establishment_never_consults_current_source() {
    let f = Fixture::new();
    let root = root("r2-pre-source");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let current = CountingSource {
        inner: source(&f, 20),
        calls: calls.clone(),
    };
    let configuration = EvidenceConfig {
        descriptor: f.descriptor.clone(),
        identity: f.identity.clone(),
        trusted_root: glade_node::signing::public_key(&ROOT),
        node_seed: NODE,
        limits: limits(),
        certificates: vec![f.certificate.clone()],
        session_capacity: 16,
    };
    let mut host = DiskHost::new(
        GenuineEvidence::new(configuration, current).unwrap(),
        config(&root, true),
    );
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let before = calls.load(std::sync::atomic::Ordering::SeqCst);
    std::fs::create_dir(root.join("trusted/replica.next")).unwrap();
    assert!(session.issue_challenge(challenge(&f)).is_err());
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        before,
        "durable bounded marker is a prerequisite for source consultation"
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn denied_response_retains_time_and_denied_ingress_retains_peer_revocation() {
    for ingress in [false, true] {
        let f = Fixture::new();
        let root = root(if ingress {
            "r2-denied-ingress"
        } else {
            "r2-denied-response"
        });
        let s = source(&f, 20);
        let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
        let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
        if ingress {
            let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
            policy.generation = 1;
            policy.previous = Some(envelope(&f.policy).digest());
            policy.replica_holders.clear();
            s.0.lock().unwrap().policy = signed(&ROOT, &policy);
            let mut denied = false;
            for _ in 0..3 {
                let LoadResult::Validated(loaded) = session.load() else {
                    panic!("retained valid denial cut")
                };
                match session.begin_ingress(guard(&loaded)) {
                    Err(Fault::BindingMismatch) => {}
                    Err(_) => {
                        denied = true;
                        break;
                    }
                    Ok(_) => panic!("revoked peer admitted"),
                }
            }
            assert!(denied);
        } else {
            let issued = session.issue_challenge(challenge(&f)).unwrap();
            let body: LocalChallengeBody = decode_signed_body(&issued, &limits()).unwrap();
            let response = signed(
                &WRITER,
                &LocalResponseBody {
                    instance: body.instance,
                    node: body.node,
                    requester: body.requester,
                    writer_key: body.writer_key,
                    signed_challenge: envelope(&issued).digest(),
                    operation: body.operation,
                    channel: body.channel,
                    session_nonce: body.session_nonce,
                    challenge_nonce: body.challenge_nonce,
                },
            );
            s.0.lock().unwrap().time = Some(TimeInterval {
                earliest_ms: 1001,
                latest_ms: 1002,
            });
            assert!(session.authenticate(response, [90; 32], [91; 32]).is_err());
        }
        let LoadResult::Validated(loaded) = session.load() else {
            panic!("exact learned denial retained")
        };
        if ingress {
            assert_eq!(loaded.authoritative_floor.issuance.policy_generation, 1);
        } else {
            assert_eq!(loaded.authoritative_floor.issuance.time_floor_ms, 1001);
        }
        drop(session);
        let digest =
            Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
        let mut host = DiskHost::new(provider(&f, source(&f, 20)), config(&root, false));
        if let Ok(mut restored) = host.open_replica(request(&f, digest)) {
            assert!(
                restored.issue_challenge(challenge(&f)).is_err(),
                "historical restoration cannot authorize from older provider policy/time"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
#[ignore = "owned public denied current-policy observation crash child"]
fn denied_policy_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_DENIED_POLICY_ROOT").unwrap());
    let f = Fixture::new();
    let s = source(&f, 20);
    let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.generation = 1;
    policy.previous = Some(envelope(&f.policy).digest());
    policy.revoked = vec![glade_node::signing::public_key(&WRITER)];
    s.0.lock().unwrap().policy = signed(&ROOT, &policy);
    assert!(session.issue_challenge(challenge(&f)).is_err());
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("learned refusal selected")
    };
    assert_eq!(loaded.authoritative_floor.issuance.policy_generation, 1);
    use std::io::Write;
    println!("denied-policy-ready");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn learned_chained_revocation_survives_actual_process_death_and_older_provider() {
    use std::io::BufRead;
    let root = root("r2-revocation-process");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("IC3_DENIED_POLICY_ROOT", &root)
        .args([
            "--exact",
            "ic3_disk_remediation::observations::denied_policy_child",
            "--ignored",
            "--nocapture",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap() == "denied-policy-ready" {
                send.send(()).unwrap();
                return;
            }
        }
    });
    let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    ready.expect("genuine denied chained policy retained before process death");
    let f = Fixture::new();
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(provider(&f, source(&f, 20)), config(&root, false));
    if let Ok(mut session) = host.open_replica(request(&f, digest)) {
        assert!(session.issue_challenge(challenge(&f)).is_err());
    }
    std::fs::remove_dir_all(root).unwrap();
}
