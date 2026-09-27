//! A carrier link after its HELLO, and the conversations it carries (plan
//! Step 4.5b, part 2): what the mesh speaks once it moves onto the carrier
//! port (part 3). Every exchange two nodes have, each a QUIC stream of its
//! own today, is a conversation on their one link, and after HELLO every
//! frame on the link begins with a 5-byte header:
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0-3 | the conversation, a `u32` little-endian: odd from the link's dialer, even from its acceptor, never 0 |
//! | 4 | what follows: `0` a frame, `1` END, `2` RESET |
//! | 5 on | with `0`, one [`Frame`] as `frame.rs` encodes it |
//!
//! - A conversation takes its number when its first frame is queued, so each
//!   end's numbers reach the peer rising, and none is used twice on a link.
//!   The peer's first frame on a new number opens a conversation, which goes
//!   to the caller's handler, and the handler chooses what it is by that
//!   frame, as the mesh chooses a stream's today.
//! - END says its sender sends nothing more, and its receiver reads
//!   `UnexpectedEof`, as a finished stream reads. RESET is sent for a
//!   conversation dropped before its END; its receiver reads an error, and so
//!   does every conversation still open when the link ends: never a clean end.
//! - A frame for a conversation that is over, or that this end never opened,
//!   is dropped. A frame too short for its header, or of another kind, ends
//!   the link.
//!
//! Two tasks per link. The writer is the link's only sender: a conversation
//! queues its frames and never waits for the network, so a conversation
//! cancelled at any await point never leaves a frame torn, which would end
//! the link for every conversation on it. The reader is the link's only
//! receiver: it puts each frame in its conversation's queue, of [`QUEUE`]
//! frames, and waits while that queue is full, so the carrier's flow control
//! holds the peer back; a conversation its handler has let go takes nothing
//! more. The reader decodes no frame: a conversation's own receive does, so a
//! bad frame ends that conversation's handler, never the link. The peer may
//! hold [`PEER_OPEN`] conversations open at once, and one more is reset at
//! once.
//!
//! The reader waits only on a full queue, which drains only while its
//! conversation is read. So no handler may hold a node lock (`cut`, `store`,
//! `links`) across a receive; every handler must keep that rule. None waits
//! for the writer: a send only queues.
//!
//! Carrier-free: it names [`CarrierLink`] and [`Frame`], and no transport's
//! types. It starts its tasks through the caller's [`Spawn`], which in the
//! node is the task seam (`tasks.rs`).

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use glade_carrier_api::CarrierLink;
use tokio::sync::mpsc;

use crate::frame::Frame;

/// A conversation frame's header: its number, then what follows.
pub const HEADER: usize = 5;

// What follows a header.
const FRAME: u8 = 0;
const END: u8 = 1;
const RESET: u8 = 2;

/// How many frames a conversation's queue holds before the reader waits.
pub const QUEUE: usize = 16;

/// How many conversations the peer may hold open at once: QUIC's default
/// limit on a connection's streams, under which the mesh runs today.
pub const PEER_OPEN: usize = 100;

/// A future one of a link's tasks runs.
pub type Work = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Which of a link's tasks a [`Work`] is, so the caller can place it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTask {
    /// The link's writer, its only sender.
    Writer,
    /// The link's reader, its only receiver.
    Reader,
    /// A conversation the peer opened, in the caller's handler.
    Inbound,
}

/// How a link's tasks are started.
pub type Spawn = Arc<dyn Fn(LinkTask, Work) + Send + Sync>;

/// What the caller does with each conversation the peer opens.
pub type Handler = Arc<dyn Fn(Conversation) -> Work + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One carrier link after its HELLO, and its conversations.
pub struct Linked {
    node: [u8; 32],
    max: usize,
    out: mpsc::UnboundedSender<Out>,
    table: Mutex<Table>,
}

/// What the writer is given: a frame to send, or the link to close.
enum Out {
    Send(Vec<u8>),
    Close,
}

