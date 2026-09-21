//! Witness ports and deterministic fakes: the contracts `async_witness`
//! declares for itself.
//!
//! The architecture names `ClockPort`, `CarrierPort` and four more as binding
//! recipes (`arch1/InjectionGraphRefinement.md`), but **none of them exists as
//! a Rust trait** — they are architecture declarations. The witness must
//! therefore declare the port it uses; it cannot import one. These are not
//! proposed Glade API replacements, and nothing here is production code.
//!
//! **This crate is the DI-E04 wall.** Its only dependency is `glade-wire`,
//! which is itself zero-dependency, and nothing here may ever name a framework:
//! no `shaku`, no `sdax`, no `tokio`, no runtime and no socket. The
//! architecture gate enforces that from the manifest, where no `#[cfg]` can
//! reach around it.

use std::any::Any;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

pub use glade_wire::generated::FrameType;

/// A boxed, `Send` future.
///
/// The async ports box their futures so they stay dyn-compatible: an injected
/// port is an `Arc<dyn CarrierPort>`, and `impl Future` in return position is
/// not (E0038 — `glade_lifecycle_api::ManagedResource` is the standing proof).
/// sdax-rs boxes every trait future for the same reason, in its own words so
/// "these traits must stay usable as `dyn`". `Pin`, `Box` and `Future` are all
/// std, so no framework enters the crate, and **no Glade contract is changed**
/// to obtain this: the boxing lives in the witness's own port.
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One frame as a carrier moves it: the `FrameType` tag and the opaque body,
/// the same split `node/src/frame.rs` encodes as `[tag byte][CBOR body]`.
pub type CarriedFrame = (FrameType, Vec<u8>);

/// Wall-clock reads in milliseconds since the epoch — the dependency
/// `sysdir::now_ms()` is today, in the unit `RegistryApi::who_serves` already
/// takes as a parameter. The port supplies the instant; lease expiry and every
/// other read-time policy stay the caller's business.
///
/// `Any + Send + Sync` is load-bearing, not decoration: the assembly-only
/// facade of `AsyncWitnessPlan.md` §4.3 is `trait Clock: ClockPort +
/// shaku::Interface {}` with a blanket impl over every implementation, and that
/// impl only compiles because a `ClockPort` already satisfies Shaku's
/// `Interface`. All three are std traits; the port never names Shaku.
pub trait ClockPort: Any + Send + Sync {
    fn now_ms(&self) -> i64;
}

/// A framed byte carrier: one `[FrameType tag][body]` frame at a time, the
/// split `node/src/frame.rs` already uses, so the node's real `PeerEndpoint`
/// and a recording fake can satisfy the same trait. The body stays opaque
/// bytes — the port carries frames, it does not decode them.
pub trait CarrierPort: Any + Send + Sync {
    /// Hand one frame to the carrier. `Err(CarrierError::Closed)` once the
    /// carrier is closed; a closed carrier never silently drops a frame.
    fn send<'a>(
        &'a self,
        frame: FrameType,
        body: &'a [u8],
    ) -> PortFuture<'a, Result<(), CarrierError>>;

    /// Take the next frame. `Ok(None)` is end of stream, not "nothing yet": a
    /// caller that sees it has been told the peer will send no more.
    fn recv(&self) -> PortFuture<'_, Result<Option<CarriedFrame>, CarrierError>>;
}

/// Append-and-scan over one share's log.
///
/// Synchronous **on purpose**: the node's `Store` is synchronous blocking file
/// I/O and has no `async fn` at all, so a port that promised otherwise would
/// witness a shape the code does not have. `&self` rather than `&mut self`
/// because an injected store is an `Arc<dyn StorePort>` shared by every
/// consumer; an implementation owns its own interior mutability, as the node
/// already does behind a mutex.
pub trait StorePort: Any + Send + Sync {
    /// Append one body and return the sequence number it was given, counting
    /// from 1 within the share.
    fn append(&self, share: &str, body: &[u8]) -> Result<i64, StoreError>;

    /// Every body from `from_seq` onward, in order. An unknown share is empty,
    /// not an error.
    fn scan(&self, share: &str, from_seq: i64) -> Vec<Vec<u8>>;
}

