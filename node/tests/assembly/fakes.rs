//! The deterministic providers a test composition of `NodeAssembly` binds.
//! Each runs its contract's conformance suite in `conformance.rs`. None is a
//! transport, a time source, a registry or cryptography, and each says what it
//! does not prove. The in-memory store and registry are not here: they are the
//! node's own `Registry` over `MemStore`, built by `Records::in_memory`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture, TransportId,
};
use glade_clock_api::ClockPort;
use glade_grant_api::conformance::{self as grant_conformance, Record as GrantRecord};
use glade_grant_api::{Denial, GrantPort, Holder};
use glade_node::assembly::{ConfigPort, Settings};
use glade_signer_api::{
    NodeId, Purpose, SignError, SignatureStatus, SignerPort, VerificationError,
};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Poll `future` to completion with no runtime. The fakes wait on nothing
/// outside the future, so one still pending after many polls is a defect.
pub fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..64 {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
    }
    panic!("still pending: a fake waited on something outside the future");
}

/// The clock a test composition substitutes once for every consumer: every
/// clone reads one atomic instant. It proves nothing about the OS clock.
#[derive(Clone)]
pub struct FakeClock(Arc<AtomicI64>);

impl FakeClock {
    pub fn at(instant: i64) -> FakeClock {
        FakeClock(Arc::new(AtomicI64::new(instant)))
    }

    pub fn set(&self, instant: i64) {
        self.0.store(instant, Ordering::SeqCst);
    }

    pub fn advance(&self, by: i64) {
        self.0.fetch_add(by, Ordering::SeqCst);
    }
}

impl ClockPort for FakeClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A fake network: one mutex, no socket, no runtime and no sleep. Ports bound
/// on one network reach one another by address, which is how two sibling
/// test nodes meet. It proves the carrier contract's shape, never a transport.
#[derive(Default)]
pub struct FakeNet(Mutex<Net>);

#[derive(Default)]
struct Net {
    /// Address to (endpoint, frame limit).
    bound: BTreeMap<CarrierAddr, (usize, usize)>,
    backlog: BTreeMap<usize, VecDeque<Box<dyn CarrierLink>>>,
    closed: BTreeSet<usize>,
    pipes: Vec<Pipe>,
    endpoints: usize,
    wakers: Vec<Waker>,
}

#[derive(Default)]
struct Pipe {
    frames: VecDeque<Vec<u8>>,
    writer_closed: bool,
    reader_closed: bool,
}

impl FakeNet {
    pub fn new() -> Arc<FakeNet> {
        Arc::new(FakeNet::default())
    }

    /// An unbound port on this network.
    pub fn port(self: &Arc<Self>) -> FakePort {
        FakePort {
            net: self.clone(),
            phase: Mutex::new(Phase::Unbound),
        }
    }

    /// Change the network, then wake every waiting future.
    fn change<T>(&self, change: impl FnOnce(&mut Net) -> T) -> T {
        let mut net = lock(&self.0);
        let out = change(&mut net);
        for waker in net.wakers.drain(..) {
            waker.wake();
        }
        out
    }

    /// Look at the network; a pending answer waits for the next change.
    fn watch<T>(&self, cx: &Context<'_>, look: impl FnOnce(&mut Net) -> Poll<T>) -> Poll<T> {
        let mut net = lock(&self.0);
        let out = look(&mut net);
        if out.is_pending() {
            net.wakers.push(cx.waker().clone());
        }
        out
    }
}

enum Phase {
    Unbound,
    Bound {
        id: usize,
        at: CarrierAddr,
        max: usize,
    },
    Closed,
}

/// One carrier endpoint on a [`FakeNet`].
pub struct FakePort {
    net: Arc<FakeNet>,
    phase: Mutex<Phase>,
}

impl FakePort {
    fn bound(&self) -> Option<(usize, usize)> {
        match *lock(&self.phase) {
            Phase::Bound { id, max, .. } => Some((id, max)),
            _ => None,
        }
    }

    fn link(&self, ends: (usize, usize), max: usize, tx: usize, rx: usize) -> Box<dyn CarrierLink> {
        let (net, (endpoint, remote)) = (self.net.clone(), ends);
        Box::new(FakeLink {
            net,
            endpoint,
            remote,
            max,
            tx,
            rx,
        })
    }
}

