//! A bounded, pull-based subscription session; no channel/runtime types.
use std::future::Future;
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event<C> {
    Snapshot {
        cursor: C,
        bytes: Vec<u8>,
    },
    /// Explicit resume handshake; no fabricated initial snapshot.
    Resumed {
        cursor: C,
    },
    Delta {
        from: C,
        to: C,
        bytes: Vec<u8>,
    },
    /// Continuity lost (retention or bounded-buffer overflow). Explicit reopen needed.
    Gap,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    Closed,
    Denied,
    Unavailable,
    InvalidData,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseError {
    Unavailable,
}

/// One authenticated, exact-binding subscription obtained from a factory.
/// Cursor is opaque and scoped to this binding, generation and delivery profile;
/// it MUST NOT be compared as a universal sequence or reused across scopes.
/// Fresh sessions MUST emit Snapshot first; resumed sessions MUST emit Resumed at
/// the requested cursor or Gap, never silently restart. Deltas MUST link from the
/// last delivered cursor; snapshots/delta bytes retain their exact shape profile.
/// This is not a universal merge algorithm or the raw terminal live channel.
///
/// Implementations MUST reauthorize deliveries and bound buffered bytes/events.
/// Overflow/retention loss MUST emit Gap (not silently drop data or claim success).
/// After Gap, every next MUST return a ReadError without waiting for new data;
/// no more events may be emitted until explicit reopen. Payload limits
/// MUST be configured at opening. Empty availability is not end-of-stream.
/// next waits asynchronously when idle; faults are explicit, not fabricated data.
///
/// Futures MUST be lazy. Dropping a pending next MUST NOT consume an undelivered
/// event. close success MUST release local owned resources, prohibit later delivery
/// and be idempotent; it does not prove the remote provider has stopped. On close
/// error local delivery still MUST stop, and close MAY be retried for cleanup.
/// Mutable borrowing serializes reads/close; no spawned tasks are mandated.
///
/// ```compile_fail
/// use glade_subscription_api::Subscription;
/// struct Missing;
/// impl Subscription for Missing { type Cursor=u64; }
/// ```
pub trait Subscription: Send {
    type Cursor: Clone + Eq + Send + Sync;
    fn next(&mut self) -> impl Future<Output = Result<Event<Self::Cursor>, ReadError>> + Send;
    fn close(&mut self) -> impl Future<Output = Result<(), CloseError>> + Send;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start<C> {
    Fresh,
    Resume(C),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open<B, C> {
    pub binding: B,
    pub start: Start<C>,
    pub max_event_bytes: std::num::NonZeroUsize,
    pub max_buffered_events: std::num::NonZeroUsize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    Denied,
    Unsupported,
    InvalidCursor,
    Gap,
    Unavailable,
    Capacity,
}

/// Open one subscription after validating authenticated caller, exact binding,
/// delivery profile and cursor scope. This MUST NOT attach a supplier or confer
/// source authority. Both per-event bytes and queued event count MUST be bounded;
/// total queued bytes MUST NOT exceed their checked product. Unrepresentable
/// limits MUST return Capacity. Opening errors MUST release acquired resources.
/// A resume gap MUST be explicit (OpenError::Gap or the first Event::Gap).
/// Futures MUST be lazy; dropped polled opens MUST not leak an unowned session.
/// No shape engine is implemented by this interface; exact adapter capabilities
/// are checked before opening. Provider routes remain outside this contract.
///
/// ```compile_fail
/// use glade_subscription_api::Subscriber;
/// struct Missing;
/// impl Subscriber for Missing {}
/// ```
pub trait Subscriber: Send + Sync {
    type Binding: Send + Sync;
    type Session: Subscription;
    fn open(
        &self,
        request: Open<Self::Binding, <Self::Session as Subscription>::Cursor>,
    ) -> impl Future<Output = Result<Self::Session, OpenError>> + Send;
}
