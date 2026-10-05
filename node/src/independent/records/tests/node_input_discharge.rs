//! The actual node's two receive sites settle a drained input through
//! `settle_node_input`: a refused settlement is discharged as retained loss, so no
//! guard stays Active and the next `begin_ingress` is admitted; storage whose
//! outcome is unknown keeps its parked continuation, and close reports Pending.
use super::super::persist::PendingIngress;
use super::*;

/// A drained full-inventory receive of this node's own empty inventory: signed,
/// bound to its guard, and valid exactly when `total_bytes` is 0 (an empty
/// manifest cannot claim payload bytes).
async fn drained(
    session: &mut DiskSession<Source>,
    total_bytes: usize,
) -> (IngressPermit, GuardId, Bytes) {
    let mut inventory = session.node_inventory(&[]).unwrap();
    inventory.total_bytes = total_bytes;
    let source = crate::signing::public_key(&NODE);
    let channel = [96; 32];
    let observed = session.node_observe().unwrap();
    let header = session
        .node_inventory_header(&observed, &inventory, source, channel)
        .unwrap();
    let raw = encode(&(header, inventory), &limits()).unwrap();
    let guard = session
        .node_guard(&observed.0, source, channel, "inventory", 1000)
        .unwrap();
    let id = guard.id.clone();
    let permit = session.begin_ingress(guard).unwrap();
    let bytes = raw.clone();
    let (permit, result) = session
        .receive_ingress(permit, || async move { Ok(bytes) })
        .unwrap()
        .await;
    assert_eq!(result, Ok(raw.clone()));
    (permit, id, raw)
}
fn status(session: &DiskSession<Source>, id: &GuardId) -> GuardStatus {
    session
        .pair
        .as_ref()
        .unwrap()
        .envelope
        .floor
        .guards
        .get(id)
        .unwrap()
        .clone()
}

#[tokio::test]
async fn refused_settlement_is_discharged_as_retained_loss_and_the_next_guard_is_admitted() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-discharge-refused");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let (permit, id, raw) = drained(&mut session, 1).await;
    assert_eq!(
        session.settle_node_input(permit, raw, vec![]),
        Err(Fault::Integrity),
        "the caller still gets the settlement's own refusal"
    );
    assert!(
        session.pending_ingress.is_empty(),
        "no continuation is left parked"
    );
    assert!(
        matches!(status(&session, &id), GuardStatus::Loss { .. }),
        "the refused input's guard is retained as loss, not Active"
    );
    assert_eq!(session.pair.as_ref().unwrap().image.permanent_loss.len(), 1);
    // Ingress is not wedged: a fresh guard is admitted, then released.
    let (cut, _) = session.node_observe().unwrap();
    let next = session
        .node_guard(
            &cut,
            crate::signing::public_key(&NODE),
            [97; 32],
            "bundle",
            1000,
        )
        .unwrap();
    let permit = session
        .begin_ingress(next)
        .expect("the next begin_ingress after a discharged refusal");
    assert!(matches!(
        session.abandon_ingress(permit),
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
async fn committed_settlement_leaves_nothing_parked_and_no_loss() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-discharge-committed");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let (permit, id, raw) = drained(&mut session, 0).await;
    assert_eq!(session.settle_node_input(permit, raw, vec![]), Ok(()));
    assert!(session.pending_ingress.is_empty());
    assert!(matches!(status(&session, &id), GuardStatus::Settled { .. }));
    assert!(session
        .pair
        .as_ref()
        .unwrap()
        .image
        .permanent_loss
        .is_empty());
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn unknown_settlement_stays_parked_unconverted_and_close_reports_pending() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-discharge-unknown");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let (permit, id, raw) = drained(&mut session, 0).await;
    session.observe_cut = Box::new(|_, _| Err(Fault::Unavailable));
    assert_eq!(
        session.settle_node_input(permit, raw, vec![]),
        Err(Fault::Unavailable)
    );
    assert!(
        matches!(
            session.pending_ingress.get(&id),
            Some(PendingIngress::Settle { .. })
        ),
        "an unresolved settlement may have been selected: it is retried, never turned into loss"
    );
    assert!(session
        .pair
        .as_ref()
        .unwrap()
        .image
        .permanent_loss
        .is_empty());
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn unknown_loss_retention_stays_parked_as_loss_and_close_reports_pending() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("c-discharge-loss-unknown");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let (permit, id, raw) = drained(&mut session, 1).await;
    // The refusal is decided before any write; only the loss's own write fails.
    session.observe_cut = Box::new(|_, _| Err(Fault::Unavailable));
    assert_eq!(
        session.settle_node_input(permit, raw, vec![]),
        Err(Fault::Integrity)
    );
    assert!(
        matches!(
            session.pending_ingress.get(&id),
            Some(PendingIngress::Loss { .. })
        ),
        "the refused settlement's loss retention was attempted and is kept parked"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Pending,
        "close does not claim a drained guard whose loss is not durable"
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
