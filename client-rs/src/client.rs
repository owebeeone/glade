//! `GladeClient` — the rust mirror of the TS `client.ts` choreography, for
//! SUPPLIERS. One websocket to a glade node, the frozen frame protocol
//! (`[FrameType tag][CBOR body]`), a background read loop dispatching inbound
//! frames, and the request/response plumbing a supplier needs: `connect`,
//! optional `hello(principal)` (S7), `subscribe` (returns once its replay is
//! in), `append` / `send_ops`, the requester side `exchange`, and the provider side
//! `on_exchange_req` + `respond_exchange` (corr preserved 1:1). Inbound ops and
//! exchange requests fan out to as many listeners as a session multiplexes
//! (mpsc receivers); `on_drop` fires when the link ends so a supplier reattaches
//! (never on a deliberate `close`). The node answers each op with a status
//! (GladeSubstrateV1 §6, R1): `append_outcome` / `send_ops_outcome` return it
//! as data, `on_refused` reports every refusal, and a refused op's chain stops
//! until a subscribe (answer 4). An op not placed is kept and sent again, zone
//! by zone, and `on_unplaced` reports it (W5); no later op of its chain goes
//! before it, and a gap refusal past it is not placed either (F6).
//! `subscribe_outcome` returns a subscribe's heads, or its refusal and reason
//! (R5, R6). A zone the node refuses after its ack is reported to
//! `on_zone_refused`, and is no longer `live` (F13). No node internals — the
//! wire + tokio only.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use glade_wire::cbor::Cbor;
use glade_wire::generated::{
    ErrorCode, ExchangeReq, ExchangeRes, FrameType, Hello, Ops, Subscribe,
};
use glade_wire::{cbor, generated};

use crate::answers::{
    zone_of, Abandoned, Answered, Answers, OpOutcome, OpStatus, RefusedAfterAck, SubscribeOutcome,
    Subscribed, Subscribes, Zone, ZoneRefusal, WAITING_BOUND,
};
use crate::session::{require_op, shape_of, Session};
use crate::ws::{self, Msg, WsWriter};

/// Who waits to hear what became of one sent op.
type Waiter = oneshot::Sender<OpOutcome>;

/// Who waits for a subscribe's answer and replay.
type SubWaiter = oneshot::Sender<io::Result<SubscribeOutcome>>;

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

fn not_connected() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "not connected")
}

/// Tell an answered op's waiter, if it has one, what became of the op.
fn tell(answered: Answered<Waiter>) {
    if let Some(waiter) = answered.waiter {
        let _ = waiter.send(answered.outcome);
    }
}

/// Subscribes none can answer any more: each fails, but one already refused
/// stays a refusal, its reason unknown (R6).
fn abandon(abandoned: Abandoned<SubWaiter>, kind: io::ErrorKind, why: &str) {
    for waiter in abandoned.failed {
        let _ = waiter.send(Err(io::Error::new(kind, why)));
    }
    for refused in abandoned.refused {
        let _ = refused.waiter.send(Ok(refused.outcome));
    }
}

struct Inner {
    origin: String,
    /// `host:port` of the last connect — reused by `reconnect` (reattach).
    endpoint: Mutex<Option<(String, u16)>>,
    /// The current connection's writer; `None` while disconnected.
    writer: Mutex<Option<WsWriter>>,
    read_task: Mutex<Option<JoinHandle<()>>>,
    session: Mutex<Session>,
    /// Subscribes waiting for their ack, reason or replay (R5-R7).
    subscribes: Mutex<Subscribes<SubWaiter>>,
    /// FIFO ack waiters — Welcome pops one hello.
    welcome_acks: Mutex<VecDeque<oneshot::Sender<()>>>,
    ex_corr: AtomicU64,
    ex_waiters: Mutex<HashMap<String, oneshot::Sender<ExchangeOutcome>>>,
    ops_senders: Mutex<Vec<mpsc::UnboundedSender<Vec<generated::Op>>>>,
    exreq_senders: Mutex<Vec<mpsc::UnboundedSender<ExchangeReq>>>,
    drop_senders: Mutex<Vec<mpsc::UnboundedSender<()>>>,
    /// Sent ops by hash until their statuses come (R1), and the ops not placed
    /// (W5). Taken after `session` when both are held.
    answers: Mutex<Answers<Waiter>>,
    refused_senders: Mutex<Vec<mpsc::UnboundedSender<OpStatus>>>,
    unplaced_senders: Mutex<Vec<mpsc::UnboundedSender<OpStatus>>>,
    zone_refused_senders: Mutex<Vec<mpsc::UnboundedSender<ZoneRefusal>>>,
    closing: AtomicBool,
}

