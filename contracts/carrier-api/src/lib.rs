//! The carrier the node assembly injects: one bound endpoint that dials and
//! accepts duplex frame links, each given up by value on close. Not a session,
//! codec, HELLO, authentication or retry engine.
//!
//! Adopted from the async witness (`glade/dev-docs/async-witness/ports/src/lib.rs`
//! and the `WitnessEndpoint` of `real/src/peer_carrier.rs`): one frame at a time,
//! `Ok(None)` as end of stream, a transport failure as text, boxed futures so the
//! ports stay `dyn`, and close by value. Two changes: no `Any` supertrait (an
//! injector's facade asks `'static` of the implementation,
//! `dev-docs/arch1/AsyncWitnessResult.md` §5, caveat 4), and frames are opaque
//! bytes, since the witness's `(FrameType, body)` split is the session's codec.

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;

/// A boxed `Send` future: `impl Future` in a trait is not dyn-compatible (E0038).
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A transport address in the adapter's syntax (e.g. `endpoint-id@ip:port`),
/// opaque to this contract.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CarrierAddr(pub String);

/// The identity a transport authenticated for the far end of a link, in the
/// adapter's own bytes (for iroh, the 32-byte endpoint id its TLS session
/// proved), opaque to this contract. It names a transport key, never a node:
/// which node speaks through it is the session's HELLO to establish.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransportId(pub Vec<u8>);

/// What `bind` is given. The limit holds for every link of the endpoint, both ways.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarrierConfig {
    pub local: CarrierAddr,
    pub max_frame_bytes: NonZeroUsize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CarrierError {
    /// Not bound, closed, or ended by its endpoint's close: nothing more is carried.
    Closed,
    /// `bind` on a port that was already bound.
    AlreadyBound,
    /// A frame over `max_frame_bytes`. From `send`, nothing was sent and the link
    /// stays usable; from `recv`, the frame was refused unbuffered and the link is
    /// closed, since the stream cannot be resynchronised.
    FrameTooLarge,
    /// The transport failed, rendered as text so no I/O type crosses the port.
    Transport(String),
}

/// One carrier endpoint, injected as `Arc<dyn CarrierPort>` and shared.
///
/// **Ownership.** The port owns at most one endpoint, from `bind` to `close`,
/// and keeps its own interior mutability. `close` takes `&self` because the
/// lifecycle owner releases through an `Arc` (an sdax release body receives
/// `Arc<T>`, never `T`), so *by value* is the provider's discipline: `close`
/// takes every transport handle it owns out of the port and closes it by value.
/// Once it resolves, no remaining `Arc` of the port and no link keeps the address
/// bound, and the endpoint's links are ended as their own `close` would end them
/// (the lifecycle still releases links first). `close` is idempotent and
/// terminal: afterwards `bind` and `dial` answer `Closed` and `accept` answers
/// `Ok(None)`, as `dial` and `accept` do before `bind`.
///
/// **Cancellation.** Futures are lazy: an unpolled one has no effect. Dropping a
/// pending `dial` or `accept` abandons that attempt only. `close` ends a pending
/// `accept` with `Ok(None)` and MUST NOT wait for it. Dropping a polled `close`
/// abandons the graceful drain, not the handle, which is already out of the port.
///
/// **Size.** `max_frame_bytes` bounds every frame of every link of the endpoint
/// in both directions. Buffers are bounded: a sender that outpaces its peer waits
/// in `send`, and no queue grows without bound.
///
/// ```compile_fail
/// use glade_carrier_api::CarrierPort;
/// struct Missing;
/// impl CarrierPort for Missing {}
/// ```
///
/// Both traits bridge onto an injector's facade with `'static` asked of the
/// implementation. `Interface` is `shaku::Interface` as Shaku defines it, and
/// `is_interface` is the bound a Shaku component's interface must meet:
///
/// ```
/// use glade_carrier_api::{CarrierLink, CarrierPort, PortFuture};
/// use std::any::Any;
/// trait Interface: Any + Send + Sync {}
/// impl<T: Any + Send + Sync> Interface for T {}
/// trait Carrier: CarrierPort + Interface {}
/// impl<T: CarrierPort + 'static> Carrier for T {}
/// trait Link: CarrierLink + Interface {}
/// impl<T: CarrierLink + 'static> Link for T {}
/// fn is_interface<I: Interface + ?Sized>() {}
/// is_interface::<dyn Carrier>();
/// is_interface::<dyn Link>();
/// fn build<T: CarrierPort + 'static>(provider: T) -> Box<dyn Carrier> { Box::new(provider) }
/// fn release(carrier: &dyn Carrier) -> PortFuture<'_, ()> { carrier.close() }
/// ```
///
/// The witness's form does not compile, because the ports name no `Any`:
///
/// ```compile_fail,E0310
/// # use glade_carrier_api::CarrierPort;
/// # trait Interface: std::any::Any + Send + Sync {}
/// # impl<T: std::any::Any + Send + Sync> Interface for T {}
/// trait Carrier: CarrierPort + Interface {}
/// impl<T: CarrierPort> Carrier for T {}
/// ```
pub trait CarrierPort: Send + Sync {
    /// Bind the endpoint and return its dialable address. Once per port: a
    /// second call answers `AlreadyBound`.
    fn bind(&self, config: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>>;

    /// Open one link to `peer`. An adapter MAY carry many links over one
    /// transport connection; this port promises nothing about reuse.
    fn dial<'a>(
        &'a self,
        peer: &'a CarrierAddr,
    ) -> PortFuture<'a, Result<Box<dyn CarrierLink>, CarrierError>>;

