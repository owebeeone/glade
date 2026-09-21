//! The witness ports exercised the way a consumer sees them: through
//! `Arc<dyn …>`. Object safety is the load-bearing property here — the bridge
//! of `AsyncWitnessPlan.md` §4.3 hands callers back an `Arc<dyn CarrierPort>`,
//! so a port that is not dyn-compatible fails DI-E01 before Shaku is involved.
//!
//! No runtime, no socket, no sleep and no file (LBT-005, LBT-008): the fakes
//! never yield, so one poll under a noop waker resolves them, exactly as
//! `glade-lifecycle-api`'s conformance suite drives `ManagedResource`.

use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use async_witness_ports::{
    CarrierError, CarrierPort, ClockPort, FakeCarrier, FakeClock, FakeStore, FrameType, PortFuture,
    StorePort,
};

/// Resolve an already-complete port future. A witness fake that yields is a
/// defect in the fake, not a scheduling problem to solve with a runtime.
fn resolve<T>(mut future: PortFuture<'_, T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("a witness fake must never yield"),
    }
}

#[test]
fn fake_clock_reads_only_what_a_test_put_there() {
    let clock = FakeClock::new(1_000);
    assert_eq!(clock.now_ms(), 1_000);
    clock.set(7);
    assert_eq!(clock.now_ms(), 7);
    assert_eq!(clock.advance(5), 12);
    assert_eq!(clock.now_ms(), 12);
}

/// One selection, observed through the injected trait object at more than one
/// consumer. This is the shape DI-E01 asserts with `Arc::ptr_eq` once Shaku
/// does the injecting; here it is the port's own guarantee.
#[test]
fn a_cloned_fake_clock_shares_one_instant_behind_dyn() {
    let clock = FakeClock::new(0);
    let first: Arc<dyn ClockPort> = Arc::new(clock.clone());
    let second: Arc<dyn ClockPort> = Arc::new(clock.clone());
    clock.set(42);
    assert_eq!(first.now_ms(), 42);
    assert_eq!(second.now_ms(), 42);
}

#[test]
fn fake_carrier_records_every_frame_in_order() {
    let carrier = FakeCarrier::new();
    let port: Arc<dyn CarrierPort> = Arc::new(carrier.clone());
    assert_eq!(resolve(port.send(FrameType::NodeHello, b"one")), Ok(()));
    assert_eq!(resolve(port.send(FrameType::Ops, b"two")), Ok(()));
    assert_eq!(
        carrier.sent(),
        vec![
            (FrameType::NodeHello, b"one".to_vec()),
            (FrameType::Ops, b"two".to_vec()),
        ]
    );
}

#[test]
fn fake_carrier_replays_its_script_then_ends_the_stream() {
    let carrier = FakeCarrier::new();
    carrier.push_inbound(FrameType::NodeWelcome, b"hi");
    let port: Arc<dyn CarrierPort> = Arc::new(carrier);
    assert_eq!(
        resolve(port.recv()),
        Ok(Some((FrameType::NodeWelcome, b"hi".to_vec())))
    );
    assert_eq!(resolve(port.recv()), Ok(None));
}

#[test]
fn a_closed_carrier_refuses_sends_and_reports_end_of_stream() {
    let carrier = FakeCarrier::new();
    carrier.push_inbound(FrameType::Ops, b"dropped");
    carrier.close();
    let port: Arc<dyn CarrierPort> = Arc::new(carrier);
    assert_eq!(
        resolve(port.send(FrameType::Ops, b"late")),
        Err(CarrierError::Closed)
    );
    assert_eq!(resolve(port.recv()), Ok(None));
}

#[test]
fn fake_store_appends_in_sequence_and_scans_from_a_point() {
    let store = FakeStore::new();
    let port: Arc<dyn StorePort> = Arc::new(store);
    assert_eq!(port.append("share-a", b"first"), Ok(1));
    assert_eq!(port.append("share-a", b"second"), Ok(2));
    assert_eq!(port.append("share-b", b"other"), Ok(1));
    assert_eq!(port.scan("share-a", 2), vec![b"second".to_vec()]);
    assert_eq!(port.scan("share-b", 1), vec![b"other".to_vec()]);
    assert!(port.scan("share-c", 1).is_empty());
}

/// Every port is usable as a trait object, and the concrete fakes reach the
/// consumer through one. A regression here is a DI-E01 blocker.
#[test]
fn every_port_is_dyn_compatible() {
    let ports: (Arc<dyn ClockPort>, Arc<dyn CarrierPort>, Arc<dyn StorePort>) = (
        Arc::new(FakeClock::new(3)),
        Arc::new(FakeCarrier::new()),
        Arc::new(FakeStore::new()),
    );
    assert_eq!(ports.0.now_ms(), 3);
    assert_eq!(resolve(ports.1.send(FrameType::Hello, b"")), Ok(()));
    assert_eq!(ports.2.append("share", b""), Ok(1));
}
