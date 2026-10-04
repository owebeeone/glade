use glade_crdt_admission_data::*;
use glade_crdt_recovery_api::*;
fn guard() -> ReceiveGuard {
    ReceiveGuard {
        id: GuardId {
            store: StoreIncarnation([1; 32]),
            instance: Instance {
                descriptor: [2; 32],
                incarnation: "i".into(),
            },
            number: 1,
        },
        owner: Owner {
            store: StoreIncarnation([1; 32]),
            generation: 1,
        },
        lease: 1,
        source: [3; 32],
        channel: [4; 32],
        role: "inventory".into(),
        base_generation: 0,
        base_digest: [0; 32],
        max_bytes: 1024,
        max_items: 8,
        deadline_ms: 100,
    }
}
use std::{
    future::{Future, ready},
    pin::Pin,
    task::{Context, Poll, Waker},
};
#[test]
fn owned_receive_drains_before_settlement_and_close() {
    let mut owner = IngressAuthority::new(8);
    let permit = owner.issue(guard()).unwrap();
    let mut receive = owner
        .receive(permit, || ready(Ok(Bytes::new(vec![7]))))
        .expect("owned receive");
    assert!(!owner.can_close());
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let Poll::Ready((permit, value)) = Pin::new(&mut receive).poll(&mut cx) else {
        panic!("ready receive");
    };
    assert_eq!(value, Ok(Bytes::new(vec![7])));
    assert!(permit.was_started());
    assert_eq!(owner.abandon(&permit), Err(Fault::Ownership));
    assert_eq!(owner.settle(&permit), Ok(()));
    assert!(owner.can_close());
}
#[test]
fn live_never_started_owned_guard_can_abandon_only_once() {
    let mut owner = IngressAuthority::new(8);
    let permit = owner.issue(guard()).unwrap();
    assert_eq!(owner.abandon(&permit), Ok(()));
    assert_eq!(owner.abandon(&permit), Err(Fault::Ownership));
    assert!(owner.can_close());
}
#[test]
fn another_authority_refuses_before_constructing_or_consuming_future() {
    let mut owner = IngressAuthority::new(8);
    let other = IngressAuthority::new(8);
    let permit = owner.issue(guard()).unwrap();
    let result = other.receive(permit, || -> std::future::Ready<Result<Bytes, Fault>> {
        panic!("foreign callback must not execute");
    });
    assert!(matches!(result, Err(Fault::Ownership)));
    assert!(!owner.can_close());
}
#[test]
fn cancelled_started_async_receive_does_not_imply_drain_or_abandon() {
    let mut owner = IngressAuthority::new(8);
    let permit = owner.issue(guard()).unwrap();
    let mut receive = owner
        .receive(permit, std::future::pending::<Result<Bytes, Fault>>)
        .unwrap();
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    assert!(Pin::new(&mut receive).poll(&mut cx).is_pending());
    drop(receive);
    assert!(!owner.can_close());
}
#[test]
fn abandoned_capability_cannot_start_work_again() {
    let mut owner = IngressAuthority::new(8);
    let permit = owner.issue(guard()).unwrap();
    owner.abandon(&permit).unwrap();
    assert!(matches!(
        owner.receive(permit, || ready(Ok(Bytes::new(vec![])))),
        Err(Fault::Ownership)
    ));
}
#[test]
fn issuer_capacity_is_finite_and_retirement_does_not_reissue_history() {
    let mut owner = IngressAuthority::new(1);
    let permit = owner.issue(guard()).unwrap();
    owner.abandon(&permit).unwrap();
    let mut second = guard();
    second.id.number = 2;
    assert!(matches!(owner.issue(second), Err(Fault::Capacity)));
}
#[test]
fn issued_guard_identity_is_not_reissued_by_same_session() {
    let mut owner = IngressAuthority::new(8);
    let _first = owner.issue(guard()).unwrap();
    assert!(matches!(owner.issue(guard()), Err(Fault::Ownership)));
}

#[test]
fn eligibility_queries_preserve_owned_lifecycle_and_refuse_foreign_closed_or_started() {
    let mut owner = IngressAuthority::new(8);
    let other = IngressAuthority::new(8);
    let permit = owner.issue(guard()).unwrap();
    assert_eq!(owner.can_abandon(&permit), Ok(()));
    assert_eq!(owner.can_abandon(&permit), Ok(()));
    assert_eq!(owner.can_settle(&permit), Err(Fault::Ownership));
    assert_eq!(other.can_abandon(&permit), Err(Fault::Ownership));
    assert!(!owner.can_close());
    let mut receive = owner
        .receive(permit, || ready(Ok(Bytes::new(vec![7]))))
        .unwrap();
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let Poll::Ready((permit, _)) = Pin::new(&mut receive).poll(&mut cx) else {
        panic!("drained receive");
    };
    assert_eq!(owner.can_abandon(&permit), Err(Fault::Ownership));
    assert_eq!(owner.can_settle(&permit), Ok(()));
    assert_eq!(owner.can_settle(&permit), Ok(()));
    assert!(!owner.can_close());
    assert_eq!(owner.settle(&permit), Ok(()));
    assert_eq!(owner.can_settle(&permit), Err(Fault::Ownership));
    assert_eq!(owner.can_abandon(&permit), Err(Fault::Ownership));
    assert!(owner.can_close());
}

#[test]
fn normal_settlement_matches_only_owned_success_and_refuses_over_bound_before_comparison() {
    let mut owner = IngressAuthority::new(8);
    let original = Bytes::new(vec![7]);
    let permit = owner.issue(guard()).unwrap();
    let mut receive = owner
        .receive(permit, || ready(Ok(original.clone())))
        .unwrap();
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready((permit, _)) = Pin::new(&mut receive).poll(&mut cx) else {
        panic!("ready")
    };
    assert_eq!(
        owner.can_settle_received(&permit, &Bytes::new(vec![0; 1025])),
        Err(Fault::Capacity)
    );
    assert_eq!(
        owner.can_settle_received(&permit, &Bytes::new(vec![7, 8])),
        Err(Fault::BindingMismatch)
    );
    assert_eq!(
        owner.can_settle_received(&permit, &Bytes::new(vec![8])),
        Err(Fault::BindingMismatch)
    );
    assert_eq!(owner.can_settle_received(&permit, &original), Ok(()));
    assert_eq!(permit.received_digest(), Some(original.digest()));
    assert!(!owner.can_close());
    owner.settle(&permit).unwrap();
    let mut g = guard();
    g.id.number = 2;
    let permit = owner.issue(g).unwrap();
    let mut receive = owner
        .receive(permit, || ready(Err(Fault::Unavailable)))
        .unwrap();
    let Poll::Ready((permit, _)) = Pin::new(&mut receive).poll(&mut cx) else {
        panic!("ready error")
    };
    assert_eq!(permit.received_digest(), None);
    assert_eq!(
        owner.can_settle_received(&permit, &original),
        Err(Fault::BindingMismatch)
    );
    assert_eq!(owner.can_settle(&permit), Ok(()));
}