    /// The next inbound link, or `Ok(None)` once the endpoint is closed. An `Err`
    /// refuses one inbound attempt; the endpoint stays usable.
    fn accept(&self) -> PortFuture<'_, Result<Option<Box<dyn CarrierLink>>, CarrierError>>;

    /// Give the endpoint up by value, awaiting the transport's drain.
    fn close(&self) -> PortFuture<'_, ()>;
}

/// One duplex link of frames, owned by whoever dialed or accepted it.
///
/// A carrier authenticates no node: who is at the other end is for the session's
/// HELLO to establish through a signer, never the carrier's word. It reports
/// only the transport identity its transport authenticated (`remote_id`),
/// which the HELLO checks against the node's binding record. Frames arrive
/// whole, once and in send order; none is split, merged or interleaved, and
/// `send` and `recv` may run concurrently. `send` resolves when the transport has
/// taken the frame, not when the peer has read it: it is not an acknowledgement.
/// Dropping a pending `recv` MUST NOT consume a frame. Dropping a polled `send`
/// leaves that frame's delivery unknown, and the link then completes it whole or
/// ends: a torn frame is never followed by another.
///
/// `close` gives the link's handles up by value, as `CarrierPort::close` does the
/// endpoint's, and is idempotent. Frames a completed `send` handed over are
/// delivered before the peer sees end of stream, unless the transport fails.
/// Afterwards `send` answers `Closed` and `recv` answers `Ok(None)`.
///
/// ```compile_fail,E0310
/// # use glade_carrier_api::CarrierLink;
/// # trait Interface: std::any::Any + Send + Sync {}
/// # impl<T: std::any::Any + Send + Sync> Interface for T {}
/// trait Link: CarrierLink + Interface {}
/// impl<T: CarrierLink> Link for T {}
/// ```
pub trait CarrierLink: Send + Sync {
    /// Send one frame of at most `max_frame_bytes` opaque bytes; an empty frame
    /// is a frame.
    fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>>;

    /// The next frame. `Ok(None)` is end of stream (the peer closed, or this link
    /// did), never "nothing yet".
    fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>>;

    /// Give the link up by value.
    fn close(&self) -> PortFuture<'_, ()>;

    /// The far end's transport identity, as the transport authenticated it,
    /// or `None` from a transport that has none. It is the same for the
    /// link's whole life, close included.
    fn remote_id(&self) -> Option<TransportId>;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Probes over three unbound ports that can reach one another. They await
    //! only the ports, so any executor that polls to completion drives them. A
    //! real adapter adds its own I/O, partial-frame, backpressure and fault tests.
    use crate::{
        CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture, TransportId,
    };
    use std::future::{Future, poll_fn};
    use std::num::NonZeroUsize;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    /// What a fixture supplies: three unbound ports on one network, and where
    /// `a` and `b` bind. `fresh` binds `b`'s address once `b` has closed.
    pub struct Fixture {
        pub a: Arc<dyn CarrierPort>,
        pub b: Arc<dyn CarrierPort>,
        pub fresh: Arc<dyn CarrierPort>,
        pub at_a: CarrierAddr,
        pub at_b: CarrierAddr,
    }

    type Link = Box<dyn CarrierLink>;

    fn config(local: &CarrierAddr, max: usize) -> CarrierConfig {
        let max_frame_bytes = NonZeroUsize::new(max).expect("a positive limit");
        let local = local.clone();
        CarrierConfig {
            local,
            max_frame_bytes,
        }
    }

