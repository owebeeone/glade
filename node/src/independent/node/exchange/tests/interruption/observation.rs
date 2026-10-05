//! IC3-DISK-006 / IC3-GUARD-003 exposure: a whole exchange round between two
//! in-process actual replicas (real Iroh, real disk) whose trusted cut is
//! unchanged writes no observation marker in its HELLO, ready, header or guard
//! steps, so no SIGKILL there can leave the marker set. Both sides hold no
//! history, so no bundle is offered and every guard is an inventory or page.
use super::*;
use crate::independent::disk::DiskOperation;
use crate::independent::disk::{floors::FloorCut, PairCut};
use crate::independent::records::tests::content::record_cuts;

/// Observation-marker cuts outside `begin_ingress`'s own barrier: that Records
/// port sets and clears its marker (seven cuts each) just before its guard.
fn outside_begin_ingress(cuts: &[(DiskOperation, PairCut)]) -> usize {
    let mut owned = vec![false; cuts.len()];
    for (index, entry) in cuts.iter().enumerate() {
        if *entry
            != (
                DiskOperation::BeginIngress,
                PairCut::Intent(FloorCut::BeforeTemp),
            )
        {
            continue;
        }
        let mut back = index;
        while back > 0
            && index - back < 14
            && matches!(cuts[back - 1].1, PairCut::ObservationMarker(_))
        {
            back -= 1;
            owned[back] = true;
        }
    }
    cuts.iter()
        .zip(owned)
        .filter(|((_, cut), owned)| matches!(cut, PairCut::ObservationMarker(_)) && !owned)
        .count()
}

/// A fresh replica with no history whose provider cut is already durable:
/// one genuine challenge observes and persists it.
fn observed_empty(h: &History, side: Side, root: &Path) {
    let mut session = host(h, side, root, true)
        .open_replica(request(&h.f, side))
        .unwrap();
    session
        .issue_challenge(SessionRequest {
            requester: "alice".into(),
            writer_key: public_key(&WRITER),
            operation: h.node_ops[0].operation.canonical().digest(),
            channel: [90; 32],
            session_nonce: [91; 32],
            challenge_nonce: [92; 32],
            window: Window {
                not_before_ms: 10,
                not_after_ms: 100,
            },
        })
        .unwrap();
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
}

/// Whether `cut` is the begin of a guard's own durable write.
fn begins_guard(entry: &(DiskOperation, PairCut)) -> bool {
    *entry
        == (
            DiskOperation::BeginIngress,
            PairCut::Intent(FloorCut::BeforeTemp),
        )
}

