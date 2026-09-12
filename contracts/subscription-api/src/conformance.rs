use crate::*;
pub fn events() -> [Event<u64>; 2] {
    [
        Event::Snapshot {
            cursor: 1,
            bytes: vec![10],
        },
        Event::Delta {
            from: 1,
            to: 2,
            bytes: vec![11],
        },
    ]
}
/// SU-001. Fixture provides a snapshot at 1 then a delta to 2.
pub async fn sequence<S: Subscription<Cursor = u64>>(subscription: &mut S) {
    assert_events(subscription, events()).await;
}
/// Adapter-reusable ordered delivery assertion with native cursors and shape bytes.
pub async fn assert_events<S: Subscription>(
    subscription: &mut S,
    expected: impl IntoIterator<Item = Event<S::Cursor>>,
) where
    S::Cursor: std::fmt::Debug,
{
    for event in expected {
        assert_eq!(subscription.next().await, Ok(event));
    }
}
/// SU-002. Fixture has lost its retained cursor; no invented snapshot.
pub async fn gap<S: Subscription>(subscription: &mut S)
where
    S::Cursor: std::fmt::Debug,
{
    assert_eq!(subscription.next().await, Ok(Event::Gap));
    assert!(
        subscription.next().await.is_err(),
        "delivery resumed after a terminal gap"
    );
}
/// SU-003. Fixture has queued events when closed.
pub async fn closed<S: Subscription>(subscription: &mut S)
where
    S::Cursor: std::fmt::Debug,
{
    assert_eq!(subscription.close().await, Ok(()));
    assert_eq!(subscription.next().await, Err(ReadError::Closed));
    assert_eq!(subscription.close().await, Ok(()));
    assert_eq!(subscription.next().await, Err(ReadError::Closed));
}

/// SU-005. notes fixture: fresh snapshot at 1, retained cursor 1, expired cursor 0.
pub async fn opening<F: Subscriber<Binding = String>>(factory: &F)
where
    F::Session: Subscription<Cursor = u64>,
{
    let request = Open {
        binding: "notes".into(),
        start: Start::Fresh,
        max_event_bytes: std::num::NonZeroUsize::new(8).unwrap(),
        max_buffered_events: std::num::NonZeroUsize::new(2).unwrap(),
    };
    let mut fresh = factory.open(request.clone()).await.expect("open");
    assert_eq!(
        fresh.next().await,
        Ok(Event::Snapshot {
            cursor: 1,
            bytes: vec![10]
        })
    );
    fresh.close().await.unwrap();
    let mut resume = request.clone();
    resume.start = Start::Resume(1);
    let mut resumed = factory.open(resume).await.expect("resume");
    assert_eq!(resumed.next().await, Ok(Event::Resumed { cursor: 1 }));
    resumed.close().await.unwrap();
    let mut expired = request.clone();
    expired.start = Start::Resume(0);
    assert!(matches!(factory.open(expired).await, Err(OpenError::Gap)));
    let mut denied = request;
    denied.binding = "other".into();
    assert!(matches!(factory.open(denied).await, Err(OpenError::Denied)));
}
