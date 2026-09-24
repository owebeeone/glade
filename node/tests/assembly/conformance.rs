//! LBT-009: every provider a `NodeAssembly` composition binds for a Step 3.1
//! port runs that port's shared conformance suite, through each contract's
//! `conformance` feature. The test composition's fakes run the whole suite.
//! The assembled path's pending grant fold and signer run the fail-closed half
//! (GR-003, SI-003), which is all a provider that refuses can pass; its
//! system clock runs CL-002 (CL-001 needs a clock a test can set). No carrier
//! on the assembled path implements `CarrierPort` yet, so CA-001..004 run on
//! the fake network only.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use glade_carrier_api::conformance::{self as carrier, Fixture};
use glade_carrier_api::{CarrierAddr, CarrierPort};
use glade_clock_api::conformance as clock;
use glade_clock_api::ClockPort;
use glade_grant_api::conformance as grant;
use glade_node::assembly::{PendingGrantFold, PendingNodeSigner, SystemClock};
use glade_signer_api::conformance as signer;

use crate::fakes::{run, FakeClock, FakeNet, KeyedTestSigner, MemGrants, OTHER, STRANGER};

fn carrier_fixture() -> Fixture {
    let net = FakeNet::new();
    let port = |net: &Arc<FakeNet>| -> Arc<dyn CarrierPort> { Arc::new(net.port()) };
    Fixture {
        a: port(&net),
        b: port(&net),
        fresh: port(&net),
        at_a: CarrierAddr("a".into()),
        at_b: CarrierAddr("b".into()),
    }
}

#[test]
fn cl_001_the_fake_clock_is_one_instant_behind_every_handle() {
    let fake = FakeClock::at(0);
    let first: Arc<dyn ClockPort> = Arc::new(fake.clone());
    let second: Arc<dyn ClockPort> = Arc::new(fake.clone());
    clock::one_instant(&*first, &*second, &|instant| fake.set(instant));
}

#[test]
fn cl_002_the_fake_clock_reads_epoch_milliseconds() {
    let fake = FakeClock::at(1_700_000_000_000);
    clock::epoch_millis(&fake, &|| fake.now_ms());
}

#[test]
fn cl_002_the_system_clock_reads_epoch_milliseconds() {
    let reference = || {
        let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        i64::try_from(since.as_millis()).unwrap()
    };
    clock::epoch_millis(&SystemClock, &reference);
}

#[test]
fn ca_001_the_fake_network_carries_frames_whole_once_and_in_order() {
    run(carrier::frames(carrier_fixture()));
}

#[test]
fn ca_002_the_fake_network_holds_the_frame_limit_both_ways() {
    run(carrier::frame_limit(carrier_fixture()));
}

#[test]
fn ca_003_the_fake_networks_futures_are_lazy_and_recv_is_cancel_safe() {
    run(carrier::cancellation(carrier_fixture()));
}

#[test]
fn ca_004_a_fake_port_gives_its_endpoint_up_by_value() {
    run(carrier::close_by_value(carrier_fixture()));
}

#[test]
fn si_001_the_keyed_test_signer_round_trips_every_purpose() {
    signer::round_trip(&KeyedTestSigner::fixture());
}

#[test]
fn si_002_the_keyed_test_signer_binds_bytes_purpose_and_signer() {
    signer::binding(&KeyedTestSigner::fixture(), &OTHER, &STRANGER);
}

#[test]
fn si_003_the_keyed_test_signer_refuses_without_key_material() {
    signer::unavailable(&KeyedTestSigner::unavailable());
}

#[test]
fn si_003_the_pending_node_signer_refuses_both_ways() {
    signer::unavailable(&PendingNodeSigner);
}

#[test]
fn gr_001_the_in_memory_fold_matches_exactly() {
    grant::exact(&MemGrants::fixture());
}

#[test]
fn gr_002_in_the_in_memory_fold_revocation_wins_in_either_order() {
    grant::revocation_wins(&MemGrants::fixture());
}

#[test]
fn gr_003_an_unreadable_in_memory_fold_is_unavailable() {
    grant::unavailable(&MemGrants::unreadable());
}

#[test]
fn gr_003_the_pending_grant_fold_is_unavailable() {
    grant::unavailable(&PendingGrantFold);
}
