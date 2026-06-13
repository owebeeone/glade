//! The glade node server (P1) — ties the store, router, and echo provider over
//! the websocket carrier. One connection per session; frames dispatched:
//! `Subscribe` registers interest and ships the resume gap, `Ops` appends +
//! fans out (minus origin) or returns an `Error`, and the directed
//! exchange/channel frames hit the echo provider. The resume/convergence and
//! verification logic all live in the carrier-free modules; this is the glue.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use glade_wire::generated::{Head, Heads, Ops, StreamHeads, Welcome};

use crate::echo::Echo;
use crate::frame::Frame;
use crate::router::{Router, SessionId};
use crate::session::{error_frame, heads_map, missing_for};
use crate::store::{Append, Store, StoreError};
use crate::ws::{self, Msg};

struct Shared {
    store: Mutex<Store>,
    router: Mutex<Router>,
    out: Mutex<BTreeMap<SessionId, mpsc::UnboundedSender<Vec<u8>>>>,
    next: AtomicU64,
}

/// A glade node bound to a store directory.
pub struct Server {
    shared: Arc<Shared>,
}

impl Server {
    pub fn open(root: impl Into<PathBuf>) -> std::io::Result<Server> {
        let store = Store::open(root).map_err(to_io)?;
        Ok(Server {
            shared: Arc::new(Shared {
                store: Mutex::new(store),
                router: Mutex::new(Router::new()),
                out: Mutex::new(BTreeMap::new()),
                next: AtomicU64::new(1),
            }),
        })
    }

    /// Accept connections until the listener errors.
    pub async fn run(self, listener: TcpListener) -> std::io::Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            let shared = self.shared.clone();
            tokio::spawn(async move {
                let _ = handle(shared, stream).await;
            });
        }
    }
}

async fn send(shared: &Arc<Shared>, sid: SessionId, frame: &Frame) {
    let tx = shared.out.lock().await.get(&sid).cloned();
    if let Some(tx) = tx {
        let _ = tx.send(frame.to_bytes());
    }
}

async fn handle(shared: Arc<Shared>, stream: TcpStream) -> std::io::Result<()> {
    let (mut reader, writer) = ws::accept(stream).await?;
    let sid = shared.next.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    shared.out.lock().await.insert(sid, tx);

    // writer task: drain this session's outbound onto the socket.
    let wtask = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if writer.send_binary(&bytes).await.is_err() {
                break;
            }
        }
    });

    let mut echo = Echo::new();
    let mut client_heads: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();

    loop {
        let bytes = match reader.read().await {
            Ok(Msg::Binary(b)) => b,
            _ => break, // close or error
        };
        let frame = match Frame::from_bytes(&bytes) {
            Ok(f) => f,
            Err(_) => continue,
        };

        // directed frames -> echo provider (not replicated)
        if matches!(
            frame,
            Frame::ExchangeReq(_) | Frame::ChannelOpen(_) | Frame::ChannelData(_) | Frame::ChannelClose(_)
        ) {
            for out in echo.handle(&frame) {
                send(&shared, sid, &out).await;
            }
            continue;
        }

        match frame {
            Frame::Hello(h) => {
                for sh in &h.heads {
                    let m = client_heads.entry(sh.share.clone()).or_default();
                    for hd in &sh.heads {
                        m.insert(hd.origin.clone(), hd.seq);
                    }
                }
                send(&shared, sid, &Frame::Welcome(Welcome { session: h.session, protocol: 1, heads: vec![] })).await;
            }
            Frame::Subscribe(s) => {
                shared.router.lock().await.subscribe(sid, &s.share, &s.glade_id);
                let their = client_heads.get(&s.share).cloned().unwrap_or_default();
                let (server_heads, gap) = {
                    let st = shared.store.lock().await;
                    (heads_map(&st, &s.share), missing_for(&st, &s.share, &their))
                };
                let ack = Frame::Heads(Heads {
                    streams: vec![StreamHeads {
                        share: s.share.clone(),
                        glade_id: String::new(),
                        key: vec![],
                        heads: server_heads
                            .iter()
                            .map(|(o, sq)| Head { origin: o.clone(), seq: *sq, hash: None })
                            .collect(),
                    }],
                });
                send(&shared, sid, &ack).await;
                if !gap.is_empty() {
                    send(&shared, sid, &Frame::Ops(Ops { ops: gap, pri: None })).await;
                }
            }
            Frame::Ops(ops) => {
                for op in ops.ops {
                    let (share, glade_id) = (op.share.clone(), op.glade_id.clone());
                    client_heads.entry(share.clone()).or_default().insert(op.origin.clone(), op.seq);
                    let res = shared.store.lock().await.append(op.clone());
                    match res {
                        Ok(Append::Appended) => {
                            let targets = shared.router.lock().await.route(sid, &share, &glade_id);
                            let frame = Frame::Ops(Ops { ops: vec![op], pri: None });
                            for t in targets {
                                send(&shared, t, &frame).await;
                            }
                        }
                        Ok(Append::Duplicate) => {}
                        Err(e) => send(&shared, sid, &error_frame(&e, &share, &glade_id)).await,
                    }
                }
            }
            _ => {}
        }
    }

    shared.out.lock().await.remove(&sid);
    shared.router.lock().await.unsubscribe_all(sid);
    wtask.abort();
    Ok(())
}

