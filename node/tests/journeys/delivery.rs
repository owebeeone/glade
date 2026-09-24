//! Part (i) of plan Step 3.4: publish, exact retry and lost acknowledgement.
//! A publishes as `claims.rs`'s `serve_workspace_on` mints: the workspace
//! entry, then the first claim, epoch 1, both under A's origin.

use glade_carrier_api::CarrierError;
use glade_node::registry::Record;
use glade_node::sysdata::WorkspaceEntry;

use crate::fakes::FakeClock;
use crate::faults::Fault;
use crate::{pair, TestNode, T0, WS};

/// A serves `WS`: its entry and its first claim, each appended new.
fn serve(a: &TestNode) -> [Record; 2] {
    let entry = WorkspaceEntry {
        workspace: WS.into(),
        name: "notes".into(),
        eligible_hosts: vec![a.name.clone()],
    };
    let records = [Record::Workspace(entry), a.lease(WS, 1)];
    for record in &records {
        assert!(a.append(record.clone()), "{record:?} is new");
    }
    records
}

/// Publish: a registration made at A is discoverable at B across the fixed
/// route, A's configured peer. A appends and persists; its record transport
/// pushes what it persisted over its peer carrier; B ingests it; both answer
/// A at their clocks, and B holds A's bytes as A persisted them. The fakes
/// prove no transport, durability or signature (the ops are unsigned). Phase 4
/// substitutes the iroh `CarrierPort` (4.2, 4.5), the served store as the
/// record host (4.4) and signing (4.1), and 4.6 runs the route.
#[test]
fn publish() {
    let (a, b) = pair(&FakeClock::at(T0));
    serve(&a);
    assert_eq!(a.serves(WS), Some("a".into()));
    assert_eq!(b.serves(WS), None, "nothing has reached B");

    assert_eq!(a.push(&a.store.ops()), Ok(2));
    let answers = b.deliver();
    assert_eq!(answers.len(), 2);
    assert!(answers.iter().all(Result::is_ok), "{answers:?}");
    assert_eq!(b.serves(WS), Some("a".into()));
    assert_eq!(b.store.snapshot(), a.store.snapshot(), "B holds A's bytes");
}

/// Exact retry: appending a record already held appends nothing and
/// persists nothing, so it never extends the lease, before or after the
/// lease lapses, and revives nothing; only a new stamp is a new record. The
/// engine is volatile, so a retry across a restart is 4.4's (the durable
/// store as the record host).
#[test]
fn exact_retry() {
    let clock = FakeClock::at(T0);
    let (a, _b) = pair(&clock);
    let [_, claim] = serve(&a);
    let accepted = a.store.snapshot();

    clock.advance(1);
    assert!(!a.append(claim.clone()), "an exact retry appends nothing");
    assert_eq!(a.store.snapshot(), accepted, "and persists nothing");

    clock.set(T0 + glade_node::claims::LEASE_TTL_MS);
    assert_eq!(a.serves(WS), None, "the retry did not extend the lease");
    assert!(!a.append(claim), "nor does one after the lapse");
    assert_eq!(a.serves(WS), None, "which revives nothing");
    assert_eq!(a.store.snapshot(), accepted);

    assert!(a.append(a.lease(WS, 1)), "a new stamp is a new record");
    assert_eq!(a.serves(WS), Some("a".into()));
}

/// Lost acknowledgement: A's push fails after B has every frame, so A is
/// told `Transport` and cannot know what arrived (the push has no
/// acknowledgement; `send` is not one). A retries: its exact retry mints
/// nothing, so it resends the bytes it persisted, and B, receiving each op
/// twice, takes each repeat as a duplicate: it answers `Ok`, holds each op
/// once and answers as before. Phase 4: the iroh adapter's failure evidence
/// and TR-002's unknown outcome (4.2, 4.5); 4.4 across a restart.
#[test]
fn lost_acknowledgement() {
    let (a, b) = pair(&FakeClock::at(T0));
    let records = serve(&a);
    let ops = a.store.ops();

    let last = Fault {
        frame: 1,
        delivered: true,
    };
    a.faults.next(last);
    let pushed = a.push(&ops);
    assert!(
        matches!(pushed, Err(CarrierError::Transport(_))),
        "{pushed:?}"
    );
    let answers = b.deliver();
    assert!(
        answers.len() == 2 && answers.iter().all(Result::is_ok),
        "{answers:?}"
    );
    let held = b.store.snapshot();

    for record in records {
        assert!(!a.append(record), "nothing to mint again");
    }
    assert_eq!(a.store.ops(), ops, "the retry carries the original bytes");
    assert_eq!(a.push(&ops), Ok(2));
    let again = b.deliver();
    assert_eq!(b.store.snapshot(), held, "B holds each op once");
    assert_eq!(b.serves(WS), Some("a".into()));

    // A byte-identical re-delivery is a duplicate, not a fork (owner,
    // 2026-09-24, plan Step 4.4), as the wire store's `Duplicate` is: this was
    // pinned as `Equivocation` until the ruling.
    assert_eq!(again.len(), 2);
    assert!(again.iter().all(Result::is_ok), "{again:?}");
}