impl CarrierPort for FakePort {
    fn bind(&self, config: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>> {
        Box::pin(async move {
            let mut phase = lock(&self.phase);
            match *phase {
                Phase::Unbound => {}
                Phase::Bound { .. } => {
                    return Err(CarrierError::AlreadyBound);
                }
                Phase::Closed => {
                    return Err(CarrierError::Closed);
                }
            }
            let (at, max) = (config.local, config.max_frame_bytes.get());
            let id = self.net.change(|net| {
                if net.bound.contains_key(&at) {
                    return Err(CarrierError::Transport("address in use".into()));
                }
                net.endpoints += 1;
                net.bound.insert(at.clone(), (net.endpoints, max));
                Ok(net.endpoints)
            })?;
            *phase = Phase::Bound {
                id,
                at: at.clone(),
                max,
            };
            Ok(at)
        })
    }

    fn dial<'a>(
        &'a self,
        peer: &'a CarrierAddr,
    ) -> PortFuture<'a, Result<Box<dyn CarrierLink>, CarrierError>> {
        Box::pin(async move {
            let Some((id, max)) = self.bound() else {
                return Err(CarrierError::Closed);
            };
            self.net.change(|net| {
                let Some(&(target, target_max)) = net.bound.get(peer) else {
                    return Err(CarrierError::Transport("nobody is bound there".into()));
                };
                let (out, back) = (net.pipes.len(), net.pipes.len() + 1);
                net.pipes.extend([Pipe::default(), Pipe::default()]);
                let accepted = self.link((target, id), target_max, back, out);
                net.backlog.entry(target).or_default().push_back(accepted);
                Ok(self.link((id, target), max, out, back))
            })
        })
    }

    fn accept(&self) -> PortFuture<'_, Result<Option<Box<dyn CarrierLink>>, CarrierError>> {
        Box::pin(poll_fn(move |cx| {
            let Some((id, _)) = self.bound() else {
                return Poll::Ready(Ok(None));
            };
            self.net.watch(cx, |net| {
                match net.backlog.get_mut(&id).and_then(VecDeque::pop_front) {
                    Some(link) => Poll::Ready(Ok(Some(link))),
                    None => Poll::Pending,
                }
            })
        }))
    }

    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let taken = std::mem::replace(&mut *lock(&self.phase), Phase::Closed);
            if let Phase::Bound { id, at, .. } = taken {
                self.net.change(|net| {
                    net.closed.insert(id);
                    net.backlog.remove(&id);
                    net.bound.remove(&at);
                });
            }
        })
    }
}

struct FakeLink {
    net: Arc<FakeNet>,
    endpoint: usize,
    /// The far end, whose number is its transport identity on this network.
    remote: usize,
    max: usize,
    tx: usize,
    rx: usize,
}

impl FakeLink {
    fn end(&self, net: &mut Net) {
        net.pipes[self.tx].writer_closed = true;
        net.pipes[self.rx].reader_closed = true;
    }
}

impl CarrierLink for FakeLink {
    fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
        Box::pin(async move {
            self.net.change(|net| {
                if net.pipes[self.tx].writer_closed || net.closed.contains(&self.endpoint) {
                    return Err(CarrierError::Closed);
                }
                if frame.len() > self.max {
                    return Err(CarrierError::FrameTooLarge);
                }
                net.pipes[self.tx].frames.push_back(frame.to_vec());
                Ok(())
            })
        })
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
        Box::pin(poll_fn(move |cx| {
            self.net.watch(cx, |net| {
                if net.pipes[self.rx].reader_closed || net.closed.contains(&self.endpoint) {
                    return Poll::Ready(Ok(None));
                }
                match net.pipes[self.rx].frames.pop_front() {
                    Some(frame) if frame.len() > self.max => {
                        self.end(net);
                        Poll::Ready(Err(CarrierError::FrameTooLarge))
                    }
                    Some(frame) => Poll::Ready(Ok(Some(frame))),
                    None if net.pipes[self.rx].writer_closed => Poll::Ready(Ok(None)),
                    None => Poll::Pending,
                }
            })
        }))
    }

    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(async move { self.net.change(|net| self.end(net)) })
    }

    fn remote_id(&self) -> Option<TransportId> {
        Some(TransportId(self.remote.to_le_bytes().to_vec()))
    }
}

