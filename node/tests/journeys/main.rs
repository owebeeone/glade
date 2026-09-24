//! Plan Step 3.4: the eight journeys of the build entry's step 2
//! (`dev-docs/GladeBuildEntry.md:52-56`), each a consumer test of `NodeAssembly`
//! in a test composition, AR-05 the criterion. The design, with what each
//! journey drives, what it asserts, what its fakes do not prove and the Phase 4
//! provider that replaces them, is `glade/dev-docs/GladeNodeAssembly.md`,
//! "Journeys (plan Step 3.4)".
//!
//! Part (i), `delivery.rs`: publish, exact retry, lost acknowledgement. Part
//! (ii), `leases.rs` and `admission.rs`: renewal, expiry, partial lookup, wrong
//! scope, unknown or denied authority. Nothing here opens a file or a socket,
//! starts a runtime or sleeps: `fakes::run` polls every future, and the fake
//! clock and each test's own steps are the whole schedule.

// Step 3.2's fakes, shared with `tests/assembly`: this binary uses some of them.
#[allow(dead_code)]
#[path = "../assembly/fakes.rs"]
mod fakes;

// Part (i): this harness, the fakes it adds, and three journeys.
mod delivery;
mod faults;

// Part (ii): five journeys.
mod admission;
mod leases;

use std::num::NonZeroUsize;
use std::sync::Arc;

use glade_carrier_api::{CarrierAddr, CarrierConfig, CarrierError};
use glade_node::assembly::{
    ClientCarrier, Clock, Config, Directory, Grants, HostError, NodeAssembly, PeerCarrier,
    RecordHost, RecordHostPort, RecordProfile, RecordTransport, Records, Settings, Signer,
};
use glade_node::cbor;
use glade_node::claims::LEASE_TTL_MS;
use glade_node::registry::Record;
use glade_node::sysdata::ServeClaim;
use glade_wire::generated::Op;
use shaku::HasComponent;

use fakes::{run, FakeClock, FakeNet, FixedSettings, KeyedTestSigner};
use faults::{Faults, FaultyPort, LiveGrants, VolatileStore};

/// The workspace every journey serves, and the instant each starts at.
pub const WS: &str = "ws-notes";
pub const T0: i64 = 1_700_000_000_000;

/// One test node: a whole test composition of `NodeAssembly` on a fake
/// network, its peer carrier bound at its name, its configuration naming its
/// peers. The handles are the test's: the engine its record host persists
/// through, and the faults its dialed links suffer.
pub struct TestNode {
    pub name: String,
    pub module: NodeAssembly,
    pub store: VolatileStore,
    pub faults: Faults,
}

impl TestNode {
    /// A node named `name`, and its origin, reading `clock`, whose one fixed
    /// peer is `peers`' first (the fixed authorized locator), and whose
    /// admission consults `grants`.
    pub fn new(
        net: &Arc<FakeNet>,
        clock: &FakeClock,
        name: &str,
        peers: &[&str],
        grants: LiveGrants,
    ) -> TestNode {
        let (store, faults) = (VolatileStore::default(), Faults::default());
        let settings = Settings {
            name: Some(name.into()),
            peers: peers.iter().map(|peer| peer.to_string()).collect(),
            ..Settings::default()
        };
        let engine = store.clone();
        let peer = FaultyPort::new(net.port(), faults.clone());
        let module = NodeAssembly::builder()
            .with_component_override::<dyn Config>(Box::new(FixedSettings(settings)))
            .with_component_override::<dyn Clock>(Box::new(clock.clone()))
            .with_component_override::<dyn PeerCarrier>(Box::new(peer))
            .with_component_override::<dyn ClientCarrier>(Box::new(net.port()))
            .with_component_override::<dyn Grants>(Box::new(grants))
            .with_component_override::<dyn Signer>(Box::new(KeyedTestSigner::fixture()))
            .with_component_override_fn::<dyn RecordHost>(Box::new(move |context| {
                let profile =
                    <NodeAssembly as HasComponent<dyn RecordProfile>>::build_component(context);
                let transport =
                    <NodeAssembly as HasComponent<dyn RecordTransport>>::build_component(context);
                Box::new(Records::in_memory_over(
                    profile,
                    transport,
                    Box::new(engine),
                ))
            }))
            .build();
        let config = CarrierConfig {
            local: CarrierAddr(name.into()),
            max_frame_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        };
        let carrier: Arc<dyn PeerCarrier> = module.resolve();
        run(carrier.bind(config)).expect("the peer carrier binds");
        let name = name.into();
        TestNode {
            name,
            module,
            store,
            faults,
        }
    }

    pub fn directory(&self) -> Arc<dyn Directory> {
        self.module.resolve()
    }

    pub fn host(&self) -> Arc<dyn RecordHostPort> {
        self.directory().host()
    }

    /// Who serves `share`, by this node's clock.
    pub fn serves(&self, share: &str) -> Option<String> {
        self.directory().serves(share).expect("an open host")
    }

    /// Append `record` under this node's origin: whether it was new.
    pub fn append(&self, record: Record) -> bool {
        self.host().append(record, &self.name).expect("appended")
    }

    /// The lease `claims.rs` mints for `share`, stamped at this node's clock.
    pub fn lease(&self, share: &str, epoch: i64) -> Record {
        let now = self.directory().clock().now_ms();
        Record::Serve(ServeClaim {
            node: self.name.clone(),
            share: share.into(),
            lease_expiry_ms: now + LEASE_TTL_MS,
            epoch,
        })
    }

    /// Push `ops`, a frame each, to this node's configured peer over its
    /// record transport: how many frames the carrier took.
    pub fn push(&self, ops: &[Op]) -> Result<usize, CarrierError> {
        let config: Arc<dyn Config> = self.module.resolve();
        let peer = CarrierAddr(config.settings().peers[0].clone());
        let frames: Vec<Vec<u8>> = ops.iter().map(|op| cbor::encode(&op.to_cbor())).collect();
        let transport: Arc<dyn RecordTransport> = self.module.resolve();
        run(transport.push(&peer, &frames))
    }

    /// The session's part, played by the test: accept the next link on this
    /// node's peer carrier and hand each op it carries to the record host, in
    /// order, until the link ends. Each op's answer.
    pub fn deliver(&self) -> Vec<Result<(), HostError>> {
        let carrier: Arc<dyn PeerCarrier> = self.module.resolve();
        let link = run(carrier.accept()).unwrap().expect("a link");
        let mut answers = Vec::new();
        while let Some(frame) = run(link.recv()).expect("a frame or the end") {
            answers.push(self.host().ingest(Op::from_cbor(&cbor::decode(&frame))));
        }
        answers
    }
}

/// A and B on one network, reading one clock: A's one peer is B. Each
/// consults the contract's fixture grant fold.
pub fn pair(clock: &FakeClock) -> (TestNode, TestNode) {
    let net = FakeNet::new();
    let a = TestNode::new(&net, clock, "a", &["b"], LiveGrants::fixture());
    (
        a,
        TestNode::new(&net, clock, "b", &[], LiveGrants::fixture()),
    )
}
