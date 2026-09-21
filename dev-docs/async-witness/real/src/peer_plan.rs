//! Step 3.1 — the sdax plan that drives two **real** witness nodes.
//!
//! ```text
//! Acceptor  <-- Served  <-- Exchange
//!     ^                        |
//!     +---- Dialed  <----------+
//!            ^
//!         Dialer
//! ```
//!
//! The arrows are `needs`; cleanup is their reverse, derived by sdax from the
//! typed edges and not declared separately (`sdax/src/view.rs:120-148`). Two
//! endpoints are acquired concurrently, two links are acquired concurrently
//! over them — one by awaiting `accept`, one by awaiting `dial` — and a single
//! step puts one real glade `Frame` across the witness's `CarrierPort`.
//!
//! `Dialed` needs **both** endpoints, because a dialer needs its own endpoint
//! to dial from and the acceptor's `addr()` to dial to. That is not an
//! accident of the fixture: it makes every endpoint the parent of every link
//! that can reach it, which is exactly the ownership AR-08 asks about.
//!
//! Every external effect is inside `cx.hold(...)`: the bind, the accept and the
//! dial. Nothing is performed before the factory and registered afterwards,
//! which would re-create by hand the window `hold` exists to close
//! (`sdax/src/lib.rs`, "the one rule of the body contract").

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_witness_ports::{CarriedFrame, CarrierPort, FrameType};
use glade_node::frame::Frame;
use glade_node::iroh_carrier::PeerEndpoint;
use glade_node::peer::NodeIdentity;
use glade_wire::generated::{Op, Ops, Shape};
use sdax::prelude::*;

use crate::peer_carrier::{WitnessCarrier, WitnessEndpoint};
use crate::shaku_bridge::{RealComposition, assemble, resolve_carrier};

/// The node names, in one place, so a test and a plan cannot disagree about a
/// spelling. `ReleaseOrder::before` matches a node by its path, and a typo
/// there answers `false` rather than failing.
pub mod node {
    /// The endpoint that accepts: the parent of the link it serves.
    pub const ACCEPTOR: &str = "Acceptor";
    /// The endpoint that dials: the parent of the link it opens.
    pub const DIALER: &str = "Dialer";
    /// The accepted link, HELLO complete, as a `CarrierPort`.
    pub const SERVED: &str = "Served";
    /// The dialed link, HELLO complete, as a `CarrierPort`.
    pub const DIALED: &str = "Dialed";
    /// One real glade `Frame` across the port.
    pub const EXCHANGE: &str = "Exchange";
    /// Step 3.3's Shaku assembly over the already-acquired carrier. Present
    /// only when the harness asks for it — it is the whole of Step 3.4's
    /// differential.
    pub const MODULE: &str = "Module";
}

/// The acceptor's glade node key. Fixed, so a test can name the `node_id` the
/// HELLO seam should have carried. The *iroh* key stays random per bind
/// (`iroh_carrier.rs:32-40`), so two runs never collide.
const ACCEPTOR_KEY: [u8; 32] = [0xA1; 32];
/// The dialer's glade node key.
const DIALER_KEY: [u8; 32] = [0xD1; 32];

/// The acceptor's glade identity, `node_id = sha256(key)`.
pub fn acceptor_identity() -> NodeIdentity {
    NodeIdentity::from_key(ACCEPTOR_KEY)
}

/// The dialer's glade identity.
pub fn dialer_identity() -> NodeIdentity {
    NodeIdentity::from_key(DIALER_KEY)
}

/// How long a link acquisition may take before it is reported as a fault
/// rather than hanging the run. Localhost QUIC plus a HELLO is milliseconds;
/// this bound exists so a broken run *ends*, not to hurry a good one.
const LINK_BUDGET: Duration = Duration::from_secs(15);

/// The shutdown budget the real plan runs with.
///
/// `iroh::Endpoint::close` documents a worst case of about three seconds and
/// there are two of them, so this is deliberately generous: plan §9.3 forbids
/// shortening a drain "to make a test quick — that turns a drain into an
/// abandonment".
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(30);

