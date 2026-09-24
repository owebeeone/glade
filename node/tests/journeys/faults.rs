//! The providers the journeys add to Step 3.2's fakes, each able to change
//! under a test's hand: a carrier whose links can suffer a transport failure,
//! an engine the test reads back, and a grant fold the test appends to. Each
//! says what it does not prove, and each port's shared suite runs below.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use glade_carrier_api::conformance::{self as carrier, Fixture};
use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture,
};
use glade_grant_api::conformance::{self as grant, Record as GrantRecord};
use glade_grant_api::{Denial, GrantPort, Holder};
use glade_node::cbor;
use glade_node::registry::StoreApi;
use glade_node::sysdata::SystemSnapshot;
use glade_wire::generated::Op;

use crate::fakes::{run, FakeNet, FakePort};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where one link's transport fails: as its `frame`-th send (from 0) is
/// made, with that frame lost or already `delivered`.
#[derive(Clone, Copy, Debug)]
pub struct Fault {
    pub frame: usize,
    pub delivered: bool,
}

/// The faults the next dialed links suffer, one per link, in order; a link
/// dialed with none queued suffers none.
#[derive(Clone, Default)]
pub struct Faults(Arc<Mutex<VecDeque<Fault>>>);

impl Faults {
    /// The next link dialed suffers `fault`.
    pub fn next(&self, fault: Fault) {
        lock(&self.0).push_back(fault);
    }
}

/// A fake port whose dialed links can suffer one transport failure each,
/// which the carrier contract allows: frames arrive whole, once and in order
/// "unless the transport fails", and `send` is no acknowledgement. At the
/// failure the link ends: the peer reads end of stream after what arrived,
/// and the sender is told `Transport`, whether or not that frame arrived. It
/// proves nothing about a real network's losses or timing, and it fails only
/// the dialing side.
pub struct FaultyPort {
    port: FakePort,
    faults: Faults,
}

impl FaultyPort {
    pub fn new(port: FakePort, faults: Faults) -> FaultyPort {
        FaultyPort { port, faults }
    }
}

impl CarrierPort for FaultyPort {
    fn bind(&self, config: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>> {
        self.port.bind(config)
    }

    fn dial<'a>(
        &'a self,
        peer: &'a CarrierAddr,
    ) -> PortFuture<'a, Result<Box<dyn CarrierLink>, CarrierError>> {
        Box::pin(async move {
            let link = self.port.dial(peer).await?;
            let fault = lock(&self.faults.0).pop_front();
            let faulty = FaultyLink {
                link,
                fault,
                sent: AtomicUsize::new(0),
                failed: AtomicBool::new(false),
            };
            Ok(Box::new(faulty) as Box<dyn CarrierLink>)
        })
    }

    fn accept(&self) -> PortFuture<'_, Result<Option<Box<dyn CarrierLink>>, CarrierError>> {
        self.port.accept()
    }

    fn close(&self) -> PortFuture<'_, ()> {
        self.port.close()
    }
}

struct FaultyLink {
    link: Box<dyn CarrierLink>,
    fault: Option<Fault>,
    sent: AtomicUsize,
    failed: AtomicBool,
}

impl CarrierLink for FaultyLink {
    fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
        Box::pin(async move {
            if self.failed.load(Ordering::SeqCst) {
                return Err(CarrierError::Closed);
            }
            let at = self.sent.fetch_add(1, Ordering::SeqCst);
            let Some(fault) = self.fault.filter(|fault| fault.frame == at) else {
                return self.link.send(frame).await;
            };
            if fault.delivered {
                self.link.send(frame).await?;
            }
            self.failed.store(true, Ordering::SeqCst);
            self.link.close().await;
            Err(CarrierError::Transport(
                "injected: the transport failed".into(),
            ))
        })
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
        self.link.recv()
    }

    fn close(&self) -> PortFuture<'_, ()> {
        self.link.close()
    }
}

