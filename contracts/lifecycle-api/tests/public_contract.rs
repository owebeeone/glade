use glade_lifecycle_api::{Failure, ManagedResource, Phase, Shutdown, ShutdownReport, conformance};
use std::future::Future;
use std::task::{Context, Poll, Waker};
struct Model {
    phase: Phase,
    fail_once: bool,
    lie: bool,
    pending_once: bool,
    executed: usize,
}
impl ManagedResource for Model {
    fn phase(&self) -> Phase {
        self.phase
    }
    async fn shutdown(&mut self, request: Shutdown) -> ShutdownReport {
        if self.phase == Phase::Closed {
            return ShutdownReport {
                remaining: vec![],
                failures: vec![],
            };
        }
        self.phase = Phase::Closing;
        if self.pending_once {
            self.pending_once = false;
            std::future::pending::<()>().await;
        }
        if request.budget.is_zero() {
            return ShutdownReport {
                remaining: vec!["work".into()],
                failures: vec![],
            };
        }
        if self.fail_once {
            self.fail_once = false;
            self.phase = Phase::Closing;
            return ShutdownReport {
                remaining: vec!["socket".into()],
                failures: vec![Failure {
                    resource: "socket".into(),
                    reason: "fixture failure".into(),
                }],
            };
        }
        if request.mode == glade_lifecycle_api::Mode::Drain {
            self.executed += 1;
        }
        self.phase = if self.lie { Phase::Open } else { Phase::Closed };
        ShutdownReport {
            remaining: vec![],
            failures: vec![],
        }
    }
}
fn model(fail_once: bool, lie: bool) -> Model {
    Model {
        phase: Phase::Open,
        fail_once,
        lie,
        pending_once: false,
        executed: 0,
    }
}
fn run(f: impl Future<Output = ()> + Send) {
    let mut f = std::pin::pin!(f);
    assert!(matches!(
        f.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(())
    ));
}
#[test]
fn lc_001_shutdown_is_terminal_and_idempotent() {
    run(conformance::closed(&mut model(false, false)));
}
#[test]
fn lc_002_cleanup_failure_is_retryable_and_visible() {
    run(conformance::failure(&mut model(true, false)));
}
#[test]
fn lc_003_unpolled_shutdown_is_lazy() {
    let mut m = model(false, false);
    let f = m.shutdown(conformance::request());
    drop(f);
    assert_eq!(m.phase(), Phase::Open);
}
#[test]
#[should_panic]
fn rejects_false_cleanup_success() {
    run(conformance::closed(&mut model(false, true)));
}

#[test]
fn lc_004_zero_budget_reports_unfinished_work() {
    run(conformance::budget(&mut model(false, false)));
}
#[test]
fn lc_005_pending_drop_retains_ownership() {
    let mut m = model(false, false);
    m.pending_once = true;
    conformance::poll_then_drop(&mut m);
    run(conformance::closed(&mut m));
}
#[test]
fn lc_006_cancel_does_not_drain_work() {
    let mut m = model(false, false);
    run(conformance::cancel(&mut m));
    assert_eq!(m.executed, 0);
}
