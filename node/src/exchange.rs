//! Directed exchange routing (Lane R step 4) — discovery.ts phase D + the
//! s-fanout-exchange asymmetry: OPS can be served by any replica of a stream;
//! an EXCHANGE must reach the claim-holding authority. The replica answers
//! "what is"; only the authority answers "do".
//!
//! An exchange surface is DECLARED data: a `dir.services` record, or a
//! `dir.bindings` record with shape `exchange` (both registered from an
//! `<app>.glade` file — `appdecl.rs`). An authority provider session attaches
//! by SUBSCRIBE-ing to the declared `(share, glade_id)`; the node routes each
//! `ExchangeReq` by the same C2 decision a subscribe gets (local provider /
//! forward to the claim holder / absent), 1:1 by correlation id, never folded,
//! never cached. Every failure arm answers `ExchangeRes{ok:false}` with the
//! reason — data, not a hang (the phase-E posture). Undeclared glade ids keep
//! the legacy echo provider, byte-for-byte.

use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{RecvStream, SendStream};

use glade_wire::generated::{ExchangeReq, ExchangeRes, Heads, StreamHeads};

use crate::echo::Echo;
use crate::frame::Frame;
use crate::mesh::{route_subscribe, Route};
use crate::peer::{read_frame, write_frame};
use crate::registry::{G_BINDINGS, G_SERVICES, HOME};
use crate::router::SessionId;
use crate::server::{send, Shared};
use crate::store::Store;
use crate::sysdata::{BindingDecl, ServiceDefinition};

/// How long the claim holder waits on its attached provider.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the requesting node waits on the claim holder — longer than
/// [`PROVIDER_TIMEOUT`] so the holder's own timeout answer arrives as data.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(12);

fn other<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// `ExchangeRes{ok:false}` carrying the reason — failure as data, corr intact.
fn res_err(corr: &str, error: &str) -> Frame {
    Frame::ExchangeRes(ExchangeRes {
        corr: corr.into(),
        ok: false,
        payload: None,
        error: Some(error.into()),
    })
}

/// Is `glade_id` a DECLARED exchange surface? A fold over the registered app
/// declarations in the local replica — base glade reads records, not apps.
pub fn declared_exchange(store: &Store, glade_id: &str) -> bool {
    for (origin, _) in store.heads(HOME, G_SERVICES, &[]) {
        for op in store.scan(HOME, G_SERVICES, &[], &origin, i64::MIN) {
            if ServiceDefinition::from_cbor(&glade_wire::cbor::decode(&op.payload)).glade_id == glade_id {
                return true;
            }
        }
    }
    for (origin, _) in store.heads(HOME, G_BINDINGS, &[]) {
        for op in store.scan(HOME, G_BINDINGS, &[], &origin, i64::MIN) {
            let b = BindingDecl::from_cbor(&glade_wire::cbor::decode(&op.payload));
            if b.glade_id == glade_id && b.shape == "exchange" {
                return true;
            }
        }
    }
    false
}

/// An authority provider attaches: a SUBSCRIBE to a declared exchange surface
/// registers the session as THE provider for `(share, glade_id)` and acks with
/// empty `Heads` (exchanges are never replicated — there is no gap to ship).
/// The keyed entry map IS the routing table, applied to the directed leg.
pub(crate) async fn attach_provider(shared: &Arc<Shared>, sid: SessionId, share: &str, glade_id: &str, key: Vec<u8>) {
    shared.providers.lock().await.insert((share.into(), glade_id.into()), sid);
    let ack = Frame::Heads(Heads {
        streams: vec![StreamHeads { share: share.into(), glade_id: glade_id.into(), key, heads: vec![] }],
    });
    send(shared, sid, &ack).await;
}