/// The link's open conversations.
struct Table {
    /// This end's next number: odd for the dialer, even for the acceptor.
    next: u32,
    /// The highest number the peer has opened.
    peer_high: u32,
    open: BTreeMap<u32, Route>,
    /// The link has ended, or is ending: nothing more is sent.
    ended: bool,
}

/// One open conversation, as the reader and this end see it.
struct Route {
    /// Where the reader puts its frames, until the peer ends it.
    tx: Option<mpsc::Sender<Item>>,
    /// This end has sent its END.
    done: bool,
}

/// What a conversation's queue holds: a frame, or the peer's END.
enum Item {
    Frame(Vec<u8>),
    End,
}

/// What the reader does with a frame.
enum Routed {
    /// Put it in its conversation's queue.
    Deliver(mpsc::Sender<Item>, Item),
    /// A conversation the peer opened, its first frame queued: the handler's.
    Opened(Conversation),
    /// Nothing: its conversation is over, or unknown.
    Dropped,
}

impl Linked {
    /// Start `link`'s writer and reader. The link's HELLO proved `node`;
    /// `dialed` says whether this end dialed it, and so whose numbers are
    /// odd; `max` is its frame limit. Each conversation the peer opens goes
    /// to `handler`.
    pub fn start(
        link: Arc<dyn CarrierLink>,
        node: [u8; 32],
        dialed: bool,
        max: usize,
        spawn: Spawn,
        handler: Handler,
    ) -> Arc<Linked> {
        let (out, queued) = mpsc::unbounded_channel();
        let (next, open) = (if dialed { 1 } else { 2 }, BTreeMap::new());
        let table = Mutex::new(Table {
            next,
            peer_high: 0,
            open,
            ended: false,
        });
        let linked = Arc::new(Linked {
            node,
            max,
            out,
            table,
        });
        spawn(LinkTask::Writer, Box::pin(write(link.clone(), queued)));
        // The peer's numbers: even when this end dialed, else odd.
        let peers = if dialed { 0 } else { 1 };
        let reader = read(link, Arc::downgrade(&linked), peers, spawn.clone(), handler);
        spawn(LinkTask::Reader, Box::pin(reader));
        linked
    }

    /// The node the link's HELLO proved.
    pub fn node(&self) -> [u8; 32] {
        self.node
    }

    /// A new conversation of this end's. The peer knows it from its first
    /// frame, so it sends before it receives.
    pub fn open(self: &Arc<Self>) -> Conversation {
        let (tx, rx) = mpsc::channel(QUEUE);
        Conversation::new(self.clone(), Number::Unsent(tx), rx)
    }

    /// End the link, from anywhere, never waiting: nothing more is queued,
    /// the writer sends what is and then closes the link, and every open
    /// conversation then reads an error.
    pub fn end(&self) {
        lock(&self.table).ended = true;
        let _ = self.out.send(Out::Close);
    }

    /// The link has ended: every open conversation reads an error, and the
    /// writer closes the link.
    fn ended(&self) {
        let routes = {
            let mut table = lock(&self.table);
            table.ended = true;
            std::mem::take(&mut table.open)
        };
        drop(routes);
        let _ = self.out.send(Out::Close);
    }

    /// Queue a header of `kind`, and `body`, for the conversation `number`
    /// names. A conversation with no number takes this end's next, and
    /// its queue's sending half goes to the reader, under the one lock, so
    /// its first frame is queued behind every lower number's.
    fn put(&self, number: &Mutex<Number>, kind: u8, body: &[u8]) -> io::Result<()> {
        let mut table = lock(&self.table);
        if table.ended {
            return Err(gone());
        }
        let mut number = lock(number);
        let id = match &*number {
            Number::Sent(id) => *id,
            Number::Unsent(tx) => {
                let id = table.next;
                let spent = || io::Error::other("the link's conversation numbers are spent");
                table.next = id.checked_add(2).ok_or_else(spent)?;
                let tx = Some(tx.clone());
                table.open.insert(id, Route { tx, done: false });
                *number = Number::Sent(id);
                id
            }
        };
        let frame = [&id.to_le_bytes()[..], &[kind], body].concat();
        self.out.send(Out::Send(frame)).map_err(|_| gone())
    }

