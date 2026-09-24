//! A fake network for the carrier probes: one mutex, no socket, no runtime and
//! no sleep. It proves the contract's shape, never a transport.
use glade_carrier_api::conformance::{self, Fixture};
use glade_carrier_api::{
    CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, PortFuture, TransportId,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::{Future, poll_fn, ready};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Deliberately wrong behaviours, each caught by one probe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrong {
    Reorders,
    IgnoresLimit,
    EagerSend,
    /// Close leaves the address held, as closing a clone of the handle would.
    KeepsAddress,
    /// A link names its own end, not the far one.
    NamesItself,
}

#[derive(Default)]
struct Pipe {
    frames: VecDeque<Vec<u8>>,
    writer_closed: bool,
    reader_closed: bool,
}

#[derive(Default)]
struct State {
    /// Address to (endpoint, frame limit).
    bound: BTreeMap<CarrierAddr, (usize, usize)>,
    backlog: BTreeMap<usize, VecDeque<Box<dyn CarrierLink>>>,
    closed: BTreeSet<usize>,
    pipes: Vec<Pipe>,
    endpoints: usize,
    wakers: Vec<Waker>,
}

#[derive(Default)]
struct Net(Mutex<State>);

impl Net {
    /// Change the network, then wake every waiting future.
    fn with<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.0.lock().expect("fake network lock");
        let out = change(&mut state);
        for waker in state.wakers.drain(..) {
            waker.wake();
        }
        out
    }

    /// Look at the network; a pending answer waits for the next change.
    fn poll<T>(&self, cx: &Context<'_>, look: impl FnOnce(&mut State) -> Poll<T>) -> Poll<T> {
        let mut state = self.0.lock().expect("fake network lock");
        let out = look(&mut state);
        if out.is_pending() {
            state.wakers.push(cx.waker().clone());
        }
        out
    }
}

struct Link {
    net: Arc<Net>,
    endpoint: usize,
    /// The endpoint at the far end, whose number is its identity here.
    remote: usize,
    max: usize,
    tx: usize,
    rx: usize,
    wrong: Option<Wrong>,
}

impl Link {
    fn push(&self, frame: &[u8]) -> Result<(), CarrierError> {
        self.net.with(|s| {
            if s.pipes[self.tx].writer_closed || s.closed.contains(&self.endpoint) {
                return Err(CarrierError::Closed);
            }
            if frame.len() > self.max && self.wrong != Some(Wrong::IgnoresLimit) {
                return Err(CarrierError::FrameTooLarge);
            }
            s.pipes[self.tx].frames.push_back(frame.to_vec());
            Ok(())
        })
    }

    fn end(&self, s: &mut State) {
        s.pipes[self.tx].writer_closed = true;
        s.pipes[self.rx].reader_closed = true;
    }
}

impl CarrierLink for Link {
    fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
        if self.wrong == Some(Wrong::EagerSend) {
            return Box::pin(ready(self.push(frame)));
        }
        Box::pin(async move { self.push(frame) })
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
        Box::pin(poll_fn(move |cx| {
            self.net.poll(cx, |s| {
                if s.pipes[self.rx].reader_closed || s.closed.contains(&self.endpoint) {
                    return Poll::Ready(Ok(None));
                }
                let frames = &mut s.pipes[self.rx].frames;
                let next = if self.wrong == Some(Wrong::Reorders) {
                    frames.pop_back()
                } else {
                    frames.pop_front()
                };
                match next {
                    Some(frame) if frame.len() > self.max => {
                        self.end(s);
                        Poll::Ready(Err(CarrierError::FrameTooLarge))
                    }
                    Some(frame) => Poll::Ready(Ok(Some(frame))),
                    None if s.pipes[self.rx].writer_closed => Poll::Ready(Ok(None)),
                    None => Poll::Pending,
                }
            })
        }))
    }

    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(async move { self.net.with(|s| self.end(s)) })
    }

    fn remote_id(&self) -> Option<TransportId> {
        let named = if self.wrong == Some(Wrong::NamesItself) {
            self.endpoint
        } else {
            self.remote
        };
        Some(TransportId(named.to_le_bytes().to_vec()))
    }
}

enum Phase {
    Unbound,
    Bound {
        id: usize,
        at: CarrierAddr,
        max: usize,
    },
    Closed,
}

struct Port {
    net: Arc<Net>,
    phase: Mutex<Phase>,
    wrong: Option<Wrong>,
}

impl Port {
    fn bound(&self) -> Option<(usize, usize)> {
        match *self.phase.lock().expect("fake port lock") {
            Phase::Bound { id, max, .. } => Some((id, max)),
            _ => None,
        }
    }

    fn link(&self, ends: (usize, usize), max: usize, tx: usize, rx: usize) -> Box<dyn CarrierLink> {
        let (net, wrong) = (self.net.clone(), self.wrong);
        let (endpoint, remote) = ends;
        Box::new(Link {
            net,
            endpoint,
            remote,
            max,
            tx,
            rx,
            wrong,
        })
    }
}

