//! The real target: sdax-rs owns the lifecycle. Phase 2 uses **fakes only** —
//! no socket, no iroh endpoint, no Shaku — so that AR-08 is decided about the
//! lifecycle library before any of Phase 3's real I/O can confuse the result.
//!
//! `AsyncWitnessPlan.md` §8.1 is why this separation matters: a failure caused
//! by sdax-rs records nothing on `async_witness`; it is an R28/Q10 matter and
//! the correct action is to stop and report. Everything in this module is
//! therefore arranged so that a failure here cannot be mistaken for a failure
//! of the injector, which is not present at all.
//!
//! # The shape, and why it is this shape
//!
//! The fakes model the node's real footgun rather than an arbitrary tree.
//! `node/src/iroh_carrier.rs:63-65` says the endpoint "**MUST outlive every
//! `PeerLink` it produces** — dropping the last handle closes the endpoint and
//! tears down live connections", and `mesh.rs:125-126` calls it "the R2
//! footgun". [`FakeEndpoint`] makes that a checkable fact: closing it while a
//! link is live is an error, not a silent teardown. AR-08's first clause —
//! "Parent resource remains owned while child cleanup is pending" — is
//! therefore falsifiable at run time here, and statically in
//! `tests/release_order.rs`.
//!
//! ```text
//! Endpoint  <-- Link  <-- Exchange          Journal  <-- Record
//! ```
//!
//! Two branches, so that "concurrent independent cleanup can progress" has
//! something to be true about. The arrows are `needs`; cleanup is their
//! reverse, derived by sdax from the typed edges and not declared separately
//! (`sdax/src/view.rs:120-148`).
//!
//! The `Arc<FakeEndpoint>` inside a [`FakeLink`] is **data, not a declaration**
//! — `sdax/docs/AI-Authoring.md` is explicit that "a struct or tuple containing
//! `Arc<Resource>` is only data; hiding a resource inside it does not declare
//! cleanup ownership". What keeps the endpoint owned through the link's cleanup
//! is the `needs` edge, and nothing else. The field only lets the fake notice
//! if that were ever untrue.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_witness_ports::{
    CarriedFrame, CarrierError, CarrierPort, FakeCarrier, FakeStore, FrameType, PortFuture,
    StorePort,
};
use sdax::prelude::*;
use tokio::sync::Notify;

/// The node names, in one place, so a test and a plan cannot disagree about a
/// spelling. `ReleaseOrder::before` matches a node by its path
/// (`sdax/src/view/model.rs:39-56`), and a typo there answers `false` rather
/// than failing, which would make a passing assertion meaningless.
pub mod node {
    /// The parent resource: the stand-in for `iroh::Endpoint`.
    pub const ENDPOINT: &str = "Endpoint";
    /// The child resource: the stand-in for a `PeerLink` the endpoint produced.
    pub const LINK: &str = "Link";
    /// Work over the child: one frame through the witness's `CarrierPort`.
    pub const EXCHANGE: &str = "Exchange";
    /// An independent resource, sharing no `needs` path with the endpoint.
    pub const JOURNAL: &str = "Journal";
    /// Work over the independent resource.
    pub const RECORD: &str = "Record";
}

/// The frame `Exchange` puts through the carrier. `Ops` is a real
/// `glade-wire` tag, not a witness invention: the port carries the node's own
/// `[FrameType tag][body]` split.
const EXCHANGE_FRAME: FrameType = FrameType::Ops;

/// The share `Record` appends to.
const RECORD_SHARE: &str = "witness";

