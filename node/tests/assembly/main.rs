//! Plan Step 3.2: `NodeAssembly` in a test composition, where every provider
//! the assembled path binds as real is overridden by a deterministic fake.
//!
//! - DI-E01 (`arch1/DependencyInjectionEvaluation.md:133`): one fake selection
//!   reaches every consumer, by `Arc::ptr_eq` and by behaviour, and no real
//!   provider is constructed. The construction counter is process-wide, so no
//!   test in this binary may build an assembly with a real provider left in
//!   it; the positive control, which does, is `tests/assembly_registration.rs`.
//! - DI-E02 (`:134`): one scope hands out one occurrence per binding, sibling
//!   scopes share nothing, and a concurrent first resolution of a `#[lazy]`
//!   binding constructs once.
//! - The recipes' own claims (`arch1/InjectionGraphRefinement.md:21-25`): each
//!   carrier role is its own occurrence, the record transport rides the peer
//!   one, and one clock substitution reaches every consumer. `DirectoryRules`
//!   scopes what the record host ingests.
//!
//! The fakes are in `fakes.rs`; `conformance.rs` runs their contracts' suites.
//! Nothing here opens a file or a socket, starts a runtime or sleeps.

mod conformance;
mod fakes;

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use glade_carrier_api::{CarrierAddr, CarrierConfig, CarrierError, CarrierPort};
use glade_clock_api::ClockPort;
use glade_grant_api::{Denial, GrantPort, Holder};
use glade_node::appdecl::parse;
use glade_node::assembly::{
    real_providers_constructed, Admission, ClientCarrier, Clock, Config, Directory, DirectoryRules,
    Grants, HostError, NodeAssembly, PeerCarrier, RecordHost, RecordHostPort, RecordProfile,
    RecordProfilePort, RecordTransport, Records, Sessions, Settings, Signer, TransportPort,
};
use glade_node::registry::{Record, RegistryError, HOME};
use glade_node::sysdata::{
    BindingDecl, BindingRetraction, CapabilityGrant, CapabilityRevocation, NodeRecord,
    PrincipalRecord, ServeClaim, ServiceDefinition, WorkspaceEntry,
};
use glade_signer_api::SignerPort;
use glade_wire::cbor;
use glade_wire::generated::{Op, Shape};
use shaku::{HasComponent, ModuleBuilder};

use fakes::{run, FakeClock, FakeNet, FixedSettings, KeyedTestSigner, MemGrants};

/// Every provider the assembled path binds as real, overridden, except the
/// signer, which a test adds (once as a value, once as a counting factory).
fn builder(net: &Arc<FakeNet>, clock: &FakeClock) -> ModuleBuilder<NodeAssembly> {
    NodeAssembly::builder()
        .with_component_override::<dyn Config>(Box::new(FixedSettings(Settings::default())))
        .with_component_override::<dyn Clock>(Box::new(clock.clone()))
        .with_component_override::<dyn PeerCarrier>(Box::new(net.port()))
        .with_component_override::<dyn ClientCarrier>(Box::new(net.port()))
        .with_component_override::<dyn Grants>(Box::new(MemGrants::fixture()))
        // The in-memory store and registry, built with this scope's own
        // profile and record transport occurrences.
        .with_component_override_fn::<dyn RecordHost>(Box::new(|context| {
            let profile =
                <NodeAssembly as HasComponent<dyn RecordProfile>>::build_component(context);
            let transport =
                <NodeAssembly as HasComponent<dyn RecordTransport>>::build_component(context);
            Box::new(Records::in_memory(profile, transport))
        }))
}

/// One test node: a whole test composition on `net`, with `clock` its time.
fn test_node(net: &Arc<FakeNet>, clock: &FakeClock) -> NodeAssembly {
    builder(net, clock)
        .with_component_override::<dyn Signer>(Box::new(KeyedTestSigner::fixture()))
        .build()
}

// Each binding as the port it carries: the use site's view, which is the one
// the comparisons below are about.
fn clock_of(node: &NodeAssembly) -> Arc<dyn ClockPort> {
    let facade: Arc<dyn Clock> = node.resolve();
    facade
}

fn peer_of(node: &NodeAssembly) -> Arc<dyn CarrierPort> {
    let facade: Arc<dyn PeerCarrier> = node.resolve();
    facade
}

