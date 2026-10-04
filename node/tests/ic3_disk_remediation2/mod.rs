//! Public regressions for exact retained authorization and oversized drained loss.
use super::*;
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Sequence(Arc<Mutex<(usize, usize, TrustedCut, TrustedCut)>>);
impl PolicyClock for Sequence {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        let mut s = self.0.lock().unwrap();
        s.0 += 1;
        Ok(if s.0 >= s.1 { s.3.clone() } else { s.2.clone() })
    }
}
fn sequence(f: &Fixture, time: TimeInterval) -> Sequence {
    let cut = TrustedCut {
        policy: f.policy.clone(),
        time: Some(time),
        time_floor_ms: 10,
    };
    Sequence(Arc::new(Mutex::new((0, usize::MAX, cut.clone(), cut))))
}
fn provider(f: &Fixture, s: Sequence) -> GenuineEvidence<Sequence> {
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
fn arm(s: &Sequence, after: usize, later: TrustedCut) {
    let mut state = s.0.lock().unwrap();
    state.0 = 0;
    state.1 = after;
    state.3 = later;
}
#[test]
fn public_begin_must_recheck_exact_retained_revocation() {
    write_start(false);
}
#[test]
fn public_begin_must_recheck_exact_retained_expiry() {
    write_start(true);
}
fn write_start(expiry: bool) {
    let root = root(if expiry { "r6-expiry" } else { "r6-revocation" });
    write_start_at(expiry, root, false);
}
fn write_start_at(expiry: bool, root: std::path::PathBuf, keep: bool) {
    let f = ic3_disk_fixture::disk_profile();
    let s = sequence(
        &f,
        TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        },
    );
    let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut later = s.0.lock().unwrap().2.clone();
    if expiry {
        later.time = Some(TimeInterval {
            earliest_ms: 1001,
            latest_ms: 1002,
        });
    } else {
        let mut p: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
        p.generation = 1;
        p.previous = Some(envelope(&f.policy).digest());
        p.revoked.push(glade_node::signing::public_key(&NODE));
        later.policy = signed(&ROOT, &p);
    }
    arm(&s, 7, later);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    let outcomes = session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    assert!(
        s.0.lock().unwrap().0 >= 7,
        "retention consultation executed"
    );
    assert!(
        !outcomes
            .iter()
            .any(|x| matches!(x, Outcome::AcceptedLocal(_))),
        "new retained revoked/expired cut must fence before Started"
    );
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("exact retained decision");
    };
    assert_eq!(
        loaded.image.recovery.attempts.len(),
        1,
        "no reminted attempt"
    );
    assert!(matches!(
        loaded.image.recovery.attempts[0].phase,
        AttemptPhase::Terminal(TerminalOutcome::NonCommit { .. })
    ));
    assert!(loaded.image.state.instances[&f.query.instance]
        .accepted
        .is_empty());
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(provider(&f, s), config(&root, false));
    let mut restored = host.open_replica(request(&f, digest)).unwrap();
    let RecoveryResult::Validated(r) = restored.recover() else {
        panic!("retained original terminal");
    };
    assert_eq!(r.attempts.len(), 1);
    assert!(matches!(
        r.attempts[0].phase,
        AttemptPhase::Terminal(TerminalOutcome::NonCommit { .. })
    ));
    drop(restored);
    if !keep {
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn ingress_uses_retained_complete_interval_not_first_narrow_interval() {
    let f = Fixture::new();
    let root = root("r6-ingress-interval");
    let s = sequence(
        &f,
        TimeInterval {
            earliest_ms: 20,
            latest_ms: 100,
        },
    );
    let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("selected wide interval");
    };
    let mut incoming = guard(&loaded);
    incoming.deadline_ms = 50;
    let wide = s.0.lock().unwrap().2.clone();
    s.0.lock().unwrap().2.time = Some(TimeInterval {
        earliest_ms: 20,
        latest_ms: 21,
    });
    arm(&s, 2, wide);
    assert!(
        matches!(session.begin_ingress(incoming), Err(Fault::Ownership)),
        "retained [20,100] straddles deadline50; no consumable permit"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn oversized_drained_result_can_discharge_only_as_compact_permanent_loss() {
    let f = Fixture::new();
    let root = root("r4-oversized");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("selected");
    };
    let incoming = guard(&loaded);
    let bytes = Bytes::new(vec![81; incoming.max_bytes + 1]);
    let permit = session.begin_ingress(incoming.clone()).unwrap();
    let (permit, result) = session
        .receive_ingress(permit, || async { Ok(bytes.clone()) })
        .unwrap()
        .await;
    assert_eq!(result, Ok(bytes.clone()));
    assert_eq!(permit.received_digest(), None);
    let observation = Observation {
        id: 1,
        instance: incoming.id.instance.clone(),
        source: incoming.source,
        channel: incoming.channel,
        guard: incoming.id.clone(),
        bytes,
        expected: vec![],
    };
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.observations.push(observation.clone());
    checkpoint.image.watermark = 1;
    assert_eq!(
        session.settle_ingress(permit, observation, checkpoint),
        PersistResult::Refused(Fault::Capacity)
    );
    assert!(
        matches!(
            session.retain_pending_ingress_loss(&incoming.id),
            PersistResult::Committed { .. }
        ),
        "M+1 owned drained result must fit compact reserved loss without normal admission"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("loss selected");
    };
    assert_eq!(loaded.image.permanent_loss.len(), 1);
    assert!(loaded.image.observations.is_empty());
    let cut = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
        panic!("cut");
    };
    assert!(!cut.complete_local);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stable_valid_complete_interval_still_admits_and_never_started_guard_abandons() {
    let f = Fixture::new();
    let root = root("r6-stable");
    let s = sequence(
        &f,
        TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        },
    );
    let mut host = DiskHost::new(provider(&f, s), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("stable image");
    };
    let mut g = guard(&loaded);
    g.deadline_ms = 50;
    let permit = session.begin_ingress(g).unwrap();
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
#[tokio::test]
#[ignore = "owned public corrected-cut and oversized-loss crash child"]
async fn remediation2_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_REM2_ROOT").unwrap());
    let mode = std::env::var("IC3_REM2_MODE")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    if mode >= 3 {
        write_start_at(mode == 4, root, true);
    } else {
        let f = Fixture::new();
        let mut host = DiskHost::new(evidence(&f), config(&root, true));
        let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
        let LoadResult::Validated(loaded) = session.load() else {
            panic!("initial");
        };
        let g = guard(&loaded);
        let bytes = Bytes::new(vec![81; g.max_bytes + 1]);
        let permit = session.begin_ingress(g.clone()).unwrap();
        let (permit, _) = session
            .receive_ingress(permit, || async { Ok(bytes.clone()) })
            .unwrap()
            .await;
        let mut observation = Observation {
            id: 1,
            instance: g.id.instance.clone(),
            source: g.source,
            channel: g.channel,
            guard: g.id.clone(),
            bytes: bytes.clone(),
            expected: vec![],
        };
        let mut checkpoint = session.checkpoint_template().unwrap();
        checkpoint.image.observations.push(observation.clone());
        checkpoint.image.watermark = 1;
        assert_eq!(
            session.settle_ingress(permit, observation.clone(), checkpoint),
            PersistResult::Refused(Fault::Capacity)
        );
        // Changed caller objects cannot overwrite the host-owned continuation.
        observation.bytes = Bytes::new(vec![]);
        if mode == 1 {
            std::fs::create_dir(root.join("trusted/replica.next")).unwrap();
            assert_eq!(
                session.retain_pending_ingress_loss(&g.id),
                PersistResult::Unknown
            );
            assert_eq!(
                session.retry_ingress(&g.id),
                PersistResult::Refused(Fault::Unavailable)
            );
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Pending
            );
        } else if mode == 2 {
            assert!(matches!(
                session.retain_pending_ingress_loss(&g.id),
                PersistResult::Committed { .. }
            ));
            let LoadResult::Validated(loaded) = session.load() else {
                panic!("compact loss");
            };
            let (_, (_, (_, observed))): (String, (ReceiveGuard, (u64, Bytes))) = decode(
                loaded.image.permanent_loss[0].value(),
                &loaded.image.limits.decode,
            )
            .unwrap();
            assert_eq!(observed.value(), bytes.digest());
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Closed
            );
        } else {
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Pending
            );
        }
        use std::io::Write;
        println!("rem2-ready");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }
    use std::io::Write;
    println!("rem2-ready");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn corrected_denials_and_oversized_loss_custody_survive_actual_process_death() {
    use std::io::BufRead;
    for mode in 0..5 {
        let root = root(&format!("rem2-process-{mode}"));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("IC3_REM2_ROOT", &root)
            .env("IC3_REM2_MODE", mode.to_string())
            .args([
                "--exact",
                "ic3_disk_remediation2::remediation2_child",
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
                if line.unwrap() == "rem2-ready" {
                    send.send(()).unwrap();
                    return;
                }
            }
        });
        let ready = receive.recv_timeout(std::time::Duration::from_secs(10));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        ready.expect("exact corrected public custody ready before SIGKILL");
        if mode == 1 {
            std::fs::remove_dir(root.join("trusted/replica.next")).unwrap();
        }
        let f = if mode >= 3 {
            ic3_disk_fixture::disk_profile()
        } else {
            Fixture::new()
        };
        let digest =
            Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
        let mut host = DiskHost::new(evidence(&f), config(&root, false));
        let mut session = host.open_replica(request(&f, digest)).unwrap();
        let LoadResult::Validated(loaded) = session.load() else {
            panic!("honest restored custody");
        };
        if mode >= 3 {
            assert_eq!(loaded.image.recovery.attempts.len(), 1);
            assert!(matches!(
                loaded.image.recovery.attempts[0].phase,
                AttemptPhase::Terminal(TerminalOutcome::NonCommit { .. })
            ));
            assert!(loaded.image.state.instances[&f.query.instance]
                .accepted
                .is_empty());
            assert!(
                session
                    .issue_challenge(SessionRequest {
                        requester: "alice".into(),
                        writer_key: glade_node::signing::public_key(&WRITER),
                        operation: f.query.operation.canonical().digest(),
                        channel: [90; 32],
                        session_nonce: [91; 32],
                        challenge_nonce: [94; 32],
                        window: Window {
                            not_before_ms: 10,
                            not_after_ms: 100
                        }
                    })
                    .is_err(),
                "older provider cannot reset retained denial"
            );
        } else {
            assert_eq!(loaded.image.permanent_loss.len(), usize::from(mode == 2));
            let cut = session.reconstruction_request().unwrap();
            let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
                panic!("cut");
            };
            assert!(!cut.complete_local);
            assert_eq!(cut.guards.len(), usize::from(mode != 2));
            assert_eq!(
                session.retain_pending_ingress_loss(&guard(&loaded).id),
                PersistResult::Refused(Fault::Ownership),
                "restart cannot manufacture live drained custody"
            );
        }
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn earlier_verify_and_seal_replies_do_not_admit_after_their_retained_cut_advances() {
    for threshold in [3, 5] {
        for expiry in [false, true] {
            let f = ic3_disk_fixture::disk_profile();
            let root = root(&format!("r6-helper-{threshold}-{expiry}"));
            let s = sequence(
                &f,
                TimeInterval {
                    earliest_ms: 20,
                    latest_ms: 21,
                },
            );
            let mut host = DiskHost::new(provider(&f, s.clone()), config(&root, true));
            let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
            ic3_disk_fixture::local_authorization(&f, &mut session, 92);
            let mut later = s.0.lock().unwrap().2.clone();
            if expiry {
                later.time = Some(TimeInterval {
                    earliest_ms: 1001,
                    latest_ms: 1002,
                });
            } else {
                let mut p: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
                p.generation = 1;
                p.previous = Some(envelope(&f.policy).digest());
                p.revoked.push(glade_node::signing::public_key(&NODE));
                later.policy = signed(&ROOT, &p);
            }
            arm(&s, threshold, later);
            let mut c = Candidate {
                instance: f.query.instance.clone(),
                operation: f.query.operation.clone(),
                evidence: f.query.evidence.clone(),
            };
            c.evidence.admission = None;
            let outcomes = session.submit_local(c, [90; 32], [91; 32]).unwrap();
            assert!(!outcomes
                .iter()
                .any(|x| matches!(x, Outcome::AcceptedLocal(_))));
            let LoadResult::Validated(loaded) = session.load() else {
                panic!("selected latest cut before exact callback");
            };
            assert!(loaded.image.state.instances[&f.query.instance]
                .accepted
                .is_empty());
            assert!(
                loaded.image.recovery.attempts.is_empty(),
                "stale query never creates Prepare"
            );
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Closed
            );
            drop(session);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
