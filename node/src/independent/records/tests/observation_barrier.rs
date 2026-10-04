//! The trusted-observation barrier runs only for a cut that differs from the
//! durable floor's; an unchanged cut is returned with no marker write.
use super::*;
use crate::independent::disk::{floors::FloorCut, slots::SlotCut, PairCut};
fn barrier() -> Vec<(DiskOperation, PairCut)> {
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
        .map(|cut| (DiskOperation::Observation, cut))
        .collect()
}
#[test]
fn unchanged_trusted_cut_writes_no_marker_and_a_changed_cut_runs_the_whole_barrier() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("observation-barrier");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    super::super::authentication::persist_observed(&mut session).unwrap();
    let seen = record_cuts(&mut session);
    let floor = std::fs::read(root.join("trusted/replica.floor")).unwrap();
    let (cut, _) = super::super::authentication::persist_observed(&mut session).unwrap();
    let unchanged = std::mem::take(&mut *seen.lock().unwrap());
    let rewritten = std::fs::read(root.join("trusted/replica.floor")).unwrap() != floor;
    assert!(
        unchanged.is_empty() && !rewritten && cut.time == session.evidence.source.0.time,
        "an unchanged cut is returned with no observation write: {} cuts {unchanged:?}, \
         floor rewritten {rewritten}",
        unchanged.len()
    );
    let later = Some(TimeInterval {
        earliest_ms: 30,
        latest_ms: 31,
    });
    session.evidence.source.0.time = later.clone();
    let (cut, _) = super::super::authentication::persist_observed(&mut session).unwrap();
    let changed = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(
        changed,
        barrier(),
        "a changed cut sets the marker, then selects the observation, which clears it"
    );
    assert_eq!(cut.time, later);
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated observed cut")
    };
    assert_eq!(loaded.authoritative_floor.issuance.time_floor_ms, 30);
    assert!(!session.pair.as_ref().unwrap().envelope.observing);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