impl Inner {
    /// Decode + dispatch one inbound frame (the read loop's body).
    async fn dispatch(self: &Arc<Self>, bytes: &[u8]) {
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
                let applied = self.session.lock().await.apply_remote(&ops);
                if let Err(e) = applied {
                    // Dropped whole, the frame leaves the replays it carries
                    // incomplete: their subscribes fail.
                    for waiter in self.subscribes.lock().await.failed(&ops) {
                        let _ = waiter.send(Err(io::Error::new(e.kind(), e.to_string())));
                    }
                    return;
                }
                self.ops_senders.lock().await.retain(|s| s.send(ops.clone()).is_ok());
                let done = self.subscribes.lock().await.received(&ops);
                self.caught_up(done).await;
            }
            FrameType::Heads => {
                // The ack of the oldest subscribe: it names the zone and the
                // heads its replay must reach (R5), or no zone, for a refusal
                // whose reason follows (R6).
                let ack = generated::Heads::from_cbor(&body);
                let acked = self.subscribes.lock().await.acked(&ack);
                match acked {
                    Ok(done) => {
                        self.caught_up(done.into_iter().collect()).await;
                    }
                    Err(abandoned) => {
                        abandon(abandoned, io::ErrorKind::InvalidData, "an ack for another zone came in a subscribe's turn");
                    }
                }
            }
            FrameType::Error => {
                // R1: a status names its op by hash. An `Error` with no `corr`
                // is a refused subscribe's reason (R6), or else refuses zones
                // after their ack (F13).
                let status = generated::Error::from_cbor(&body);
                let refused = self.subscribes.lock().await.reason(&status);
                if let Some(refused) = refused {
                    let _ = refused.waiter.send(Ok(refused.outcome));
                    return;
                }
                let after_ack = self.subscribes.lock().await.refused_after_ack(&status);
                if !after_ack.zones.is_empty() {
                    self.zones_refused(after_ack).await;
                    return;
                }
                let answered = {
                    let mut session = self.session.lock().await;
                    self.answers.lock().await.status(&mut session, &status)
                };
                if let Some(answered) = answered {
                    let done = self.subscribes.lock().await.answered(&answered.op, &answered.outcome);
                    self.answer(answered).await;
                    self.caught_up(done).await;
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
            _ => {} // channel frames: ignored (echo/channel are P3)
        }
    }

    /// `report`, then start the zone's resend timer while it has ops not
    /// placed (W5).
    async fn answer(self: &Arc<Self>, answered: Answered<Waiter>) {
        let zone = zone_of(&answered.op);
        self.report(answered).await;
        if let Some(timer) = self.answers.lock().await.start_resending(&zone) {
            tokio::spawn(resend_loop(Arc::downgrade(self), zone, timer));
        }
    }

    /// Report a refusal to `on_refused`, and an op not placed, once, to
    /// `on_unplaced`; and tell the op's waiter.
    async fn report(&self, answered: Answered<Waiter>) {
        let op = answered.op.clone();
        match &answered.outcome {
            OpOutcome::Refused { code, message } => {
                let status = OpStatus { op: op.clone(), code: *code, message: message.clone() };
                self.refused_senders.lock().await.retain(|s| s.send(status.clone()).is_ok());
            }
            OpOutcome::NotPlaced { message } if answered.newly_unplaced => {
                let status = OpStatus { op: op.clone(), code: ErrorCode::UnknownShare, message: message.clone() };
                self.unplaced_senders.lock().await.retain(|s| s.send(status.clone()).is_ok());
            }
            _ => {}
        }
        tell(answered);
    }

