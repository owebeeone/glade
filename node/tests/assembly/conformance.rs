//! LBT-009: every provider a `NodeAssembly` composition binds for a Step 3.1
//! port runs that port's shared conformance suite, through each contract's
//! `conformance` feature. The test composition's fakes run the whole suite.
//! The node's grant fold (plan Step 4.3), the adapter the served node checks
//! and the assembled path binds, runs the whole of GR-001..003: over a registry
//! holding the fixture's grants and revocations as records, and built holding
//! no fold, as the module builds it; its Ed25519
//! signer (plan Step 4.1a) runs the whole of SI-001..003, SI-003 with no key
//! lent; its system clock runs CL-002 (CL-001 needs a clock a test can set).
//! CA-001..005 run here on the fake network. The assembled path's iroh
//! adapter (plan Step 4.2c) runs them on real iroh over loopback in its own
//! module's tests (`src/iroh_carrier.rs`), since this binary opens no socket.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use glade_carrier_api::conformance::{self as carrier, Fixture};
use glade_carrier_api::{CarrierAddr, CarrierPort};
use glade_clock_api::conformance as clock;
use glade_clock_api::ClockPort;
use glade_grant_api::conformance::{self as grant, Record as GrantRecord};
use glade_grant_api::Holder;
use glade_node::assembly::SystemClock;
use glade_node::grants::PolicyView;
use glade_node::peer::NodeIdentity;
use glade_node::registry::{Record, Registry, RegistryApi};
use glade_node::signing::NodeSigner;
use glade_node::sysdata::{CapabilityGrant, CapabilityRevocation};
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
fn ca_005_the_fake_network_names_each_far_end() {
    run(carrier::remote_identity(carrier_fixture()));
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

/// The Ed25519 signer over a key, as a booted node's assembly builds it.
fn node_signer() -> NodeSigner {
    NodeSigner::new(Some(NodeIdentity::from_key([11; 32])))
}

#[test]
fn si_001_the_node_signer_round_trips_every_purpose() {
    signer::round_trip(&node_signer());
}

/// On real keys: `other` is a node the signer has recorded as authenticated,
/// `stranger` a genuine key it has not, so it resolves the one and not the
/// other.
#[test]
fn si_002_the_node_signer_binds_bytes_purpose_and_signer() {
    let port = node_signer();
    let other = NodeIdentity::from_key([22; 32]).node_id;
    let stranger = NodeIdentity::from_key([33; 32]).node_id;
    port.authenticated(other);
    signer::binding(&port, &other, &stranger);
}

/// With no key lent, as the legacy form builds it.
#[test]
fn si_003_the_node_signer_without_a_key_refuses_both_ways() {
    signer::unavailable(&NodeSigner::new(None));
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

/// The name a grant record gives a fixture holder: a node by its id in
/// lower-case hex, a principal by its name.
fn named(holder: &Holder) -> String {
    match holder {
        Holder::Node(id) => id.iter().map(|b| format!("{b:02x}")).collect(),
        Holder::Principal(name) => name.clone(),
    }
}

/// The node's grant fold over a registry holding the fixture fold, record by
/// record in its order, as `seed` and `revoke` lines register them.
fn node_fold() -> PolicyView {
    let mut registry = Registry::new();
    for record in grant::fold() {
        let record = match record {
            GrantRecord::Grant {
                holder,
                share,
                verbs,
            } => Record::Grant(CapabilityGrant {
                principal: named(&holder),
                share: share.into(),
                verbs: verbs.iter().map(|verb| verb.to_string()).collect(),
            }),
            GrantRecord::Revoke { holder, share } => Record::Revoke(CapabilityRevocation {
                principal: named(&holder),
                share: share.into(),
            }),
        };
        registry.append(record, "node-1").unwrap();
    }
    PolicyView::of(registry.policy())
}

#[test]
fn gr_001_to_003_the_nodes_grant_fold_keeps_the_grant_contract() {
    let fold = node_fold();
    grant::exact(&fold);
    grant::revocation_wins(&fold);
    fold.replace(None);
    grant::unavailable(&fold);
}

#[test]
fn gr_003_the_grant_fold_the_module_builds_is_unavailable() {
    grant::unavailable(&PolicyView::unavailable());
}