fn client_of(node: &NodeAssembly) -> Arc<dyn CarrierPort> {
    let facade: Arc<dyn ClientCarrier> = node.resolve();
    facade
}

fn transport_of(node: &NodeAssembly) -> Arc<dyn TransportPort> {
    let facade: Arc<dyn RecordTransport> = node.resolve();
    facade
}

fn host_of(node: &NodeAssembly) -> Arc<dyn RecordHostPort> {
    let facade: Arc<dyn RecordHost> = node.resolve();
    facade
}

fn profile_of(node: &NodeAssembly) -> Arc<dyn RecordProfilePort> {
    let facade: Arc<dyn RecordProfile> = node.resolve();
    facade
}

fn grants_of(node: &NodeAssembly) -> Arc<dyn GrantPort> {
    let facade: Arc<dyn Grants> = node.resolve();
    facade
}

fn signer_of(node: &NodeAssembly) -> Arc<dyn SignerPort> {
    let facade: Arc<dyn Signer> = node.resolve();
    facade
}

fn bind(port: &Arc<dyn CarrierPort>, at: &str) -> Result<CarrierAddr, CarrierError> {
    let config = CarrierConfig {
        local: CarrierAddr(at.into()),
        max_frame_bytes: NonZeroUsize::new(1 << 16).unwrap(),
    };
    run(port.bind(config))
}

fn claim(node: &str, share: &str, lease_expiry_ms: i64) -> Record {
    let (node, share) = (node.into(), share.into());
    Record::Serve(ServeClaim {
        node,
        share,
        lease_expiry_ms,
        epoch: 1,
    })
}

fn alice() -> Holder {
    Holder::Principal("alice".into())
}

/// DI-E01: one clock substitution reaches both consumers the recipe names,
/// and it is one instant for both: the directory's lease reading and the
/// admission's decision move together when the one fake moves.
#[test]
fn one_clock_substitution_reaches_the_directory_and_admission() {
    let clock = FakeClock::at(1_000);
    let node = test_node(&FakeNet::new(), &clock);
    let directory: Arc<dyn Directory> = node.resolve();
    let admission: Arc<dyn Admission> = node.resolve();

    let selected = clock_of(&node);
    for (consumer, reached) in [
        ("directory", directory.clock()),
        ("admission", admission.clock()),
    ] {
        assert!(
            Arc::ptr_eq(&selected, &reached),
            "the clock did not reach {consumer}"
        );
    }

    let appended = directory.host().append(claim("n1", "ws-a", 1_500), "n1");
    assert!(appended.expect("the claim is appended"));
    assert_eq!(
        directory.serves("ws-a").expect("an open host"),
        Some("n1".into())
    );
    assert_eq!(admission.admit(&alice(), "read", "ws-a").at_ms, 1_000);

    clock.advance(600);
    assert_eq!(
        directory.serves("ws-a").expect("an open host"),
        None,
        "the lease lapsed at 1_600"
    );
    assert_eq!(admission.admit(&alice(), "read", "ws-a").at_ms, 1_600);
    assert_eq!(real_providers_constructed(), 0);
}

/// DI-E01 for the carriers, and the recipes' role rule: each role is its own
/// occurrence, and the record transport rides the peer occurrence, never the
/// client one. Binding the peer role through `Sessions` binds the very port
/// the record transport holds.
#[test]
fn each_carrier_role_is_its_own_occurrence_and_the_record_transport_rides_the_peer_one() {
    let node = test_node(&FakeNet::new(), &FakeClock::at(0));
    let sessions: Arc<dyn Sessions> = node.resolve();
    let transport: Arc<dyn RecordTransport> = node.resolve();
    let (peer, client) = (peer_of(&node), client_of(&node));

    assert!(!Arc::ptr_eq(&peer, &client), "two roles, one occurrence");
    assert!(Arc::ptr_eq(&peer, &sessions.peer()));
    assert!(Arc::ptr_eq(&client, &sessions.client()));
    assert!(Arc::ptr_eq(&peer, &transport.carrier()));
    assert!(!Arc::ptr_eq(&client, &transport.carrier()));

    assert!(bind(&sessions.peer(), "a-peer").is_ok());
    assert_eq!(
        bind(&transport.carrier(), "elsewhere"),
        Err(CarrierError::AlreadyBound)
    );
    assert!(
        bind(&sessions.client(), "a-client").is_ok(),
        "the client role is another port"
    );
    assert_eq!(real_providers_constructed(), 0);
}

