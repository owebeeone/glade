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
