//! Plan Step 4.4: a restart in the middle of a round, and a retry after a
//! known failure, over the engine this binary gives the journeys. Over the
//! last snapshot in memory (the fast loop) they prove the fold is reloaded and
//! resumed from, never durability; over records.json (`tests/durable`) they
//! prove the round trip through the node's engine on disk, not crash safety
//! or fsync. What "resumes from its heads" promises is set out in
//! `glade/dev-docs/GladeNodeAssembly.md`, "Durable store and restart".

use std::collections::BTreeMap;

use glade_node::appdecl::parse;
use glade_node::assembly::HostError;
use glade_node::cbor;
use glade_node::chain::op_hash;
use glade_node::claims::{LEASE_TTL_MS, RENEW_EVERY_MS};
use glade_node::registry::Record;
use glade_node::sysdata::WorkspaceEntry;
use glade_wire::generated::{Op, StreamHeads};

use crate::fakes::FakeClock;
use crate::faults::Fault;
use crate::{pair, TestNode, T0, WS};

/// A chain tip: its seq and hash, by (stream, origin).
type Heads = BTreeMap<(String, String), (i64, Vec<u8>)>;

/// The heads `node` stored with its snapshot (`SystemSnapshot.heads`): the
/// resume vector a round restarts from.
fn stored_heads(node: &TestNode) -> Heads {
    let mut heads = Heads::new();
    for bytes in node.store.snapshot().heads {
        let stream = StreamHeads::from_cbor(&cbor::decode(&bytes));
        for head in stream.heads {
            let hash = head.hash.expect("a stored head carries its hash");
            heads.insert((stream.glade_id.clone(), head.origin), (head.seq, hash));
        }
    }
    heads
}

/// The tip each of `ops`' chains reaches, as a peer holding exactly them
/// would store it.
fn tips_of(ops: &[Op]) -> Heads {
    let tip = |op: &Op| {
        let chain = (op.glade_id.clone(), op.origin.clone());
        (chain, (op.seq, op_hash(op).to_vec()))
    };
    ops.iter().map(tip).collect()
}

/// What `ops` holds past `heads`: the ops a resumed round ships.
fn past(ops: Vec<Op>, heads: &Heads) -> Vec<Op> {
    let behind = |op: &Op| {
        let chain = (op.glade_id.clone(), op.origin.clone());
        heads.get(&chain).is_none_or(|(seq, _)| op.seq > *seq)
    };
    ops.into_iter().filter(behind).collect()
}

/// Restart mid-round: a node restarted in the middle of a round resumes from
/// its heads. A serves `WS` and renews twice, four records on two streams,
/// and pushes them to B over a link that fails once B has the first two. Both
/// restart. B's stored heads are the tips of what it accepted, with A's
/// hashes; A's exact retry saves nothing; the round ships only what lies past
/// B's heads, and B takes each op once, holding A's bytes and answering A past
/// the first lease, where before the resumption it answered none; A's next
/// renewal extends its reloaded chain, and B takes it. Phase 4 leaves open a
/// restart after a crash (the instance lock), resending a lost push (no
/// outbox) and the served store's own restart.
#[test]
fn restart_mid_round() {
    let clock = FakeClock::at(T0);
    let (a, b) = pair(&clock);
    let entry = WorkspaceEntry {
        workspace: WS.into(),
        name: "notes".into(),
        eligible_hosts: vec![a.name.clone()],
    };
    let mut records = vec![Record::Workspace(entry), a.lease(WS, 1)];
    for _ in 0..2 {
        clock.advance(RENEW_EVERY_MS as i64);
        records.push(a.lease(WS, 1));
    }
    for record in &records {
        assert!(a.append(record.clone()), "{record:?} is new");
    }
    let ops = a.store.ops();
    assert_eq!(ops.len(), 4);

    a.faults.next(Fault {
        frame: 1,
        delivered: true,
    });
    assert!(a.push(&ops).is_err(), "the link fails mid-round");
    let first = b.deliver();
    assert!(
        first.len() == 2 && first.iter().all(Result::is_ok),
        "{first:?}"
    );

    let (a, b) = (a.restart(), b.restart());
    let heads = stored_heads(&b);
    assert_eq!(heads, tips_of(&ops[..2]), "B resumes from what it accepted");
    let accepted = a.store.snapshot();
    assert!(!a.append(records[3].clone()), "an exact retry, restarted");
    assert_eq!(a.store.snapshot(), accepted, "saves nothing");

    clock.set(T0 + LEASE_TTL_MS);
    assert_eq!(b.serves(WS), None, "B holds only the lapsed first lease");
    let rest = past(a.store.ops(), &heads);
    assert_eq!(rest, ops[2..].to_vec(), "only what lies past B's heads");
    assert_eq!(a.push(&rest), Ok(2));
    let resumed = b.deliver();
    assert!(
        resumed.len() == 2 && resumed.iter().all(Result::is_ok),
        "{resumed:?}"
    );
    assert_eq!(b.store.snapshot(), a.store.snapshot(), "each op once");
    let answers = (a.serves(WS), b.serves(WS));
    assert_eq!(answers, (Some("a".into()), Some("a".into())));

    assert!(a.append(a.lease(WS, 1)), "a renewal after the restart");
    let renewal = a.store.ops().pop().expect("the renewal");
    let tip = &ops[3];
    let extends = (tip.seq + 1, Some(op_hash(tip).to_vec()));
    assert_eq!((renewal.seq, renewal.prev.clone()), extends, "no fork");
    assert_eq!(a.push(&[renewal]), Ok(1));
    assert!(b.deliver().iter().all(Result::is_ok));
    assert_eq!(b.store.snapshot(), a.store.snapshot());
}

