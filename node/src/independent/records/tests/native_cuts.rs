//! The actual signed host path at every paired native attempt cut.
use super::*;
use crate::independent::disk::{floors::FloorCut, slots::SlotCut, PairCut};
use std::io::{BufRead, Write};
fn cuts() -> Vec<PairCut> {
    let floor = [
        FloorCut::BeforeTemp,
        FloorCut::TempCreated,
        FloorCut::PartialWritten,
        FloorCut::Written,
        FloorCut::FileSynced,
        FloorCut::Renamed,
        FloorCut::DirectorySynced,
    ];
    floor
        .into_iter()
        .map(PairCut::Intent)
        .chain(
            [
                SlotCut::BeforeWrite,
                SlotCut::HeaderWritten,
                SlotCut::PartialWritten,
                SlotCut::Written,
                SlotCut::FileSynced,
            ]
            .into_iter()
            .map(PairCut::Image),
        )
        .chain(floor.into_iter().map(PairCut::Selection))
        .collect()
}
fn kind(index: usize) -> DiskOperation {
    match (index / 3, index % 3) {
        (0, 0) => DiskOperation::PrepareAccepted,
        (1, 0) => DiskOperation::PrepareCandidate,
        (2, 0) => DiskOperation::PrepareSecurity,
        (3, 0) => DiskOperation::PrepareFork,
        (0..=3, 1) => DiskOperation::Start,
        (0, 2) => DiskOperation::PublishAccepted,
        (1, 2) => DiskOperation::PublishCandidate,
        (2, 2) => DiskOperation::PublishSecurity,
        (3, 2) => DiskOperation::PublishFork,
        _ => panic!("closed kind selection"),
    }
}
#[test]
#[ignore = "owned child of authenticated native cut qualification"]
fn native_cut_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_NATIVE_ROOT").unwrap());
    let stage = std::env::var("IC3_NATIVE_STAGE")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let cut = std::env::var("IC3_NATIVE_CUT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let f = ic3_disk_fixture::disk_profile();
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    if stage / 3 == 3 {
        let mut initial = Candidate {
            instance: f.query.instance.clone(),
            operation: f.query.operation.clone(),
            evidence: f.query.evidence.clone(),
        };
        initial.evidence.admission = None;
        session.submit_local(initial, [90; 32], [91; 32]).unwrap();
    }
    session.observe_cut = Box::new(move |operation, observed| {
        if operation == kind(stage) && observed == cuts()[cut] {
            println!("native-cut-ready");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    });
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    if stage / 3 == 0 {
        session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    } else {
        let candidate = match stage / 3 {
            1 => super::kinds::offered(
                &f,
                br#"{"after":null,"atom_id":"b","kind":"insert","text":"B"}"#,
                1,
            ),
            2 => super::kinds::offered(&f, b"invalid payload", 0),
            3 => super::kinds::offered(
                &f,
                br#"{"after":null,"atom_id":"b","kind":"insert","text":"B"}"#,
                0,
            ),
            _ => unreachable!(),
        };
        let effects =
            super::super::runtime::event(&mut session, Event::OfferReplica(Box::new(candidate)))
                .unwrap();
        super::super::runtime::drain(&mut session, effects).unwrap();
    }
    panic!("owned exact native cut must be reached");
}
#[test]
fn actual_signed_host_process_kill_reopens_prepare_started_and_terminal_cuts() {
    qualify(0..3);
}
#[test]
fn actual_signed_candidate_security_and_fork_native_kinds_171_os_cuts() {
    qualify(3..12);
}
fn qualify(stages: std::ops::Range<usize>) {
    let f = ic3_disk_fixture::disk_profile();
    for stage in stages {
        for (cut, selected) in cuts().into_iter().enumerate() {
            let root = root(&format!("native-cut-{stage}-{cut}"));
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .env_clear()
                .env("IC3_NATIVE_ROOT", &root)
                .env("IC3_NATIVE_STAGE", stage.to_string())
                .env("IC3_NATIVE_CUT", cut.to_string())
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
            ready.expect("actual authenticated native path reaches exact cut");
            let digest =
                Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
            let mut host = DiskHost::new(evidence(&f), config(&root, false));
            let mut session = host
                .open_replica(request(&f, digest))
                .unwrap_or_else(|fault| {
                    panic!("native {stage}/{selected:?} must reattach retained plan: {fault:?}")
                });
            let LoadResult::Validated(loaded) = session.load() else {
                panic!("native selected state {stage}/{selected:?}");
            };
            let value = &loaded.image.state.instances[&f.query.instance];
            let (accepted, attempts, expected_kind) = match stage / 3 {
                0 => (1, 1, CommitKind::AcceptedBatch),
                1 => (0, 1, CommitKind::Candidate),
                2 => (0, 1, CommitKind::SecurityEvidence),
                3 => (2, 2, CommitKind::QualifiedFork),
                _ => unreachable!(),
            };
            assert_eq!(
                value.accepted.len(),
                accepted,
                "original full plan must complete exactly once {stage}/{selected:?}"
            );
            if stage / 3 == 1 {
                assert_eq!(value.candidates.len(), 1);
                assert!(!value.pending.is_empty());
            }
            if stage / 3 == 2 {
                assert_eq!(value.security_evidence.len(), 1);
            }
            if stage / 3 == 3 {
                assert_eq!(value.qualified_forks.len(), 1);
                assert!(!value.quarantine.is_empty());
            }
            let recovery = &loaded.image.recovery;
            assert_eq!(recovery.attempts.len(), attempts);
            let last = recovery.attempts.last().unwrap();
            assert_eq!(last.attempt.binding.kind, expected_kind);
            assert!(matches!(
                last.phase,
                AttemptPhase::Terminal(TerminalOutcome::Committed { .. })
            ));
            assert_eq!(loaded.image.state.terminal_results.len(), attempts);
            super::kinds::retained_prepare_replays(&mut session, &loaded.image);
            let reconstruction = session.reconstruction_request().unwrap();
            let ReconstructResult::Cut(result) = session.reconstruct_cut(reconstruction) else {
                panic!("combined native recovery cut");
            };
            let missing_next = matches!(
                selected,
                PairCut::Intent(FloorCut::Renamed | FloorCut::DirectorySynced)
                    | PairCut::Image(
                        SlotCut::BeforeWrite | SlotCut::HeaderWritten | SlotCut::PartialWritten
                    )
            );
            if missing_next {
                assert!(
                    !result.complete_local,
                    "unknown abandoned physical intent cannot be erased by retry"
                );
            }
            drop(session);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