fn to_io(e: StoreError) -> std::io::Error {
    match e {
        StoreError::Io(e) => e,
        other => std::io::Error::new(std::io::ErrorKind::Other, format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::generated::{ExchangeReq, Op, Ops, Shape, Subscribe};

    fn op(origin: &str, seq: i64, payload: &[u8]) -> Op {
        Op {
            share: "sh".into(),
            glade_id: "g".into(),
            key: vec![],
            origin: origin.into(),
            seq,
            prev: None,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }
    fn subscribe() -> Vec<u8> {
        Frame::Subscribe(Subscribe { share: "sh".into(), glade_id: "g".into(), key: None, from: None }).to_bytes()
    }
    async fn recv(r: &mut ws::WsReader) -> Frame {
        match r.read().await.unwrap() {
            Msg::Binary(b) => Frame::from_bytes(&b).unwrap(),
            Msg::Close => panic!("unexpected close"),
        }
    }

    /// §11 localhost role end-to-end over a real websocket: two clients exchange
    /// an op (routing), a late joiner resumes the op (gap-ship), and the echo
    /// provider round-trips an exchange.
    #[tokio::test]
    async fn end_to_end_over_websocket() {
        let dir = std::env::temp_dir().join("glade-server-e2e");
        let _ = std::fs::remove_dir_all(&dir);
        let server = Server::open(&dir).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));

        let (mut r1, w1) = ws::connect("127.0.0.1", port).await.unwrap();
        let (mut r2, w2) = ws::connect("127.0.0.1", port).await.unwrap();

        // both subscribe; the Heads ack confirms registration (ordering point)
        w1.send_binary(&subscribe()).await.unwrap();
        assert!(matches!(recv(&mut r1).await, Frame::Heads(_)));
        w2.send_binary(&subscribe()).await.unwrap();
        assert!(matches!(recv(&mut r2).await, Frame::Heads(_)));

        // client 1 writes an op; client 2 receives it (fan-out minus origin)
        w1.send_binary(&Frame::Ops(Ops { ops: vec![op("a", 0, b"hello")], pri: None }).to_bytes())
            .await
            .unwrap();
        match recv(&mut r2).await {
            Frame::Ops(o) => assert_eq!(o.ops[0].payload, b"hello"),
            other => panic!("client 2 expected Ops, got {other:?}"),
        }

        // a late joiner subscribes and resumes the op from history (gap-ship)
        let (mut r3, w3) = ws::connect("127.0.0.1", port).await.unwrap();
        w3.send_binary(&subscribe()).await.unwrap();
        assert!(matches!(recv(&mut r3).await, Frame::Heads(_)));
        match recv(&mut r3).await {
            Frame::Ops(o) => assert_eq!(o.ops[0].payload, b"hello"),
            other => panic!("client 3 expected resume Ops, got {other:?}"),
        }

        // echo provider: exchange round-trips with its correlation id
        w1.send_binary(
            &Frame::ExchangeReq(ExchangeReq {
                share: "sh".into(),
                glade_id: "echo".into(),
                corr: "x1".into(),
                payload: b"ping".to_vec(),
            })
            .to_bytes(),
        )
        .await
        .unwrap();
        match recv(&mut r1).await {
            Frame::ExchangeRes(res) => {
                assert_eq!(res.corr, "x1");
                assert_eq!(res.payload.as_deref(), Some(b"ping".as_slice()));
            }
            other => panic!("client 1 expected ExchangeRes, got {other:?}"),
        }
    }
}
