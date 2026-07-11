//! `GladeClient` — the rust mirror of the TS `client.ts` choreography, for
//! SUPPLIERS. One websocket to a glade node, the frozen frame protocol
//! (`[FrameType tag][CBOR body]`), a background read loop dispatching inbound
//! frames, and the request/response plumbing a supplier needs: `connect`,
//! optional `hello(principal)` (S7), `subscribe` (Heads-acked), `append` /
//! `send_ops`, the requester side `exchange`, and the provider side
//! `on_exchange_req` + `respond_exchange` (corr preserved 1:1). Inbound ops and
//! exchange requests fan out to as many listeners as a session multiplexes
//! (mpsc receivers); `on_drop` fires when the link ends so a supplier reattaches
//! (never on a deliberate `close`). No node internals — the wire + tokio only.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use glade_wire::cbor::Cbor;
use glade_wire::generated::{
    ExchangeReq, ExchangeRes, FrameType, Hello, Ops, Subscribe,
};
use glade_wire::{cbor, generated};

use crate::session::{shape_of, Session};
use crate::ws::{self, Msg, WsWriter};

/// A provider's answer, as the requester sees it — the decoded `ExchangeRes`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExchangeOutcome {
    pub ok: bool,
    pub payload: Option<Vec<u8>>,
    pub error: Option<String>,
}

/// `[FrameType tag][CBOR body]` — the frozen framing (`frame.rs`), inline.
fn frame(ty: FrameType, body: Cbor) -> Vec<u8> {
    let mut out = vec![ty.wire() as u8];
    out.extend_from_slice(&cbor::encode(&body));
    out
}

fn dropped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "connection dropped")
}

struct Inner {
    origin: String,
    /// `host:port` of the last connect — reused by `reconnect` (reattach).
    endpoint: Mutex<Option<(String, u16)>>,
    /// The current connection's writer; `None` while disconnected.
    writer: Mutex<Option<WsWriter>>,
    read_task: Mutex<Option<JoinHandle<()>>>,
    session: Mutex<Session>,
    /// FIFO ack waiters — Heads pops one subscribe, Welcome pops one hello.
    sub_acks: Mutex<VecDeque<oneshot::Sender<()>>>,
    welcome_acks: Mutex<VecDeque<oneshot::Sender<()>>>,
    ex_corr: AtomicU64,
    ex_waiters: Mutex<HashMap<String, oneshot::Sender<ExchangeOutcome>>>,
    ops_senders: Mutex<Vec<mpsc::UnboundedSender<Vec<generated::Op>>>>,
    exreq_senders: Mutex<Vec<mpsc::UnboundedSender<ExchangeReq>>>,
    drop_senders: Mutex<Vec<mpsc::UnboundedSender<()>>>,
    closing: AtomicBool,
}

impl Inner {
    /// Decode + dispatch one inbound frame (the read loop's body).
    async fn dispatch(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let ty = FrameType::from_wire(bytes[0] as i64);
        let body = cbor::decode(&bytes[1..]);
        match ty {
            FrameType::Ops => {
                let ops = Ops::from_cbor(&body).ops;
                // The session folds every inbound op (so this client's own
                // `fold_*` is live); listeners are an additive fan-out for a
                // supplier serving several surfaces over one session.
                self.session.lock().await.apply_remote(&ops);
                self.ops_senders.lock().await.retain(|s| s.send(ops.clone()).is_ok());
            }
            FrameType::Heads => {
                if let Some(tx) = self.sub_acks.lock().await.pop_front() {
                    let _ = tx.send(());
                }
            }
            FrameType::Welcome => {
                if let Some(tx) = self.welcome_acks.lock().await.pop_front() {
                    let _ = tx.send(());
                }
            }
            FrameType::ExchangeReq => {
                // This session is the attached provider (it Subscribed a declared
                // exchange surface); surface the request to every provider loop.
                let req = ExchangeReq::from_cbor(&body);
                self.exreq_senders.lock().await.retain(|s| s.send(req.clone()).is_ok());
            }
            FrameType::ExchangeRes => {
                let res = ExchangeRes::from_cbor(&body);
                if let Some(tx) = self.ex_waiters.lock().await.remove(&res.corr) {
                    let _ = tx.send(ExchangeOutcome { ok: res.ok, payload: res.payload, error: res.error });
                }
            }
            _ => {} // Error / channel frames: ignored (echo/channel are P3)
        }
    }

