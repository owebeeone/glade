use glade_invocation_api::{
    Call, Completion, Invoker, Outcome, Rejection, UnknownReason, conformance,
};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
struct Model {
    calls: AtomicUsize,
    wrong_corr: bool,
    mode: u8,
    live: std::sync::Mutex<std::collections::BTreeSet<u64>>,
}
struct Active<'a> {
    live: &'a std::sync::Mutex<std::collections::BTreeSet<u64>>,
    id: u64,
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.live.lock().unwrap().remove(&self.id);
    }
}
impl Invoker for Model {
    type Binding = String;
    async fn invoke(&self, call: Call<String>) -> Outcome {
        if call.binding != "authorized-exchange" {
            return Outcome::Rejected(Rejection::Denied);
        }
        if call.budget.is_zero() {
            return Outcome::Rejected(Rejection::Deadline);
        }
        if !self.live.lock().unwrap().insert(call.correlation) {
            return Outcome::Rejected(Rejection::InvalidRequest);
        }
        let _active = Active {
            live: &self.live,
            id: call.correlation,
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.mode == 1 {
            return Outcome::Unknown(UnknownReason::ConnectionLost);
        }
        if self.mode == 2 {
            std::future::pending::<()>().await;
        }
        if self.mode == 3 {
            return Outcome::Unknown(UnknownReason::Deadline);
        }
        Outcome::Completed(Completion {
            correlation: if self.wrong_corr {
                call.correlation + 1
            } else {
                call.correlation
            },
            result: if call.payload == b"fail" {
                Err("application rejection".into())
            } else {
                Ok(call.payload)
            },
        })
    }
}
fn model(mode: u8, wrong_corr: bool) -> Model {
    Model {
        calls: AtomicUsize::new(0),
        wrong_corr,
        mode,
        live: Default::default(),
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
fn iv_001_correlated_success_and_application_failure() {
    run(conformance::roundtrip(&model(0, false)));
}
#[test]
fn iv_002_rejected_before_execution() {
    let m = model(0, false);
    run(conformance::rejected(&m));
    assert_eq!(m.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn iv_003_unknown_is_not_safe_retry() {
    let m = model(1, false);
    run(conformance::unknown(&m));
    assert_eq!(m.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn iv_004_unpolled_is_lazy() {
    let m = model(0, false);
    let f = m.invoke(conformance::call());
    drop(f);
    assert_eq!(m.calls.load(Ordering::SeqCst), 0);
}
#[test]
#[should_panic]
fn rejects_mismatched_correlation() {
    run(conformance::roundtrip(&model(0, true)));
}

#[test]
fn iv_005_post_dispatch_deadline_is_unknown() {
    let m = model(3, false);
    run(conformance::deadline(&m));
    assert_eq!(m.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn iv_006_duplicate_live_correlation_is_rejected() {
    let m = model(2, false);
    run(conformance::duplicate_live(&m));
    assert_eq!(m.calls.load(Ordering::SeqCst), 1);
    assert!(m.live.lock().unwrap().is_empty());
}