/// One whole round between two in-process actual replicas (node dials, peer
/// accepts) opened from `roots`, every physical cut each side reaches recorded
/// from its open until the round ends.
struct Exchanged {
    node: Arc<IndependentReplica>,
    peer: Arc<IndependentReplica>,
    dialer: Result<Digest, Fault>,
    acceptor: Result<Digest, Fault>,
    cuts: [Vec<(DiskOperation, PairCut)>; 2],
    lines: [Lines; 2],
}
async fn exchange(h: &History, roots: &[PathBuf]) -> Exchanged {
    let ports = [port(), port()];
    let node_profile = profile(h, UNDER_TEST, &roots[0], PEER, ports[1]);
    let peer_profile = profile(h, PEER, &roots[1], UNDER_TEST, ports[0]);
    let node = open_peer(&node_profile, ports[0]).await;
    let peer = open_peer(&peer_profile, ports[1]).await;
    let recorded = [&node, &peer]
        .map(|replica| record_cuts(replica.session.lock().unwrap().as_mut().unwrap()));
    let address = CarrierAddr(node.provision.peers[0].1 .1.clone());
    let (dialed, accepted) = tokio::join!(
        tokio::time::timeout(
            Duration::from_millis(DEADLINE_MS),
            node.carrier.dial(&address)
        ),
        accept(&peer)
    );
    let dialed: Arc<dyn CarrierLink> = Arc::from(dialed.unwrap().unwrap());
    let accepted: Arc<dyn CarrierLink> = Arc::from(accepted);
    let lines = [Lines::default(), Lines::default()];
    let (dialer, acceptor) = tokio::join!(
        round(&node, dialed.clone(), true, &lines[0]),
        round(&peer, accepted.clone(), false, &lines[1])
    );
    dialed.close().await;
    accepted.close().await;
    let cuts = recorded.map(|seen| std::mem::take(&mut *seen.lock().unwrap()));
    Exchanged {
        node,
        peer,
        dialer,
        acceptor,
        cuts,
        lines,
    }
}
fn roots(name: &str) -> Vec<PathBuf> {
    let base = std::env::temp_dir().canonicalize().unwrap();
    ["node", "peer"]
        .iter()
        .map(|side| {
            let root = base.join(format!("glade-ic3-{name}-{side}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            root
        })
        .collect()
}
async fn finish(round: Exchanged, roots: Vec<PathBuf>) {
    for replica in [&round.node, &round.peer] {
        replica.carrier.close().await;
        replica.close().unwrap();
    }
    for root in roots {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unchanged_cut_round_writes_no_observation_marker_in_hello_ready_header_or_guard() {
    let roots = roots("observe");
    let h = history();
    observed_empty(&h, UNDER_TEST, &roots[0]);
    observed_empty(&h, PEER, &roots[1]);
    let round = exchange(&h, &roots).await;
    let outside = round
        .cuts
        .each_ref()
        .map(|cuts| outside_begin_ingress(cuts));
    let begun = round
        .cuts
        .each_ref()
        .map(|cuts| cuts.iter().filter(|entry| begins_guard(entry)).count());
    assert!(
        round.dialer.is_ok() && round.acceptor.is_ok() && begun == [2, 2],
        "a complete round on both sides, guarding its inventory and page: dialer \
         {:?}, acceptor {:?}, guards begun {begun:?}; {:?} {:?}",
        round.dialer,
        round.acceptor,
        round.lines[0].all(),
        round.lines[1].all()
    );
    assert_eq!(
        outside,
        [0, 0],
        "observation-marker cuts in the round's HELLO, ready, header and guard steps \
         (node, peer)"
    );
    finish(round, roots).await;
}

/// The same round with a history on each side, so each receives the other's
/// bundles (every one verified Historically and guarded): no observation
/// marker at all, `begin_ingress` and the verification of a received bundle
/// included.
#[tokio::test(flavor = "multi_thread")]
async fn unchanged_cut_round_that_receives_bundles_writes_no_observation_marker() {
    let roots = roots("observe-bundles");
    let h = history();
    prepare(&h, UNDER_TEST, &roots[0], &h.node_ops);
    prepare(&h, PEER, &roots[1], &h.peer_ops);
    let round = exchange(&h, &roots).await;
    let begun = round
        .cuts
        .each_ref()
        .map(|cuts| cuts.iter().filter(|entry| begins_guard(entry)).count());
    let held = [&round.node, &round.peer].map(|replica| {
        with(replica, |session| session.node_history())
            .unwrap()
            .len()
    });
    assert!(
        round.dialer.is_ok() && round.acceptor.is_ok() && held == [3, 3],
        "both sides receive the other's bundles: dialer {:?}, acceptor {:?}, history {held:?}, \
         guards begun {begun:?}; {:?} {:?}",
        round.dialer,
        round.acceptor,
        round.lines[0].all(),
        round.lines[1].all()
    );
    assert_eq!(
        begun,
        [2 + 2, 2 + 1],
        "inventory and page guards, then a bundle guard per bundle received"
    );
    let markers = round.cuts.each_ref().map(|cuts| {
        cuts.iter()
            .filter(|(_, cut)| matches!(cut, PairCut::ObservationMarker(_)))
            .count()
    });
    assert_eq!(
        markers,
        [0, 0],
        "observation-marker cuts in the whole round (node, peer)"
    );
    finish(round, roots).await;
}