/// The one frame the exchange puts through the port: a real glade `Frame`
/// carrying a real `glade-wire` `Op`, not a witness invention.
pub fn exchange_frame() -> Frame {
    Frame::Ops(Ops {
        ops: vec![Op {
            share: "witness".into(),
            glade_id: "g".into(),
            key: vec![],
            origin: "a".into(),
            seq: 1,
            prev: None,
            lamport: 1,
            refs: vec![],
            shape: Shape::Value,
            payload: b"one frame over real iroh".to_vec(),
        }],
        pri: None,
    })
}

/// Split a frame into the `[tag][body]` pair the port carries. The port is
/// deliberately opaque about the body: it moves frames, it does not decode
/// them.
pub fn split(frame: &Frame) -> CarriedFrame {
    let bytes = frame.to_bytes();
    let (&tag, body) = bytes.split_first().expect("a frame is never empty");
    (FrameType::from_wire(i64::from(tag)), body.to_vec())
}

/// The frame the **Shaku-assembled** port sends, distinct from the exchange's
/// so a test can tell which port put which frame on the wire.
pub fn module_frame() -> Frame {
    Frame::Ops(Ops {
        ops: vec![Op {
            share: "witness".into(),
            glade_id: "g".into(),
            key: vec![],
            origin: "a".into(),
            seq: 2,
            prev: None,
            lamport: 2,
            refs: vec![],
            shape: Shape::Value,
            payload: b"one frame through an assembled module".to_vec(),
        }],
        pri: None,
    })
}

/// Put a carried frame back together and decode it, so a test can say "the
/// frame that arrived is the frame that was sent" about a `Frame` rather than
/// about a byte vector.
pub fn join(carried: &CarriedFrame) -> Result<Frame, String> {
    let mut bytes = Vec::with_capacity(1 + carried.1.len());
    bytes.push(carried.0.wire() as u8);
    bytes.extend_from_slice(&carried.1);
    Frame::from_bytes(&bytes)
}