/// DI-E01 for the record host, its profile and its transport: the directory
/// writes to the one host, and the host holds this scope's own profile and
/// record transport. An app file registered twice appends nothing the second
/// time, through the node's own `register` over the in-memory registry.
#[test]
fn one_record_host_selection_reaches_the_directory() {
    let node = test_node(&FakeNet::new(), &FakeClock::at(0));
    let directory: Arc<dyn Directory> = node.resolve();
    let host: Arc<dyn RecordHost> = node.resolve();

    assert!(Arc::ptr_eq(&host_of(&node), &directory.host()));
    assert!(Arc::ptr_eq(&profile_of(&node), &host.profile()));
    assert!(Arc::ptr_eq(&transport_of(&node), &host.transport()));

    let decl = parse("glade-app v1\napp x\nbinding x.one value share commons latest\n").unwrap();
    let first = directory.register(&decl, "n1").expect("registered");
    let again = directory.register(&decl, "n1").expect("registered again");
    assert_eq!((first.appended, first.unchanged), (1, 0));
    assert_eq!((again.appended, again.unchanged), (0, 1));
    assert_eq!(real_providers_constructed(), 0);
}

/// DI-E01 for the grant binding: admission consults the one fold selected.
#[test]
fn one_grant_selection_reaches_admission() {
    let node = test_node(&FakeNet::new(), &FakeClock::at(0));
    let admission: Arc<dyn Admission> = node.resolve();
    assert!(Arc::ptr_eq(&grants_of(&node), &admission.grants()));
    assert_eq!(admission.admit(&alice(), "write", "ws-a").outcome, Ok(()));
    let eve = Holder::Principal("eve".into());
    assert_eq!(
        admission.admit(&eve, "read", "ws-a").outcome,
        Err(Denial::Revoked)
    );
    assert_eq!(real_providers_constructed(), 0);
}

/// DI-E01's second half: resolving every binding and participant, the lazy
/// ones included, and using each, constructs no real provider. Every provider
/// that could open a file or a socket is overridden; what is left is the
/// profile, the participants and the record transport's view, which are pure.
#[test]
fn a_test_composition_constructs_no_real_provider() {
    let node = test_node(&FakeNet::new(), &FakeClock::at(7));
    let config: Arc<dyn Config> = node.resolve();
    let directory: Arc<dyn Directory> = node.resolve();
    let admission: Arc<dyn Admission> = node.resolve();
    let sessions: Arc<dyn Sessions> = node.resolve();

    assert_eq!(config.settings(), &Settings::default());
    assert_eq!(directory.serves(HOME).expect("an open host"), None);
    assert_eq!(admission.admit(&alice(), "read", "ws-a").at_ms, 7);
    assert!(bind(&sessions.client(), "client").is_ok());
    assert_eq!(profile_of(&node).share(), HOME);
    let signed = signer_of(&node).sign(glade_signer_api::Purpose::PeerHello, b"hello");
    assert!(signed.is_ok());
    assert_eq!(real_providers_constructed(), 0);
}

/// DI-E02, first claim: every resolution of a binding from one scope is one
/// occurrence, and so is every copy a consumer holds.
#[test]
fn one_scope_hands_out_one_occurrence_of_every_binding() {
    let node = test_node(&FakeNet::new(), &FakeClock::at(0));
    assert!(Arc::ptr_eq(&clock_of(&node), &clock_of(&node)));
    assert!(Arc::ptr_eq(&peer_of(&node), &peer_of(&node)));
    assert!(Arc::ptr_eq(&client_of(&node), &client_of(&node)));
    assert!(Arc::ptr_eq(&transport_of(&node), &transport_of(&node)));
    assert!(Arc::ptr_eq(&host_of(&node), &host_of(&node)));
    assert!(Arc::ptr_eq(&profile_of(&node), &profile_of(&node)));
    assert!(Arc::ptr_eq(&grants_of(&node), &grants_of(&node)));
    assert!(Arc::ptr_eq(&signer_of(&node), &signer_of(&node)));

    let first: Arc<dyn Directory> = node.resolve();
    let again: Arc<dyn Directory> = node.resolve();
    assert!(Arc::ptr_eq(&first, &again));
    let sessions: Arc<dyn Sessions> = node.resolve();
    let sessions_again: Arc<dyn Sessions> = node.resolve();
    assert!(Arc::ptr_eq(&sessions, &sessions_again));
}

