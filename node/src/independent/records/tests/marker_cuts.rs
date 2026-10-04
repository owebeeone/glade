//! Process death on the closed pre-observation marker and exact observation selection.
use super::*;
use crate::independent::disk::{floors::FloorCut, slots::SlotCut, PairCut};
use std::io::{BufRead, Write};
fn cuts() -> Vec<PairCut> {
    let floors = [
        FloorCut::BeforeTemp,
        FloorCut::TempCreated,
        FloorCut::PartialWritten,
        FloorCut::Written,
        FloorCut::FileSynced,
        FloorCut::Renamed,
        FloorCut::DirectorySynced,
    ];
    floors
        .into_iter()
        .map(PairCut::ObservationMarker)
        .chain(floors.into_iter().map(PairCut::Intent))
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
        .chain(floors.into_iter().map(PairCut::Selection))
        .collect()
}
fn challenge(f: &Fixture) -> SessionRequest {
    SessionRequest {
        requester: "alice".into(),
        writer_key: crate::signing::public_key(&WRITER),
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
#[ignore = "owned authenticated pre-observation crash child"]
fn marker_cut_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_MARKER_ROOT").unwrap());
    let index = std::env::var("IC3_MARKER_CUT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let f = ic3_disk_fixture::disk_profile();
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    session.evidence.source.0.time = Some(TimeInterval {
        earliest_ms: 1001,
        latest_ms: 1002,
    });
    session.observe_cut = Box::new(move |operation, observed| {
        if operation == DiskOperation::Observation && observed == cuts()[index] {
            println!("marker-cut-ready");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    });
    assert!(session.issue_challenge(challenge(&f)).is_err());
    panic!("exact protected observation cut must be reached");
}
#[test]
fn actual_process_death_at_all_26_marker_and_observation_selection_cuts() {
    let f = ic3_disk_fixture::disk_profile();
    for (index, cut) in cuts().into_iter().enumerate() {
        let root = root(&format!("marker-cut-{index}"));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("IC3_MARKER_ROOT", &root)
            .env("IC3_MARKER_CUT", index.to_string())
            .args([
                "--exact",
                "independent::records::tests::content::marker_cuts::marker_cut_child",
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
                if line.unwrap() == "marker-cut-ready" {
                    send.send(()).unwrap();
                    return;
                }
            }
        });
        let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        ready.expect("exact observation cut");
        let floor_path = root.join("trusted/replica.floor");
        let before = std::fs::read(&floor_path).unwrap();
        let digest = Bytes::new(before.clone()).digest();
        let before_consult = matches!(
            cut,
            PairCut::ObservationMarker(
                FloorCut::BeforeTemp
                    | FloorCut::TempCreated
                    | FloorCut::PartialWritten
                    | FloorCut::Written
                    | FloorCut::FileSynced
            )
        );
        let complete_next = matches!(
            cut,
            PairCut::Image(SlotCut::Written | SlotCut::FileSynced) | PairCut::Selection(_)
        );
        let mut provider = evidence(&f);
        if complete_next {
            provider.source.0.time = Some(TimeInterval {
                earliest_ms: 1001,
                latest_ms: 1002,
            });
        }
        let mut host = DiskHost::new(provider, config(&root, false));
        let opened = host.open_replica(request(&f, digest));
        if before_consult || complete_next {
            let mut session = opened
                .expect("old cut before source consultation or fully validated exact next cut");
            let LoadResult::Validated(loaded) = session.load() else {
                panic!("usable only exact authorized cut")
            };
            assert!(loaded.image.state.instances[&f.query.instance]
                .accepted
                .is_empty());
            assert!(loaded.image.recovery.attempts.is_empty());
            if complete_next {
                assert_eq!(loaded.authoritative_floor.issuance.time_floor_ms, 1001);
                assert!(session.issue_challenge(challenge(&f)).is_err());
            } else {
                assert!(session.issue_challenge(challenge(&f)).is_ok());
            }
            drop(session);
        } else {
            assert!(
                opened.is_err(),
                "unresolved protected source observation {cut:?} cannot reset"
            );
            assert_eq!(std::fs::read(&floor_path).unwrap(), before);
            let mut host = DiskHost::new(evidence(&f), config(&root, false));
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
            assert_eq!(std::fs::read(&floor_path).unwrap(), before);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
