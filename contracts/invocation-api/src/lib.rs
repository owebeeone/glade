//! Runtime-independent directed exchange. Not a delivery shape or command retry engine.
use std::{future::Future, time::Duration};
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

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    use crate::*;
    /// Adapter-reusable outcome assertion with a real binding type and request payload.
    pub async fn assert_outcome<I: Invoker>(
        invoker: &I,
        request: Call<I::Binding>,
        expected: Outcome,
    ) {
        if let Outcome::Completed(completion) = &expected {
            assert_eq!(completion.correlation, request.correlation);
        }
        assert_eq!(invoker.invoke(request).await, expected);
    }
    pub fn call() -> Call<String> {
        Call {
            binding: "authorized-exchange".into(),
            correlation: 7,
            payload: vec![1, 2],
            budget: Duration::from_secs(1),
        }
    }
    /// IV-001. Echo fixture; payload "fail" yields an application error.
    pub async fn roundtrip<I: Invoker<Binding = String>>(invoker: &I) {
        let request = call();
        assert_eq!(
            invoker.invoke(request.clone()).await,
            Outcome::Completed(Completion {
                correlation: 7,
                result: Ok(vec![1, 2])
            })
        );
        let mut request = request;
        request.correlation = 8;
        request.payload = b"fail".to_vec();
        assert_eq!(
            invoker.invoke(request).await,
            Outcome::Completed(Completion {
                correlation: 8,
                result: Err("application rejection".into())
            })
        );
    }
    /// IV-002. Caller must also assert the adapter's dispatch counter remains zero.
    pub async fn rejected<I: Invoker<Binding = String>>(invoker: &I) {
        let mut request = call();
        request.binding = "denied".into();
        assert_eq!(
            invoker.invoke(request).await,
            Outcome::Rejected(Rejection::Denied)
        );
        let mut request = call();
        request.budget = Duration::ZERO;
        assert_eq!(
            invoker.invoke(request).await,
            Outcome::Rejected(Rejection::Deadline)
        );
    }
    /// IV-003. Fixture loses the connection after dispatch. One attempt only.
    pub async fn unknown<I: Invoker<Binding = String>>(invoker: &I) {
        assert_outcome(
            invoker,
            call(),
            Outcome::Unknown(UnknownReason::ConnectionLost),
        )
        .await;
    }

    /// IV-005. Fixture deadline fires after dispatch; outcome cannot promise no effect.
    pub async fn deadline<I: Invoker<Binding = String>>(invoker: &I) {
        assert_eq!(
            invoker.invoke(call()).await,
            Outcome::Unknown(UnknownReason::Deadline)
        );
    }
    /// IV-006. First request remains pending after dispatch. Duplicate MUST NOT execute.
    pub async fn duplicate_live<I: Invoker<Binding = String>>(invoker: &I) {
        use std::task::{Context, Poll, Waker};
        let mut first = Box::pin(invoker.invoke(call()));
        assert!(matches!(
            first.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        assert_eq!(
            invoker.invoke(call()).await,
            Outcome::Rejected(Rejection::InvalidRequest)
        );
        drop(first);
    }
}