/// DI-E02, second claim: a sibling test node is a scope of its own. The two
/// share no occurrence, their clocks and hosts move independently, and they
/// meet only through the network both are bound on: A's record transport
/// pushes two records to B's peer address, and B's host still holds nothing.
#[test]
fn sibling_scopes_share_nothing_and_meet_only_through_the_network() {
    let net = FakeNet::new();
    let (clock_a, clock_b) = (FakeClock::at(10), FakeClock::at(20));
    let (a, b) = (test_node(&net, &clock_a), test_node(&net, &clock_b));

    assert!(!Arc::ptr_eq(&clock_of(&a), &clock_of(&b)));
    assert!(!Arc::ptr_eq(&peer_of(&a), &peer_of(&b)));
    assert!(!Arc::ptr_eq(&client_of(&a), &client_of(&b)));
    assert!(!Arc::ptr_eq(&transport_of(&a), &transport_of(&b)));
    assert!(!Arc::ptr_eq(&host_of(&a), &host_of(&b)));
    assert!(!Arc::ptr_eq(&profile_of(&a), &profile_of(&b)));
    assert!(!Arc::ptr_eq(&grants_of(&a), &grants_of(&b)));
    assert!(!Arc::ptr_eq(&signer_of(&a), &signer_of(&b)));

    clock_a.advance(5);
    assert_eq!((clock_of(&a).now_ms(), clock_of(&b).now_ms()), (15, 20));

    let (dir_a, dir_b): (Arc<dyn Directory>, Arc<dyn Directory>) = (a.resolve(), b.resolve());
    assert!(dir_a
        .host()
        .append(claim("n-a", "ws-a", 1_000), "n-a")
        .unwrap());
    assert_eq!(dir_a.serves("ws-a").unwrap(), Some("n-a".into()));
    assert_eq!(dir_b.serves("ws-a").unwrap(), None);

    let at_b = bind(&peer_of(&b), "b-peer").expect("B binds its peer role");
    bind(&peer_of(&a), "a-peer").expect("A binds its peer role");
    let records = [b"one".to_vec(), b"two".to_vec()];
    let transport: Arc<dyn RecordTransport> = a.resolve();
    assert_eq!(run(transport.push(&at_b, &records)), Ok(2));
    let link = run(peer_of(&b).accept())
        .unwrap()
        .expect("A's link reaches B");
    assert_eq!(run(link.recv()), Ok(Some(b"one".to_vec())));
    assert_eq!(run(link.recv()), Ok(Some(b"two".to_vec())));
    assert_eq!(run(link.recv()), Ok(None), "the push closes its link");
    assert_eq!(dir_b.serves("ws-a").unwrap(), None);
    assert_eq!(real_providers_constructed(), 0);
}

/// A test node whose `#[lazy]` signer is built by a factory that counts its
/// constructions, so a count is the construction itself.
fn counted_node(built: &Arc<AtomicUsize>) -> NodeAssembly {
    let counted = built.clone();
    builder(&FakeNet::new(), &FakeClock::at(0))
        .with_component_override_fn::<dyn Signer>(Box::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::new(KeyedTestSigner::fixture())
        }))
        .build()
}

