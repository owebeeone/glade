//! Physical paired restart schedule; full authenticated host has separate witnesses.
use super::*;
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
    let images = [
        SlotCut::BeforeWrite,
        SlotCut::HeaderWritten,
        SlotCut::PartialWritten,
        SlotCut::Written,
        SlotCut::FileSynced,
    ];
    floors
        .into_iter()
        .map(PairCut::Intent)
        .chain(images.into_iter().map(PairCut::Image))
        .chain(floors.into_iter().map(PairCut::Selection))
        .collect()
}
fn next(pair: &PairStore) -> (ReplicaImage, ReplicaFloor) {
    let mut image = pair.image.clone();
    image.generation += 1;
    image.predecessor = pair.envelope.floor.selected.digest;
    image.prior_floor = pair.envelope.floor.clone();
    image.state.next_effect_id = 17;
    let mut floor = pair.envelope.floor.clone();
    floor.issuance.next_effect_id = 17;
    floor.next_attempt = 23;
    floor.invocation_high_water = 31;
    floor.guard_high_water = 47;
    floor.issuance.time_floor_ms = 101;
    floor.issuance.policy_generation = 5;
    floor.issuance.policy_digest = [10; 32];
    floor.issuance.origins.insert(
        "origin".into(),
        OriginFloor {
            certificate: [11; 32],
            next_seq: 4,
            lamport: 19,
            prev: Some([12; 32]),
            reserved_intents: vec![Bytes::new(vec![13])],
            frontier: vec![],
        },
    );
    (image, floor)
}
#[test]
#[ignore = "owned child of paired process-kill schedule"]
fn pair_cut_child() {
    let root = PathBuf::from(std::env::var_os("IC3_B2_PAIR_ROOT").unwrap());
    let index = std::env::var("IC3_B2_PAIR_CUT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let (_, _, bounds) = initial();
    let layout = DiskLayout::open(&root.join("data"), &root.join("trusted"), [6; 32]).unwrap();
    let mut pair = PairStore::open(layout, bounds).unwrap();
    let (image, floor) = next(&pair);
    let cut = cuts()[index];
    pair.transact(image, floor, DiskOperation::PublishAccepted, |observed| {
        if observed == cut {
            println!("paired-cut-ready");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    })
    .unwrap();
    panic!("selected crash cut was not reached");
}
#[test]
fn actual_process_kill_reopens_every_intent_image_selection_cut() {
    for (index, cut) in cuts().into_iter().enumerate() {
        let root = root(&format!("process-{index}"));
        let data = root.join("data");
        let trusted = root.join("trusted");
        let (image, floor, bounds) = initial();
        let layout = DiskLayout::provision(&data, &trusted, [6; 32]).unwrap();
        let pair = PairStore::create(layout, image, floor, bounds.clone()).unwrap();
        let old = pair.envelope.floor.selected.clone();
        let (_, projected) = next(&pair);
        drop(pair);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("IC3_B2_PAIR_ROOT", &root)
            .env("IC3_B2_PAIR_CUT", index.to_string())
            .args([
                "--exact",
                "independent::disk::paired::tests::content::process::pair_cut_child",
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
                let line = line.unwrap();
                if line == "paired-cut-ready" {
                    send.send(()).unwrap();
                    return;
                }
            }
        });
        let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        ready.expect("child must reach exact paired crash cut");
        let layout = DiskLayout::open(&data, &trusted, [6; 32]).unwrap();
        let pair = PairStore::open(layout, bounds).unwrap();
        let old_floor = matches!(
            cut,
            PairCut::Intent(
                FloorCut::BeforeTemp
                    | FloorCut::TempCreated
                    | FloorCut::PartialWritten
                    | FloorCut::Written
                    | FloorCut::FileSynced
            )
        );
        let next_image = matches!(
            cut,
            PairCut::Image(SlotCut::Written | SlotCut::FileSynced) | PairCut::Selection(_)
        );
        if old_floor {
            assert_eq!(pair.image.generation, 0, "{cut:?}");
            assert_eq!(pair.envelope.floor.issuance.next_effect_id, 0, "{cut:?}");
            assert_eq!(pair.envelope.floor.selected, old);
        } else {
            assert_eq!(
                pair.envelope.floor.issuance, projected.issuance,
                "all advanced namespaces/origin floors at {cut:?}"
            );
            assert_eq!(
                (
                    pair.envelope.floor.next_attempt,
                    pair.envelope.floor.invocation_high_water,
                    pair.envelope.floor.guard_high_water
                ),
                (23, 31, 47)
            );
        }
        if next_image {
            assert_eq!(pair.image.generation, 1, "{cut:?}");
            assert_eq!(pair.image.state.next_effect_id, 17);
            assert!(pair.envelope.unmaterialized.is_empty());
        } else if !old_floor {
            assert_eq!(pair.image.generation, 0, "{cut:?}");
            assert_eq!(pair.envelope.unmaterialized.len(), 1);
            assert_eq!(
                pair.envelope.unmaterialized[0].kind,
                DiskOperation::PublishAccepted
            );
            assert_eq!(pair.envelope.floor.selected, old);
        }
        assert!(pair.envelope.pending.is_none());
        assert!(pair.envelope.completion.is_none());
        assert!(pair.envelope.floor.intent.is_none());
        drop(pair);
        fs::remove_dir_all(root).unwrap();
    }
}