/// Route one inbound `ExchangeReq` from session `sid` (trace D1/D2 · X1):
/// undeclared → the legacy echo provider; declared → the C2 decision on the
/// SHARE, and the replica never answers regardless of what it caches.
pub(crate) async fn handle_request(shared: &Arc<Shared>, sid: SessionId, req: ExchangeReq, echo: &mut Echo) {
    let declared = {
        let st = shared.store.lock().await;
        declared_exchange(&st, &req.glade_id)
    };
    if !declared {
        // the pre-R4 contract, byte-for-byte (echo answers on this session).
        for out in echo.handle(&Frame::ExchangeReq(req)) {
            send(shared, sid, &out).await;
        }
        return;
    }
    match route_subscribe(shared, &req.share).await {
        Route::Local => {
            let provider =
                shared.providers.lock().await.get(&(req.share.clone(), req.glade_id.clone())).copied();
            match provider {
                Some(psid) => {
                    // corr → requester; the provider's ExchangeRes routes back
                    // through handle_response. Corr preserved 1:1 (trace D2).
                    shared.pending.lock().await.insert(req.corr.clone(), sid);
                    send(shared, psid, &Frame::ExchangeReq(req)).await;
                }
                None => {
                    let reason =
                        format!("no authority provider attached for {}/{}", req.share, req.glade_id);
                    send(shared, sid, &res_err(&req.corr, &reason)).await;
                }
            }
        }
        Route::Forward(peer) => {
            let shared = shared.clone();
            tokio::spawn(async move {
                forward_exchange(&shared, peer, req, sid).await;
            });
        }
        Route::Absent(reason) => {
            // no live claim / holder unreachable: bounded, immediate, data —
            // the exchange twin of the subscribe path's Error/UnknownShare.
            send(shared, sid, &res_err(&req.corr, &reason)).await;
        }
    }
}

/// An inbound `ExchangeRes` (the attached provider answering): resolve the
/// pending correlation and deliver to the recorded requester (trace D4/D5).
pub(crate) async fn handle_response(shared: &Arc<Shared>, res: ExchangeRes) {
    let target = shared.pending.lock().await.remove(&res.corr);
    if let Some(t) = target {
        send(shared, t, &Frame::ExchangeRes(res)).await;
    } // unknown corr: dropped — never folded, never broadcast
}

/// The requesting node's Forward arm: one fresh stream on the claim holder's
/// link carries exactly one exchange; the response (or the bounded failure)
/// is delivered to the requester as an `ExchangeRes`.
async fn forward_exchange(shared: &Arc<Shared>, peer: String, req: ExchangeReq, requester: SessionId) {
    let corr = req.corr.clone();
    let frame = match try_forward(shared, &peer, req).await {
        Ok(res) => Frame::ExchangeRes(res),
        Err(e) => res_err(&corr, &format!("exchange to claim holder failed: {e}")),
    };
    send(shared, requester, &frame).await;
}

async fn try_forward(shared: &Arc<Shared>, peer: &str, req: ExchangeReq) -> io::Result<ExchangeRes> {
    let mesh = shared.mesh.get().cloned().ok_or_else(|| other("mesh not enabled"))?;
    let conn = mesh.links.lock().await.get(peer).cloned().ok_or_else(|| other("no live peer link"))?;
    let (mut qsend, mut recv) = conn.open_bi().await.map_err(other)?;
    write_frame(&mut qsend, &Frame::ExchangeReq(req)).await?;
    let frame = tokio::time::timeout(FORWARD_TIMEOUT, read_frame(&mut recv))
        .await
        .map_err(|_| other("timeout awaiting ExchangeRes from claim holder"))??;
    match frame {
        Frame::ExchangeRes(res) => Ok(res),
        got => Err(other(format!("expected ExchangeRes, got {got:?}"))),
    }
}