/// Why a carrier refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CarrierError {
    /// The carrier is closed; no further frame will be sent.
    Closed,
    /// The transport failed. A real provider renders its `std::io::Error` into
    /// this rather than putting an I/O type in the port's public signature.
    Transport(String),
}

impl fmt::Display for CarrierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CarrierError::Closed => write!(f, "carrier closed"),
            CarrierError::Transport(reason) => write!(f, "carrier transport failed: {reason}"),
        }
    }
}

impl std::error::Error for CarrierError {}

/// Why a store refused an append.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The store is closed and admits no further append.
    Closed,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Closed => write!(f, "store closed"),
        }
    }
}

impl std::error::Error for StoreError {}

/// A clock over one atomic: deterministic, with no sleep and no wall clock.
///
/// `Clone` shares the instant, so every clone reads what any one of them set.
/// That is the fixture; the single-selection claim DI-E01 makes is about the
/// injected `Arc`, and is asserted there with `Arc::ptr_eq`.
#[derive(Clone, Default)]
pub struct FakeClock(Arc<AtomicI64>);

impl FakeClock {
    pub fn new(now_ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(now_ms)))
    }

    pub fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }

    /// Move time forward and return the new instant.
    pub fn advance(&self, delta_ms: i64) -> i64 {
        self.0.fetch_add(delta_ms, Ordering::SeqCst) + delta_ms
    }
}

impl ClockPort for FakeClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A carrier that records what it was given and replays what it was scripted
/// with. No socket, no runtime, no sleep: both futures are already complete
/// when they are returned, so one poll resolves them.
#[derive(Clone, Default)]
pub struct FakeCarrier {
    sent: Arc<Mutex<Vec<CarriedFrame>>>,
    inbound: Arc<Mutex<VecDeque<CarriedFrame>>>,
    closed: Arc<AtomicBool>,
}

impl FakeCarrier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script one frame for a later `recv`, at the back of the queue.
    pub fn push_inbound(&self, frame: FrameType, body: &[u8]) {
        self.inbound
            .lock()
            .expect("fake carrier inbound lock")
            .push_back((frame, body.to_vec()));
    }

    /// Every frame handed to `send`, in order.
    pub fn sent(&self) -> Vec<CarriedFrame> {
        self.sent.lock().expect("fake carrier sent lock").clone()
    }

    /// Close the carrier: `send` refuses and `recv` reports end of stream,
    /// discarding whatever the script still held.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

impl CarrierPort for FakeCarrier {
    fn send<'a>(
        &'a self,
        frame: FrameType,
        body: &'a [u8],
    ) -> PortFuture<'a, Result<(), CarrierError>> {
        let outcome = if self.closed.load(Ordering::SeqCst) {
            Err(CarrierError::Closed)
        } else {
            self.sent
                .lock()
                .expect("fake carrier sent lock")
                .push((frame, body.to_vec()));
            Ok(())
        };
        Box::pin(std::future::ready(outcome))
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<CarriedFrame>, CarrierError>> {
        let outcome = if self.closed.load(Ordering::SeqCst) {
            None
        } else {
            self.inbound
                .lock()
                .expect("fake carrier inbound lock")
                .pop_front()
        };
        Box::pin(std::future::ready(Ok(outcome)))
    }
}

/// An in-memory log per share: no files, no directories, nothing to clean up.
#[derive(Clone, Default)]
pub struct FakeStore {
    shares: Arc<Mutex<BTreeMap<String, Vec<Vec<u8>>>>>,
    closed: Arc<AtomicBool>,
}

impl FakeStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Close the store: every later append is refused.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

impl StorePort for FakeStore {
    fn append(&self, share: &str, body: &[u8]) -> Result<i64, StoreError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(StoreError::Closed);
        }
        let mut shares = self.shares.lock().expect("fake store lock");
        let log = shares.entry(share.to_owned()).or_default();
        log.push(body.to_vec());
        Ok(log.len() as i64)
    }

    fn scan(&self, share: &str, from_seq: i64) -> Vec<Vec<u8>> {
        let shares = self.shares.lock().expect("fake store lock");
        let Some(log) = shares.get(share) else {
            return Vec::new();
        };
        let skip = from_seq.max(1).saturating_sub(1) as usize;
        log.iter().skip(skip).cloned().collect()
    }
}
