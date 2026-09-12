use glade_subscription_api::{CloseError, Event, ReadError, Subscription, conformance};
use std::collections::VecDeque;
use std::future::Future;
use std::task::{Context, Poll, Waker};
struct Model {
    events: VecDeque<Event<u64>>,
    closed: bool,
    ignore_close: bool,
}
impl Subscription for Model {
    type Cursor = u64;
    async fn next(&mut self) -> Result<Event<u64>, ReadError> {
        if self.closed && !self.ignore_close {
            return Err(ReadError::Closed);
        }
        self.events.pop_front().ok_or(ReadError::Unavailable)
    }
    async fn close(&mut self) -> Result<(), CloseError> {
        self.closed = true;
        Ok(())
    }
}
fn run(f: impl Future<Output = ()> + Send) {
    let mut f = std::pin::pin!(f);
    assert!(matches!(
        f.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(())
    ));
}
fn normal() -> Model {
    Model {
        events: VecDeque::from(conformance::events()),
        closed: false,
        ignore_close: false,
    }
}
#[test]
fn su_001_snapshot_then_contiguous_delta() {
    run(conformance::sequence(&mut normal()));
}
#[test]
fn su_002_gap_is_explicit() {
    let mut m = normal();
    m.events = VecDeque::from([Event::Gap]);
    run(conformance::gap(&mut m));
}
#[test]
fn su_003_close_is_idempotent_and_terminal() {
    run(conformance::closed(&mut normal()));
}
#[test]
fn su_004_unpolled_read_does_not_consume() {
    let mut m = normal();
    let f = m.next();
    drop(f);
    run(conformance::sequence(&mut m));
}
#[test]
#[should_panic]
fn rejects_update_after_close() {
    let mut m = normal();
    m.ignore_close = true;
    run(conformance::closed(&mut m));
}
#[test]
#[should_panic]
fn rejects_delta_gap() {
    let mut m = normal();
    m.events = VecDeque::from([
        Event::Snapshot {
            cursor: 1,
            bytes: vec![10],
        },
        Event::Delta {
            from: 0,
            to: 2,
            bytes: vec![11],
        },
    ]);
    run(conformance::sequence(&mut m));
}

struct Factory {
    forget_resume: bool,
}

#[test]
fn su_006_buffer_product_overflow_is_rejected() {
    run(async {
        use glade_subscription_api::{Open, OpenError, Start, Subscriber};
        let request = Open {
            binding: "notes".to_string(),
            start: Start::Fresh,
            max_event_bytes: std::num::NonZeroUsize::new(usize::MAX).unwrap(),
            max_buffered_events: std::num::NonZeroUsize::new(2).unwrap(),
        };
        assert!(matches!(
            Factory {
                forget_resume: false
            }
            .open(request)
            .await,
            Err(OpenError::Capacity)
        ));
    });
}
impl glade_subscription_api::Subscriber for Factory {
    type Binding = String;
    type Session = Model;
    async fn open(
        &self,
        request: glade_subscription_api::Open<String, u64>,
    ) -> Result<Model, glade_subscription_api::OpenError> {
        use glade_subscription_api::{OpenError, Start};
        if request.binding != "notes" {
            return Err(OpenError::Denied);
        }
        if request
            .max_event_bytes
            .get()
            .checked_mul(request.max_buffered_events.get())
            .is_none()
        {
            return Err(OpenError::Capacity);
        }
        let event = match request.start {
            Start::Fresh => Event::Snapshot {
                cursor: 1,
                bytes: vec![10],
            },
            Start::Resume(1) if !self.forget_resume => Event::Resumed { cursor: 1 },
            Start::Resume(1) => Event::Snapshot {
                cursor: 1,
                bytes: vec![10],
            },
            Start::Resume(_) => return Err(OpenError::Gap),
        };
        Ok(Model {
            events: VecDeque::from([event]),
            closed: false,
            ignore_close: false,
        })
    }
}
#[test]
fn su_005_open_and_resume() {
    run(conformance::opening(&Factory {
        forget_resume: false,
    }));
}
#[test]
#[should_panic]
fn rejects_silent_resume_reset() {
    run(conformance::opening(&Factory {
        forget_resume: true,
    }));
}

#[test]
#[should_panic]
fn rejects_delta_after_gap() {
    let mut m = normal();
    m.events = VecDeque::from([
        Event::Gap,
        Event::Delta {
            from: 1,
            to: 2,
            bytes: vec![11],
        },
    ]);
    run(conformance::gap(&mut m));
}