/// One thing a body did, in the order the run did it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerEvent {
    /// A resource's `acquire` body registered its value.
    Acquired(&'static str),
    /// A step's `run` body completed.
    Ran(&'static str),
    /// A resource's `release` body completed.
    Released(&'static str),
}

/// The switches a test sets before a run, and the ledger it reads afterwards.
///
/// One plan shape and one code path, as in Phase 2: the order
/// `tests/peer_carrier.rs` asserts statically is the order the run executes.
#[derive(Clone)]
pub struct PeerHarness {
    events: Arc<Mutex<Vec<PeerEvent>>>,
    ports: Arc<Mutex<BTreeMap<&'static str, u16>>>,
    peers: Arc<Mutex<BTreeMap<&'static str, [u8; 32]>>>,
    received: Arc<Mutex<Vec<CarriedFrame>>>,
    module_received: Arc<Mutex<Vec<CarriedFrame>>>,
    escaped: Arc<Mutex<Option<PeerEndpoint>>>,
    kept_module: Arc<Mutex<Option<RealComposition>>>,
    escape_a_clone: bool,
    with_module: bool,
    keep_the_module: bool,
}

impl Default for PeerHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerHarness {
    /// A harness with nothing switched on.
    pub fn new() -> PeerHarness {
        PeerHarness {
            events: Arc::new(Mutex::new(Vec::new())),
            ports: Arc::new(Mutex::new(BTreeMap::new())),
            peers: Arc::new(Mutex::new(BTreeMap::new())),
            received: Arc::new(Mutex::new(Vec::new())),
            module_received: Arc::new(Mutex::new(Vec::new())),
            escaped: Arc::new(Mutex::new(None)),
            kept_module: Arc::new(Mutex::new(None)),
            escape_a_clone: false,
            with_module: false,
            keep_the_module: false,
        }
    }

    /// Add the Step 3.3 node: a step that `.needs` the acquired carrier and
    /// assembles a Shaku module over it.
    ///
    /// This switch **is** Step 3.4's differential. Nothing else about the plan
    /// changes with it, which is what lets the two runs be compared at all.
    #[must_use]
    pub fn with_shaku_module(mut self) -> PeerHarness {
        self.with_module = true;
        self
    }

    /// Let the assembled module outlive the step that built it, by stashing it
    /// here instead of dropping it at the end of the body.
    ///
    /// The plan's §4.2 names this as the sharp risk at the seam: "The Shaku
    /// module holds `Arc` clones of things derived from the endpoint, and an
    /// escaped clone keeps the UDP socket bound." Whether it does is a fact to
    /// measure, not to assume.
    #[must_use]
    pub fn keep_the_module(mut self) -> PeerHarness {
        self.keep_the_module = true;
        self
    }

    /// Whether the assembled module is still held.
    pub fn holds_the_module(&self) -> bool {
        self.kept_module.lock().expect("module cell lock").is_some()
    }

    /// Resolve the port out of the kept module, if there is one — proof that a
    /// module asserted to be "still alive" really is.
    pub fn resolve_kept_module(&self) -> Option<Arc<dyn CarrierPort>> {
        self.kept_module
            .lock()
            .expect("module cell lock")
            .as_ref()
            .map(resolve_carrier)
    }

    /// Every frame the served carrier read that the **assembled** port sent.
    pub fn module_received(&self) -> Vec<CarriedFrame> {
        self.module_received
            .lock()
            .expect("witness module inbox lock")
            .clone()
    }

    /// Let one clone of the **acceptor's** endpoint escape the composition,
    /// from inside the acquire body that binds it.
    ///
    /// This exists so that Step 3.2's proof can fail. A re-bind that always
    /// succeeds would witness nothing about the composition; this switch makes
    /// the same assertion answer "still bound" for a reason the test names.
    #[must_use]
    pub fn let_a_clone_escape(mut self) -> PeerHarness {
        self.escape_a_clone = true;
        self
    }

    /// Whether the escaped clone is still held.
    pub fn holds_an_escaped_clone(&self) -> bool {
        self.escaped.lock().expect("escape cell lock").is_some()
    }

    /// Drop the escaped clone. Returns whether there was one to drop.
    pub fn drop_escaped_clone(&self) -> bool {
        self.escaped
            .lock()
            .expect("escape cell lock")
            .take()
            .is_some()
    }

    /// Every event, in the order the run produced them.
    pub fn events(&self) -> Vec<PeerEvent> {
        self.events.lock().expect("witness ledger lock").clone()
    }

    /// The UDP port each endpoint reported from `addr()` at bind time.
    pub fn ports(&self) -> BTreeMap<&'static str, u16> {
        self.ports.lock().expect("witness port lock").clone()
    }

    /// The port one endpoint bound, or `None` if it never bound.
    pub fn port(&self, endpoint: &'static str) -> Option<u16> {
        self.ports
            .lock()
            .expect("witness port lock")
            .get(endpoint)
            .copied()
    }

    /// The glade `node_id` a link's HELLO seam vouched for.
    pub fn learned_peer(&self, link: &'static str) -> Option<[u8; 32]> {
        self.peers
            .lock()
            .expect("witness peer lock")
            .get(link)
            .copied()
    }

    /// Every frame the served carrier took out of `recv`.
    pub fn received(&self) -> Vec<CarriedFrame> {
        self.received.lock().expect("witness inbox lock").clone()
    }

    /// Where an event sits in the ledger, or `None` if it never happened.
    pub fn position(&self, event: &PeerEvent) -> Option<usize> {
        self.events().iter().position(|seen| seen == event)
    }

    /// Whether `a`'s release completed before `b`'s did. `None` when either
    /// never completed — an absent release must never read as "in order".
    pub fn released_before(&self, a: &'static str, b: &'static str) -> Option<bool> {
        let first = self.position(&PeerEvent::Released(a))?;
        let second = self.position(&PeerEvent::Released(b))?;
        Some(first < second)
    }

    fn record(&self, event: PeerEvent) {
        self.events.lock().expect("witness ledger lock").push(event);
    }

    fn record_port(&self, endpoint: &'static str, port: u16) {
        self.ports
            .lock()
            .expect("witness port lock")
            .insert(endpoint, port);
    }

    fn record_peer(&self, link: &'static str, peer_id: [u8; 32]) {
        self.peers
            .lock()
            .expect("witness peer lock")
            .insert(link, peer_id);
    }

    fn record_received(&self, frame: CarriedFrame) {
        self.received
            .lock()
            .expect("witness inbox lock")
            .push(frame);
    }

    fn record_module_received(&self, frame: CarriedFrame) {
        self.module_received
            .lock()
            .expect("witness module inbox lock")
            .push(frame);
    }

    fn escape(&self, endpoint: PeerEndpoint) {
        *self.escaped.lock().expect("escape cell lock") = Some(endpoint);
    }

    fn keep(&self, module: RealComposition) {
        *self.kept_module.lock().expect("module cell lock") = Some(module);
    }
}

/// Declare one endpoint resource: bind inside `cx.hold`, close by value on
/// release.
fn endpoint(
    p: &mut PlanBuilder<(), ()>,
    harness: &PeerHarness,
    name: &'static str,
    identity: NodeIdentity,
) -> Key<WitnessEndpoint> {
    let acquiring = harness.clone();
    let releasing = harness.clone();
    p.resource(name)
        .acquire(move |cx: Cx<Acquire>, ()| {
            let harness = acquiring.clone();
            async move {
                cx.hold(move || async move {
                    // The external effect is INSIDE the factory, so the engine
                    // records the obligation in the poll that sees the bind
                    // succeed. Registering a bind performed before this point
                    // would leave the X1 window open by hand.
                    let endpoint = WitnessEndpoint::bind(identity).await?;
                    harness.record_port(name, endpoint.port());
                    if harness.escape_a_clone && name == node::ACCEPTOR {
                        // Step 3.2's negative fixture, and the only clone the
                        // composition ever takes.
                        let escaping = endpoint.escaping_clone().await;
                        if let Some(clone) = escaping {
                            harness.escape(clone);
                        }
                    }
                    harness.record(PeerEvent::Acquired(name));
                    Ok::<_, io::Error>(endpoint)
                })
                .await
            }
        })
        .release(move |_cx: Cx<Release>, endpoint: Arc<WitnessEndpoint>| {
            let harness = releasing.clone();
            async move {
                // `close` takes the `PeerEndpoint` out of the handle and gives
                // it to `PeerEndpoint::close(self)`, so the `Arc` the engine
                // keeps in its slots is an empty shell afterwards.
                endpoint.close().await;
                harness.record(PeerEvent::Released(name));
                Ok(())
            }
        })
}

/// The accepted link: the acceptor's child.
fn served_link(
    p: &mut PlanBuilder<(), ()>,
    harness: &PeerHarness,
    acceptor: Key<WitnessEndpoint>,
) -> Key<WitnessCarrier> {
    let acquiring = harness.clone();
    let releasing = harness.clone();
    p.resource(node::SERVED)
        .needs(acceptor)
        .within(LINK_BUDGET)
        .acquire(move |cx: Cx<Acquire>, acceptor: Arc<WitnessEndpoint>| {
            let harness = acquiring.clone();
            async move {
                cx.hold(move || async move {
                    let link = acceptor.accept().await?.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotConnected, "the endpoint is closed")
                    })?;
                    let carrier = WitnessCarrier::over(link);
                    harness.record_peer(node::SERVED, carrier.peer().peer_id);
                    harness.record(PeerEvent::Acquired(node::SERVED));
                    Ok::<_, io::Error>(carrier)
                })
                .await
            }
        })
        .release(move |_cx: Cx<Release>, carrier: Arc<WitnessCarrier>| {
            let harness = releasing.clone();
            async move {
                carrier.close().await;
                harness.record(PeerEvent::Released(node::SERVED));
                Ok(())
            }
        })
}