/// One thing a body did, in the order the run did it.
///
/// A release that ran is the evidence AR-08 asks for; a release that ran
/// *after* its parent's would be the failure. Recording both, in order, is
/// what lets Step 2.2 assert the ordering from the run as well as from the
/// declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A resource's `acquire` body registered its value.
    Acquired(&'static str),
    /// A step's `run` body completed.
    Ran(&'static str),
    /// A resource's `release` body completed, successfully.
    Released(&'static str),
    /// A resource's `release` body started and never returned.
    ReleaseStalled(&'static str),
}

/// The switches a test sets before a run, and the ledger it reads afterwards.
///
/// One plan shape and one code path: the order `tests/release_order.rs`
/// asserts statically is the order `tests/lifecycle.rs` runs. A test that
/// wanted a second shape would have to declare it, and then the static
/// assertion would no longer be about the thing that ran.
#[derive(Clone)]
pub struct Harness {
    events: Arc<Mutex<Vec<Event>>>,
    acquisitions: Arc<AtomicUsize>,
    exchange_blocks: Arc<AtomicBool>,
    exchange_started: Arc<Notify>,
    endpoint_release_hangs: Arc<AtomicBool>,
    shutdown_budget: Duration,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    /// A harness with nothing switched on and an unhurried shutdown budget.
    ///
    /// Ten seconds, deliberately: `AsyncWitnessPlan.md` §9.3 forbids shortening
    /// a drain budget to make a test quick, "that turns a drain into an
    /// abandonment, which is exactly what `report.incomplete` exists to
    /// reveal". The fakes finish in microseconds, so the budget costs nothing.
    pub fn new() -> Self {
        Harness {
            events: Arc::new(Mutex::new(Vec::new())),
            acquisitions: Arc::new(AtomicUsize::new(0)),
            exchange_blocks: Arc::new(AtomicBool::new(false)),
            exchange_started: Arc::new(Notify::new()),
            endpoint_release_hangs: Arc::new(AtomicBool::new(false)),
            shutdown_budget: Duration::from_secs(10),
        }
    }

    /// Bound the plan's whole settle-and-cleanup phase by `budget`.
    ///
    /// Only ever shortened together with [`hang_endpoint_release`] — an
    /// obligation that can never be discharged expires at *every* budget, so
    /// the short one is honesty about the fixture, not a hurried drain.
    ///
    /// [`hang_endpoint_release`]: Self::hang_endpoint_release
    #[must_use]
    pub fn with_shutdown_budget(mut self, budget: Duration) -> Self {
        self.shutdown_budget = budget;
        self
    }

    /// Make `Exchange` never complete, so the run is still in flight and can
    /// be cancelled from outside.
    pub fn block_exchange(&self) {
        self.exchange_blocks.store(true, Ordering::SeqCst);
    }

    /// Make the endpoint's `release` body never return: the obligation the
    /// shutdown budget must then abandon *and report*.
    pub fn hang_endpoint_release(&self) {
        self.endpoint_release_hangs.store(true, Ordering::SeqCst);
    }

    /// Resolves once `Exchange` has started and is about to block. Racing
    /// `cancel()` against an unstarted body would test nothing.
    pub async fn exchange_started(&self) {
        self.exchange_started.notified().await;
    }

    /// Every event, in the order the run produced them.
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().expect("witness ledger lock").clone()
    }

    /// How many `acquire` bodies registered a value.
    pub fn acquisitions(&self) -> usize {
        self.acquisitions.load(Ordering::SeqCst)
    }

    /// Where an event sits in the ledger, or `None` if it never happened.
    pub fn position(&self, event: &Event) -> Option<usize> {
        self.events().iter().position(|seen| seen == event)
    }

    /// Whether `a`'s release completed before `b`'s did. `None` when either
    /// never completed — an absent release must never read as "in order".
    pub fn released_before(&self, a: &'static str, b: &'static str) -> Option<bool> {
        let first = self.position(&Event::Released(a))?;
        let second = self.position(&Event::Released(b))?;
        Some(first < second)
    }

    fn record(&self, event: Event) {
        self.events.lock().expect("witness ledger lock").push(event);
    }
}

/// Why a fake endpoint refused to close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseError {
    /// Closing would have torn down links that are still live. This is the
    /// `iroh_carrier.rs:63-65` footgun, made into a refusal: if it is ever
    /// observed, the parent was released while a child's cleanup was pending
    /// and AR-08's first clause is false.
    LinksStillLive(usize),
    /// A second close of an already closed endpoint.
    AlreadyClosed,
}

impl fmt::Display for CloseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CloseError::LinksStillLive(live) => {
                write!(f, "endpoint closed while {live} link(s) were still live")
            }
            CloseError::AlreadyClosed => write!(f, "endpoint already closed"),
        }
    }
}

