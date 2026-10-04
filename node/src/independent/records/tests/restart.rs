//! Actual OS termination with terminal ahead of cached kernel callback.
use super::*;
use std::io::{BufRead, Write};
#[test]
#[ignore = "owned child of genuine native terminal restart witness"]
fn terminal_before_callback_child() {
    let f = ic3_disk_fixture::disk_profile();
    let stop = std::env::var("IC3_NATIVE_STOP").unwrap_or_else(|_| "terminal".into());
    let root = std::path::PathBuf::from(std::env::var_os("IC3_NATIVE_ROOT").unwrap());
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    let mut effects =
        super::super::runtime::event(&mut session, Event::SubmitLocal(Box::new(candidate)))
            .unwrap();
    loop {
        if stop == "seal-query"
            && effects
                .iter()
                .any(|effect| matches!(effect, Effect::Seal(_)))
        {
            println!("native-terminal-ready");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
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
            Effect::Prepare { request, .. } => {
                Event::PrepareResolved(super::super::storage::prepare(&mut session, request))
            }
            Effect::Begin(request) => {
                let reply = super::super::storage::begin(&mut session, request);
                assert!(matches!(reply.result, ResolveResult::Terminal(_)));
                println!("native-terminal-ready");
                std::io::stdout().flush().unwrap();
                loop {
                    std::thread::park();
                }
            }
            _ => panic!("unexpected effect"),
        };
        effects = super::super::runtime::event(&mut session, event).unwrap();
    }
}
#[test]
fn actual_process_kill_recovers_selected_terminal_through_original_core_callback_before_load() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("terminal-before-callback");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .env("IC3_NATIVE_ROOT", &root)
        .args([
            "--exact",
            "independent::records::tests::content::restart::terminal_before_callback_child",
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
            if line.unwrap() == "native-terminal-ready" {
                send.send(()).unwrap();
                return;
            }
        }
    });
    let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    ready.expect("actual signed provider reaches selected terminal before callback");
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host
        .open_replica(request(&f, digest))
        .expect("recover selected terminal and original issued request");
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated original terminal replay");
    };
    assert_eq!(
        loaded.image.state.instances[&f.query.instance].revision, 1,
        "before any usable read, replay exact original committed callback"
    );
    assert_eq!(
        loaded.image.state.instances[&f.query.instance]
            .accepted
            .len(),
        1
    );
    assert_eq!(loaded.image.state.terminal_results.len(), 1);
    let charges = loaded.image.state.instances[&f.query.instance].used.clone();
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    crate::test_support::children::settled();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let LoadResult::Validated(twice) = session.load() else {
        panic!("second reopen validates same accounting");
    };
    assert_eq!(twice.image.state.instances[&f.query.instance].used, charges);
    assert_eq!(
        twice.image.state.instances[&f.query.instance]
            .accepted
            .len(),
        1
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn actual_process_kill_recovers_original_owned_seal_continuation_without_new_link_authority() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("seal-query");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .env("IC3_NATIVE_ROOT", &root)
        .env("IC3_NATIVE_STOP", "seal-query")
        .args([
            "--exact",
            "independent::records::tests::content::restart::terminal_before_callback_child",
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
            if line.unwrap() == "native-terminal-ready" {
                send.send(()).unwrap();
                return;
            }
        }
    });
    let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    ready.expect("actual verified/seal request durably selected before kill");
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("genuine owned seal continuation recovery");
    };
    assert_eq!(
        loaded.image.state.instances[&f.query.instance]
            .accepted
            .len(),
        1,
        "restore original scoped signed possession/verification continuation, no harness Facts"
    );
    assert!(loaded.image.state.queries.is_empty());
    let candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    assert_eq!(
        session.submit_local(candidate, [90; 32], [91; 32]),
        Err(Fault::Ownership),
        "recovery never grants a newly connected writer generic cached authorization"
    );
    assert!(session.evidence.authorized.is_empty());
    assert!(session.evidence.sealable.is_empty());
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