    /// This end is done with `number`'s conversation, by `kind`, END or
    /// RESET: queue it, and forget the conversation once both ends are done
    /// with it, or at once for a RESET. One that never sent is known to no
    /// one, and sends nothing.
    fn finish(&self, number: &Mutex<Number>, kind: u8) {
        let mut table = lock(&self.table);
        let id = match &*lock(number) {
            Number::Sent(id) => *id,
            Number::Unsent(_) => return,
        };
        if !table.ended {
            let _ = self.out.send(Out::Send(marker(id, kind)));
        }
        if let Entry::Occupied(mut route) = table.open.entry(id) {
            if kind == RESET || route.get().tx.is_none() {
                route.remove();
            } else {
                route.get_mut().done = true;
            }
        }
    }

    /// Where the reader puts a frame for conversation `id` of `kind`; the
    /// peer's numbers are those whose parity is `peers`.
    fn route(self: &Arc<Self>, id: u32, kind: u8, body: &[u8], peers: u32) -> Routed {
        let mut table = lock(&self.table);
        if let Entry::Occupied(mut route) = table.open.entry(id) {
            // After the peer's END or RESET, nothing more is taken.
            let Some(tx) = route.get().tx.clone() else {
                return Routed::Dropped;
            };
            if kind == FRAME {
                return Routed::Deliver(tx, Item::Frame(body.to_vec()));
            }
            // The peer sends nothing more. END reaches the queue; a RESET
            // drops its sending half, which its receiver reads as an error.
            route.get_mut().tx = None;
            if route.get().done {
                route.remove();
            }
            return match kind {
                END => Routed::Deliver(tx, Item::End),
                _ => Routed::Dropped,
            };
        }
        if id % 2 != peers || id <= table.peer_high {
            return Routed::Dropped;
        }
        table.peer_high = id;
        if kind == RESET {
            return Routed::Dropped;
        }
        let held = table.open.keys().filter(|id| *id % 2 == peers).count();
        if held >= PEER_OPEN {
            let _ = self.out.send(Out::Send(marker(id, RESET)));
            return Routed::Dropped;
        }
        let (tx, rx) = mpsc::channel(QUEUE);
        let first = match kind {
            FRAME => Item::Frame(body.to_vec()),
            _ => Item::End,
        };
        // An empty queue takes the first frame; after an END, nothing more.
        let _ = tx.try_send(first);
        let tx = (kind == FRAME).then_some(tx);
        table.open.insert(id, Route { tx, done: false });
        drop(table);
        Routed::Opened(Conversation::new(self.clone(), Number::Sent(id), rx))
    }
}

fn gone() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "the link ended")
}

/// Conversation `id`'s END or RESET: a header alone.
fn marker(id: u32, kind: u8) -> Vec<u8> {
    [&id.to_le_bytes()[..], &[kind]].concat()
}

/// The link's writer, its only sender: each queued frame, whole and in
/// order, until the link is to close or a send fails; then it closes the
/// link.
async fn write(link: Arc<dyn CarrierLink>, mut queued: mpsc::UnboundedReceiver<Out>) {
    while let Some(Out::Send(frame)) = queued.recv().await {
        if link.send(&frame).await.is_err() {
            break;
        }
    }
    link.close().await;
}

/// The link's reader, its only receiver: each frame into its conversation's
/// queue, until the link ends. Then every open conversation reads an error.
async fn read(
    link: Arc<dyn CarrierLink>,
    owner: Weak<Linked>,
    peers: u32,
    spawn: Spawn,
    handler: Handler,
) {
    while let Ok(Some(bytes)) = link.recv().await {
        // A link no one holds is over; so is one that breaks its framing.
        let (Some(linked), Some((id, kind, body))) = (owner.upgrade(), header(&bytes)) else {
            break;
        };
        match linked.route(id, kind, body, peers) {
            // A conversation let go takes nothing: the send fails at once.
            Routed::Deliver(tx, item) => {
                let _ = tx.send(item).await;
            }
            Routed::Opened(conversation) => spawn(LinkTask::Inbound, handler(conversation)),
            Routed::Dropped => {}
        }
    }
    if let Some(linked) = owner.upgrade() {
        linked.ended();
    }
}

