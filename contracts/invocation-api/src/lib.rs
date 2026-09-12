//! Runtime-independent directed exchange. Not a delivery shape or command retry engine.
use std::{future::Future, time::Duration};
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call<B> {
    pub binding: B,
    /// Unique among live calls in the authenticated session. NOT an idempotency key.
    pub correlation: u64,
    pub payload: Vec<u8>,
    /// Monotonic elapsed-time budget starting on first poll; zero forbids dispatch.
    pub budget: Duration,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub correlation: u64,
    pub result: Result<Vec<u8>, String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    Denied,
    Unsupported,
    InvalidRequest,
    Deadline,
    Unavailable,
    Capacity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    Deadline,
    ConnectionLost,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed(Completion),
    Rejected(Rejection),
    Unknown(UnknownReason),
}

/// Handle bound to an authenticated caller/session. Binding is a narrow consumer
/// type; implementations MUST revalidate exact surface, capability, principal and
/// provider epoch before effects. DTO principal fields MUST NOT override session
/// identity. Authenticated provider call context MUST travel beside opaque payloads;
/// forwarding MUST preserve requester/evidence/correlation and reject substitution.
///
/// A completion MUST match the request correlation; application failure is data.
/// Rejected means positively no dispatch; after possible dispatch, loss/deadline
/// MUST be Unknown, never a safe-retry rejection. Implementations MUST NOT
/// automatically retry possibly executed commands, including on reconnection.
/// A successful response is not a generic durable commit or replication receipt.
/// Execution may already have effects even when an application result is Err.
///
/// Futures MUST be lazy. Dropping a polled future does not imply remote cancellation
/// or rollback; adapter-owned cleanup MUST release local waiters. The time budget
/// MUST terminate local waiting, not promise remote work stops. Buffers are bounded.
/// Duplicate live correlations MUST be rejected before dispatch.
///
/// ```compile_fail
/// use glade_invocation_api::Invoker;
/// struct Missing;
/// impl Invoker for Missing { type Binding=String; }
/// ```
pub trait Invoker: Send + Sync {
    type Binding: Send + Sync;
    fn invoke(&self, call: Call<Self::Binding>) -> impl Future<Output = Outcome> + Send;
}
