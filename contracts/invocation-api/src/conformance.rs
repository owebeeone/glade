use crate::*;
/// Adapter-reusable outcome assertion with a real binding type and request payload.
pub async fn assert_outcome<I: Invoker>(invoker: &I, request: Call<I::Binding>, expected: Outcome) {
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