/// A frame's number, kind and body; none for a frame too short for its
/// header, or of a kind no end sends.
fn header(bytes: &[u8]) -> Option<(u32, u8, &[u8])> {
    let (head, body) = bytes.split_at_checked(HEADER)?;
    let id = u32::from_le_bytes(head[..4].try_into().ok()?);
    (head[4] <= RESET).then_some((id, head[4], body))
}

/// One conversation: frames each way, in order, until each end's END.
pub struct Conversation {
    linked: Arc<Linked>,
    number: Mutex<Number>,
    rx: mpsc::Receiver<Item>,
    /// The peer's END has been read.
    finished: bool,
    /// This end's END is queued, so a drop resets nothing.
    ended: bool,
}

/// A conversation's number, once its first frame is queued; until then, the
/// sending half of its queue, which the reader then holds.
enum Number {
    Unsent(mpsc::Sender<Item>),
    Sent(u32),
}

impl Conversation {
    fn new(linked: Arc<Linked>, number: Number, rx: mpsc::Receiver<Item>) -> Conversation {
        let number = Mutex::new(number);
        Conversation {
            linked,
            number,
            rx,
            finished: false,
            ended: false,
        }
    }

    /// Queue `frame` for the link's writer, never waiting. A frame over the
    /// link's limit, with its header, is refused here, and so is every frame
    /// once the link has ended.
    pub fn send(&self, frame: &Frame) -> io::Result<()> {
        self.send_encoded(&frame.to_bytes())
    }

    /// [`Conversation::send`] for a frame already encoded, as a session's
    /// queue holds it (plan Step 4.5b, part 3).
    pub fn send_encoded(&self, frame: &[u8]) -> io::Result<()> {
        if HEADER + frame.len() > self.linked.max {
            let why = "the frame is over the link's frame limit";
            return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
        }
        self.linked.put(&self.number, FRAME, frame)
    }