/// The claim holder's side of a forwarded exchange (trace D2→D4): a synthetic
/// session whose outbound IS the stream, so the ordinary request/response
/// plumbing (provider lookup, pending map) serves the peer unchanged. One
/// stream, one exchange, close.
pub(crate) async fn serve_peer_exchange(
    shared: Arc<Shared>,
    mut qsend: SendStream,
    _recv: RecvStream,
    req: ExchangeReq,
) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let corr = req.corr.clone();
    let sid = shared.next.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    shared.out.lock().await.insert(sid, tx);

    let mut echo = Echo::new(); // undeclared ids keep the echo answer even here
    handle_request(&shared, sid, req, &mut echo).await;
    let bytes = match tokio::time::timeout(PROVIDER_TIMEOUT, rx.recv()).await {
        Ok(Some(b)) => b,
        _ => res_err(&corr, "provider timeout at claim holder").to_bytes(),
    };

    shared.out.lock().await.remove(&sid);
    // a never-answered pending entry must not leak (nor swallow a corr reuse):
    let mut pending = shared.pending.lock().await;
    if pending.get(&corr) == Some(&sid) {
        pending.remove(&corr);
    }
    drop(pending);

    qsend.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    qsend.write_all(&bytes).await?;
    qsend.flush().await?;
    qsend.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appdecl;
    use crate::frame::Frame;
    use crate::iroh_carrier::PeerEndpoint;
    use crate::registry::{Record, Registry, RegistryApi, G_BINDINGS, G_GRANTS};
    use crate::server::Server;
    use crate::sysdata::{CapabilityGrant, ServeClaim, WorkspaceEntry};
    use crate::sysdir::{boot_at, now_ms};
    use crate::ws;
    use glade_wire::generated::{Op, Ops, Shape, Subscribe};
    use std::path::PathBuf;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-exchange-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn grazel_decl() -> appdecl::AppDecl {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../apps/grazel-app.glade");
        appdecl::load(path).unwrap()
    }

    fn sub(share: &str, glade_id: &str) -> Vec<u8> {
        Frame::Subscribe(Subscribe { share: share.into(), glade_id: glade_id.into(), key: None, from: None })
            .to_bytes()
    }

    fn xreq(share: &str, glade_id: &str, corr: &str, payload: &[u8]) -> Vec<u8> {
        Frame::ExchangeReq(ExchangeReq {
            share: share.into(),
            glade_id: glade_id.into(),
            corr: corr.into(),
            payload: payload.to_vec(),
        })
        .to_bytes()
    }

    /// Read the next frame, bounded — a hang is a failure (failure surfaces as
    /// data, never silence).
    async fn next_frame(r: &mut ws::WsReader, what: &str) -> Frame {
        let msg = tokio::time::timeout(Duration::from_secs(5), r.read())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .unwrap();
        match msg {
            ws::Msg::Binary(b) => Frame::from_bytes(&b).unwrap(),
            _ => panic!("unexpected close waiting for {what}"),
        }
    }

    /// The LOCAL leg on one node: a declared exchange surface routes to the
    /// attached authority provider (corr preserved 1:1, response routed back),
    /// answers `ok:false` data BEFORE any provider attaches, and an UNDECLARED
    /// glade id keeps the legacy echo answer byte-for-byte.
    #[tokio::test]
    async fn local_provider_round_trip_absence_and_echo_fallback() {
        // a non-grazel app: the routing is app-agnostic.
        let decl = appdecl::parse(
            "glade-app v0\napp demo\nservice demo d.ops\n",
        )
        .unwrap();
        let mut reg = Registry::new();
        appdecl::register(&decl, &mut reg, "n1").unwrap();

        let server = Server::open(fresh("local-store")).unwrap();
        server.seed_registry(&reg.snapshot()).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));

        let (mut rc, wc) = ws::connect("127.0.0.1", port).await.unwrap(); // requester

        // (1) declared surface, nobody attached -> failure as data, bounded.
        wc.send_binary(&xreq("s", "d.ops", "c0", b"status")).await.unwrap();
        match next_frame(&mut rc, "no-provider answer").await {
            Frame::ExchangeRes(res) => {
                assert_eq!(res.corr, "c0");
                assert!(!res.ok);
                assert!(res.error.unwrap().contains("no authority provider"), "reason rides the response");
            }
            other => panic!("expected ExchangeRes, got {other:?}"),
        }

        // (2) the authority provider attaches: SUBSCRIBE to the declared surface.
        let (mut rp, wp) = ws::connect("127.0.0.1", port).await.unwrap();
        wp.send_binary(&sub("s", "d.ops")).await.unwrap();
        assert!(matches!(next_frame(&mut rp, "provider attach ack").await, Frame::Heads(_)));

        // (3) request -> provider (corr + payload intact) -> response -> requester.
        wc.send_binary(&xreq("s", "d.ops", "c1", b"workspace.status")).await.unwrap();
        match next_frame(&mut rp, "provider receives the request").await {
            Frame::ExchangeReq(req) => {
                assert_eq!((req.corr.as_str(), req.payload.as_slice()), ("c1", b"workspace.status".as_slice()));
            }
            other => panic!("provider expected ExchangeReq, got {other:?}"),
        }
        wp.send_binary(
            &Frame::ExchangeRes(ExchangeRes {
                corr: "c1".into(),
                ok: true,
                payload: Some(b"12 clean".to_vec()),
                error: None,
            })
            .to_bytes(),
        )
        .await
        .unwrap();
        match next_frame(&mut rc, "requester receives the response").await {
            Frame::ExchangeRes(res) => {
                assert!(res.ok);
                assert_eq!(res.corr, "c1");
                assert_eq!(res.payload.as_deref(), Some(b"12 clean".as_slice()));
            }
            other => panic!("requester expected ExchangeRes, got {other:?}"),
        }

        // (4) an UNDECLARED glade id still gets the legacy echo answer.
        wc.send_binary(&xreq("s", "echo", "c2", b"ping")).await.unwrap();
        match next_frame(&mut rc, "echo fallback").await {
            Frame::ExchangeRes(res) => {
                assert!(res.ok);
                assert_eq!(res.corr, "c2");
                assert_eq!(res.payload.as_deref(), Some(b"ping".as_slice()));
            }
            other => panic!("expected echoed ExchangeRes, got {other:?}"),
        }
    }

    fn tree_op(seq: i64, prev: Option<Vec<u8>>, payload: &[u8]) -> Op {
        Op {
            share: "ws-razel".into(),
            glade_id: "ws.tree".into(),
            key: vec![],
            origin: "grazel-b".into(),
            seq,
            prev,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }

    /// The grazel-attach E2E — the final stage-1 builder. Two booted nodes over
    /// real iroh + real websockets; grazel-app.glade LOADED as data on B:
    ///
    ///   (a) the registered declarations + compiled ACL-seed grants appear at
    ///       node A as ordinary records via directory subscriptions (s-app-
    ///       register RL/RC/RM — reads are subscriptions, no privileged plane);
    ///   (b) a client subscribing a DECLARED grazel surface (ws.tree) is served
    ///       by the authority through the ordinary routed path, ops converging
    ///       end to end (discovery C);
    ///   (c) a gwz exchange (gwz.ops) round-trips: A routes it to the claim
    ///       holder B — never answered from A's replica (fan-out asymmetry) —
    ///       B's attached grazel provider answers, corr preserved 1:1 (D);
    ///   (d) an exchange against a share with no live claim answers bounded
    ///       `ok:false` data with the reason, and the session stays usable (E).
    #[tokio::test(flavor = "multi_thread")]
    async fn grazel_attach_end_to_end() {
        // ---- node B: boot + LOAD grazel-app.glade + claim its workspace -----
        let mut boot_b = boot_at(fresh("e2e-b-sys"), "gianni").unwrap();
        let b_id = boot_b.node_id.clone();
        let loaded = appdecl::register(&grazel_decl(), &mut boot_b.registry, &b_id).unwrap();
        assert_eq!(loaded.appended, 7, "4 bindings + 1 service + 2 seeds registered");
        boot_b
            .registry
            .append(
                Record::Workspace(WorkspaceEntry { workspace: "ws-razel".into(), name: "razel".into(), eligible_hosts: vec![b_id.clone()] }),
                &b_id,
            )
            .unwrap();
        boot_b
            .registry
            .append(
                Record::Serve(ServeClaim { node: b_id.clone(), share: "ws-razel".into(), lease_expiry_ms: now_ms() + 30_000, epoch: 1 }),
                &b_id,
            )
            .unwrap();
        // the sleeping share for (d): known to the directory, claim LAPSED.
        boot_b
            .registry
            .append(
                Record::Serve(ServeClaim { node: "attic-mini".into(), share: "ws-attic".into(), lease_expiry_ms: now_ms() - 1_000, epoch: 1 }),
                &b_id,
            )
            .unwrap();

        let boot_a = boot_at(fresh("e2e-a-sys"), "gianni").unwrap();

        let a = Server::open(fresh("e2e-a-store")).unwrap();
        let b = Server::open(fresh("e2e-b-store")).unwrap();
        a.seed_registry(&boot_a.registry.snapshot()).await;
        b.seed_registry(&boot_b.registry.snapshot()).await;

        let ep_a = PeerEndpoint::bind_with(boot_a.identity().unwrap()).await.unwrap();
        let ep_b = PeerEndpoint::bind_with(boot_b.identity().unwrap()).await.unwrap();
        a.enable_mesh(ep_a).await.unwrap();
        let addr_b = b.enable_mesh(ep_b).await.unwrap();
        a.connect_peer(&addr_b).await.unwrap();

        let lis_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let lis_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (port_a, port_b) = (lis_a.local_addr().unwrap().port(), lis_b.local_addr().unwrap().port());
        tokio::spawn(a.run(lis_a));
        tokio::spawn(b.run(lis_b));

        // ---- the grazel authority session on B (trace C4): one session ------
        // attaches as gwz.ops provider AND appends the ws.tree binding content.
        let (mut rp, wp) = ws::connect("127.0.0.1", port_b).await.unwrap();
        wp.send_binary(&sub("ws-razel", "gwz.ops")).await.unwrap();
        assert!(matches!(next_frame(&mut rp, "grazel attach ack").await, Frame::Heads(_)));
        let o0 = tree_op(0, None, b"tree-v0");
        let o1 = tree_op(1, Some(crate::chain::op_hash(&o0).to_vec()), b"tree-v1");
        wp.send_binary(&Frame::Ops(Ops { ops: vec![o0, o1], pri: None }).to_bytes()).await.unwrap();

        // ---- (a) registered surfaces appear at A as ordinary records --------
        let (mut rc, wc) = ws::connect("127.0.0.1", port_a).await.unwrap();
        wc.send_binary(&sub(HOME, G_BINDINGS)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "dir.bindings ack").await, Frame::Heads(_)));
        let mut bindings = Vec::new();
        while bindings.len() < 4 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "BindingDecl records").await {
                for op in ops.ops {
                    assert_eq!(op.origin, b_id, "declarations ride the registrant's chain");
                    bindings.push(BindingDecl::from_cbor(&glade_wire::cbor::decode(&op.payload)).glade_id);
                }
            }
        }
        bindings.sort();
        assert_eq!(bindings, vec!["term.log", "ws.diff", "ws.files", "ws.tree"]);
        // the ACL seeds compiled to ORDINARY grant records (s-app-register A5).
        wc.send_binary(&sub(HOME, G_GRANTS)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "dir.grants ack").await, Frame::Heads(_)));
        let mut grants = Vec::new();
        while grants.len() < 2 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "seeded grant records").await {
                for op in ops.ops {
                    let g = CapabilityGrant::from_cbor(&glade_wire::cbor::decode(&op.payload));
                    grants.push((g.principal, g.share, g.verbs.join(",")));
                }
            }
        }
        grants.sort();
        assert_eq!(
            grants,
            vec![
                ("owner".to_string(), "grazel".to_string(), "gwz.*".to_string()),
                ("owner".to_string(), "grazel".to_string(), "read.*".to_string()),
            ]
        );

        // ---- (b) the declared binding is served end to end ------------------
        wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "ws.tree ack").await, Frame::Heads(_)));
        let mut payloads = Vec::new();
        while payloads.len() < 2 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "routed tree ops").await {
                payloads.extend(ops.ops.into_iter().map(|o| o.payload));
            }
        }
        assert_eq!(payloads, vec![b"tree-v0".to_vec(), b"tree-v1".to_vec()], "authority content converges in order");

        // ---- (c) the gwz exchange round-trips through the authority ---------
        wc.send_binary(&xreq("ws-razel", "gwz.ops", "x-42", b"workspace.status")).await.unwrap();
        match next_frame(&mut rp, "grazel receives the forwarded exchange").await {
            Frame::ExchangeReq(req) => {
                assert_eq!(req.corr, "x-42", "correlation id preserved 1:1 across the hop");
                assert_eq!(req.payload, b"workspace.status");
            }
            other => panic!("grazel expected ExchangeReq, got {other:?}"),
        }
        wp.send_binary(
            &Frame::ExchangeRes(ExchangeRes {
                corr: "x-42".into(),
                ok: true,
                payload: Some(b"12 clean, 1 dirty".to_vec()),
                error: None,
            })
            .to_bytes(),
        )
        .await
        .unwrap();
        match next_frame(&mut rc, "exchange response at the requester").await {
            Frame::ExchangeRes(res) => {
                assert!(res.ok);
                assert_eq!(res.corr, "x-42");
                assert_eq!(res.payload.as_deref(), Some(b"12 clean, 1 dirty".as_slice()));
            }
            other => panic!("requester expected ExchangeRes, got {other:?}"),
        }

        // ---- (d) missing/unclaimed target: bounded failure as data ----------
        wc.send_binary(&xreq("ws-attic", "gwz.ops", "x-43", b"workspace.status")).await.unwrap();
        match next_frame(&mut rc, "ws-attic exchange status").await {
            Frame::ExchangeRes(res) => {
                assert_eq!(res.corr, "x-43");
                assert!(!res.ok);
                assert!(res.error.unwrap().contains("no live ServeClaim"), "the reason rides the response");
            }
            other => panic!("expected ExchangeRes failure data, got {other:?}"),
        }
        // failure is data, not a dead session: the next ask still answers.
        wc.send_binary(&sub(HOME, crate::registry::G_CLAIMS)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "post-failure ack").await, Frame::Heads(_)));
    }
}
