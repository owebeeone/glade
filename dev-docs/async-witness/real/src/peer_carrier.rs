//! Step 3.1 — a `CarrierPort` over the node's **real** iroh peer carrier.
//!
//! The port is `async_witness_ports::CarrierPort`, which names no framework at
//! all; the provider is `glade_node::iroh_carrier::PeerEndpoint`, unmodified.
//! That pairing is what DI-E04 asks for: "the selected real async Glade port
//! remains usable without framework imports in its contract/pure libraries".
//!
//! # Everything here is about giving a handle back by value
//!
//! `PeerEndpoint::close(self)` **consumes** the handle, because iroh frees the
//! UDP socket only once every clone is gone (`iroh_carrier.rs:126-140`,
//! `iroh-1.2.0/src/endpoint.rs:1717-1718`). An sdax release body receives an
//! `Arc<T>`, never a `T` (`sdax/src/terminals.rs:73-77`), and an async release
//! leaves that `Arc` in the engine's slots rather than taking it out
//! (`sdax/src/host/bodies.rs:404-421` takes the value only for a `by_drop`
//! release). Closing a *clone* would therefore leave one live clone in the
//! slots for as long as the run's storage lives, and the port would stay bound.
//!
//! So both handles here are owned as `Mutex<Option<..>>` and the release body
//! **takes** them out: after it returns, the `Arc` the engine still holds is an
//! empty shell. Nothing about when a slot is dropped can then keep a socket
//! bound. The `Mutex` is interior mutability inside the *provider*, which is
//! where the plan puts it — `ports/src/lib.rs` already says "an implementation
//! owns its own interior mutability", and no Glade contract is touched.
//!
//! The same argument applies to the link, and it rests on a **measurement**,
//! not on a derivation. iroh 1.2.0 does not run on quinn: `cargo tree -p iroh
//! --depth 1` lists `noq`, `noq-proto` and `noq-udp`, and `cargo tree --invert
//! quinn` matches no package in this workspace at all. noq's own endpoint
//! driver stops when the connection map is empty **and** either the handle
//! count is zero **or** `close` has been called
//! (`noq-1.3.0/src/endpoint.rs:471-476`), so after a close a remaining handle
//! does not keep the *driver* alive — which means the driver's exit condition
//! cannot settle the question in either direction. What a surviving handle does
//! to the *socket* is therefore measured: with one clone of the served link's
//! `Connection` escaping the composition, the acceptor's UDP port stays bound
//! for the whole two-second bound and comes back tens of microseconds after
//! that `Connection` is dropped
//! (`tests/peer_release.rs`,
//! `an_escaped_connection_holds_the_port_as_an_endpoint_clone_does`). A
//! `Connection` left alive in a slot does hold the endpoint's socket open, just
//! as an endpoint clone does, so taking the link's handles by value is
//! load-bearing rather than tidy.

use std::io;
use std::sync::Arc;

use async_witness_ports::{CarriedFrame, CarrierError, CarrierPort, FrameType, PortFuture};
use glade_node::iroh_carrier::{PeerAddr, PeerEndpoint, PeerLink};
use glade_node::peer::{NodeIdentity, PeerHello};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

/// Render a transport failure as the port's own error rather than putting an
/// `std::io` type in a public port signature.
fn transport(e: io::Error) -> CarrierError {
    CarrierError::Transport(e.to_string())
}

/// Write one length-prefixed record, byte for byte as
/// `glade_node::peer::write_frame` writes one (`peer.rs:36-42`).
///
/// Generic on purpose. `iroh::endpoint::SendStream` **is** `noq::SendStream`,
/// re-exported (`iroh-1.2.0/src/endpoint/quic.rs:15-44`), and it carries noq's
/// *inherent* `write_all` (`noq-1.3.0/src/send_stream.rs:74`), which shadows
/// the `AsyncWriteExt` method and answers a `WriteError`; a generic bound
/// selects tokio's trait method, so the bytes and the error type are the node's
/// own rather than a second convention. `RecvStream` shadows `read_exact` the
/// same way (`noq-1.3.0/src/recv_stream.rs:89`, answering a `ReadExactError`),
/// which is why [`read_framed`] is generic too.
async fn write_framed<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame longer than u32"))?;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(bytes).await?;
    w.flush().await?;
    Ok(())
}