    /// The next frame. `UnexpectedEof` once the peer's END is read, as a
    /// finished stream reads; `ConnectionReset` once the peer has reset the
    /// conversation, and `ConnectionAborted` once the link has ended.
    pub async fn recv(&mut self) -> io::Result<Frame> {
        let ended = || io::Error::new(io::ErrorKind::UnexpectedEof, "the conversation ended");
        if self.finished {
            return Err(ended());
        }
        let number = self
            .number
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if let Number::Unsent(_) = number {
            let why = "a conversation this end opened sends before it receives";
            return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
        }
        match self.rx.recv().await {
            Some(Item::Frame(bytes)) => {
                Frame::from_bytes(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
            Some(Item::End) => {
                self.finished = true;
                Err(ended())
            }
            None if lock(&self.linked.table).ended => Err(gone()),
            None => {
                let why = "the conversation was reset";
                Err(io::Error::new(io::ErrorKind::ConnectionReset, why))
            }
        }
    }

    /// END: this end sends nothing more on it. Dropped before this, the
    /// conversation is reset.
    pub fn end(mut self) {
        self.ended = true;
        self.linked.finish(&self.number, END);
    }
}

impl Drop for Conversation {
    fn drop(&mut self) {
        if !self.ended {
            self.linked.finish(&self.number, RESET);
        }
    }
}

// The in-memory link pair and the iroh ports the conversation and HELLO tests
// run on. A braced module, so the condition encloses the whole section.
#[cfg(test)]
pub(crate) mod testing {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use glade_carrier_api::{
        CarrierAddr, CarrierConfig, CarrierError, CarrierLink, CarrierPort, ChannelBinding,
        PortFuture, TransportId,
    };
    use tokio::sync::{mpsc, Mutex, Notify};

    use crate::iroh_carrier::{IrohCarrier, Lent, FIRST_WORD};
    use crate::netconf::Relays;
    use crate::peer::HELLO_LABEL;
    use crate::transport::EndpointKey;

    /// How many frames an in-memory link holds in flight before a send waits.
    const DEPTH: usize = 4;

    /// One end of an in-memory link: what it sends, its far end receives,
    /// whole and in order, and a close ends both. No socket, and none of a
    /// transport's timing or losses.
    pub(crate) struct MemLink {
        out: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
        inbox: Mutex<mpsc::Receiver<Vec<u8>>>,
        closed: AtomicBool,
        closing: Notify,
        remote: Vec<u8>,
        exported: Option<[u8; 32]>,
        max: usize,
    }

    /// Two in-memory links, each the other's far end, carrying frames of at
    /// most `max` bytes. The dialer's end names `ids[1]` as its far end's
    /// key, and the acceptor's `ids[0]`. With `exported`, both answer it under
    /// HELLO's label and a mix of it and the label under any other; without
    /// it, neither binds a session.
    pub(crate) fn pair(
        ids: [Vec<u8>; 2],
        exported: Option<[u8; 32]>,
        max: usize,
    ) -> [Arc<MemLink>; 2] {
        let (to_acceptor, acceptor_inbox) = mpsc::channel(DEPTH);
        let (to_dialer, dialer_inbox) = mpsc::channel(DEPTH);
        let [dialer, acceptor] = ids;
        let end = |out, inbox, remote| {
            Arc::new(MemLink {
                out: Mutex::new(Some(out)),
                inbox: Mutex::new(inbox),
                closed: AtomicBool::new(false),
                closing: Notify::new(),
                remote,
                exported,
                max,
            })
        };
        [
            end(to_acceptor, dialer_inbox, acceptor),
            end(to_dialer, acceptor_inbox, dialer),
        ]
    }

    impl CarrierLink for MemLink {
        fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
            Box::pin(async move {
                let out = self.out.lock().await;
                let Some(out) = out.as_ref() else {
                    return Err(CarrierError::Closed);
                };
                if frame.len() > self.max {
                    return Err(CarrierError::FrameTooLarge);
                }
                let sent = out.send(frame.to_vec()).await;
                sent.map_err(|_| CarrierError::Closed)
            })
        }

        fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
            Box::pin(async move {
                let mut inbox = self.inbox.lock().await;
                if self.closed.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                tokio::select! {
                    frame = inbox.recv() => Ok(frame),
                    () = self.closing.notified() => Ok(None),
                }
            })
        }

        fn close(&self) -> PortFuture<'_, ()> {
            Box::pin(async move {
                self.closed.store(true, Ordering::SeqCst);
                self.closing.notify_one();
                self.out.lock().await.take();
            })
        }

        fn remote_id(&self) -> Option<TransportId> {
            Some(TransportId(self.remote.clone()))
        }

