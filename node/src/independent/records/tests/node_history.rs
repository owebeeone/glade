//! Exact original remote receipts remain part of complete committed custody.
use super::*;
#[tokio::test]
async fn full_inventory_keeps_two_native_original_receipts_for_one_operation() {
    let mut f = ic3_disk_fixture::disk_profile();
    let second_seed = [34; 32];
    let second_key = crate::signing::public_key(&second_seed);
    let nodes = vec![crate::signing::public_key(&NODE), second_key];
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.admitting_nodes = nodes.clone();
    policy.replica_holders = nodes.clone();
    f.policy = signed(&ROOT, &policy);
    let wrapper: PermitWrapper =
        decode_profile(f.query.evidence.permit.value(), &limits()).unwrap();
    let mut permit: PermitBody = decode_signed_body(&wrapper.signed_permit, &limits()).unwrap();
    permit.admitting_nodes = nodes;
    permit.policy = envelope(&f.policy).digest();
    let signed_permit = signed(&ROOT, &permit);
    let mut proof: OperationProofBody =
        decode_signed_body(&wrapper.writer_proof, &limits()).unwrap();
    proof.signed_permit = envelope(&signed_permit).digest();
    f.query.evidence.policy = envelope(&f.policy);
    f.query.evidence.admission = None;
    f.query.evidence.permit = encode_profile(
        &PermitWrapper {
            signed_permit,
            writer_proof: signed(&WRITER, &proof),
        },
        &limits(),
    )
    .unwrap();
    let roots = [root("c-history-a"), root("c-history-b")];
    let mut first_host = DiskHost::new(evidence(&f), config(&roots[0], true));
    let mut second_evidence = evidence(&f);
    second_evidence.config.node_seed = second_seed;
    let mut second_config = config(&roots[1], true);
    second_config.registration = [77; 32];
    let mut second_host = DiskHost::new(second_evidence, second_config);
    let mut first = first_host.open_replica(request(&f, [0; 32])).unwrap();
    let mut second_request = request(&f, [0; 32]);
    second_request.owner.store = StoreIncarnation([74; 32]);
    second_request.plan_namespace = [75; 32];
    second_request.invocation_namespace = [76; 32];
    let mut second = second_host.open_replica(second_request).unwrap();
    for session in [&mut first, &mut second] {
        ic3_disk_fixture::local_authorization(&f, session, 92);
        session
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
    }
    let original_a = first.node_history().unwrap().remove(0);
    let original_b = second.node_history().unwrap().remove(0);
    assert_eq!(
        original_a.candidate.operation,
        original_b.candidate.operation
    );
    assert_ne!(
        original_a.receipt, original_b.receipt,
        "each actual node sealed its own original admission"
    );
    let inventory = second
        .node_inventory(std::slice::from_ref(&original_b))
        .unwrap();
    let channel = [99; 32];
    let header = second
        .node_inventory_header(&inventory, crate::signing::public_key(&NODE), channel)
        .unwrap();
    let raw = encode(&(header, inventory.clone()), &limits()).unwrap();
    retain(
        &mut first,
        second_key,
        channel,
        "inventory",
        raw,
        inventory.entries.clone(),
    )
    .await;
    let raw = encode_profile(&original_b, &limits()).unwrap();
    let reference = super::super::node_ingress::bundle_ref(&original_b, &raw);
    retain(
        &mut first,
        second_key,
        channel,
        "bundle",
        raw,
        vec![reference.clone()],
    )
    .await;
    first
        .node_offer(
            original_b.clone(),
            &reference,
            &inventory,
            second_key,
            channel,
        )
        .unwrap();
    let all = first.node_history().unwrap();
    assert!(all.contains(&original_a));
    assert!(
        all.contains(&original_b),
        "highest/native accepted record cannot erase original peer receipt custody"
    );
    assert_eq!(all.len(), 2);
    let missing = first.node_missing(&inventory).unwrap();
    assert!(missing.entries.is_empty());
    let before = first.pair.as_ref().unwrap().image.clone();
    assert_eq!(
        ReplicaRecoverySession::close(&mut first),
        CloseResult::Closed
    );
    drop(first);
    let digest =
        Bytes::new(std::fs::read(roots[0].join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&roots[0], false));
    let mut restored = host.open_replica(request(&f, digest)).unwrap();
    assert_eq!(restored.node_history().unwrap(), all);
    assert!(
        restored.pair.as_ref().unwrap().image.state == before.state,
        "native State and original receipts are unchanged on reopen"
    );
    assert!(restored.pair.as_ref().unwrap().image.observations == before.observations);
    assert!(restored.pair.as_ref().unwrap().image.obligations == before.obligations);
    assert_eq!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    );
    drop(restored);
    drop(second);
    for root in roots {
        std::fs::remove_dir_all(root).unwrap();
    }
}
async fn retain(
    session: &mut DiskSession<Source>,
    source: Digest,
    channel: Digest,
    role: &str,
    raw: Bytes,
    expected: Vec<RecordRef>,
) {
    let guard = session.node_guard(source, channel, role, 1000).unwrap();
    let permit = session.begin_ingress(guard).unwrap();
    let bytes = raw.clone();
    let (permit, result) = session
        .receive_ingress(permit, || async move { Ok(bytes) })
        .unwrap()
        .await;
    assert_eq!(result, Ok(raw.clone()));
    session.finish_node_input(permit, raw, expected).unwrap();
}
#[tokio::test]
async fn peer_bundle_cannot_settle_without_earlier_exact_full_manifest() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-no-manifest");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    let original = session.node_history().unwrap().remove(0);
    let raw = encode_profile(&original, &limits()).unwrap();
    let reference = super::super::node_ingress::bundle_ref(&original, &raw);
    let guard = session
        .node_guard(crate::signing::public_key(&NODE), [99; 32], "bundle", 1000)
        .unwrap();
    let id = guard.id.clone();
    let permit = session.begin_ingress(guard).unwrap();
    let bytes = raw.clone();
    let (permit, result) = session
        .receive_ingress(permit, || async move { Ok(bytes) })
        .unwrap()
        .await;
    assert_eq!(result, Ok(raw.clone()));
    assert_eq!(
        session.finish_node_input(permit, raw, vec![reference]),
        Err(Fault::BindingMismatch),
        "native validity does not substitute for exact earlier authenticated manifest custody"
    );
    assert!(session.pending_ingress.contains_key(&id));
    assert!(matches!(
        session.retain_pending_ingress_loss(&id),
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
async fn promoted_child_keeps_original_candidate_custody_after_ancestor_arrival() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-candidate-ancestor");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let parent = super::kinds::offered(
        &f,
        br#"{"after":null,"atom_id":"a","kind":"insert","text":"A"}"#,
        0,
    );
    let mut operation = parent.operation.inner().clone();
    operation.seq = 1;
    operation.lamport = 1;
    operation.prev = Some(parent.operation.canonical().digest().to_vec());
    operation.payload = br#"{"after":"a","atom_id":"b","kind":"insert","text":"B"}"#.to_vec();
    let operation = Operation::new(operation);
    let child = Candidate {
        instance: f.query.instance.clone(),
        evidence: Fixture::proofs(
            &f.query.instance,
            &f.identity,
            &f.descriptor,
            &f.certificate,
            &f.policy,
            &operation,
            Some((crate::signing::public_key(&NODE), &NODE)),
        ),
        operation,
    };
    let effects =
        super::super::runtime::event(&mut session, Event::OfferReplica(Box::new(child.clone())))
            .unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    let raw_child = session
        .node_history()
        .unwrap()
        .into_iter()
        .find(|value| value.kind == RecordKind::Candidate)
        .unwrap();
    let inventory = session
        .node_inventory(std::slice::from_ref(&raw_child))
        .unwrap();
    let source = crate::signing::public_key(&NODE);
    let channel = [98; 32];
    let header = session
        .node_inventory_header(&inventory, source, channel)
        .unwrap();
    retain(
        &mut session,
        source,
        channel,
        "inventory",
        encode(&(header, inventory.clone()), &limits()).unwrap(),
        inventory.entries.clone(),
    )
    .await;
    let raw = encode_profile(&raw_child, &limits()).unwrap();
    let reference = super::super::node_ingress::bundle_ref(&raw_child, &raw);
    retain(
        &mut session,
        source,
        channel,
        "bundle",
        raw,
        vec![reference.clone()],
    )
    .await;
    session.classify_node_inputs().unwrap();
    let effects =
        super::super::runtime::event(&mut session, Event::OfferReplica(Box::new(parent))).unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    session.classify_node_inputs().unwrap();
    let image = &session.pair.as_ref().unwrap().image;
    assert_eq!(image.state.instances[&f.query.instance].accepted.len(), 2);
    assert!(image.state.instances[&f.query.instance]
        .candidates
        .is_empty());
    assert!(
        image
            .obligations
            .iter()
            .all(|value| value.classification == Classification::Retained(reference.bundle)),
        "promotion cannot forget exact previously guarded child candidate identity"
    );
    assert!(session.node_history().unwrap().contains(&raw_child));
    let request = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(request) else {
        panic!("promoted ancestry cut");
    };
    assert!(cut.complete_local);
    assert_eq!(cut.kernel.eligible.len(), 2);
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn signed_full_inventory_wrong_total_cannot_claim_complete_known_cut() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-manifest-total");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    let records = session.node_history().unwrap();
    let mut inventory = session.node_inventory(&records).unwrap();
    inventory.total_bytes += 1;
    let source = crate::signing::public_key(&NODE);
    let channel = [97; 32];
    let header = session
        .node_inventory_header(&inventory, source, channel)
        .unwrap();
    retain(
        &mut session,
        source,
        channel,
        "inventory",
        encode(&(header, inventory.clone()), &limits()).unwrap(),
        inventory.entries.clone(),
    )
    .await;
    session.classify_node_inputs().unwrap();
    let request = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(request) else {
        panic!("retained inconsistent manifest cut");
    };
    assert!(
        !cut.complete_local,
        "authenticated wrong byte total cannot settle a truthful complete full cut"
    );
    assert_eq!(
        cut.kernel.eligible.len(),
        1,
        "valid original app receipt keeps its exact strength"
    );
    assert!(session
        .pair
        .as_ref()
        .unwrap()
        .image
        .obligations
        .iter()
        .all(|value| value.classification == Classification::Missing));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn empty_full_manifest_cannot_claim_unaccounted_payload_bytes() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-empty-total");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let mut inventory = session.node_inventory(&[]).unwrap();
    inventory.total_bytes = 1;
    let source = crate::signing::public_key(&NODE);
    let channel = [96; 32];
    let header = session
        .node_inventory_header(&inventory, source, channel)
        .unwrap();
    let raw = encode(&(header, inventory), &limits()).unwrap();
    let guard = session
        .node_guard(source, channel, "inventory", 1000)
        .unwrap();
    let id = guard.id.clone();
    let permit = session.begin_ingress(guard).unwrap();
    let bytes = raw.clone();
    let (permit, result) = session
        .receive_ingress(permit, || async move { Ok(bytes) })
        .unwrap()
        .await;
    assert_eq!(result, Ok(raw.clone()));
    assert_eq!(
        session.finish_node_input(permit, raw, vec![]),
        Err(Fault::Integrity)
    );
    assert!(session.pending_ingress.contains_key(&id));
    assert!(matches!(
        session.retain_pending_ingress_loss(&id),
        PersistResult::Committed { .. }
    ));
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

// This producer uses genuine accepted B runtime/provider paths, then the actual
// production binary owns all automatic peer transfer. No client ferries history.
#[tokio::test(flavor = "multi_thread")]
async fn actual_binaries_exchange_ancestry_and_security_evidence_both_arrival_orders() {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    struct Process {
        child: Child,
        lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        readers: Vec<std::thread::JoinHandle<()>>,
    }
    impl Process {
        fn start(profile: &std::path::Path, port: u16) -> Self {
            let binary =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/glade-node");
            let mut child = Command::new(binary)
                .env_clear()
                .args([
                    "--independent-config",
                    profile.to_str().unwrap(),
                    &port.to_string(),
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let mut readers = Vec::new();
            let outputs: Vec<Box<dyn std::io::Read + Send>> = vec![
                Box::new(child.stdout.take().unwrap()),
                Box::new(child.stderr.take().unwrap()),
            ];
            for output in outputs {
                let owned = lines.clone();
                readers.push(std::thread::spawn(move || {
                    for line in BufReader::new(output).lines() {
                        match line {
                            Ok(line) => owned.lock().unwrap().push(line),
                            Err(_) => {
                                break;
                            }
                        }
                    }
                }));
            }
            println!(
                "actual ancestry node pid={} profile={}",
                child.id(),
                profile.display()
            );
            Self {
                child,
                lines,
                readers,
            }
        }
        async fn wait(&mut self, prefix: &str) {
            let result = tokio::time::timeout(std::time::Duration::from_secs(8), async {
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
                        panic!(
                            "actual node pid {} exited {status}: {:?}",
                            self.child.id(),
                            self.lines.lock().unwrap()
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;
            assert!(
                result.is_ok(),
                "actual node missing {prefix}: {:?}",
                self.lines.lock().unwrap()
            );
        }
        async fn synchronized(a: &Self, b: &Self, require_complete: bool) {
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                loop {
                    let owned = [
                        a.lines.lock().unwrap().clone(),
                        b.lines.lock().unwrap().clone(),
                    ];
                    let latest: Vec<_> = owned
                        .iter()
                        .map(|lines| {
                            lines
                                .iter()
                                .rev()
                                .find_map(|line| line.strip_prefix("independent idle "))
                                .map(str::to_owned)
                        })
                        .collect();
                    if latest[0].is_some()
                        && latest[0] == latest[1]
                        && (!require_complete
                            || latest[0]
                                .as_ref()
                                .is_some_and(|line| line.ends_with(" complete=true")))
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("actual ancestry/security settled matching idle cut before shutdown");
        }
        async fn stop(&mut self) {
            assert!(Command::new("/bin/kill")
                .env_clear()
                .args(["-TERM", &self.child.id().to_string()])
                .status()
                .unwrap()
                .success());
            let status = tokio::time::timeout(std::time::Duration::from_secs(8), async {
                loop {
                    if let Some(status) = self.child.try_wait().unwrap() {
                        break status;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            for reader in self.readers.drain(..) {
                reader.join().unwrap();
            }
            assert!(
                status.success(),
                "actual node close: {:?}",
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
    fn port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    fn profile(
        f: &Fixture,
        roots: &[std::path::PathBuf; 2],
        ports: &[u16; 2],
        index: usize,
    ) -> Vec<Bytes> {
        let other = 1 - index;
        let mut request = request(f, [0; 32]);
        if index == 1 {
            request.owner.store = StoreIncarnation([74; 32]);
            request.plan_namespace = [75; 32];
            request.invocation_namespace = [76; 32];
        }
        let seed = if index == 0 { NODE } else { [34; 32] };
        let endpoint_seed = if index == 0 { [44; 32] } else { [45; 32] };
        let remote = crate::signing::public_key(&if other == 0 { NODE } else { [34; 32] });
        let endpoint = crate::signing::public_key(&if other == 0 { [44; 32] } else { [45; 32] });
        let key: String = endpoint.iter().map(|byte| format!("{byte:02x}")).collect();
        vec![
            Bytes::new(b"glade/ic3/node-qualification/v1".to_vec()),
            encode(&request, &limits()).unwrap(),
            encode(&f.policy, &limits()).unwrap(),
            encode(&vec![f.certificate.clone()], &limits()).unwrap(),
            Bytes::new(seed.to_vec()),
            Bytes::new(endpoint_seed.to_vec()),
            encode(
                &vec![
                    roots[index].join("data").display().to_string(),
                    roots[index].join("trusted").display().to_string(),
                ],
                &limits(),
            )
            .unwrap(),
            Bytes::new(if index == 0 { [73; 32] } else { [77; 32] }.to_vec()),
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
            encode(
                &vec![(
                    remote,
                    (endpoint, format!("{key}@127.0.0.1:{}", ports[other])),
                )],
                &limits(),
            )
            .unwrap(),
            encode(&(50_u64, 1000_u64), &limits()).unwrap(),
        ]
    }
    for reverse in [false, true] {
        println!("actual ancestry matrix order={reverse} phase=initial");
        let mut f = ic3_disk_fixture::disk_profile();
        let nodes = vec![
            crate::signing::public_key(&NODE),
            crate::signing::public_key(&[34; 32]),
        ];
        let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
        policy.admitting_nodes = nodes.clone();
        policy.replica_holders = nodes;
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
        let roots = [
            root(&format!("c-real-ancestry-source-{reverse}")),
            root(&format!("c-real-ancestry-recipient-{reverse}")),
        ];
        let mut source = DiskHost::new(evidence(&f), config(&roots[0], true))
            .open_replica(request(&f, [0; 32]))
            .unwrap();
        let mut child = super::kinds::offered(
            &f,
            br#"{"after":"a","atom_id":"b","kind":"insert","text":"B"}"#,
            1,
        );
        let mut op = child.operation.inner().clone();
        op.prev = Some(f.query.operation.canonical().digest().to_vec());
        child.operation = Operation::new(op);
        child.evidence = Fixture::proofs(
            &f.query.instance,
            &f.identity,
            &f.descriptor,
            &f.certificate,
            &f.policy,
            &child.operation,
            Some((crate::signing::public_key(&NODE), &NODE)),
        );
        let security = super::kinds::offered(&f, b"invalid signed text payload", 0);
        for offered in [&child, &security] {
            let effects = super::super::runtime::event(
                &mut source,
                Event::OfferReplica(Box::new(offered.clone())),
            )
            .unwrap();
            super::super::runtime::drain(&mut source, effects).unwrap();
        }
        assert!(source
            .node_history()
            .unwrap()
            .iter()
            .any(|record| record.kind == RecordKind::Candidate && record.candidate == child));
        assert!(source
            .node_history()
            .unwrap()
            .iter()
            .any(|record| record.kind == RecordKind::Security && record.candidate == security));
        assert_eq!(
            ReplicaRecoverySession::close(&mut source),
            CloseResult::Closed
        );
        drop(source);
        let mut adapter = evidence(&f);
        adapter.config.node_seed = [34; 32];
        let mut cfg = config(&roots[1], true);
        cfg.registration = [77; 32];
        let mut host = DiskHost::new(adapter, cfg);
        let mut open = request(&f, [0; 32]);
        open.owner.store = StoreIncarnation([74; 32]);
        open.plan_namespace = [75; 32];
        open.invocation_namespace = [76; 32];
        let mut empty = host.open_replica(open.clone()).unwrap();
        assert_eq!(
            ReplicaRecoverySession::close(&mut empty),
            CloseResult::Closed
        );
        drop(empty);
        let ports = [port(), port()];
        let paths = [roots[0].join("profile"), roots[1].join("profile")];
        for (index, path) in paths.iter().enumerate() {
            std::fs::write(
                path,
                encode(&profile(&f, &roots, &ports, index), &limits())
                    .unwrap()
                    .value(),
            )
            .unwrap();
        }
        let mut first = Process::start(&paths[usize::from(reverse)], ports[usize::from(reverse)]);
        first.wait("listening ").await;
        let mut second =
            Process::start(&paths[usize::from(!reverse)], ports[usize::from(!reverse)]);
        second.wait("listening ").await;
        if reverse {
            first.wait("independent exchanged ").await;
        } else {
            second.wait("independent exchanged ").await;
        }
        Process::synchronized(&first, &second, false).await;
        first.stop().await;
        second.stop().await;
        for (index, root_path) in roots.iter().enumerate() {
            let digest =
                Bytes::new(std::fs::read(root_path.join("trusted/replica.floor")).unwrap())
                    .digest();
            let mut adapter = evidence(&f);
            adapter.config.node_seed = if index == 0 { NODE } else { [34; 32] };
            let mut cfg = config(root_path, false);
            if index == 1 {
                cfg.registration = [77; 32];
            }
            let mut host = DiskHost::new(adapter, cfg);
            let open = if index == 0 {
                request(&f, digest)
            } else {
                let mut value = open.clone();
                value.expected_floor = digest;
                value
            };
            let mut session = host.open_replica(open.clone()).unwrap();
            let LoadResult::Validated(value) = session.load() else {
                panic!("actual first-round custody");
            };
            let state = &value.image.state.instances[&f.query.instance];
            assert!(state.candidates.contains(&child));
            assert!(state.security_evidence.contains(&security));
            assert!(state.accepted.is_empty());
            if index == 0 {
                ic3_disk_fixture::local_authorization(&f, &mut session, 92);
                let outcomes = session
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
                session.classify_node_inputs().unwrap();
                let LoadResult::Validated(local) = session.load() else {
                    panic!("source ancestor State");
                };
                assert_eq!(
                    local.image.state.instances[&f.query.instance]
                        .accepted
                        .len(),
                    2,
                    "native ancestor outcomes {outcomes:?}"
                );
            }
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Closed
            );
            drop(session);
        }
        println!("actual ancestry matrix order={reverse} phase=ancestor-heal");
        let mut first = Process::start(&paths[usize::from(!reverse)], ports[usize::from(!reverse)]);
        first.wait("listening ").await;
        let mut second = Process::start(&paths[usize::from(reverse)], ports[usize::from(reverse)]);
        second.wait("listening ").await;
        if reverse {
            second.wait("independent exchanged ").await;
        } else {
            first.wait("independent exchanged ").await;
        }
        Process::synchronized(&first, &second, true).await;
        first.stop().await;
        second.stop().await;
        let mut histories = Vec::new();
        for (index, root_path) in roots.iter().enumerate() {
            let digest =
                Bytes::new(std::fs::read(root_path.join("trusted/replica.floor")).unwrap())
                    .digest();
            let mut adapter = evidence(&f);
            adapter.config.node_seed = if index == 0 { NODE } else { [34; 32] };
            let mut cfg = config(root_path, false);
            if index == 1 {
                cfg.registration = [77; 32];
            }
            let mut host = DiskHost::new(adapter, cfg);
            let open = if index == 0 {
                request(&f, digest)
            } else {
                let mut value = open.clone();
                value.expected_floor = digest;
                value
            };
            let mut session = host.open_replica(open).unwrap();
            let LoadResult::Validated(value) = session.load() else {
                panic!("actual ancestor heal");
            };
            let state = &value.image.state.instances[&f.query.instance];
            assert_eq!(state.accepted.len(), 2);
            assert!(state.security_evidence.contains(&security));
            let cut = session.reconstruct_cut(session.reconstruction_request().unwrap());
            let ReconstructResult::Cut(cut) = cut else {
                panic!("actual combined ancestry cut");
            };
            assert!(cut.complete_local);
            assert_eq!(cut.kernel.eligible.len(), 2);
            let history = session.node_history().unwrap();
            assert!(history
                .iter()
                .any(|item| item.kind == RecordKind::Candidate && item.candidate == child));
            assert!(history
                .iter()
                .any(|item| item.kind == RecordKind::Security && item.candidate == security));
            histories.push(history);
            assert_eq!(
                ReplicaRecoverySession::close(&mut session),
                CloseResult::Closed
            );
            drop(session);
        }
        assert!(
            histories[0] == histories[1],
            "actual duplex preserves full candidate, security and original ancestor identities"
        );
        for root in roots {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