/// A keyed test signer: a signature is FNV-1a over a per-node secret, the
/// purpose and the message. Anyone who reads this file can forge one; it pins
/// `SignerPort`'s shape (SI-001..003) and is never cryptography.
#[derive(Clone)]
pub struct KeyedTestSigner {
    me: NodeId,
    secrets: BTreeMap<NodeId, u64>,
    available: bool,
}

/// The node a test composition's signer signs as, and one other it resolves.
pub const ME: NodeId = [1; 32];
pub const OTHER: NodeId = [2; 32];
/// A node whose key no test signer resolves.
pub const STRANGER: NodeId = [3; 32];

impl KeyedTestSigner {
    /// Signs as [`ME`] and resolves [`ME`] and [`OTHER`], never [`STRANGER`].
    pub fn fixture() -> KeyedTestSigner {
        KeyedTestSigner {
            me: ME,
            secrets: BTreeMap::from([(ME, 11), (OTHER, 22)]),
            available: true,
        }
    }

    /// The same keys, configured unavailable (SI-003).
    pub fn unavailable() -> KeyedTestSigner {
        KeyedTestSigner {
            available: false,
            ..KeyedTestSigner::fixture()
        }
    }
}

fn checksum(secret: u64, purpose: Purpose, message: &[u8]) -> Vec<u8> {
    let domain = purpose as u8;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ secret;
    for byte in std::iter::once(&domain).chain(message) {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash.to_le_bytes().to_vec()
}

impl SignerPort for KeyedTestSigner {
    fn node_id(&self) -> NodeId {
        self.me
    }

    fn sign(&self, purpose: Purpose, message: &[u8]) -> Result<Vec<u8>, SignError> {
        match self.secrets.get(&self.me) {
            Some(secret) if self.available => Ok(checksum(*secret, purpose, message)),
            _ => Err(SignError::Unavailable),
        }
    }

    fn verify(
        &self,
        signer: &NodeId,
        purpose: Purpose,
        message: &[u8],
        signature: &[u8],
    ) -> Result<SignatureStatus, VerificationError> {
        let Some(secret) = self.secrets.get(signer).filter(|_| self.available) else {
            return Err(VerificationError::Unavailable);
        };
        if checksum(*secret, purpose, message) == signature {
            return Ok(SignatureStatus::Valid);
        }
        Ok(SignatureStatus::Invalid)
    }
}

/// An in-memory grant fold: grants union, and a revocation denies its
/// (holder, share) pair in either order. Not the node's registry: it has no
/// chain, origin or persistence.
#[derive(Clone)]
pub struct MemGrants {
    records: Arc<Vec<GrantRecord>>,
    readable: bool,
}

impl MemGrants {
    /// The contract's fixture fold (GR-001, GR-002), loaded in order.
    pub fn fixture() -> MemGrants {
        MemGrants {
            records: Arc::new(grant_conformance::fold()),
            readable: true,
        }
    }

    /// The same fold, configured unreadable (GR-003).
    pub fn unreadable() -> MemGrants {
        MemGrants {
            readable: false,
            ..MemGrants::fixture()
        }
    }
}

impl GrantPort for MemGrants {
    fn check(&self, holder: &Holder, verb: &str, share: &str) -> Result<(), Denial> {
        if !self.readable {
            return Err(Denial::Unavailable);
        }
        let (mut granted, mut revoked) = (false, false);
        for record in self.records.iter() {
            match record {
                GrantRecord::Grant {
                    holder: h,
                    share: s,
                    verbs,
                } if h == holder && *s == share => {
                    granted |= verbs.contains(&verb);
                }
                GrantRecord::Revoke {
                    holder: h,
                    share: s,
                } if h == holder && *s == share => {
                    revoked = true;
                }
                _ => {}
            }
        }
        if revoked {
            return Err(Denial::Revoked);
        }
        if granted {
            return Ok(());
        }
        Err(Denial::NoGrant)
    }
}

/// Fixed configuration: settings a test chooses, read from no argument list
/// and no environment.
pub struct FixedSettings(pub Settings);

impl ConfigPort for FixedSettings {
    fn settings(&self) -> &Settings {
        &self.0
    }
}