    /// The connection ended (close/EOF): forget the writer, fail every pending
    /// waiter so awaiting calls return `dropped()` (a supplier re-issues them on
    /// reattach), and signal `on_drop` unless the caller closed us deliberately.
    async fn on_connection_end(&self) {
        *self.writer.lock().await = None;
        self.sub_acks.lock().await.clear();
        self.welcome_acks.lock().await.clear();
        self.ex_waiters.lock().await.clear();
        if !self.closing.load(Ordering::SeqCst) {
            self.drop_senders.lock().await.retain(|s| s.send(()).is_ok());
        }
    }

    async fn send(&self, bytes: Vec<u8>) -> io::Result<()> {
        let w = self.writer.lock().await.clone();
        match w {
            Some(w) => w.send_binary(&bytes).await,
            None => Err(io::Error::new(io::ErrorKind::NotConnected, "not connected")),
        }
    }
}

async fn read_loop(inner: Arc<Inner>, mut reader: ws::WsReader) {
    loop {
        match reader.read().await {
            Ok(Msg::Binary(bytes)) => inner.dispatch(&bytes).await,
            _ => break, // close or error
        }
    }
    inner.on_connection_end().await;
}

/// A cheaply-cloneable handle to one glade session over the wire.
#[derive(Clone)]
pub struct GladeClient {
    inner: Arc<Inner>,
}

impl GladeClient {
    pub fn new(origin: impl Into<String>) -> Self {
        let origin = origin.into();
        GladeClient {
            inner: Arc::new(Inner {
                origin: origin.clone(),
                endpoint: Mutex::new(None),
                writer: Mutex::new(None),
                read_task: Mutex::new(None),
                session: Mutex::new(Session::new(origin)),
                sub_acks: Mutex::new(VecDeque::new()),
                welcome_acks: Mutex::new(VecDeque::new()),
                ex_corr: AtomicU64::new(0),
                ex_waiters: Mutex::new(HashMap::new()),
                ops_senders: Mutex::new(Vec::new()),
                exreq_senders: Mutex::new(Vec::new()),
                drop_senders: Mutex::new(Vec::new()),
                closing: AtomicBool::new(false),
            }),
        }
    }

    pub fn origin(&self) -> &str {
        &self.inner.origin
    }

    /// Connect to `url` (`ws://host:port`, or bare `host:port`). Remembers the
    /// endpoint so `reconnect` can reattach to the same node.
    pub async fn connect(&self, url: &str) -> io::Result<()> {
        let (host, port) = parse_url(url)?;
        *self.inner.endpoint.lock().await = Some((host.clone(), port));
        self.establish(&host, port).await
    }

