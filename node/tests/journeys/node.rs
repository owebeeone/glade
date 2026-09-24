//! The harness every journey shares: test nodes, each a whole test composition
//! of `NodeAssembly` on one fake network, and each persisting through the
//! engine its test binary supplies (`crate::Engine`, made by `crate::engine`):
//! the last snapshot in memory in `journeys`, the fast loop, and records.json
//! in a temp directory in `durable` (plan Step 4.4). A node can restart: it
//! closes its peer carrier, drops its composition, and is built again over the
//! same engine.

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

use crate::fakes::{run, FakeClock, FakeNet, FixedSettings, KeyedTestSigner};
use crate::faults::{Faults, FaultyPort, LiveGrants};

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
    pub store: crate::Engine,
    pub faults: Faults,
    net: Arc<FakeNet>,
    clock: FakeClock,
    peers: Vec<String>,
    grants: LiveGrants,
}

impl TestNode {
    /// A node named `name`, and its origin, reading `clock`, whose one fixed
    /// peer is `peers`' first (the fixed authorized locator), and whose
    /// admission consults `grants`. Its engine is a fresh one from
    /// `crate::engine`.
    pub fn new(
        net: &Arc<FakeNet>,
        clock: &FakeClock,
        name: &str,
        peers: &[&str],
        grants: LiveGrants,
    ) -> TestNode {
        let peers = peers.iter().map(|peer| peer.to_string()).collect();
        let (net, clock, store) = (net.clone(), clock.clone(), crate::engine(name));
        TestNode::build(
            net,
            clock,
            name.into(),
            peers,
            grants,
            store,
            Faults::default(),
        )
    }

    fn build(
        net: Arc<FakeNet>,
        clock: FakeClock,
        name: String,
        peers: Vec<String>,
        grants: LiveGrants,
        store: crate::Engine,
        faults: Faults,
    ) -> TestNode {
        let settings = Settings {
            name: Some(name.clone()),
            peers: peers.clone(),
            ..Settings::default()
        };
        let engine = store.clone();
        let peer = FaultyPort::new(net.port(), faults.clone());
        let module = NodeAssembly::builder()
            .with_component_override::<dyn Config>(Box::new(FixedSettings(settings)))
            .with_component_override::<dyn Clock>(Box::new(clock.clone()))
            .with_component_override::<dyn PeerCarrier>(Box::new(peer))
            .with_component_override::<dyn ClientCarrier>(Box::new(net.port()))
            .with_component_override::<dyn Grants>(Box::new(grants.clone()))
            .with_component_override::<dyn Signer>(Box::new(KeyedTestSigner::fixture()))
            .with_component_override_fn::<dyn RecordHost>(Box::new(move |context| {
                let profile =
                    <NodeAssembly as HasComponent<dyn RecordProfile>>::build_component(context);
                let transport =
                    <NodeAssembly as HasComponent<dyn RecordTransport>>::build_component(context);
                let records = Records::over(profile, transport, engine.boxed());
                Box::new(records.expect("the record host loads what its engine holds"))
            }))
            .build();
        let config = CarrierConfig {
            local: CarrierAddr(name.clone()),
            max_frame_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        };
        let carrier: Arc<dyn PeerCarrier> = module.resolve();
        run(carrier.bind(config)).expect("the peer carrier binds");
        TestNode {
            name,
            module,
            store,
            faults,
            net,
            clock,
            peers,
            grants,
        }
    }

    /// This node, restarted: its peer carrier is closed, which ends its links
    /// and frees its address, its composition is dropped with the fold it
    /// held, and a new composition is built over the same engine, with the
    /// same name, clock, peers and grant fold.
    pub fn restart(self) -> TestNode {
        let carrier: Arc<dyn PeerCarrier> = self.module.resolve();
        run(carrier.close());
        drop(carrier);
        let TestNode {
            name,
            module,
            store,
            faults,
            net,
            clock,
            peers,
            grants,
        } = self;
        drop(module);
        TestNode::build(net, clock, name, peers, grants, store, faults)
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
