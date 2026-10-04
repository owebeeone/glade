//! A lost trusted observation image cannot reset its already retained floor.
use super::*;
use crate::independent::disk::{floors::FloorCut, slots::SlotCut, PairCut};
use std::io::{BufRead, Write};
#[test]
#[ignore = "owned child of genuine observation floor-ahead witness"]
fn observation_floor_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_OBSERVATION_ROOT").unwrap());
    let f = ic3_disk_fixture::disk_profile();
    let mut existing = evidence(&f);
    existing.config.certificates = vec![f.certificate.clone()];
    let provider = GenuineEvidence::new(
        existing.config,
        controls::ShiftingClock {
            cut: existing.source.0,
            calls: 0,
        },
    )
    .unwrap();
    let mut host = DiskHost::new(provider, config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut ordinal = 0usize;
    session.observe_cut = Box::new(move |operation, cut| {
        if operation == DiskOperation::Observation {
            if cut == PairCut::Intent(FloorCut::BeforeTemp) {
                ordinal += 1;
            }
            if ordinal == 2 && cut == PairCut::Image(SlotCut::BeforeWrite) {
                println!("observation-floor-ready");
                std::io::stdout().flush().unwrap();
                loop {
                    std::thread::park();
                }
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
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    panic!("fresh observed floor intent must be durable before selected image");
}
#[test]
fn actual_process_kill_keeps_new_observed_floor_and_recovers_old_valid_pending_image() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("observation-floor-ahead");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("IC3_OBSERVATION_ROOT", &root)
        .args([
            "--exact",
            "independent::records::tests::content::observation_restart::observation_floor_child",
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
            if line.unwrap() == "observation-floor-ready" {
                send.send(()).unwrap();
                return;
            }
        }
    });
    let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    ready.expect("fresh provider observation floor intent precedes all next-image bytes");
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut provider = evidence(&f);
    provider.source.0.time = Some(TimeInterval {
        earliest_ms: 80,
        latest_ms: 81,
    });
    let mut host = DiskHost::new(provider, config(&root, false));
    let mut session = host
        .open_replica(request(&f, digest))
        .expect("old valid image reattaches while authoritative new floor is never reset");
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("truthful pending image after lost current observation image");
    };
    assert_eq!(loaded.authoritative_floor.issuance.time_floor_ms, 80);
    assert_eq!(
        loaded.image.state.instances[&f.query.instance].time_floor_ms,
        80
    );
    assert!(loaded.image.state.instances[&f.query.instance]
        .accepted
        .is_empty());
    assert!(!loaded.image.state.queries.is_empty());
    let reconstruction = session.reconstruction_request().unwrap();
    let ReconstructResult::Cut(cut) = session.reconstruct_cut(reconstruction) else {
        panic!("combined unknown observation cut");
    };
    assert!(!cut.complete_local);
    assert!(session.evidence.authorized.is_empty());
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