impl std::error::Error for CloseError {}

/// The parent resource: what `iroh::Endpoint` is in Phase 3.
///
/// `iroh-1.2.0/src/endpoint.rs:1717-1718` says "the underlying UDP sockets are
/// only closed once all clones of the respective `Endpoint` are dropped", which
/// is why Phase 3 can prove a leak by re-binding the port. Nothing that cheap
/// exists for a fake, so this one counts its live children instead and refuses
/// a close that would orphan one.
pub struct FakeEndpoint {
    live_links: AtomicUsize,
    closed: AtomicBool,
}

impl FakeEndpoint {
    fn open() -> Self {
        FakeEndpoint {
            live_links: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }

    /// Produce a child link. Refused once the endpoint is closed — a closed
    /// parent admits no new child, which is AR-08's admission clause in
    /// miniature.
    fn open_link(self: &Arc<Self>) -> Result<FakeLink, CarrierError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(CarrierError::Closed);
        }
        self.live_links.fetch_add(1, Ordering::SeqCst);
        Ok(FakeLink {
            endpoint: Arc::clone(self),
            carrier: FakeCarrier::new(),
            closed: AtomicBool::new(false),
        })
    }

    fn close(&self) -> Result<(), CloseError> {
        let live = self.live_links.load(Ordering::SeqCst);
        if live != 0 {
            return Err(CloseError::LinksStillLive(live));
        }
        if self.closed.swap(true, Ordering::SeqCst) {
            return Err(CloseError::AlreadyClosed);
        }
        Ok(())
    }

    /// How many children have not yet released.
    pub fn live_links(&self) -> usize {
        self.live_links.load(Ordering::SeqCst)
    }

    /// Whether the endpoint's own cleanup completed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// The child resource: what a `PeerLink` is in Phase 3, and a `CarrierPort`
/// implementation, so what the engine owns here is the witness's own port and
/// not a type invented for the test.
pub struct FakeLink {
    endpoint: Arc<FakeEndpoint>,
    carrier: FakeCarrier,
    closed: AtomicBool,
}

impl FakeLink {
    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.carrier.close();
        self.endpoint.live_links.fetch_sub(1, Ordering::SeqCst);
    }

    /// Every frame handed to `send`, in order.
    pub fn sent(&self) -> Vec<CarriedFrame> {
        self.carrier.sent()
    }
}

impl CarrierPort for FakeLink {
    fn send<'a>(
        &'a self,
        frame: FrameType,
        body: &'a [u8],
    ) -> PortFuture<'a, Result<(), CarrierError>> {
        self.carrier.send(frame, body)
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<CarriedFrame>, CarrierError>> {
        self.carrier.recv()
    }
}

