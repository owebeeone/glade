//! Part (ii) of plan Step 3.4: renewal, expiry and partial lookup, on the
//! lease `claims.rs` mints (`LEASE_TTL_MS`, renewed every `RENEW_EVERY_MS`
//! with the same epoch), driven by the fake clock. `claims.rs` itself is not
//! run: its loop sleeps on tokio and stamps with `sysdir::now_ms()`, so these
//! journeys write the records it writes, at the injected clock.

use glade_carrier_api::CarrierError;
use glade_clock_api::ClockPort;
use glade_node::assembly::HostError;
use glade_node::claims::{LEASE_TTL_MS, RENEW_EVERY_MS};
use glade_node::registry::RegistryError;
use glade_node::sysdata::ServeClaim;

use crate::fakes::{FakeClock, FakeNet};
use crate::faults::{Fault, LiveGrants};
use crate::{pair, TestNode, T0, WS};

const RENEW: i64 = RENEW_EVERY_MS as i64;

/// Push `from`'s persisted ops from index `next` on to `to`, which ingests
/// every one: the index to push from next time.
fn relay(from: &TestNode, to: &TestNode, next: usize) -> usize {
    let ops = from.store.ops();
    assert_eq!(from.push(&ops[next..]), Ok(ops.len() - next));
    let answers = to.deliver();
    assert!(answers.iter().all(Result::is_ok), "{answers:?}");
    ops.len()
}

/// Renewal: every `RENEW_EVERY_MS` A appends a fresh stamp of its claim, a
/// new record with the same epoch, and pushes it; past the first lease's
/// horizon both A and B still answer A, and a lease after the last renewal
/// both answer none. The fake proves nothing of `claims.rs`'s loop or its
/// clock reads; Phase 4 runs that loop, owned by `Records` (3.3), on the
/// clock binding and over iroh.
#[test]
fn renewal() {
    let clock = FakeClock::at(T0);
    let (a, b) = pair(&clock);
    assert!(a.append(a.lease(WS, 1)));
    let mut next = relay(&a, &b, 0);
    for tick in 1..=3 {
        clock.advance(RENEW);
        assert!(a.append(a.lease(WS, 1)), "renewal {tick} is a new record");
        next = relay(&a, &b, next);
        for node in [&a, &b] {
            assert_eq!(node.serves(WS), Some("a".into()), "tick {tick}");
        }
    }
    assert_eq!(next, 4);
    assert_eq!(clock.now_ms(), T0 + LEASE_TTL_MS, "past the first lease");

    let last = T0 + 3 * RENEW + LEASE_TTL_MS;
    clock.set(last - 1);
    assert_eq!(
        (a.serves(WS), b.serves(WS)),
        (Some("a".into()), Some("a".into()))
    );
    clock.set(last);
    assert_eq!((a.serves(WS), b.serves(WS)), (None, None));
    for op in b.store.ops() {
        let claim = ServeClaim::from_cbor(&glade_node::cbor::decode(&op.payload));
        assert_eq!(claim.epoch, 1, "a renewal keeps the epoch");
    }
}

/// Expiry: one stamp, judged at each reader's own instant, live while the
/// stamp is ahead of the reader (`lease > now`); B's clock runs ahead, so B
/// sees the lapse first. The records do not change: expiry is read, never
/// written. The fakes prove no real clock or skew bound; Phase 4 has the
/// system clock, and clock uncertainty is not decided (slice profile SP-C2).
#[test]
fn expiry() {
    const AHEAD: i64 = 5_000;
    let net = FakeNet::new();
    let (clock_a, clock_b) = (FakeClock::at(T0), FakeClock::at(T0 + AHEAD));
    let a = TestNode::new(&net, &clock_a, "a", &["b"], LiveGrants::fixture());
    let b = TestNode::new(&net, &clock_b, "b", &[], LiveGrants::fixture());
    assert!(a.append(a.lease(WS, 1)));
    relay(&a, &b, 0);
    let held = (a.store.snapshot(), b.store.snapshot());
    let lease = T0 + LEASE_TTL_MS;

    clock_a.set(lease - AHEAD - 1);
    clock_b.set(lease - 1);
    assert_eq!(
        (a.serves(WS), b.serves(WS)),
        (Some("a".into()), Some("a".into()))
    );
    clock_a.advance(1);
    clock_b.advance(1);
    assert_eq!((a.serves(WS), b.serves(WS)), (Some("a".into()), None));
    clock_a.set(lease);
    assert_eq!(a.serves(WS), None);
    assert_eq!((a.store.snapshot(), b.store.snapshot()), held);
}

/// Partial lookup: B answers from the records it holds, verified, and from
/// nothing else. A pushes three stamps, one link each; the second link loses
/// its frame, so B refuses the third (`Gap`) rather than fold around the hole
/// and, holding only the first, answers none where A answers A. The rest,
/// pushed again from B's head as a sync round resumes, makes B answer A. The
/// node's lookup answers one node or none: it has no truncation, limit or
/// observed-at stamp, and whether it must report clock uncertainty is not
/// decided (slice profile §8 item 11). Phase 4: 4.6's sync round over iroh.
#[test]
fn partial_lookup() {
    let clock = FakeClock::at(T0);
    let (a, b) = pair(&clock);
    assert!(a.append(a.lease(WS, 1)));
    relay(&a, &b, 0);

    clock.advance(RENEW);
    assert!(a.append(a.lease(WS, 1)));
    a.faults.next(Fault {
        frame: 0,
        delivered: false,
    });
    let lost = a.push(&a.store.ops()[1..]);
    assert!(matches!(lost, Err(CarrierError::Transport(_))), "{lost:?}");
    assert!(b.deliver().is_empty(), "the second stamp is lost");

    clock.advance(RENEW);
    assert!(a.append(a.lease(WS, 1)));
    assert_eq!(a.push(&a.store.ops()[2..]), Ok(1));
    let past_the_hole = b.deliver();
    let gap = RegistryError::Gap {
        expected: 1,
        got: 2,
    };
    assert!(
        matches!(&past_the_hole[..], [Err(HostError::Rejected(e))] if *e == gap),
        "{past_the_hole:?}"
    );

    clock.set(T0 + LEASE_TTL_MS + 5_000);
    assert_eq!(a.serves(WS), Some("a".into()));
    assert_eq!(b.serves(WS), None, "B answers from its prefix");

    let head = b.store.ops().iter().map(|op| op.seq).max();
    let rest: Vec<_> = a
        .store
        .ops()
        .into_iter()
        .filter(|op| Some(op.seq) > head)
        .collect();
    assert_eq!(a.push(&rest), Ok(2));
    assert!(b.deliver().iter().all(Result::is_ok));
    assert_eq!(b.serves(WS), Some("a".into()));
}
