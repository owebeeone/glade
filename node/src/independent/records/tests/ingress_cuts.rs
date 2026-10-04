//! Actual process death, not Drop, at all owned guard establishment and discharge cuts.
use super::*;
use crate::independent::disk::{floors::FloorCut, slots::SlotCut, PairCut};
use std::io::{BufRead, Write};
fn cuts(stage: usize) -> Vec<PairCut> {
    let floor = [
        FloorCut::BeforeTemp,
        FloorCut::TempCreated,
        FloorCut::PartialWritten,
        FloorCut::Written,
        FloorCut::FileSynced,
        FloorCut::Renamed,
        FloorCut::DirectorySynced,
    ];
    let begin: Vec<_> = floor.into_iter().map(PairCut::Intent).collect();
    if stage == 0 {
        return begin;
    }
    begin
        .into_iter()
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
pub(super) fn empty(f: &Fixture, guard: &ReceiveGuard) -> Observation {
    let inventory = Inventory {
        instance: f.descriptor.instance.clone(),
        source: guard.source,
        number: 1,
        entries: vec![],
        total_bytes: 0,
    };
    let header = signed(
        &NODE,
        &InventoryHeaderBody {
            instance: inventory.instance.clone(),
            identity: f.identity.clone(),
            source: guard.source,
            target: crate::signing::public_key(&NODE),
            channel: guard.channel,
            nonce: [78; 32],
            number: 1,
            pages: 0,
            entries: 0,
            total_bytes: 0,
            manifest: encode_profile(&inventory, &limits()).unwrap().digest(),
        },
    );
    Observation {
        id: 1,
        instance: inventory.instance.clone(),
        source: guard.source,
        channel: guard.channel,
        guard: guard.id.clone(),
        bytes: encode(&(header, inventory), &limits()).unwrap(),
        expected: vec![],
    }
}
#[test]
#[ignore = "owned real guard process child"]
fn ingress_cut_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_GUARD_ROOT").unwrap());
    let stage = std::env::var("IC3_GUARD_STAGE")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let cut = std::env::var("IC3_GUARD_CUT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let f = ic3_disk_fixture::disk_profile();
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let guard = super::ingress::incoming(&mut session);
    let observe = move |operation, observed| {
        let expected = match stage {
            0 => DiskOperation::BeginIngress,
            2 => DiskOperation::AbandonIngress,
            _ => DiskOperation::SettleIngress,
        };
        if operation == expected && observed == cuts(stage)[cut] {
            println!("guard-cut-ready");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    };
    if stage == 0 {
        session.observe_cut = Box::new(observe);
        let _ = session.begin_ingress(guard);
    } else {
        let permit = session.begin_ingress(guard.clone()).unwrap();
        if stage == 2 {
            session.observe_cut = Box::new(observe);
            session.abandon_ingress(permit);
        } else {
            let observation = empty(&f, &guard);
            let bytes = if stage == 3 {
                Bytes::new(b"partial header".to_vec())
            } else {
                observation.bytes.clone()
            };
            let mut receive = session
                .receive_ingress(permit, || std::future::ready(Ok(bytes)))
                .unwrap();
            let waker = std::task::Waker::noop();
            let mut cx = std::task::Context::from_waker(waker);
            let std::task::Poll::Ready((permit, value)) =
                std::future::Future::poll(std::pin::Pin::new(&mut receive), &mut cx)
            else {
                panic!("drained finite read");
            };
            if stage == 4 {
                println!("guard-cut-ready");
                std::io::stdout().flush().unwrap();
                loop {
                    std::thread::park();
                }
            }
            session.observe_cut = Box::new(observe);
            if stage == 3 {
                session.retain_ingress_loss(permit, value.unwrap());
            } else {
                let mut checkpoint = session.checkpoint_template().unwrap();
                checkpoint.image.observations.push(observation.clone());
                checkpoint.image.watermark = 1;
                session.settle_ingress(permit, observation, checkpoint);
            }
        }
    }
    panic!("all exact selected cuts must be reached");
}
fn killed(stage: usize, cut: usize, root: &std::path::Path) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("IC3_GUARD_ROOT", root)
        .env("IC3_GUARD_STAGE", stage.to_string())
        .env("IC3_GUARD_CUT", cut.to_string())
        .args([
            "--exact",
            "independent::records::tests::content::ingress_cuts::ingress_cut_child",
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
            if line.unwrap() == "guard-cut-ready" {
                send.send(()).unwrap();
                return;
            }
        }
    });
    let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    ready.expect("actual owned receive path reaches requested physical cut");
}
#[test]
fn actual_process_kill_guard_begin_settle_abandon_loss_and_consumed_prefix_reopen_honestly() {
    let f = ic3_disk_fixture::disk_profile();
    for stage in 0..5 {
        let count = if stage == 4 { 1 } else { cuts(stage).len() };
        for cut in 0..count {
            let root = root(&format!("guard-cut-{stage}-{cut}"));
            killed(stage, cut, &root);
            let digest =
                Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
            let mut host = DiskHost::new(evidence(&f), config(&root, false));
            let mut session = host
                .open_replica(request(&f, digest))
                .unwrap_or_else(|fault| panic!("guard {stage}/{cut}: {fault:?}"));
            let LoadResult::Validated(loaded) = session.load() else {
                panic!("full retained guard image {stage}/{cut}");
            };
            let ReconstructResult::Cut(value) =
                session.reconstruct_cut(session.reconstruction_request().unwrap())
            else {
                panic!("combined cut {stage}/{cut}");
            };
            let active = loaded
                .authoritative_floor
                .guards
                .values()
                .any(|status| matches!(status, GuardStatus::Active(_)));
            if active || stage >= 3 {
                assert!(
                    !value.complete_local,
                    "consumed/unknown history cannot regain completeness {stage}/{cut}"
                );
            }
            if stage == 0 && cut < 5 {
                assert!(loaded.authoritative_floor.guards.is_empty());
                assert!(value.complete_local);
            }
            if stage == 1 && !active {
                assert_eq!(loaded.image.observations.len(), 1);
                assert!(value.complete_local);
            }
            if stage == 2 && !active {
                assert!(matches!(
                    loaded.authoritative_floor.guards.values().next(),
                    Some(GuardStatus::Abandoned { .. })
                ));
                assert!(value.complete_local);
            }
            if stage == 3 && !active {
                assert!(value.permanent_loss);
                assert_eq!(loaded.image.permanent_loss.len(), 1);
            }
            if stage == 4 {
                ic3_disk_fixture::local_authorization(&f, &mut session, 92);
                let mut candidate = Candidate {
                    instance: f.query.instance.clone(),
                    operation: f.query.operation.clone(),
                    evidence: f.query.evidence.clone(),
                };
                candidate.evidence.admission = None;
                assert!(
                    session
                        .submit_local(candidate, [90; 32], [91; 32])
                        .unwrap()
                        .iter()
                        .any(|outcome| matches!(outcome, Outcome::AcceptedLocal(_))),
                    "orphan guard uncertainty does not forbid a separately valid local append"
                );
                let ReconstructResult::Cut(value) =
                    session.reconstruct_cut(session.reconstruction_request().unwrap())
                else {
                    panic!("append with orphan guard");
                };
                assert!(!value.complete_local);
            }
            drop(session);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