/// The witness's parent/child plan over fakes.
///
/// Built from the declaration alone: `Plan::inspect()` reads it without running
/// a body, which is what makes Step 2.1 a statement about the *declaration*
/// rather than about one lucky run.
///
/// The export is the number of frames the carrier recorded, so a clean run has
/// a completed output to hand back and Step 2.2 can say what `is_clean()` does
/// and does not cover.
pub fn witness_plan(harness: &Harness) -> Plan<usize> {
    let mut p = Plan::builder("AsyncWitness");

    let endpoint = {
        let acquiring = harness.clone();
        let releasing = harness.clone();
        p.resource(node::ENDPOINT)
            .acquire(move |cx: Cx<Acquire>, ()| {
                let harness = acquiring.clone();
                async move {
                    // The external action goes INSIDE the lazy `hold` factory,
                    // so the engine records the obligation in the same poll
                    // that sees the acquisition succeed (`sdax/docs/Cleanup.md`;
                    // `cx/acquisition.rs:46-52`). Phase 3 puts a real
                    // `PeerEndpoint::bind_with()` in exactly this position.
                    cx.hold(|| {
                        harness.acquisitions.fetch_add(1, Ordering::SeqCst);
                        harness.record(Event::Acquired(node::ENDPOINT));
                        std::future::ready(Ok::<_, Error>(FakeEndpoint::open()))
                    })
                    .await
                }
            })
            .release(move |_cx: Cx<Release>, endpoint: Arc<FakeEndpoint>| {
                let harness = releasing.clone();
                async move {
                    if harness.endpoint_release_hangs.load(Ordering::SeqCst) {
                        harness.record(Event::ReleaseStalled(node::ENDPOINT));
                        std::future::pending::<()>().await;
                    }
                    endpoint.close()?;
                    harness.record(Event::Released(node::ENDPOINT));
                    Ok(())
                }
            })
    };

    let link = {
        let acquiring = harness.clone();
        let releasing = harness.clone();
        p.resource(node::LINK)
            .needs(endpoint)
            .acquire(move |cx: Cx<Acquire>, endpoint: Arc<FakeEndpoint>| {
                let harness = acquiring.clone();
                async move {
                    cx.hold(|| {
                        harness.acquisitions.fetch_add(1, Ordering::SeqCst);
                        harness.record(Event::Acquired(node::LINK));
                        std::future::ready(endpoint.open_link())
                    })
                    .await
                }
            })
            .release(move |_cx: Cx<Release>, link: Arc<FakeLink>| {
                let harness = releasing.clone();
                async move {
                    link.close();
                    harness.record(Event::Released(node::LINK));
                    Ok(())
                }
            })
    };

    let exchange = {
        let harness = harness.clone();
        p.step(node::EXCHANGE)
            .needs(link)
            .run(move |_cx: Cx<Run>, link: Arc<FakeLink>| {
                let harness = harness.clone();
                async move {
                    if harness.exchange_blocks.load(Ordering::SeqCst) {
                        harness.exchange_started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    link.send(EXCHANGE_FRAME, b"witness").await?;
                    harness.record(Event::Ran(node::EXCHANGE));
                    Ok(link.sent().len())
                }
            })
    };

    let journal = {
        let acquiring = harness.clone();
        let releasing = harness.clone();
        p.resource(node::JOURNAL)
            .acquire(move |cx: Cx<Acquire>, ()| {
                let harness = acquiring.clone();
                async move {
                    cx.hold(|| {
                        harness.acquisitions.fetch_add(1, Ordering::SeqCst);
                        harness.record(Event::Acquired(node::JOURNAL));
                        std::future::ready(Ok::<_, Error>(FakeStore::new()))
                    })
                    .await
                }
            })
            .release(move |_cx: Cx<Release>, store: Arc<FakeStore>| {
                let harness = releasing.clone();
                async move {
                    store.close();
                    harness.record(Event::Released(node::JOURNAL));
                    Ok(())
                }
            })
    };

    {
        let harness = harness.clone();
        p.step(node::RECORD)
            .needs(journal)
            .run(move |_cx: Cx<Run>, store: Arc<FakeStore>| {
                let harness = harness.clone();
                async move {
                    store.append(RECORD_SHARE, b"one")?;
                    harness.record(Event::Ran(node::RECORD));
                    Ok(())
                }
            });
    }

    p.export(exchange)
        .build(
            Policy::FailFast,
            Shutdown::within(harness.shutdown_budget),
            Mode::Finite,
        )
        .expect("the witness plan is valid by construction")
}