/// The dialed link: a child of both endpoints, because it needs its own to
/// dial from and the acceptor's address to dial to.
fn dialed_link(
    p: &mut PlanBuilder<(), ()>,
    harness: &PeerHarness,
    dialer: Key<WitnessEndpoint>,
    acceptor: Key<WitnessEndpoint>,
) -> Key<WitnessCarrier> {
    let acquiring = harness.clone();
    let releasing = harness.clone();
    p.resource(node::DIALED)
        .needs((dialer, acceptor))
        .within(LINK_BUDGET)
        .acquire(
            move |cx: Cx<Acquire>,
                  (dialer, acceptor): (Arc<WitnessEndpoint>, Arc<WitnessEndpoint>)| {
                let harness = acquiring.clone();
                async move {
                    cx.hold(move || async move {
                        let link = dialer.dial(&acceptor.addr()).await?;
                        let carrier = WitnessCarrier::over(link);
                        harness.record_peer(node::DIALED, carrier.peer().peer_id);
                        harness.record(PeerEvent::Acquired(node::DIALED));
                        Ok::<_, io::Error>(carrier)
                    })
                    .await
                }
            },
        )
        .release(move |_cx: Cx<Release>, carrier: Arc<WitnessCarrier>| {
            let harness = releasing.clone();
            async move {
                carrier.close().await;
                harness.record(PeerEvent::Released(node::DIALED));
                Ok(())
            }
        })
}