/// The engine a test node's record host persists through, which the test
/// reads back: the last snapshot saved, in memory. It is not durability:
/// nothing outlives the process, and no save fails.
#[derive(Clone, Default)]
pub struct VolatileStore(Arc<Mutex<SystemSnapshot>>);

impl VolatileStore {
    /// What was last persisted.
    pub fn snapshot(&self) -> SystemSnapshot {
        lock(&self.0).clone()
    }

    /// The ops last persisted, in the order the fold holds them.
    pub fn ops(&self) -> Vec<Op> {
        let records = self.snapshot().records;
        records
            .iter()
            .map(|bytes| Op::from_cbor(&cbor::decode(bytes)))
            .collect()
    }
}

impl StoreApi for VolatileStore {
    fn load(&self) -> io::Result<SystemSnapshot> {
        Ok(self.snapshot())
    }

    fn save(&mut self, snap: &SystemSnapshot) -> io::Result<()> {
        *lock(&self.0) = snap.clone();
        Ok(())
    }
}

/// A grant fold a journey changes between two decisions: records load in
/// order, grants union, and a revocation denies its (holder, share) pair
/// whichever came first. It reads its records at every check, so nothing is
/// cached across a change. Not the node's fold: no chain, origin, issuer or
/// persistence.
#[derive(Clone)]
pub struct LiveGrants(Arc<Mutex<(Vec<GrantRecord>, bool)>>);

impl LiveGrants {
    /// The contract's fixture fold (GR-001, GR-002), loaded record by record.
    pub fn fixture() -> LiveGrants {
        let fold = LiveGrants(Arc::new(Mutex::new((Vec::new(), true))));
        for record in grant::fold() {
            fold.load(record);
        }
        fold
    }

    pub fn load(&self, record: GrantRecord) {
        lock(&self.0).0.push(record);
    }

    /// Whether the fold can be read: an unreadable one answers `Unavailable`.
    pub fn set_readable(&self, readable: bool) {
        lock(&self.0).1 = readable;
    }
}

impl GrantPort for LiveGrants {
    fn check(&self, holder: &Holder, verb: &str, share: &str) -> Result<(), Denial> {
        let fold = lock(&self.0);
        if !fold.1 {
            return Err(Denial::Unavailable);
        }
        let pair = |h: &Holder, s: &str| h == holder && s == share;
        let revoked = fold.0.iter().any(|record| match record {
            GrantRecord::Revoke {
                holder: h,
                share: s,
            } => pair(h, s),
            GrantRecord::Grant { .. } => false,
        });
        let granted = fold.0.iter().any(|record| match record {
            GrantRecord::Grant {
                holder: h,
                share: s,
                verbs,
            } => pair(h, s) && verbs.contains(&verb),
            GrantRecord::Revoke { .. } => false,
        });
        match (revoked, granted) {
            (true, _) => Err(Denial::Revoked),
            (false, true) => Ok(()),
            (false, false) => Err(Denial::NoGrant),
        }
    }
}

/// CA-001..004 need three ports on one network; each is faulty, with no
/// fault queued.
fn carrier_fixture() -> Fixture {
    let net = FakeNet::new();
    let port = |net: &Arc<FakeNet>| -> Arc<dyn CarrierPort> {
        Arc::new(FaultyPort::new(net.port(), Faults::default()))
    };
    Fixture {
        a: port(&net),
        b: port(&net),
        fresh: port(&net),
        at_a: CarrierAddr("a".into()),
        at_b: CarrierAddr("b".into()),
    }
}

#[test]
fn ca_001_to_004_a_faulty_port_with_no_fault_queued_keeps_the_carrier_contract() {
    run(carrier::frames(carrier_fixture()));
    run(carrier::frame_limit(carrier_fixture()));
    run(carrier::cancellation(carrier_fixture()));
    run(carrier::close_by_value(carrier_fixture()));
}

#[test]
fn gr_001_to_003_the_live_fold_keeps_the_grant_contract() {
    let fold = LiveGrants::fixture();
    grant::exact(&fold);
    grant::revocation_wins(&fold);
    fold.set_readable(false);
    grant::unavailable(&fold);
}