impl CarrierPort for Port {
    fn bind(&self, config: CarrierConfig) -> PortFuture<'_, Result<CarrierAddr, CarrierError>> {
        Box::pin(async move {
            let mut phase = self.phase.lock().expect("fake port lock");
            match *phase {
                Phase::Unbound => {}
                Phase::Bound { .. } => return Err(CarrierError::AlreadyBound),
                Phase::Closed => return Err(CarrierError::Closed),
            }
            let (at, max) = (config.local, config.max_frame_bytes.get());
            let id = self.net.with(|s| {
                if s.bound.contains_key(&at) {
                    return Err(CarrierError::Transport("address in use".into()));
                }
                s.endpoints += 1;
                s.bound.insert(at.clone(), (s.endpoints, max));
                Ok(s.endpoints)
            })?;
            *phase = Phase::Bound {
                id,
                at: at.clone(),
                max,
            };
            Ok(at)
        })
    }

    fn dial<'a>(
        &'a self,
        peer: &'a CarrierAddr,
    ) -> PortFuture<'a, Result<Box<dyn CarrierLink>, CarrierError>> {
        Box::pin(async move {
            let Some((id, max)) = self.bound() else {
                return Err(CarrierError::Closed);
            };
            self.net.with(|s| {
                let Some(&(target, target_max)) = s.bound.get(peer) else {
                    return Err(CarrierError::Transport("nobody is bound there".into()));
                };
                let (out, back) = (s.pipes.len(), s.pipes.len() + 1);
                s.pipes.extend([Pipe::default(), Pipe::default()]);
                let accepted = self.link((target, id), target_max, back, out);
                s.backlog.entry(target).or_default().push_back(accepted);
                Ok(self.link((id, target), max, out, back))
            })
        })
    }

    fn accept(&self) -> PortFuture<'_, Result<Option<Box<dyn CarrierLink>>, CarrierError>> {
        Box::pin(poll_fn(move |cx| {
            let Some((id, _)) = self.bound() else {
                return Poll::Ready(Ok(None));
            };
            self.net.poll(cx, |s| {
                match s.backlog.get_mut(&id).and_then(VecDeque::pop_front) {
                    Some(link) => Poll::Ready(Ok(Some(link))),
                    None => Poll::Pending,
                }
            })
        }))
    }

    fn close(&self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let taken = std::mem::replace(&mut *self.phase.lock().expect("lock"), Phase::Closed);
            if let Phase::Bound { id, at, .. } = taken {
                self.net.with(|s| {
                    s.closed.insert(id);
                    s.backlog.remove(&id);
                    if self.wrong != Some(Wrong::KeepsAddress) {
                        s.bound.remove(&at);
                    }
                });
            }
        })
    }
}

fn fixture(wrong: Option<Wrong>) -> Fixture {
    let net = Arc::new(Net::default());
    let port = || -> Arc<dyn CarrierPort> {
        let phase = Mutex::new(Phase::Unbound);
        Arc::new(Port {
            net: net.clone(),
            phase,
            wrong,
        })
    };
    Fixture {
        a: port(),
        b: port(),
        fresh: port(),
        at_a: CarrierAddr("a".into()),
        at_b: CarrierAddr("b".into()),
    }
}

/// Poll a probe to completion without a runtime. The fake waits on nothing
/// outside the probe, so a probe still pending after many polls is a defect.
fn run(probe: impl Future<Output = ()>) {
    let mut probe = pin!(probe);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..64 {
        if probe.as_mut().poll(&mut cx).is_ready() {
            return;
        }
    }
    panic!("the probe is still pending: the fake waited on something outside it");
}

#[test]
fn ca_001_frames_cross_whole_once_and_in_order() {
    run(conformance::frames(fixture(None)));
}

#[test]
fn ca_002_the_frame_limit_binds_both_ways() {
    run(conformance::frame_limit(fixture(None)));
}

#[test]
fn ca_003_futures_are_lazy_and_recv_is_cancel_safe() {
    run(conformance::cancellation(fixture(None)));
}

#[test]
fn ca_004_close_gives_the_endpoint_up_by_value() {
    run(conformance::close_by_value(fixture(None)));
}

#[test]
fn ca_005_each_link_names_the_far_ends_transport_identity() {
    run(conformance::remote_identity(fixture(None)));
}

#[test]
#[should_panic(expected = "CA-001 whole, once, in order")]
fn rejects_reordered_frames() {
    run(conformance::frames(fixture(Some(Wrong::Reorders))));
}

#[test]
#[should_panic(expected = "CA-002 an oversized send")]
fn rejects_an_ignored_frame_limit() {
    run(conformance::frame_limit(fixture(Some(Wrong::IgnoresLimit))));
}

#[test]
#[should_panic(expected = "CA-003 an unpolled send sends nothing")]
fn rejects_a_send_that_acts_before_it_is_polled() {
    run(conformance::cancellation(fixture(Some(Wrong::EagerSend))));
}

#[test]
#[should_panic(expected = "CA-004 close frees the address by value")]
fn rejects_a_close_that_keeps_the_address() {
    run(conformance::close_by_value(fixture(Some(
        Wrong::KeepsAddress,
    ))));
}

#[test]
#[should_panic(expected = "CA-005 two endpoints name the endpoint that reached both alike")]
fn rejects_a_link_that_names_its_own_end() {
    run(conformance::remote_identity(fixture(Some(
        Wrong::NamesItself,
    ))));
}