    /// One poll, to observe "not yet" without awaiting it.
    fn poll_once<T>(future: &mut PortFuture<'_, T>) -> Poll<T> {
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }

    /// Drive two futures together, as two tasks would: a dial MAY complete only
    /// once the other side accepts.
    async fn join<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
        let (mut a, mut b) = (pin!(a), pin!(b));
        let (mut done_a, mut done_b) = (None, None);
        poll_fn(|cx| {
            if done_a.is_none() {
                if let Poll::Ready(out) = a.as_mut().poll(cx) {
                    done_a = Some(out);
                }
            }
            if done_b.is_none() {
                if let Poll::Ready(out) = b.as_mut().poll(cx) {
                    done_b = Some(out);
                }
            }
            match (done_a.take(), done_b.take()) {
                (Some(out_a), Some(out_b)) => Poll::Ready((out_a, out_b)),
                (out_a, out_b) => {
                    (done_a, done_b) = (out_a, out_b);
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// Bind `a` and `b` with these limits and link them.
    async fn link(f: &Fixture, max_a: usize, max_b: usize) -> (CarrierAddr, Link, Link) {
        let at_b = f.b.bind(config(&f.at_b, max_b)).await.expect("bind b");
        f.a.bind(config(&f.at_a, max_a)).await.expect("bind a");
        let (dialed, accepted) = join(f.a.dial(&at_b), f.b.accept()).await;
        let accepted = accepted.expect("accept").expect("an inbound link");
        (at_b, dialed.expect("dial"), accepted)
    }

    /// CA-001. Frames cross whole, once and in order, both ways; close delivers
    /// what was sent, then ends the stream on both sides.
    pub async fn frames(f: Fixture) {
        let (_, dialed, accepted) = link(&f, 64, 64).await;
        let frames = [&b"one"[..], b"", b"three"];
        for frame in frames {
            dialed.send(frame).await.expect("CA-001 send");
        }
        accepted.send(b"back").await.expect("CA-001 send back");
        for frame in frames {
            let got = accepted.recv().await;
            assert_eq!(
                got,
                Ok(Some(frame.to_vec())),
                "CA-001 whole, once, in order"
            );
        }
        assert_eq!(
            dialed.recv().await,
            Ok(Some(b"back".to_vec())),
            "CA-001 duplex"
        );
        dialed.send(b"last").await.expect("CA-001 send");
        dialed.close().await;
        let got = accepted.recv().await;
        assert_eq!(
            got,
            Ok(Some(b"last".to_vec())),
            "CA-001 close delivers what was sent"
        );
        assert_eq!(
            accepted.recv().await,
            Ok(None),
            "CA-001 then the stream ends"
        );
        let late = dialed.send(b"late").await;
        assert_eq!(
            late,
            Err(CarrierError::Closed),
            "CA-001 a closed link refuses"
        );
        assert_eq!(
            dialed.recv().await,
            Ok(None),
            "CA-001 a closed link has ended"
        );
        dialed.close().await;
    }

    /// CA-002. The limit binds both ways: an oversized send sends nothing and
    /// keeps the link; an oversized arrival is refused and ends the link.
    pub async fn frame_limit(f: Fixture) {
        let (_, dialed, accepted) = link(&f, 4, 8).await;
        let refused = dialed.send(b"12345").await;
        assert_eq!(
            refused,
            Err(CarrierError::FrameTooLarge),
            "CA-002 an oversized send"
        );
        dialed
            .send(b"1234")
            .await
            .expect("CA-002 a frame at the limit");
        let got = accepted.recv().await;
        assert_eq!(
            got,
            Ok(Some(b"1234".to_vec())),
            "CA-002 the refused frame was not sent"
        );
        accepted
            .send(b"123456")
            .await
            .expect("CA-002 within the sender's limit");
        let got = dialed.recv().await;
        assert_eq!(
            got,
            Err(CarrierError::FrameTooLarge),
            "CA-002 an oversized arrival"
        );
        assert_eq!(
            dialed.recv().await,
            Ok(None),
            "CA-002 the refusal ends the link"
        );
        let late = dialed.send(b"1").await;
        assert_eq!(
            late,
            Err(CarrierError::Closed),
            "CA-002 the refusal ends the link"
        );
    }

    /// CA-003. Futures are lazy, and a dropped pending `recv` consumes no frame.
    pub async fn cancellation(f: Fixture) {
        let at_b = f.b.bind(config(&f.at_b, 64)).await.expect("bind b");
        f.a.bind(config(&f.at_a, 64)).await.expect("bind a");
        drop(f.a.dial(&at_b));
        let mut accepting = f.b.accept();
        let nobody = poll_once(&mut accepting).is_pending();
        assert!(nobody, "CA-003 an unpolled dial reaches nobody");
        let (dialed, accepted) = join(f.a.dial(&at_b), accepting).await;
        let (dialed, accepted) = (dialed.expect("dial"), accepted.expect("accept"));
        let accepted = accepted.expect("an inbound link");
        drop(dialed.send(b"never"));
        let mut receiving = accepted.recv();
        let nothing = poll_once(&mut receiving).is_pending();
        assert!(nothing, "CA-003 an unpolled send sends nothing");
        drop(receiving);
        dialed.send(b"kept").await.expect("CA-003 send");
        let got = accepted.recv().await;
        assert_eq!(
            got,
            Ok(Some(b"kept".to_vec())),
            "CA-003 a dropped recv consumes nothing"
        );
    }

    /// CA-004. Close gives the endpoint up by value while a clone of the port and
    /// a link survive, ends its links and a pending `accept`, and is terminal.
    pub async fn close_by_value(f: Fixture) {
        let (at_b, _dialed, accepted) = link(&f, 64, 64).await;
        let again = f.b.bind(config(&f.at_b, 64)).await;
        assert_eq!(
            again,
            Err(CarrierError::AlreadyBound),
            "CA-004 one endpoint per port"
        );
        let survivor = Arc::clone(&f.b);
        let mut accepting = f.b.accept();
        let waiting = poll_once(&mut accepting).is_pending();
        assert!(waiting, "CA-004 accept waits while nobody dials");
        f.b.close().await;
        let ended = matches!(accepting.await, Ok(None));
        assert!(ended, "CA-004 close ends a pending accept");
        let late = accepted.send(b"late").await;
        assert_eq!(
            late,
            Err(CarrierError::Closed),
            "CA-004 close ends the endpoint's links"
        );
        let rebound = f.fresh.bind(config(&at_b, 64)).await;
        assert!(
            rebound.is_ok(),
            "CA-004 close frees the address by value: {rebound:?}"
        );
        survivor.close().await;
        let again = survivor.bind(config(&f.at_b, 64)).await;
        assert_eq!(again, Err(CarrierError::Closed), "CA-004 close is terminal");
        let dialed = matches!(survivor.dial(&at_b).await, Err(CarrierError::Closed));
        assert!(dialed, "CA-004 close is terminal");
        let accepted = matches!(survivor.accept().await, Ok(None));
        assert!(accepted, "CA-004 close is terminal");
    }

    /// CA-005. Each link names the far end's transport identity. `a` dials
    /// `b`, then, once `b` has closed, `fresh` bound where `b` was: the two
    /// endpoints `a` reached are named apart, both name `a` alike, a
    /// transport with no identity names none on any link, and a name
    /// outlives the link's close. It assumes the fixture's three ports are
    /// three endpoints: `fresh` is not `b` again under the same identity.
    pub async fn remote_identity(f: Fixture) {
        let (at_b, a_to_b, b_from_a) = link(&f, 64, 64).await;
        f.b.close().await;
        f.fresh
            .bind(config(&at_b, 64))
            .await
            .expect("bind fresh where b was");
        let (a_to_fresh, fresh_from_a) = join(f.a.dial(&at_b), f.fresh.accept()).await;
        let a_to_fresh = a_to_fresh.expect("dial");
        let fresh_from_a = fresh_from_a.expect("accept").expect("an inbound link");
        let (b, fresh): (Option<TransportId>, _) = (a_to_b.remote_id(), a_to_fresh.remote_id());
        let a = b_from_a.remote_id();
        let alike = "CA-005 two endpoints name the endpoint that reached both alike";
        assert_eq!(fresh_from_a.remote_id(), a, "{alike}");
        match (&a, &b, &fresh) {
            (Some(a), Some(b), Some(fresh)) => {
                let apart = a != b && b != fresh && a != fresh;
                assert!(apart, "CA-005 three endpoints are named apart");
            }
            (None, None, None) => {}
            _ => panic!("CA-005 a transport names every far end, or none"),
        }
        for link in [&a_to_b, &b_from_a, &a_to_fresh, &fresh_from_a] {
            link.close().await;
        }
        assert_eq!(a_to_b.remote_id(), b, "CA-005 a name outlives the close");
    }
}