    /// Re-establish the connection to the remembered endpoint (reattach-on-drop).
    pub async fn reconnect(&self) -> io::Result<()> {
        let (host, port) = self
            .inner
            .endpoint
            .lock()
            .await
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "never connected"))?;
        self.establish(&host, port).await
    }

    async fn establish(&self, host: &str, port: u16) -> io::Result<()> {
        let (reader, writer) = ws::connect(host, port).await?;
        *self.inner.writer.lock().await = Some(writer);
        if let Some(old) = self.inner.read_task.lock().await.take() {
            old.abort();
        }
        let task = tokio::spawn(read_loop(self.inner.clone(), reader));
        *self.inner.read_task.lock().await = Some(task);
        Ok(())
    }

    /// Send the wire Hello, optionally binding this session to a `principal`
    /// (S7): the node auto-appends an unknown principal to `dir.principals`.
    /// Resolves on the node's Welcome. Absent principal = origin-as-identity.
    pub async fn hello(&self, principal: Option<&str>) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.inner.welcome_acks.lock().await.push_back(tx);
        let body = Hello {
            session: self.inner.origin.clone(),
            protocol: 1,
            principal: principal.map(|s| s.to_string()),
            capability: None,
            heads: vec![],
        };
        self.inner.send(frame(FrameType::Hello, body.to_cbor())).await?;
        rx.await.map_err(|_| dropped())
    }

    /// Subscribe to a zone-surface (share, glade_id, key); resolves on the
    /// node's Heads ack. A subscribe to a DECLARED exchange surface registers
    /// this session as THE provider (`exchange.rs::attach_provider`); a
    /// value/log surface streams its ops back. Empty/absent key = commons.
    pub async fn subscribe(&self, share: &str, glade_id: &str, key: Option<&[u8]>) -> io::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.inner.sub_acks.lock().await.push_back(tx);
        let body = Subscribe {
            share: share.into(),
            glade_id: glade_id.into(),
            key: key.filter(|k| !k.is_empty()).map(|k| k.to_vec()),
            from: None,
        };
        self.inner.send(frame(FrameType::Subscribe, body.to_cbor())).await?;
        rx.await.map_err(|_| dropped())
    }

    /// Append a local op in a zone (default commons) and ship it — the value/log
    /// SERVING act. Returns the authoritative op. Fails fast when disconnected
    /// WITHOUT advancing the chain: a supplier must not build phantom ops the
    /// node can't reconcile after a reattach (stage-1; the offline outbox is a
    /// separate rider, GAP-11). The next append after reconnect is contiguous.
    pub async fn append(&self, share: &str, glade_id: &str, shape: &str, payload: Vec<u8>, key: Option<&[u8]>) -> io::Result<generated::Op> {
        if self.inner.writer.lock().await.is_none() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "not connected"));
        }
        let k = key.map(|k| k.to_vec()).unwrap_or_default();
        let op = self.inner.session.lock().await.append(share, glade_id, shape_of(shape), payload, k);
        self.inner.send(frame(FrameType::Ops, Ops { ops: vec![op.clone()], pri: None }.to_cbor())).await?;
        Ok(op)
    }

    /// Ship already-built ops to the node (the caller owns the chain).
    pub async fn send_ops(&self, ops: Vec<generated::Op>) -> io::Result<()> {
        self.inner.send(frame(FrameType::Ops, Ops { ops, pri: None }.to_cbor())).await
    }

    /// A directed request to a provider; resolves with its `ExchangeRes`
    /// (failure is data — `ok:false` with a reason, never a hang).
    pub async fn exchange(&self, share: &str, glade_id: &str, payload: Vec<u8>) -> io::Result<ExchangeOutcome> {
        let corr = format!("c{}", self.inner.ex_corr.fetch_add(1, Ordering::SeqCst) + 1);
        let (tx, rx) = oneshot::channel();
        self.inner.ex_waiters.lock().await.insert(corr.clone(), tx);
        let body = ExchangeReq { share: share.into(), glade_id: glade_id.into(), corr, payload };
        self.inner.send(frame(FrameType::ExchangeReq, body.to_cbor())).await?;
        rx.await.map_err(|_| dropped())
    }

    /// Answer a directed request as the attached provider: ship a tag-7
    /// `ExchangeRes`, `corr` preserved 1:1 (the node relays it to the requester).
    pub async fn respond_exchange(&self, res: ExchangeRes) -> io::Result<()> {
        self.inner.send(frame(FrameType::ExchangeRes, res.to_cbor())).await
    }

    /// A fresh receiver for inbound ops (fan-out). Every subscribed surface's
    /// ops arrive here; a supplier filters by (share, glade_id, key).
    pub async fn on_ops(&self) -> mpsc::UnboundedReceiver<Vec<generated::Op>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.ops_senders.lock().await.push(tx);
        rx
    }

    /// A fresh receiver for inbound `ExchangeReq` frames (the provider loop).
    pub async fn on_exchange_req(&self) -> mpsc::UnboundedReceiver<ExchangeReq> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.exreq_senders.lock().await.push(tx);
        rx
    }

    /// A fresh receiver that fires once per link drop (never on `close`).
    pub async fn on_drop(&self) -> mpsc::UnboundedReceiver<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.drop_senders.lock().await.push(tx);
        rx
    }

    /// Fold a bound value surface (lww) over what this session has seen.
    pub async fn fold_value(&self, share: &str, glade_id: &str, key: Option<&[u8]>) -> Option<Vec<u8>> {
        self.inner.session.lock().await.fold_value(share, glade_id, key.unwrap_or(&[]))
    }

    /// Fold a bound log surface (ordered) over what this session has seen.
    pub async fn fold_log(&self, share: &str, glade_id: &str, key: Option<&[u8]>) -> Vec<Vec<u8>> {
        self.inner.session.lock().await.fold_log(share, glade_id, key.unwrap_or(&[]))
    }

    /// Close deliberately — NOT a drop (no `on_drop`, no reattach).
    pub async fn close(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
        if let Some(task) = self.inner.read_task.lock().await.take() {
            task.abort();
        }
        *self.inner.writer.lock().await = None;
    }
}

/// Parse `ws://host:port` (or bare `host:port`) into `(host, port)`.
fn parse_url(url: &str) -> io::Result<(String, u16)> {
    let s = url.strip_prefix("ws://").unwrap_or(url);
    let s = s.split('/').next().unwrap_or(s);
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "url needs host:port"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad port"))?;
    Ok((host.to_string(), port))
}