    /// Zones the node refused after their ack (F13): `on_zone_refused` hears
    /// of each, and a subscribe of one still waiting for its replay returns
    /// the refusal.
    async fn zones_refused(&self, after_ack: RefusedAfterAck<SubWaiter>) {
        for refusal in after_ack.zones {
            let mut senders = self.zone_refused_senders.lock().await;
            senders.retain(|s| s.send(refusal.clone()).is_ok());
        }
        for subscribed in after_ack.subscribes {
            let _ = subscribed.waiter.send(Ok(subscribed.outcome));
        }
    }

    /// Subscribes whose replay is in (R7). Before each returns, its zone's
    /// chain that a refusal stopped goes on, and its zone's ops not placed go
    /// again (answer 4, W5), so the caller's next op follows them.
    async fn caught_up(&self, done: Vec<Subscribed<SubWaiter>>) {
        for subscribed in done {
            let (share, glade_id, key) = &subscribed.zone;
            let again = {
                let mut session = self.session.lock().await;
                session.resumed(share, glade_id, key);
                self.answers.lock().await.unplaced_in(&subscribed.zone)
            };
            if !again.is_empty() {
                let waiters = again.iter().map(|_| None).collect();
                let _ = self.ship(again, waiters).await;
            }
            let _ = subscribed.waiter.send(Ok(subscribed.outcome));
        }
    }

    /// The connection ended (close/EOF): forget the writer, fail every pending
    /// waiter so awaiting calls return `dropped()` (a supplier re-issues them on
    /// reattach), and signal `on_drop` unless the caller closed us deliberately.
    async fn on_connection_end(&self) {
        *self.writer.lock().await = None;
        self.welcome_acks.lock().await.clear();
        self.ex_waiters.lock().await.clear();
        self.end_waiting().await;
        if !self.closing.load(Ordering::SeqCst) {
            self.drop_senders.lock().await.retain(|s| s.send(()).is_ok());
        }
    }

    /// R7: nothing sent on a connection that ended is answered. Each waiting
    /// op's fate is unknown, and each waiting subscribe fails as `dropped()`,
    /// but for one already refused.
    async fn end_waiting(&self) {
        for answered in self.answers.lock().await.ended() {
            tell(answered);
        }
        let abandoned = self.subscribes.lock().await.ended();
        abandon(abandoned, io::ErrorKind::BrokenPipe, "connection dropped");
    }

    /// Send ops in one frame, each kept by its hash, with its waiter, until its
    /// status comes (R1). One behind an op of its chain not placed is held,
    /// to go after that op when it is sent again (F6).
    async fn ship(&self, ops: Vec<generated::Op>, waiters: Vec<Option<Waiter>>) -> io::Result<()> {
        for op in &ops {
            require_op(op.shape, &op.payload, "send_ops")?;
        }
        if self.writer.lock().await.is_none() {
            return Err(not_connected());
        }
        let (ops, due) = self.answers.lock().await.to_send(ops, waiters);
        self.put(ops, due).await
    }

    /// Give the answers `to_send` found due at once, and send the ops it let
    /// go now, if any, in one frame.
    async fn put(&self, ops: Vec<generated::Op>, due: Vec<Answered<Waiter>>) -> io::Result<()> {
        for answered in due {
            self.report(answered).await;
        }
        if ops.is_empty() {
            return Ok(());
        }
        let bytes = frame(FrameType::Ops, Ops { ops, pri: None }.to_cbor());
        self.send(bytes).await
    }

    async fn send(&self, bytes: Vec<u8>) -> io::Result<()> {
        let w = self.writer.lock().await.clone();
        match w {
            Some(w) => w.send_binary(&bytes).await,
            None => Err(not_connected()),
        }
    }
}

