use super::*;
#[tokio::test]
async fn public_refused_drained_settlement_can_select_conservative_loss_and_close() {
    let f = Fixture::new();
    let root = root("r4-public-malformed");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated")
    };
    let incoming = guard(&loaded);
    let bytes = Bytes::new(b"malformed inventory".to_vec());
    let permit = session.begin_ingress(incoming.clone()).unwrap();
    let (permit, _) = session
        .receive_ingress(permit, || async { Ok(bytes.clone()) })
        .unwrap()
        .await;
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
    assert!(matches!(
        session.settle_ingress(permit, observation, checkpoint),
        PersistResult::Refused(_)
    ));
    assert!(
        matches!(
            session.retain_pending_ingress_loss(&incoming.id),
            PersistResult::Committed { .. }
        ),
        "public host-owned continuation must reach conservative loss without private map extraction"
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
        panic!("retained loss")
    };
    assert_eq!(loaded.image.permanent_loss.len(), 1);
    let cut = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
        panic!("cut")
    };
    assert!(!cut.complete_local);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "owned public ingress continuation crash child"]
async fn public_continuation_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_PUBLIC_LOSS_ROOT").unwrap());
    let mode = std::env::var("IC3_PUBLIC_LOSS_MODE")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let f = if mode == 0 || mode >= 3 {
        Fixture::new()
    } else {
        ic3_disk_fixture::disk_profile_with(|limit| {
            limit.storage.critical_bytes = limit.image_bytes - 1;
        })
    };
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated")
    };
    let incoming = guard(&loaded);
    let mut observation = Observation {
        id: 1,
        instance: incoming.id.instance.clone(),
        source: incoming.source,
        channel: incoming.channel,
        guard: incoming.id.clone(),
        bytes: inventory(&f, &incoming, vec![]),
        expected: vec![],
    };
    let received = match mode {
        0 => Ok(Bytes::new(b"malformed actually received".to_vec())),
        3 => Ok(inventory(&f, &incoming, vec![reference()])),
        4 => Err(Fault::Unavailable),
        _ => Ok(observation.bytes.clone()),
    };
    let permit = session.begin_ingress(incoming.clone()).unwrap();
    let (permit, _) = session
        .receive_ingress(permit, || async { received })
        .unwrap()
        .await;
    let mut checkpoint = session.checkpoint_template().unwrap();
    checkpoint.image.observations.push(observation.clone());
    checkpoint.image.watermark = 1;
    let refused = session.settle_ingress(permit, observation.clone(), checkpoint);
    if mode == 0 || mode >= 3 {
        assert!(matches!(
            refused,
            PersistResult::Refused(Fault::BindingMismatch)
        ));
    } else {
        assert!(
            matches!(refused, PersistResult::Refused(Fault::Capacity)),
            "legitimate received inventory exceeds aggregate ordinary image capacity"
        );
        if mode == 1 {
            std::fs::create_dir(root.join("trusted/replica.next")).unwrap();
        }
        let loss = session.retain_pending_ingress_loss(&incoming.id);
        if mode == 1 {
            assert_eq!(loss, PersistResult::Unknown);
            assert!(matches!(
                session.retry_ingress(&incoming.id),
                PersistResult::Refused(Fault::Unavailable)
            ));
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Pending
            );
        } else {
            assert!(matches!(loss, PersistResult::Committed { .. }));
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Closed
            );
        }
    }
    observation.bytes = Bytes::new(vec![]); // Caller objects cannot alter retained host custody.
    use std::io::Write;
    println!("public-loss-ready");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn public_refusal_unknown_and_capacity_loss_survive_actual_process_death() {
    use std::io::BufRead;
    for mode in 0..5 {
        let root = root(&format!("r4-public-process-{mode}"));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("IC3_PUBLIC_LOSS_ROOT", &root)
            .env("IC3_PUBLIC_LOSS_MODE", mode.to_string())
            .args([
                "--exact",
                "ic3_disk_remediation::loss::public_continuation_child",
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
                if line.unwrap() == "public-loss-ready" {
                    send.send(()).unwrap();
                    return;
                }
            }
        });
        let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        ready.expect("public-only retained ingress continuation reaches exact outcome");
        if mode == 1 {
            std::fs::remove_dir(root.join("trusted/replica.next")).unwrap();
        }
        let f = if mode == 0 || mode >= 3 {
            Fixture::new()
        } else {
            ic3_disk_fixture::disk_profile_with(|limit| {
                limit.storage.critical_bytes = limit.image_bytes - 1;
            })
        };
        let digest =
            Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
        let mut host = DiskHost::new(evidence(&f), config(&root, false));
        let mut session = host.open_replica(request(&f, digest)).unwrap();
        let LoadResult::Validated(loaded) = session.load() else {
            panic!("truthful cut")
        };
        assert_eq!(loaded.image.permanent_loss.len(), usize::from(mode == 2));
        let cut = session.reconstruction_request().unwrap();
        let ReconstructResult::Cut(cut) = session.reconstruct_cut(cut) else {
            panic!("cut")
        };
        assert!(!cut.complete_local);
        assert_eq!(cut.guards.len(), usize::from(mode != 2));
        assert!(
            matches!(
                session.retain_pending_ingress_loss(&GuardId {
                    store: loaded.authoritative_floor.owner.store,
                    instance: loaded.image.instance,
                    number: 0
                }),
                PersistResult::Refused(Fault::Ownership)
            ),
            "restart cannot remint an in-memory permit from a durable active guard"
        );
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}
