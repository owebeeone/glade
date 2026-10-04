use super::*;
fn prepared(session: &mut DiskSession<Source>, f: &Fixture) -> BeginRequest {
    ic3_disk_fixture::local_authorization(f, session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    let mut effects =
        super::super::runtime::event(session, Event::SubmitLocal(Box::new(candidate))).unwrap();
    loop {
        let effect = effects
            .into_iter()
            .find(|effect| {
                matches!(
                    effect,
                    Effect::Verify(_) | Effect::Seal(_) | Effect::Prepare { .. } | Effect::Begin(_)
                )
            })
            .unwrap();
        let event = match effect {
            Effect::Verify(query) => {
                Event::EvidenceResolved(Box::new(glade_crdt_evidence_api::EvidencePort::verify(
                    &mut session.evidence,
                    glade_crdt_evidence_api::BoundQuery {
                        query,
                        identity: f.identity.clone(),
                    },
                )))
            }
            Effect::Seal(query) => {
                Event::EvidenceResolved(Box::new(glade_crdt_evidence_api::EvidencePort::seal(
                    &mut session.evidence,
                    glade_crdt_evidence_api::BoundQuery {
                        query,
                        identity: f.identity.clone(),
                    },
                )))
            }
            Effect::Prepare { request, .. } => Event::PrepareResolved(session.prepare(request)),
            Effect::Begin(request) => return request,
            _ => panic!("bounded original effects"),
        };
        effects = super::super::runtime::event(session, event).unwrap();
    }
}
#[test]
fn close_retains_actual_reserved_plan_and_stops_new_input_until_original_callback_drains() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("close-reserved");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let begin = prepared(&mut session, &f);
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending,
        "unresolved original native callback keeps root ownership"
    );
    let reply = session.begin(begin);
    assert!(matches!(reply.result, ResolveResult::Terminal(_)));
    let effects = super::super::runtime::event(&mut session, Event::CommitResolved(reply)).unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

fn changed_policy(session: &mut DiskSession<Source>, f: &Fixture) {
    let mut body: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    body.generation = 1;
    body.previous = Some(envelope(&f.policy).digest());
    body.revoked = vec![crate::signing::public_key(&WRITER)];
    session.evidence.source.0.policy = signed(&ROOT, &body);
    super::super::authentication::persist(session).unwrap();
}
#[test]
fn actual_reserved_fence_wins_and_late_exact_or_contrary_callbacks_never_reopen_terminal() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("reserved-fence");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let begin = prepared(&mut session, &f);
    changed_policy(&mut session, &f);
    let state = &session.pair.as_ref().unwrap().image.state;
    // ObservePolicy now drives its selected Fence callback automatically;
    // recover the same original request whether still live or retired.
    let fence = state
        .storage_invocations
        .values()
        .chain(state.retired_storage_invocations.values())
        .find_map(|invocation| match invocation {
            StorageInvocation::Resolve(request) if request.operation == OperationKind::Fence => {
                Some(request.clone())
            }
            _ => None,
        })
        .expect("original core-issued fence");
    let reply = session.request_fence(fence.clone());
    assert!(
        matches!(reply.result, ResolveResult::Terminal(ref terminal) if matches!(terminal.outcome, TerminalOutcome::NonCommit { .. }))
    );
    let effects =
        super::super::runtime::event(&mut session, Event::FenceResolved(reply.clone())).unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    let late = session.begin(begin);
    assert_eq!(
        late.result, reply.result,
        "late Begin retrieves same original immutable NonCommit"
    );
    let effects = super::super::runtime::event(&mut session, Event::CommitResolved(late)).unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    let mut contrary = reply.clone();
    let ResolveResult::Terminal(terminal) = &mut contrary.result else {
        panic!("original terminal");
    };
    terminal.outcome = TerminalOutcome::Committed {
        revision: fence.attempt.binding.expected_revision + 1,
        storage: fence.attempt.binding.storage.clone(),
        receipts: fence.attempt.binding.receipts.clone(),
        start_cut: fence
            .attempt
            .binding
            .prestart
            .as_ref()
            .map(|p| p.cut.clone()),
    };
    let effects =
        super::super::runtime::event(&mut session, Event::FenceResolved(contrary)).unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("retained genuine fence + authenticated contrary diagnostic");
    };
    assert!(loaded.image.state.instances[&f.query.instance].storage_integrity_failed);
    assert!(loaded.image.state.instances[&f.query.instance]
        .accepted
        .is_empty());
    assert_eq!(session.request_fence(fence).result, reply.result);
    assert_eq!(loaded.image.state.terminal_conflicts.len(), 1);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn actual_killed_protected_started_cut_commits_original_receipt_despite_current_revocation() {
    use std::io::BufRead;
    let f = ic3_disk_fixture::disk_profile();
    let root = root("started-revocation");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .env("IC3_NATIVE_ROOT", &root)
        .env("IC3_NATIVE_STAGE", "1")
        .env("IC3_NATIVE_CUT", "18")
        .args([
            "--exact",
            "independent::records::tests::content::native_cuts::native_cut_child",
            "--ignored",
            "--nocapture",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = crate::test_support::children::spawn(&mut command).unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap() == "native-cut-ready" {
                send.send(()).unwrap();
                return;
            }
        }
    });
    let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    ready.expect("actual Started selected before kill");
    let mut provider = evidence(&f);
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.generation = 1;
    policy.previous = Some(envelope(&f.policy).digest());
    policy.revoked = vec![crate::signing::public_key(&WRITER)];
    provider.source.0.policy = signed(&ROOT, &policy);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(provider, config(&root, false));
    let mut session = host
        .open_replica(request(&f, digest))
        .expect("protected Started replays original continuation under newer revoked policy");
    super::super::authentication::persist(&mut session).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("historical original receipt after genuine revocation");
    };
    let binding = &loaded.image.recovery.attempts[0].attempt.binding;
    assert_eq!(loaded.authoritative_floor.issuance.policy_generation, 1);
    assert_eq!(
        loaded.image.state.instances[&f.query.instance].accepted[0].receipt,
        binding.receipts[0]
    );
    assert!(
        matches!(loaded.image.recovery.attempts[0].phase, AttemptPhase::Terminal(TerminalOutcome::Committed { ref start_cut, .. }) if *start_cut == binding.prestart.as_ref().map(|p| p.cut.clone()))
    );
    let original = loaded
        .image
        .state
        .retired_storage_invocations
        .values()
        .find_map(|invocation| match invocation {
            StorageInvocation::Resolve(request) if request.operation == OperationKind::Begin => {
                Some(request.clone())
            }
            _ => None,
        })
        .unwrap();
    let late = session.begin(BeginRequest {
        request: original,
        current: LocalPrecondition {
            policy: [255; 32],
            time: TimeInterval {
                earliest_ms: 0,
                latest_ms: u64::MAX,
            },
        },
    });
    assert!(
        matches!(late.result, ResolveResult::Terminal(ref terminal) if terminal.outcome == match &loaded.image.recovery.attempts[0].phase { AttemptPhase::Terminal(outcome) => outcome.clone(), _ => unreachable!() })
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