/// Read one length-prefixed record, or `None` at a clean end of stream —
/// `read_frame` calls that `UnexpectedEof` and reads it as "peer done"
/// (`peer.rs:44-53`).
async fn read_framed<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

/// One bound `PeerEndpoint`, owned so that it can be given up by value.
///
/// The address is recorded at bind time and kept as a plain field, so a dialer
/// can read it without taking the endpoint's lock — the acceptor holds that
/// lock for the whole of its `accept`.
pub struct WitnessEndpoint {
    endpoint: Mutex<Option<PeerEndpoint>>,
    addr: PeerAddr,
    port: u16,
}

impl WitnessEndpoint {
    /// Bind a localhost QUIC endpoint with an explicit glade identity.
    ///
    /// This is the call that belongs **inside** `cx.hold(...)`: it is the
    /// external effect, and the engine records the obligation in the same poll
    /// that observes it succeeding.
    pub async fn bind(identity: NodeIdentity) -> io::Result<WitnessEndpoint> {
        let endpoint = PeerEndpoint::bind_with(identity).await?;
        let addr = endpoint.addr()?;
        Ok(WitnessEndpoint {
            endpoint: Mutex::new(Some(endpoint)),
            addr,
            port: addr.socket.port(),
        })
    }

    /// This endpoint's dialable address, as `addr()` reported it at bind time.
    pub fn addr(&self) -> PeerAddr {
        self.addr
    }

    /// The UDP port the bind was given. Recorded before the run ends, because
    /// after the run there is nothing left to ask.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Accept one inbound peer connection, HELLO included. `Ok(None)` once the
    /// endpoint has been given up.
    pub async fn accept(&self) -> io::Result<Option<PeerLink>> {
        let guard = self.endpoint.lock().await;
        let Some(endpoint) = guard.as_ref() else {
            return Ok(None);
        };
        endpoint.accept().await
    }

    /// Dial a peer, HELLO included.
    pub async fn dial(&self, addr: &PeerAddr) -> io::Result<PeerLink> {
        let guard = self.endpoint.lock().await;
        let Some(endpoint) = guard.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the endpoint has already been closed",
            ));
        };
        endpoint.dial(addr).await
    }

    /// Take the endpoint out of this handle and close it **by value**, awaiting
    /// iroh's drain. Idempotent: a second call has nothing to take.
    ///
    /// This is the one place `PeerEndpoint::close(self)` can be satisfied from
    /// an `Arc<WitnessEndpoint>`, and it is why nothing else in the composition
    /// ever needs a clone.
    pub async fn close(&self) {
        let taken = self.endpoint.lock().await.take();
        if let Some(endpoint) = taken {
            endpoint.close().await;
        }
    }

    /// A clone of the live endpoint, for the **negative** fixture of Step 3.2
    /// only: a proof that cannot fail is not evidence, so the witness needs a
    /// way to make a clone escape on purpose. Nothing in the ordinary
    /// composition calls this.
    pub async fn escaping_clone(&self) -> Option<PeerEndpoint> {
        self.endpoint.lock().await.clone()
    }
}

/// One established `PeerLink` as a `CarrierPort`.
///
/// Three separate locks rather than one, so a send and a receive on the same
/// link do not serialise against each other; the connection has its own because
/// release takes all three and a caller holds none of them across a release.
pub struct WitnessCarrier {
    peer: PeerHello,
    send: Mutex<Option<SendStream>>,
    recv: Mutex<Option<RecvStream>>,
    conn: Mutex<Option<Connection>>,
}