/// W5: while a zone has ops not placed, send them again on the zone's
/// backoff, 1 s doubling to 30 s. The timer stops once every op of the zone is
/// placed, at the connection's end, or when the client is closed or gone.
async fn resend_loop(inner: Weak<Inner>, zone: Zone, timer: u64) {
    loop {
        let wait = match inner.upgrade() {
            Some(inner) => inner.answers.lock().await.next_resend(&zone),
            None => {
                return;
            }
        };
        tokio::time::sleep(wait).await;
        let Some(inner) = inner.upgrade() else {
            return;
        };
        if inner.closing.load(Ordering::SeqCst) {
            return;
        }
        let Some(again) = inner.answers.lock().await.resend_tick(&zone, timer) else {
            return;
        };
        if !again.is_empty() {
            let waiters = again.iter().map(|_| None).collect();
            let _ = inner.ship(again, waiters).await;
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
                subscribes: Mutex::new(Subscribes::default()),
                welcome_acks: Mutex::new(VecDeque::new()),
                ex_corr: AtomicU64::new(0),
                ex_waiters: Mutex::new(HashMap::new()),
                ops_senders: Mutex::new(Vec::new()),
                exreq_senders: Mutex::new(Vec::new()),
                drop_senders: Mutex::new(Vec::new()),
                answers: Mutex::new(Answers::new(WAITING_BOUND)),
                refused_senders: Mutex::new(Vec::new()),
                unplaced_senders: Mutex::new(Vec::new()),
                zone_refused_senders: Mutex::new(Vec::new()),
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
        if let Some(old) = self.inner.read_task.lock().await.take() {
            old.abort();
        }
        // R7: nothing sent before this connection is answered on it.
        *self.inner.writer.lock().await = None;
        self.inner.end_waiting().await;
        *self.inner.writer.lock().await = Some(writer);
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

    /// Subscribe to a zone-surface (share, glade_id, key), and return once its
    /// replay is in (R7). A subscribe to a DECLARED exchange surface registers
    /// this session as THE provider (`exchange.rs::attach_provider`); a
    /// value/log surface streams its ops back. Empty/absent key = commons. A
    /// refused subscribe returns as an empty zone (R6).
    pub async fn subscribe(&self, share: &str, glade_id: &str, key: Option<&[u8]>) -> io::Result<()> {
        self.subscribe_outcome(share, glade_id, key).await.map(|_| ())
    }

    /// `subscribe`, returning the node's answer as data: each origin's head
    /// in the zone by seq and hash, once the replay is in (R5, R7), or the
    /// refusal and its reason (R6). `Err` if the connection ends first, or
    /// the replay brings a frame the session cannot take.
    pub async fn subscribe_outcome(&self, share: &str, glade_id: &str, key: Option<&[u8]>) -> io::Result<SubscribeOutcome> {
        let key = key.filter(|k| !k.is_empty());
        let zone: Zone = (share.into(), glade_id.into(), key.unwrap_or_default().to_vec());
        let body = Subscribe { share: share.into(), glade_id: glade_id.into(), key: key.map(|k| k.to_vec()), from: None };
        let (tx, rx) = oneshot::channel();
        {
            // Kept in the order sent, which is the order of the acks: its ack
            // cannot be read before it is kept.
            let mut subscribes = self.inner.subscribes.lock().await;
            self.inner.send(frame(FrameType::Subscribe, body.to_cbor())).await?;
            subscribes.sent(zone, tx);
        }
        rx.await.map_err(|_| dropped())?
    }

    /// Append a local op in a zone (default commons) and ship it — the value/log
    /// SERVING act. Returns the authoritative op. Fails fast when disconnected
    /// WITHOUT advancing the chain: a supplier must not build phantom ops the
    /// node can't reconcile after a reattach (stage-1; the offline outbox is a
    /// separate rider, GAP-11). The next append after reconnect is contiguous.
    /// Fails too on a chain a refusal stopped, until a subscribe of its zone.
    pub async fn append(&self, share: &str, glade_id: &str, shape: &str, payload: Vec<u8>, key: Option<&[u8]>) -> io::Result<generated::Op> {
        self.append_op(share, glade_id, shape, payload, key, None).await
    }

    /// `append`, then the node's answer to the op, as data (R1, R7): `Unknown`
    /// if the connection ends first. A node from before the client-writes
    /// plan's Phase 2 sends no status, so against one this waits until the
    /// connection ends, or until later sends pass the bound on those kept.
    /// An op held behind one of its chain not placed is `NotPlaced` at once,
    /// with no answer from the node: it goes after that op (F6).
    pub async fn append_outcome(&self, share: &str, glade_id: &str, shape: &str, payload: Vec<u8>, key: Option<&[u8]>) -> io::Result<(generated::Op, OpOutcome)> {
        let (tx, rx) = oneshot::channel();
        let op = self.append_op(share, glade_id, shape, payload, key, Some(tx)).await?;
        Ok((op, rx.await.unwrap_or(OpOutcome::Unknown)))
    }

    async fn append_op(&self, share: &str, glade_id: &str, shape: &str, payload: Vec<u8>, key: Option<&[u8]>, waiter: Option<Waiter>) -> io::Result<generated::Op> {
        // Capability is resolved before connectivity checks, chain allocation,
        // or session mutation; unsupported names never become Value ops.
        let shape = shape_of(shape)?;
        require_op(shape, &payload, "append")?;
        if self.inner.writer.lock().await.is_none() {
            return Err(not_connected());
        }
        let k = key.map(|k| k.to_vec()).unwrap_or_default();
        let (op, ops, due) = {
            // Kept under the session's lock, so a status is never applied
            // between the op's making and its keeping.
            let mut session = self.inner.session.lock().await;
            let op = session.append(share, glade_id, shape, payload, k)?;
            let mut answers = self.inner.answers.lock().await;
            let (ops, due) = answers.to_send(vec![op.clone()], vec![waiter]);
            (op, ops, due)
        };
        self.inner.put(ops, due).await?;
        Ok(op)
    }

    /// Ship already-built ops to the node (the caller owns the chain).
    pub async fn send_ops(&self, ops: Vec<generated::Op>) -> io::Result<()> {
        let waiters = ops.iter().map(|_| None).collect();
        self.inner.ship(ops, waiters).await
    }

    /// `send_ops`, then the node's answer to each op, in the order given; for
    /// an op held behind one of its chain not placed, `NotPlaced` (F6).
    pub async fn send_ops_outcome(&self, ops: Vec<generated::Op>) -> io::Result<Vec<OpOutcome>> {
        let (waiters, answers): (Vec<_>, Vec<_>) = ops
            .iter()
            .map(|_| {
                let (tx, rx) = oneshot::channel();
                (Some(tx), rx)
            })
            .unzip();
        self.inner.ship(ops, waiters).await?;
        let mut outcomes = Vec::new();
        for rx in answers {
            outcomes.push(rx.await.unwrap_or(OpOutcome::Unknown));
        }
        Ok(outcomes)
    }

    /// A fresh receiver for the node's refusals of this client's ops: the op,
    /// the code and the reason. An op the session made is already dropped,
    /// with the rest of its chain, when its refusal arrives here.
    pub async fn on_refused(&self) -> mpsc::UnboundedReceiver<OpStatus> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.refused_senders.lock().await.push(tx);
        rx
    }

    /// A fresh receiver for this client's ops the node could not place (W5),
    /// each told once: the client keeps it and its chain, and sends them again.
    /// An op held behind one of them, or refused as a gap past one, is told
    /// too (F6); all come as `UnknownShare`, with the node's or the client's
    /// reason.
    pub async fn on_unplaced(&self) -> mpsc::UnboundedReceiver<OpStatus> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.unplaced_senders.lock().await.push(tx);
        rx
    }

    /// A fresh receiver for zones the node refused after their subscribe was
    /// acked (F13): an `Error` naming the zone and no op, as a forwarding node
    /// relays its claim holder's refusal of the read (F5), and the grant
    /// re-check pass sends. The node has ended the subscription: the zone is
    /// no longer `live`, no more of its ops come, and the session keeps what
    /// it holds of it. A subscribe of it still waiting for its replay returns
    /// the refusal, and a later subscribe asks the node again.
    pub async fn on_zone_refused(&self) -> mpsc::UnboundedReceiver<ZoneRefusal> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.zone_refused_senders.lock().await.push(tx);
        rx
    }

    /// Whether this connection is subscribed to the zone: an ack named it,
    /// and the node has not refused it since (F13). No subscription outlives
    /// its connection.
    pub async fn live(&self, share: &str, glade_id: &str, key: Option<&[u8]>) -> bool {
        let key = key.unwrap_or_default().to_vec();
        let zone: Zone = (share.into(), glade_id.into(), key);
        self.inner.subscribes.lock().await.live(&zone)
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

    /// Whether a refusal stopped this client's chain in the zone (answer 4):
    /// its appends fail until a subscribe of the zone resumes it.
    pub(crate) async fn stopped(&self, share: &str, glade_id: &str, key: Option<&[u8]>) -> bool {
        let session = self.inner.session.lock().await;
        session.stopped(share, glade_id, key.unwrap_or(&[]))
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
        self.inner.end_waiting().await;
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
