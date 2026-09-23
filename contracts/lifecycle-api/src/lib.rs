//! Resource cleanup obligations, not a scheduler, actor system or sdax port.
use std::{future::Future, time::Duration};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Open,
    Closing,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Drain,
    Cancel,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shutdown {
    pub mode: Mode,
    pub budget: Duration,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub resource: String,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    pub remaining: Vec<String>,
    pub failures: Vec<Failure>,
}

/// A resource owner, not an async executor. Shutdown MUST stop admitting new work
/// when first polled. Drain permits existing work within the monotonic budget;
/// Cancel requests cooperative cancellation. Neither means rollback of external
/// effects. Budget expiry MUST return an honest remaining-resource report.
///
/// Reports MUST retain cleanup failures (one failure MUST NOT hide another).
/// Closed requires no remaining owned work/resources; otherwise phase is Closing.
/// Repeated shutdown MUST retry unfinished cleanup without repeating completed
/// irreversible cleanup, and Closed MUST remain terminal. Errors may remain in
/// the report after resources were successfully released; they are not success
/// claims. Resource lists MUST be bounded/configured at ownership acquisition.
///
/// Futures MUST be lazy. Dropping a polled shutdown leaves ownership and its
/// progress ledger recoverable for another shutdown attempt; drop is not cleanup.
/// Implementations MUST NOT detach untracked tasks to manufacture completion.
/// Dependency ordering belongs to composition; independent cleanup MAY run in
/// parallel. This trait chooses neither channels, threads, macros nor a runtime.
/// Timely return requires cooperative/pollable host operations; adapters must not
/// block an executor thread in an uncancellable operation.
///
/// ```compile_fail
/// use glade_lifecycle_api::ManagedResource;
/// struct Missing;
/// impl ManagedResource for Missing {}
/// ```
pub trait ManagedResource: Send {
    fn phase(&self) -> Phase;
    fn shutdown(&mut self, request: Shutdown) -> impl Future<Output = ShutdownReport> + Send;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    use crate::*;
    pub fn request() -> Shutdown {
        Shutdown {
            mode: Mode::Drain,
            budget: Duration::from_secs(1),
        }
    }
    /// LC-001. Resource can close completely without a fault.
    pub async fn closed<R: ManagedResource>(resource: &mut R) {
        let report = resource.shutdown(request()).await;
        assert!(report.remaining.is_empty());
        assert!(report.failures.is_empty());
        assert_eq!(resource.phase(), Phase::Closed);
        assert_eq!(resource.shutdown(request()).await, report);
        assert_eq!(resource.phase(), Phase::Closed);
    }
    /// LC-002. First close fails for socket; retry succeeds and preserves ownership.
    pub async fn failure<R: ManagedResource>(resource: &mut R) {
        assert_eq!(
            resource.shutdown(request()).await,
            ShutdownReport {
                remaining: vec!["socket".into()],
                failures: vec![Failure {
                    resource: "socket".into(),
                    reason: "fixture failure".into()
                }]
            }
        );
        assert_eq!(resource.phase(), Phase::Closing);
        closed(resource).await;
    }

    /// LC-004. Fixture has outstanding work and cannot synchronously finish at zero budget.
    pub async fn budget<R: ManagedResource>(resource: &mut R) {
        let report = resource
            .shutdown(Shutdown {
                mode: Mode::Drain,
                budget: Duration::ZERO,
            })
            .await;
        assert_eq!(report.remaining, vec!["work"]);
        assert_eq!(resource.phase(), Phase::Closing);
        closed(resource).await;
    }
    /// LC-005. Inject a pending first cleanup, drop the future, retain ownership.
    pub fn poll_then_drop<R: ManagedResource>(resource: &mut R) {
        use std::task::{Context, Poll, Waker};
        let mut future = Box::pin(resource.shutdown(request()));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        drop(future);
        assert_eq!(resource.phase(), Phase::Closing);
    }
    /// LC-006. Fixture can cancel outstanding work without executing it.
    pub async fn cancel<R: ManagedResource>(resource: &mut R) {
        let report = resource
            .shutdown(Shutdown {
                mode: Mode::Cancel,
                budget: Duration::from_secs(1),
            })
            .await;
        assert!(report.remaining.is_empty());
        assert!(report.failures.is_empty());
        assert_eq!(resource.phase(), Phase::Closed);
    }
}
