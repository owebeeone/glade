//! Plan Step 4.5c, the done-when's first clause
//! (`glade/dev-docs/GladeDirectoryCheckpoints.md`, section 11): a week of
//! renewals, folded as a running node folds them, leaves records.json, the
//! boot and the served store bounded, and every claims fold answering as
//! over every renewal kept.

use std::time::Instant;

use glade_node::cbor;
use glade_node::checkpoint;
use glade_node::claims::Leases;
use glade_node::registry::{BlobStore, Record, Registry, RegistryApi, StoreApi, HOME};
use glade_node::server::Server;
use glade_node::store::Store;
use glade_node::sysdata::{ServeClaim, WorkspaceEntry};
use glade_node::sysdir::{boot_at, now_ms};
use glade_wire::generated::Op;

use crate::disk::ScratchDir;

/// A node that serves `home` and `ws-razel` renews for a week at F1's
/// defaults, 6,048 ticks and 12,096 renewals, through the registry's tick at
/// the default threshold, then saves once to records.json and boots from it.
/// records.json then holds its content C, one checkpoint and at most N + S =
/// 1,002 claims, where a week without checkpoints holds C + 12,096, and no
/// checkpoint folded more claims than that. The boot checks each record once
/// and quarantines none, and adoption's served store holds as many. At
/// instants across the week and past its last lease, every claims fold
/// answers as over a registry that kept every renewal. The boot's time is
/// printed, not asserted.
#[tokio::test]
async fn a_simulated_week_leaves_records_json_and_the_boot_bounded() {
    let leases = Leases::default();
    let (sys, served) = (ScratchDir::new("week-sys"), ScratchDir::new("week-store"));
    let mut boot = boot_at(sys.path().to_path_buf(), "local").unwrap();
    let node = boot.node_id.clone();
    let entry = Record::Workspace(WorkspaceEntry {
        workspace: "ws-razel".into(),
        name: "razel".into(),
        eligible_hosts: vec![node.clone()],
    });
    boot.registry.append(entry, &node).unwrap();
    let start = now_ms();
    let claim = |share: &str, at: i64| ServeClaim {
        node: node.clone(),
        share: share.into(),
        lease_expiry_ms: at + leases.lease_ms,
        epoch: 1,
    };
    let first = Record::Serve(claim("ws-razel", start));
    boot.registry.append(first, &node).unwrap();
    // Every claim the node makes, in a registry that never folds.
    let mut whole = Registry::from_snapshot(&boot.registry.snapshot()).0;
    let (mut folds, mut widest) = (0, 0);
    let renew_ms = leases.renew_ms as i64;
    let week = 7 * 24 * 3_600_000 / renew_ms;
    for tick in 1..=week {
        let at = start + tick * renew_ms;
        let renewals = [claim(HOME, at), claim("ws-razel", at)];
        for renewal in &renewals {
            whole.append(Record::Serve(renewal.clone()), &node).unwrap();
        }
        let (renewals, after) = (renewals.to_vec(), leases.checkpoint_after);
        let ticked = checkpoint::tick(&mut boot.registry, &node, renewals, after);
        if let Some(folded) = ticked.unwrap().1 {
            (folds, widest) = (folds + 1, widest.max(folded.dropped + folded.carried));
        }
    }
    assert_eq!(week, 6_048);
    boot.store.save(&boot.registry.snapshot()).unwrap();
    drop(boot);

    let saved = BlobStore::new(sys.path()).load().unwrap();
    let read = |bytes: &Vec<u8>| Op::from_cbor(&cbor::decode(bytes));
    let ops: Vec<Op> = saved.records.iter().map(read).collect();
    let on = |stream: &str| ops.iter().filter(|op| op.glade_id == stream).count();
    let (claims, checkpoints) = (on("dir.claims"), on("dir.checkpoints"));
    let content = ops.len() - claims - checkpoints;
    let above = ops.len() - content;
    let said = format!("C + {above} records where at most C + 1,003 were expected");
    assert!(above <= 1_003, "records.json holds {said}");
    assert_eq!(checkpoints, 1, "records.json's checkpoints");
    assert!(widest <= 1_002, "a checkpoint folded {widest} claims");
    eprintln!("the week: {folds} checkpoint(s), records.json holds C = {content} + {above}");

    let started = Instant::now();
    let boot = boot_at(sys.path().to_path_buf(), "local").unwrap();
    let took = started.elapsed();
    assert_eq!(boot.rejected, 0, "the boot quarantined a record");
    let checked = boot.registry.snapshot().records.len();
    assert_eq!(checked, ops.len(), "the boot checks and keeps each record");
    eprintln!("the week: the boot checked {checked} record(s) in {took:?}");
    let end = start + week * renew_ms + leases.lease_ms;
    let days = (0..=14).map(|half| start + half * 12 * 3_600_000);
    let instants = [start - 1, end - 1, end, end + 1].into_iter().chain(days);
    for at in instants {
        for share in [HOME, "ws-razel", "ws-never"] {
            let answer = boot.registry.who_serves(share, at);
            let what = format!("who_serves({share}) at {}", at - start);
            assert_eq!(answer, whole.who_serves(share, at), "{what}");
        }
    }

    let server = Server::open(served.path()).unwrap();
    server.adopt_boot(boot).await.unwrap();
    let store = Store::open(served.path()).unwrap();
    let held = |(share, glade_id, key): (String, String, Vec<u8>)| {
        store.scan(&share, &glade_id, &key, &node, i64::MIN).len()
    };
    let zones = store.zones().into_iter();
    let held: usize = zones.filter(|(share, ..)| share == HOME).map(held).sum();
    let said = format!("the served store holds {held} of the node's records");
    assert!(held <= content + 1_003, "{said}");
}