        fn channel_binding(&self, label: &[u8]) -> Option<ChannelBinding> {
            let mut bytes = self.exported?;
            if label != HELLO_LABEL {
                for (at, byte) in label.iter().enumerate() {
                    bytes[at % 32] ^= byte;
                }
            }
            Some(ChannelBinding(bytes))
        }
    }

    /// An iroh port on loopback, lent `key` and no door, bound with frames of
    /// at most `max` bytes: its address.
    pub(crate) async fn iroh_port(key: EndpointKey, max: usize) -> (IrohCarrier, CarrierAddr) {
        let (door, relays, first_word) = (None, Relays::Off, FIRST_WORD);
        let port = IrohCarrier::new(Some(Lent {
            key,
            door,
            relays,
            first_word,
        }));
        let local = CarrierAddr("127.0.0.1:0".into());
        let max_frame_bytes = NonZeroUsize::new(max).unwrap();
        let config = CarrierConfig {
            local,
            max_frame_bytes,
        };
        let at = port.bind(config).await.unwrap();
        (port, at)
    }

    /// A link `a` dials to `b`, at `at`: the dialer's end, then the
    /// acceptor's.
    pub(crate) async fn iroh_link(
        a: &IrohCarrier,
        at: &CarrierAddr,
        b: &IrohCarrier,
    ) -> [Arc<dyn CarrierLink>; 2] {
        let (dialed, accepted) = tokio::try_join!(a.dial(at), b.accept()).unwrap();
        [Arc::from(dialed), Arc::from(accepted.unwrap())]
    }

    /// A fresh endpoint key.
    pub(crate) fn endpoint_key() -> EndpointKey {
        EndpointKey::from_seed(crate::signing::random_seed().unwrap())
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::testing::{endpoint_key, iroh_link, iroh_port, pair};
    use super::*;
    use glade_wire::generated::{ChannelData, ExchangeReq, Heads, Subscribe};
    use std::time::Duration;

    /// A frame that says `what`.
    fn said(what: impl Into<String>) -> Frame {
        let (channel, data) = (what.into(), Vec::new());
        Frame::ChannelData(ChannelData { channel, data })
    }

    /// What a frame says, as [`said`] wrote it.
    fn what(frame: Frame) -> String {
        match frame {
            Frame::ChannelData(data) => data.channel,
            other => format!("{other:?}"),
        }
    }

    /// `future`, within 5 s: a test that would hang fails.
    async fn within<T>(future: impl Future<Output = T>) -> T {
        let bounded = tokio::time::timeout(Duration::from_secs(5), future).await;
        bounded.expect("within 5 s")
    }

    /// Whether `future` still waits after 100 ms.
    async fn waits<T>(future: impl Future<Output = T>) -> bool {
        let bounded = tokio::time::timeout(Duration::from_millis(100), future).await;
        bounded.is_err()
    }

    /// One end of a link, started: its conversations the peer opens are
    /// handed to the test.
    type End = (Arc<Linked>, mpsc::UnboundedReceiver<Conversation>);

    /// Both ends of a link whose frames are at most `max` bytes, started.
    fn started(links: [Arc<dyn CarrierLink>; 2], max: usize) -> [End; 2] {
        let spawn: Spawn = Arc::new(|_, work| {
            tokio::spawn(work);
        });
        let end = |link, dialed| {
            let (handed, conversations) = mpsc::unbounded_channel();
            let handler: Handler = Arc::new(move |conversation| {
                let _ = handed.send(conversation);
                Box::pin(async {})
            });
            let linked = Linked::start(link, [0; 32], dialed, max, spawn.clone(), handler);
            (linked, conversations)
        };
        let [dialer, acceptor] = links;
        [end(dialer, true), end(acceptor, false)]
    }

    /// An in-memory link, started at both ends.
    fn in_memory() -> [End; 2] {
        let [dialer, acceptor] = pair([vec![1; 32], vec![2; 32]], None, 1 << 16);
        started([dialer, acceptor], 1 << 16)
    }

    /// Plan Step 4.5b, over an in-memory link: three conversations each way
    /// interleave, and each ends by its END while the others go on, the n-th
    /// after n + 1 frames. Each end hands the conversations its peer opens to
    /// a handler, which chooses them by their first frames: a pull by
    /// `Heads`, an interest by `Subscribe`, an exchange by `ExchangeReq`.
    #[tokio::test]
    async fn conversations_interleave_on_one_link_and_end_apart() {
        let [(a, mut from_b), (b, mut from_a)] = in_memory();
        let firsts = || {
            let (share, glade_id) = (String::from("s"), String::from("g"));
            let subscribe = Subscribe {
                share: share.clone(),
                glade_id: glade_id.clone(),
                key: None,
                from: None,
            };
            let (corr, payload) = ("c".into(), Vec::new());
            let exchange = ExchangeReq {
                share,
                glade_id,
                corr,
                payload,
            };
            let heads = Heads { streams: vec![] };
            [
                Frame::Heads(heads),
                Frame::Subscribe(subscribe),
                Frame::ExchangeReq(exchange),
            ]
        };
        let mut sending = Vec::new();
        for (end, name) in [(&a, "a"), (&b, "b")] {
            for (n, first) in firsts().iter().enumerate() {
                let conversation = end.open();
                conversation.send(first).unwrap();
                sending.push((format!("{name}{n}"), n, Some(conversation)));
            }
        }
        for round in 0..3 {
            for (name, n, conversation) in &mut sending {
                if round <= *n {
                    let frame = said(format!("{name}.{round}"));
                    conversation.as_ref().unwrap().send(&frame).unwrap();
                }
                if round == *n {
                    conversation.take().unwrap().end();
                }
            }
        }
        for (from, peer) in [(&mut from_a, "a"), (&mut from_b, "b")] {
            for (n, kind) in ["pull", "interest", "exchange"].into_iter().enumerate() {
                let mut conversation = within(from.recv()).await.unwrap();
                let chosen = match within(conversation.recv()).await.unwrap() {
                    Frame::Heads(_) => "pull",
                    Frame::Subscribe(_) => "interest",
                    Frame::ExchangeReq(_) => "exchange",
                    other => panic!("{peer}{n} opened with {other:?}"),
                };
                assert_eq!(chosen, kind, "{peer}{n}: handed in the order opened");
                let mut got = Vec::new();
                let ended = loop {
                    match within(conversation.recv()).await {
                        Ok(frame) => got.push(what(frame)),
                        Err(e) => break e,
                    }
                };
                assert_eq!(
                    ended.kind(),
                    io::ErrorKind::UnexpectedEof,
                    "{peer}{n}: {ended}"
                );
                let wanted: Vec<String> = (0..=n).map(|r| format!("{peer}{n}.{r}")).collect();
                assert_eq!(got, wanted, "{peer}{n}: its own frames, in order");
            }
        }
    }

    /// Plan Step 4.5b: a conversation dropped before its END is reset: the
    /// far end reads its frames, then an error, never a clean end.
    #[tokio::test]
    async fn a_conversation_dropped_before_its_end_is_reset() {
        let [(a, _), (_b, mut from_a)] = in_memory();
        let dropped = a.open();
        dropped.send(&said("before")).unwrap();
        drop(dropped);
        let mut far = within(from_a.recv()).await.unwrap();
        assert_eq!(what(within(far.recv()).await.unwrap()), "before");
        let reset = within(far.recv()).await.expect_err("more after a reset");
        assert_eq!(reset.kind(), io::ErrorKind::ConnectionReset, "{reset}");
    }

    /// Plan Step 4.5b, over two iroh ports on loopback: a conversation's task
    /// is aborted while the writer sends its 4 MiB frame, held past the
    /// peer's window, since the peer's reader waits on a full queue. The
    /// frame still arrives whole, then the conversation's reset, then another
    /// conversation's frame, whole: the link lives.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_conversation_cancelled_mid_send_never_ends_its_link() {
        let max = 8 << 20;
        let (a, _) = iroh_port(endpoint_key(), max).await;
        let (b, at_b) = iroh_port(endpoint_key(), max).await;
        let [(a_end, _), (_b_end, mut from_a)] = started(iroh_link(&a, &at_b, &b).await, max);
        let held = a_end.open();
        for n in 0..=QUEUE {
            held.send(&said(format!("held {n}"))).unwrap();
        }
        let mut holding = within(from_a.recv()).await.unwrap();
        let big = a_end.open();
        let sending = tokio::spawn(async move {
            let (channel, data) = ("big".into(), vec![7; 4 << 20]);
            big.send(&Frame::ChannelData(ChannelData { channel, data }))
                .unwrap();
            std::future::pending::<()>().await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        sending.abort();
        let _ = sending.await;
        let after = a_end.open();
        after.send(&said("after")).unwrap();
        for n in 0..=QUEUE {
            let got = what(within(holding.recv()).await.unwrap());
            assert_eq!(got, format!("held {n}"));
        }
        let mut big_far = within(from_a.recv()).await.unwrap();
        match within(big_far.recv()).await.unwrap() {
            Frame::ChannelData(data) => assert_eq!(data.data.len(), 4 << 20, "whole"),
            other => panic!("not the big frame: {other:?}"),
        }
        let reset = within(big_far.recv()).await.expect_err("its reset");
        assert_eq!(reset.kind(), io::ErrorKind::ConnectionReset, "{reset}");
        let mut after_far = within(from_a.recv()).await.unwrap();
        let got = what(within(after_far.recv()).await.unwrap());
        assert_eq!(got, "after", "the link lives");
    }

    /// Plan Step 4.5b: a full queue holds the reader, and every conversation
    /// behind it, until its conversation is read. A conversation its handler
    /// has let go holds nothing: its later frames are dropped, and the others
    /// flow.
    #[tokio::test]
    async fn a_slow_conversation_holds_its_queue_and_one_let_go_holds_nothing() {
        let [(a, _), (_b, mut from_a)] = in_memory();
        let slow = a.open();
        for n in 0..=QUEUE {
            slow.send(&said(format!("slow {n}"))).unwrap();
        }
        let other = a.open();
        other.send(&said("other")).unwrap();
        let mut slow_far = within(from_a.recv()).await.unwrap();
        assert!(
            waits(from_a.recv()).await,
            "the reader went past a full queue"
        );
        for n in 0..=QUEUE {
            let got = what(within(slow_far.recv()).await.unwrap());
            assert_eq!(got, format!("slow {n}"));
        }
        let mut other_far = within(from_a.recv()).await.unwrap();
        assert_eq!(what(within(other_far.recv()).await.unwrap()), "other");

        let gone = a.open();
        gone.send(&said("gone 0")).unwrap();
        drop(within(from_a.recv()).await.unwrap());
        for n in 1..=QUEUE + 1 {
            let _ = gone.send(&said(format!("gone {n}")));
        }
        let last = a.open();
        last.send(&said("last")).unwrap();
        let mut last_far = within(from_a.recv()).await.unwrap();
        let got = what(within(last_far.recv()).await.unwrap());
        assert_eq!(got, "last", "a conversation let go held the reader");
    }

    /// Plan Step 4.5b: the peer may hold a hundred conversations open at
    /// once. The hundred and first is reset at once and handed to no one,
    /// and the hundred held stay open.
    #[tokio::test]
    async fn the_hundred_and_first_conversation_is_reset() {
        let [(a, _), (_b, mut from_a)] = in_memory();
        let mut opened = Vec::new();
        for n in 0..=PEER_OPEN {
            let conversation = a.open();
            conversation.send(&said(format!("{n}"))).unwrap();
            opened.push(conversation);
        }
        let mut held = Vec::new();
        for _ in 0..PEER_OPEN {
            held.push(within(from_a.recv()).await.unwrap());
        }
        let mut last = opened.pop().unwrap();
        let reset = within(last.recv())
            .await
            .expect_err("an answer to the 101st");
        assert_eq!(reset.kind(), io::ErrorKind::ConnectionReset, "{reset}");
        assert!(waits(from_a.recv()).await, "the 101st was handed on");
        assert!(waits(opened[0].recv()).await, "a held conversation ended");
    }

    /// Plan Step 4.5b: the link's end ends every open conversation, at both
    /// ends, with an error, never a clean end, and nothing more is sent.
    #[tokio::test]
    async fn the_links_end_ends_every_conversation_with_an_error() {
        let [(a, _), (_b, mut from_a)] = in_memory();
        let mut mine = a.open();
        mine.send(&said("mine")).unwrap();
        let mut theirs = within(from_a.recv()).await.unwrap();
        assert_eq!(what(within(theirs.recv()).await.unwrap()), "mine");
        a.end();
        for (end, conversation) in [("the dialer's", &mut mine), ("the acceptor's", &mut theirs)] {
            let ended = within(conversation.recv()).await.expect_err(end);
            assert_eq!(
                ended.kind(),
                io::ErrorKind::ConnectionAborted,
                "{end}: {ended}"
            );
        }
        assert!(
            mine.send(&said("late")).is_err(),
            "sent after the link's end"
        );
    }
}
