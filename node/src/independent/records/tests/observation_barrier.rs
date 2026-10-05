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
fn markers(cuts: &[(DiskOperation, PairCut)]) -> Vec<(DiskOperation, PairCut)> {
    cuts.iter()
        .filter(|(_, cut)| matches!(cut, PairCut::ObservationMarker(_)))
        .cloned()
        .collect()
}
fn begun(cuts: &[(DiskOperation, PairCut)]) -> bool {
    cuts.contains(&(
        DiskOperation::BeginIngress,
        PairCut::Intent(FloorCut::BeforeTemp),
    ))
}

#[test]
fn begin_ingress_with_an_unchanged_cut_writes_no_marker_and_a_changed_cut_keeps_the_barrier() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("begin-ingress-barrier");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    super::super::authentication::persist_observed(&mut session).unwrap();
    let seen = record_cuts(&mut session);
    let source = crate::signing::public_key(&NODE);
    let (cut, _) = session.node_observe().unwrap();
    let guard = session
        .node_guard(&cut, source, [97; 32], "bundle", 1000)
        .unwrap();
    assert!(std::mem::take(&mut *seen.lock().unwrap()).is_empty());
    let permit = session.begin_ingress(guard).unwrap();
    let unchanged = std::mem::take(&mut *seen.lock().unwrap());
    assert!(
        begun(&unchanged) && markers(&unchanged).is_empty(),
        "an unchanged cut begins its guard with no observation write: {unchanged:?}"
    );
    assert!(matches!(
        session.abandon_ingress(permit),
        PersistResult::Committed { .. }
    ));
    // The provider's cut moves on between the guard's bound and its begin: the
    // barrier runs (and the guard, now stale against the new selection, is refused).
    let guard = session
        .node_guard(&cut, source, [98; 32], "bundle", 1000)
        .unwrap();
    session.evidence.source.0.time = Some(TimeInterval {
        earliest_ms: 30,
        latest_ms: 31,
    });
    std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(
        session.begin_ingress(guard).err(),
        Some(Fault::BindingMismatch)
    );
    let changed = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(
        changed,
        barrier(),
        "a changed cut still sets the marker, selects the observation and clears it"
    );
    assert!(!session.pair.as_ref().unwrap().envelope.observing);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn offer_verified_historically_with_an_unchanged_cut_writes_no_marker() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("historical-verify-barrier");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    super::super::authentication::persist_observed(&mut session).unwrap();
    let seen = record_cuts(&mut session);
    let candidate = super::kinds::offered(
        &f,
        br#"{"after":null,"atom_id":"a","kind":"insert","text":"A"}"#,
        0,
    );
    let effects = super::super::runtime::event(
        &mut session,
        Event::OfferReplica(Box::new(candidate.clone())),
    )
    .unwrap();
    super::super::runtime::drain(&mut session, effects).unwrap();
    let cuts = std::mem::take(&mut *seen.lock().unwrap());
    assert!(
        !cuts.is_empty() && markers(&cuts).is_empty(),
        "an offered record's Historical verification writes no observation marker: {} cuts, \
         markers {:?}",
        cuts.len(),
        markers(&cuts)
    );
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated after the offer");
    };
    assert!(loaded.image.state.instances[&f.descriptor.instance]
        .accepted
        .iter()
        .any(|accepted| accepted.candidate.operation == candidate.operation));
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_submit_keeps_its_observation_barrier() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("local-submit-barrier");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    super::super::authentication::persist_observed(&mut session).unwrap();
    let seen = record_cuts(&mut session);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    let cuts = std::mem::take(&mut *seen.lock().unwrap());
    assert!(
        !markers(&cuts).is_empty(),
        "a local verify and its seal consult authority and keep their barriers"
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