impl WitnessCarrier {
    /// Take ownership of an established, HELLO'd link.
    pub fn over(link: PeerLink) -> WitnessCarrier {
        WitnessCarrier {
            peer: link.peer,
            send: Mutex::new(Some(link.send)),
            recv: Mutex::new(Some(link.recv)),
            conn: Mutex::new(Some(link.conn)),
        }
    }

    /// The peer identity the HELLO seam vouched for.
    pub fn peer(&self) -> PeerHello {
        self.peer
    }

    /// A clone of the live `Connection`, for the **measurement** fixture of the
    /// module docs only: whether a `Connection` value outliving the run holds
    /// the endpoint's UDP port is a fact to measure, not to derive. Nothing in
    /// the ordinary composition calls this.
    pub async fn escaping_connection(&self) -> Option<Connection> {
        self.conn.lock().await.clone()
    }

    /// Finish the stream, close the connection and **drop every handle**, so
    /// that nothing left in a slot can hold the endpoint's socket open.
    pub async fn close(&self) {
        let sender = self.send.lock().await.take();
        if let Some(mut sender) = sender {
            // `ClosedStream` here only means the peer already went away.
            let _ = sender.finish();
        }
        drop(self.recv.lock().await.take());
        let conn = self.conn.lock().await.take();
        if let Some(conn) = conn {
            conn.close(0u32.into(), b"witness released");
        }
    }
}

impl CarrierPort for WitnessCarrier {
    /// One frame, framed exactly as `glade_node::peer::write_frame` frames it:
    /// a `u32` little-endian length, then `[tag byte][CBOR body]`.
    fn send<'a>(
        &'a self,
        frame: FrameType,
        body: &'a [u8],
    ) -> PortFuture<'a, Result<(), CarrierError>> {
        Box::pin(async move {
            let mut guard = self.send.lock().await;
            let Some(stream) = guard.as_mut() else {
                return Err(CarrierError::Closed);
            };
            let mut bytes = Vec::with_capacity(1 + body.len());
            bytes.push(frame.wire() as u8);
            bytes.extend_from_slice(body);
            write_framed(stream, &bytes).await.map_err(transport)
        })
    }

    /// `Ok(None)` is end of stream, as the port declares: a clean close at a
    /// frame boundary reads as `UnexpectedEof`, which is what
    /// `glade_node::peer::read_frame` calls "peer done" (`peer.rs:44-45`).
    fn recv(&self) -> PortFuture<'_, Result<Option<CarriedFrame>, CarrierError>> {
        Box::pin(async move {
            let mut guard = self.recv.lock().await;
            let Some(stream) = guard.as_mut() else {
                return Ok(None);
            };
            let Some(buf) = read_framed(stream).await.map_err(transport)? else {
                return Ok(None);
            };
            let Some((&tag, body)) = buf.split_first() else {
                return Err(CarrierError::Transport("empty frame".to_owned()));
            };
            Ok(Some((FrameType::from_wire(i64::from(tag)), body.to_vec())))
        })
    }
}

/// A handle an injector can own: every port call goes to the carrier the
/// **engine** acquired, so assembling over an acquired handle never constructs
/// a second one.
///
/// Step 3.3 hands one of these to Shaku. It is in this module rather than in
/// the bridge because it names no framework: it is a port over a port.
pub struct AcquiredCarrier(Arc<dyn CarrierPort>);

impl AcquiredCarrier {
    pub fn over(carrier: Arc<dyn CarrierPort>) -> AcquiredCarrier {
        AcquiredCarrier(carrier)
    }
}

impl CarrierPort for AcquiredCarrier {
    fn send<'a>(
        &'a self,
        frame: FrameType,
        body: &'a [u8],
    ) -> PortFuture<'a, Result<(), CarrierError>> {
        self.0.send(frame, body)
    }

    fn recv(&self) -> PortFuture<'_, Result<Option<CarriedFrame>, CarrierError>> {
        self.0.recv()
    }
}