/// The witness's real-port plan, ready to `start`.
///
/// The export is the number of frames the served carrier took out of `recv`,
/// so a clean run has a completed output and the test can say what
/// `is_clean()` does and does not cover.
pub fn peer_plan(harness: &PeerHarness) -> Plan<usize> {
    let mut p = Plan::builder("AsyncWitnessPeer");

    let acceptor = endpoint(&mut p, harness, node::ACCEPTOR, acceptor_identity());
    let dialer = endpoint(&mut p, harness, node::DIALER, dialer_identity());
    let served = served_link(&mut p, harness, acceptor);
    let dialed = dialed_link(&mut p, harness, dialer, acceptor);

    let exchange = {
        let harness = harness.clone();
        p.step(node::EXCHANGE)
            .needs((served, dialed))
            .within(LINK_BUDGET)
            .run(
                move |_cx: Cx<Run>, (served, dialed): (Arc<WitnessCarrier>, Arc<WitnessCarrier>)| {
                    let harness = harness.clone();
                    async move {
                        let (tag, body) = split(&exchange_frame());
                        dialed.send(tag, &body).await?;
                        let Some(got) = served.recv().await? else {
                            return Err(Error::from("the served carrier reached end of stream"));
                        };
                        harness.record_received(got);
                        harness.record(PeerEvent::Ran(node::EXCHANGE));
                        Ok(harness.received().len())
                    }
                },
            )
    };

    if harness.with_module {
        module_step(&mut p, harness, exchange, served, dialed);
    }

    p.export(exchange)
        .build(
            Policy::FailFast,
            Shutdown::within(SHUTDOWN_BUDGET),
            Mode::Finite,
        )
        .expect("the witness peer plan is valid by construction")
}

/// Step 3.3's node: Shaku assembles over the handle the engine already holds.
///
/// It needs the exchange as well as the two carriers, for a reason that is
/// about determinism rather than about dependency: without that edge the two
/// steps would be `unordered` and would contend for the same stream, and a
/// witness whose result depends on which body reached the lock first would be
/// evidence of nothing.
fn module_step(
    p: &mut PlanBuilder<(), ()>,
    harness: &PeerHarness,
    exchange: Key<usize>,
    served: Key<WitnessCarrier>,
    dialed: Key<WitnessCarrier>,
) {
    let harness = harness.clone();
    p.step(node::MODULE)
        .needs((exchange, served, dialed))
        .within(LINK_BUDGET)
        .run(
            move |_cx: Cx<Run>,
                  (_frames, served, dialed): (
                Arc<usize>,
                Arc<WitnessCarrier>,
                Arc<WitnessCarrier>,
            )| {
                let harness = harness.clone();
                async move {
                    // Assembly, over a handle acquired by the engine three
                    // nodes ago. Shaku never learns that a lifecycle exists.
                    let module = assemble(dialed);
                    let port = resolve_carrier(&module);

                    let (tag, body) = split(&module_frame());
                    port.send(tag, &body).await?;
                    let Some(got) = served.recv().await? else {
                        return Err(Error::from("the served carrier reached end of stream"));
                    };
                    harness.record_module_received(got);
                    harness.record(PeerEvent::Ran(node::MODULE));

                    // The module is a plain value produced by a step, so its
                    // ordinary fate is to be dropped here. The harness switch
                    // exists to measure what happens when it is not.
                    drop(port);
                    if harness.keep_the_module {
                        harness.keep(module);
                    }
                    Ok(())
                }
            },
        );
}