/// DI-E02, third claim: four threads meet at one barrier and race the first
/// resolution of a `#[lazy]` binding, the signer, two in each of two sibling
/// scopes. Each scope constructs it exactly once and hands both its racers
/// that one occurrence, and the two scopes' occurrences differ. Shaku backs a
/// lazy slot with `std::sync::OnceLock`.
#[test]
fn a_concurrent_first_access_of_a_lazy_binding_constructs_once_per_scope() {
    let built_a = Arc::new(AtomicUsize::new(0));
    let built_b = Arc::new(AtomicUsize::new(0));
    let (a, b) = (counted_node(&built_a), counted_node(&built_b));
    let counts = || {
        let a = built_a.load(Ordering::SeqCst);
        (a, built_b.load(Ordering::SeqCst))
    };
    assert_eq!(counts(), (0, 0), "lazy: the race below is the first access");

    let gate = Barrier::new(4);
    // Captures only `&gate`, so each racer gets its own copy.
    let race = |node: &NodeAssembly| -> Arc<dyn Signer> {
        gate.wait();
        node.resolve()
    };
    let [a1, a2, b1, b2] = std::thread::scope(|scope| {
        [&a, &a, &b, &b]
            .map(|node| scope.spawn(move || race(node)))
            .map(|racer| racer.join().expect("a racer"))
    });
    assert_eq!(counts(), (1, 1), "a scope constructed its signer twice");
    assert!(Arc::ptr_eq(&a1, &a2));
    assert!(Arc::ptr_eq(&b1, &b2));
    assert!(
        !Arc::ptr_eq(&a1, &b1),
        "sibling scopes shared an occurrence"
    );
    assert_eq!(real_providers_constructed(), 0);
}

/// `DirectoryRules` is the directory profile: the home share, and a stream
/// for every kind of directory record and for nothing else.
#[test]
fn the_directory_profile_hosts_every_directory_record_kind_and_nothing_else() {
    let s = String::new;
    let kinds = [
        Record::Node(NodeRecord {
            node_id: s(),
            operator: s(),
        }),
        Record::Workspace(WorkspaceEntry {
            workspace: s(),
            name: s(),
            eligible_hosts: vec![],
        }),
        claim("", "", 0),
        Record::Grant(CapabilityGrant {
            principal: s(),
            share: s(),
            verbs: vec![],
        }),
        Record::Revoke(CapabilityRevocation {
            principal: s(),
            share: s(),
        }),
        Record::Binding(BindingDecl::default()),
        Record::Retract(BindingRetraction {
            app: s(),
            glade_id: s(),
        }),
        Record::Service(ServiceDefinition {
            app: s(),
            name: s(),
            glade_id: s(),
        }),
        Record::Principal(PrincipalRecord { principal: s() }),
    ];
    assert_eq!(DirectoryRules.share(), HOME);
    for record in &kinds {
        assert!(
            DirectoryRules.hosts(record.glade_id()),
            "{}",
            record.glade_id()
        );
    }
    assert!(!DirectoryRules.hosts("app.notes"));
}

fn node_op(share: &str, glade_id: &str, seq: i64, prev: Option<Vec<u8>>) -> Op {
    let record = NodeRecord {
        node_id: "peer-1".into(),
        operator: "o".into(),
    };
    Op {
        share: share.into(),
        glade_id: glade_id.into(),
        key: vec![],
        origin: "peer-1".into(),
        seq,
        prev,
        lamport: seq,
        refs: vec![],
        shape: Shape::Log,
        payload: cbor::encode(&record.to_cbor()),
    }
}

/// The record host scopes what it ingests by the profile (the wrong-scope
/// journey of Step 3.4 stands on this): an op on another share, or on a
/// stream the profile does not host, is refused before the registry's
/// verify-as-ingest sees it; an op in scope then meets the chain checks.
#[test]
fn records_refuse_an_op_outside_the_directory_profile_before_verifying_it() {
    let host = host_of(&test_node(&FakeNet::new(), &FakeClock::at(0)));
    let outside = host.ingest(node_op("ws-x", "dir.nodes", 0, None));
    assert!(
        matches!(outside, Err(HostError::OutOfScope { .. })),
        "{outside:?}"
    );
    let unhosted = host.ingest(node_op(HOME, "app.notes", 0, None));
    assert!(
        matches!(unhosted, Err(HostError::OutOfScope { .. })),
        "{unhosted:?}"
    );
    assert!(host.ingest(node_op(HOME, "dir.nodes", 0, None)).is_ok());
    let broken = host.ingest(node_op(HOME, "dir.nodes", 1, Some(vec![0; 32])));
    let chain_break = RegistryError::ChainBreak {
        origin: "peer-1".into(),
        seq: 1,
    };
    assert!(matches!(broken, Err(HostError::Rejected(e)) if e == chain_break));
}