/// Retry after a known failure (slice profile SP-L1): a record whose save
/// failed was not accepted, so no reader sees it and nothing may send it, and
/// a retry appends and saves it. A's save fails: the append is `Err(Io)`,
/// nothing is persisted, and A still answers none; once saves work, the same
/// append is `Ok(true)`, persisted, and A answers A. B's ingest and A's
/// registration behave alike, and across a restart the record is held. The
/// failure is injected: the volatile engine refuses when told, and on disk
/// (`tests/durable`) a directory stands where the temp file goes. Neither is a
/// full disk or a failing device.
#[test]
fn retry_after_a_failed_save() {
    let (a, b) = pair(&FakeClock::at(T0));
    let lease = a.lease(WS, 1);
    a.store.refuse_saves(true);
    let refused = a.host().append(lease.clone(), &a.name);
    assert!(matches!(refused, Err(HostError::Io(_))), "{refused:?}");
    assert!(a.store.ops().is_empty(), "nothing persisted");
    assert_eq!(a.serves(WS), None, "nor read");
    a.store.refuse_saves(false);
    assert!(a.append(lease.clone()), "the retry appends");
    assert_eq!(a.store.ops().len(), 1, "and saves");
    assert_eq!(a.serves(WS), Some("a".into()));

    b.store.refuse_saves(true);
    assert_eq!(a.push(&a.store.ops()), Ok(1));
    let refused = b.deliver();
    assert!(
        matches!(&refused[..], [Err(HostError::Io(_))]),
        "{refused:?}"
    );
    assert_eq!(b.serves(WS), None);
    b.store.refuse_saves(false);
    assert_eq!(a.push(&a.store.ops()), Ok(1));
    let landed = b.deliver();
    assert!(matches!(&landed[..], [Ok(())]), "{landed:?}");
    assert_eq!(b.serves(WS), Some("a".into()));

    let line = "binding notes.list value share commons latest\n";
    let decl = parse(&format!("glade-app v1\napp notes\n{line}")).unwrap();
    a.store.refuse_saves(true);
    let refused = a.directory().register(&decl, &a.name);
    assert!(matches!(refused, Err(HostError::Io(_))), "{refused:?}");
    a.store.refuse_saves(false);
    let registered = a.directory().register(&decl, &a.name).unwrap();
    let counts = (registered.appended, registered.unchanged);
    assert_eq!(counts, (1, 0), "the retry registers the line");

    let a = a.restart();
    assert!(!a.append(lease), "held across the restart");
    assert_eq!(a.serves(WS), Some("a".into()));
}
