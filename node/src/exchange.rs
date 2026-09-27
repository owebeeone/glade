//! Directed exchange routing (Lane R step 4) — discovery.ts phase D + the
//! s-fanout-exchange asymmetry: OPS can be served by any replica of a stream;
//! an EXCHANGE must reach the claim-holding authority. The replica answers
//! "what is"; only the authority answers "do".
//!
//! An exchange surface is DECLARED data: a `dir.services` record, or a live
//! `dir.bindings` declaration with shape `exchange` — live by the binding fold
//! (R9(a): newest wins, a retraction takes it down) — both registered from an
//! `<app>.glade` file (`appdecl.rs`). An authority provider session attaches
//! by SUBSCRIBE-ing to the declared `(share, glade_id)`; the node routes each
//! `ExchangeReq` by the same C2 decision a subscribe gets (local provider /
//! forward to the claim holder / absent), 1:1 by correlation id, never folded,
//! never cached. Every failure arm answers `ExchangeRes{ok:false}` with the
//! reason — data, not a hang (the phase-E posture). Undeclared glade ids keep
//! the legacy echo provider, byte-for-byte.

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use glade_grant_api::{GrantPort, Holder};
use glade_wire::generated::{ExchangeReq, ExchangeRes, Heads, StreamHeads};
use glade_wire::wellformed;

use crate::conversation::Conversation;
use crate::echo::Echo;
use crate::envelope;
use crate::frame::Frame;
use crate::grants::refusal;
use crate::mesh::{route_subscribe, Route};
use crate::registry::{BindingFold, G_BINDINGS, G_BINDING_RETRACTIONS, G_SERVICES, HOME};
use crate::router::SessionId;
use crate::server::{send, Shared};
use crate::store::Store;
use crate::sysdata::{ServiceDefinition, WorkspaceCreateReq};
use crate::tasks::Site;

/// The reserved built-in create surface (s-create D1–D3, audit F2): a system
/// glade id the NODE answers itself, never a supplier — creation precedes
/// claims, so it cannot ride claim routing. Reserved like the `dir.*` ids.
pub const WORKSPACE_CREATE: &str = "workspace.create";

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
/// `dir.bindings` is read through the binding fold, so only a LIVE
/// declaration counts: a superseded or retracted one keeps nothing routable.
pub fn declared_exchange(store: &Store, glade_id: &str) -> bool {
    for (origin, _) in store.heads(HOME, G_SERVICES, &[]) {
        for op in store.scan(HOME, G_SERVICES, &[], &origin, i64::MIN) {
            let service = envelope::folded(&op, ServiceDefinition::from_cbor);
            if service.is_some_and(|service| service.glade_id == glade_id) {
                return true;
            }
        }
    }
    let mut family = Vec::new();
    for stream in [G_BINDINGS, G_BINDING_RETRACTIONS] {
        for (origin, _) in store.heads(HOME, stream, &[]) {
            family.extend(store.scan(HOME, stream, &[], &origin, i64::MIN));
        }
    }
    BindingFold::over(&family).live().iter().any(|b| b.glade_id == glade_id && b.shape == "exchange")
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

/// The calls handed to an attached provider and not yet answered, in the
/// exchange's shared state (F16). Each is filed under a correlation this node
/// mints, unique within the node: callers number their calls alike (every
/// client counts from `c1`), so under theirs two calls in flight on one
/// exchange would meet, and one caller would receive the other's reply.
#[derive(Default)]
pub(crate) struct Pending {
    /// How many correlations this node has minted.
    minted: u64,
    /// Minted correlation -> the caller's session and its own correlation.
    calls: BTreeMap<String, (SessionId, String)>,
}

impl Pending {
    /// File `sid`'s call `corr`, returning the correlation the handler sees.
    fn file(&mut self, sid: SessionId, corr: String) -> String {
        self.minted += 1;
        let minted = format!("n{}", self.minted);
        self.calls.insert(minted.clone(), (sid, corr));
        minted
    }

    /// The caller a reply on `minted` answers, filed no longer: `None` for a
    /// correlation this node never minted, or whose call is already answered.
    fn answered(&mut self, minted: &str) -> Option<(SessionId, String)> {
        self.calls.remove(minted)
    }

    /// Forget every call `sid` filed: its session has ended.
    fn forget(&mut self, sid: SessionId) {
        self.calls.retain(|_, (caller, _)| *caller != sid);
    }
}

/// Route one inbound `ExchangeReq` from session `sid` (trace D1/D2 · X1):
/// the reserved `workspace.create` id → the built-in TARGET-routed handler;
/// undeclared → the legacy echo provider; declared → the C2 decision on the
/// SHARE, and the replica never answers regardless of what it caches.
pub(crate) async fn handle_request(shared: &Arc<Shared>, sid: SessionId, mut req: ExchangeReq, echo: &mut Echo) {
    if req.glade_id == WORKSPACE_CREATE {
        handle_create(shared, sid, req).await;
        return;
    }
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
                    // the handler sees a correlation this node minted, never
                    // the caller's, and echoes it 1:1; handle_response routes
                    // its ExchangeRes back on it (trace D2, F16).
                    req.corr = shared.pending.lock().await.file(sid, req.corr);
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
            let forward = shared.clone();
            shared.tasks.spawn(Site::ForwardExchange, async move {
                forward_exchange(&forward, peer, req, sid).await;
            });
        }
        Route::Absent(reason) => {
            // no live claim / holder unreachable: bounded, immediate, data —
            // the exchange twin of the subscribe path's Error/UnknownShare.
            send(shared, sid, &res_err(&req.corr, &reason)).await;
        }
    }
}

/// Route one `workspace.create` exchange (s-create D1–D3, audit F2). The
/// request names its TARGET node IN THE PAYLOAD (`WorkspaceCreateReq` — the
/// wire is untouched; the target rides the opaque exchange payload): creation
/// is the one routed operation that cannot consult a ServeClaim, because it
/// MAKES the thing claims will be about. Target == self → perform locally
/// (mint entry + claim under our own origin, `claims::create_workspace`);
/// target == a linked peer → forward the frame unchanged over the peer link
/// (corr preserved 1:1; `serve_peer_exchange` at the target re-enters here and
/// hits the self arm); anything else → `ExchangeRes{ok:false}` with the
/// reason — an unlinked target fails as DATA, never a hang.
async fn handle_create(shared: &Arc<Shared>, sid: SessionId, req: ExchangeReq) {
    if req.payload.is_empty() {
        send(shared, sid, &res_err(&req.corr, "workspace.create needs a WorkspaceCreateReq payload")).await;
        return;
    }
    let Some(create) = create_request(shared, sid, &req).await else {
        return;
    };
    if create.workspace.is_empty() || create.target.is_empty() {
        send(shared, sid, &res_err(&req.corr, "workspace.create needs {workspace, target}")).await;
        return;
    }
    let Some(mesh) = shared.mesh.get() else {
        send(shared, sid, &res_err(&req.corr, "workspace.create requires a booted node (no mesh)")).await;
        return;
    };
    if create.target == mesh.self_id {
        let frame = match crate::claims::create_workspace(shared, &create).await {
            Ok(res) => Frame::ExchangeRes(ExchangeRes {
                corr: req.corr.clone(),
                ok: true,
                payload: Some(glade_wire::cbor::encode(&res.to_cbor())),
                error: None,
            }),
            Err(e) => res_err(&req.corr, &format!("create failed at target: {e}")),
        };
        send(shared, sid, &frame).await;
        return;
    }
    if mesh.links.lock().await.contains_key(&create.target) {
        let (forward, peer) = (shared.clone(), create.target.clone());
        shared.tasks.spawn(Site::ForwardCreate, async move {
            forward_exchange(&forward, peer, req, sid).await;
        });
    } else {
        let reason = format!("create target {} is not self or a linked peer", create.target);
        send(shared, sid, &res_err(&req.corr, &reason)).await;
    }
}

/// `req`'s `WorkspaceCreateReq`, or `None` once its requester is answered
/// why not: bytes the wire codec's decode would panic on, or recurse too
/// deep for, are refused as data, and the session goes on (F15b).
async fn create_request(
    shared: &Arc<Shared>,
    sid: SessionId,
    req: &ExchangeReq,
) -> Option<WorkspaceCreateReq> {
    match wellformed::decode(&req.payload) {
        Ok(create) => Some(WorkspaceCreateReq::from_cbor(&create)),
        Err(why) => {
            let reason = format!("workspace.create refused its payload: {why}");
            send(shared, sid, &res_err(&req.corr, &reason)).await;
            None
        }
    }
}

/// An inbound `ExchangeRes` (the attached provider answering): resolve the
/// minted correlation and deliver to the recorded requester, under its own
/// correlation again (trace D4/D5, F16).
pub(crate) async fn handle_response(shared: &Arc<Shared>, mut res: ExchangeRes) {
    let caller = shared.pending.lock().await.answered(&res.corr);
    if let Some((sid, corr)) = caller {
        res.corr = corr;
        send(shared, sid, &Frame::ExchangeRes(res)).await;
    } // unknown corr: dropped — never folded, never broadcast
}

/// The requesting node's Forward arm: one conversation on the claim holder's
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
    let linked = mesh.linked(peer).await;
    let linked = linked.ok_or_else(|| other("no live peer link"))?;
    let glade_id = req.glade_id.clone();
    let mut conversation = linked.open();
    conversation.send(&Frame::ExchangeReq(req))?;
    let frame = tokio::time::timeout(FORWARD_TIMEOUT, conversation.recv())
        .await
        .map_err(|_| other("timeout awaiting ExchangeRes from claim holder"))??;
    conversation.end();
    match frame {
        Frame::ExchangeRes(res) => forwarded(&glade_id, res),
        got => Err(other(format!("expected ExchangeRes, got {got:?}"))),
    }
}

/// The claim holder's answer to an exchange on `glade_id` forwarded to it
/// (F15b). A `workspace.create` answer carries the node's own
/// `WorkspaceCreateRes`, which the requester decodes, so one whose payload
/// `wellformed::decode` refuses fails the exchange. Any other exchange's
/// payload is its app's, opaque to the node, and passes as it came.
fn forwarded(glade_id: &str, res: ExchangeRes) -> io::Result<ExchangeRes> {
    let payload = res.payload.as_deref();
    let created = payload.filter(|_| glade_id == WORKSPACE_CREATE);
    if let Some(Err(why)) = created.map(wellformed::decode) {
        return Err(other(format!("its {WORKSPACE_CREATE} answer: {why}")));
    }
    Ok(res)
}

/// The claim holder's side of a forwarded exchange (trace D2→D4): a synthetic
/// session whose outbound answers on the conversation, so the ordinary
/// request/response plumbing (provider lookup, pending map) serves the peer
/// unchanged. One conversation, one exchange, END.
///
/// The grant check (plan Step 4.3), enforced for every peer: on a share other
/// than `home` the exchange is asked for by its glade id, and a node the fold
/// does not grant it is answered `ok: false` with the reason, the path's
/// failure form. `home` stays exempt, so a forwarded `workspace.create` is
/// answered as before.
pub(crate) async fn serve_peer_exchange(
    shared: Arc<Shared>,
    node: [u8; 32],
    conversation: Conversation,
    req: ExchangeReq,
) -> io::Result<()> {
    let corr = req.corr.clone();
    let holder = Holder::Node(node);
    let refused = match req.share.as_str() {
        HOME => None,
        share => shared.policy.check(&holder, &req.glade_id, share).err(),
    };
    let bytes = match refused {
        Some(denial) => {
            let why = refusal(&holder, &req.glade_id, &req.share, denial);
            res_err(&corr, &why).to_bytes()
        }
        None => answer_forwarded(&shared, req, PROVIDER_TIMEOUT).await,
    };
    conversation.send_encoded(&bytes)?;
    conversation.end();
    Ok(())
}

/// Answer an admitted forwarded exchange through a synthetic session: the
/// provider's `ExchangeRes`, or the timeout's after `wait`, as the bytes of
/// one frame. The session then ends, and forgets a call it left unanswered.
async fn answer_forwarded(shared: &Arc<Shared>, req: ExchangeReq, wait: Duration) -> Vec<u8> {
    let corr = req.corr.clone();
    let sid = shared.next.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    shared.out.lock().await.insert(sid, tx);

    let mut echo = Echo::new(); // undeclared ids keep the echo answer even here
    handle_request(shared, sid, req, &mut echo).await;
    let bytes = match tokio::time::timeout(wait, rx.recv()).await {
        Ok(Some(b)) => b,
        _ => res_err(&corr, "provider timeout at claim holder").to_bytes(),
    };

    shared.out.lock().await.remove(&sid);
    // a call the session filed and never saw answered must not leak.
    shared.pending.lock().await.forget(sid);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appdecl;
    use crate::frame::Frame;
    use crate::mesh::testing::meshed;
    use crate::registry::{Record, Registry, RegistryApi, G_BINDINGS, G_GRANTS};
    use crate::server::Server;
    use crate::sysdata::{BindingDecl, BindingRetraction, CapabilityGrant, ServeClaim};
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
    /// attached authority provider (on the node's corr, 1:1; the response
    /// routed back on the caller's own, F16),
    /// answers `ok:false` data BEFORE any provider attaches, and an UNDECLARED
    /// glade id keeps the legacy echo answer byte-for-byte.
    #[tokio::test]
    async fn local_provider_round_trip_absence_and_echo_fallback() {
        // a non-grazel app: the routing is app-agnostic.
        let decl = appdecl::parse(
            "glade-app v0\napp demo\nservice demo d.ops\n",
        )
        .unwrap();
        let (mut reg, n1) = sealed();
        appdecl::register(&decl, &mut reg, &n1).unwrap();

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

        // (3) request -> provider (on the node's corr, F16) -> response -> requester (on its own).
        wc.send_binary(&xreq("s", "d.ops", "c1", b"workspace.status")).await.unwrap();
        let corr = match next_frame(&mut rp, "provider receives the request").await {
            Frame::ExchangeReq(req) => {
                assert_eq!(req.payload, b"workspace.status");
                req.corr
            }
            other => panic!("provider expected ExchangeReq, got {other:?}"),
        };
        wp.send_binary(
            &Frame::ExchangeRes(ExchangeRes {
                corr,
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

    /// A registry sealed as a test node, and the node's id, its origin, so
    /// the served store takes what it appends (plan Step 4.1b).
    fn sealed() -> (Registry, String) {
        let identity = crate::peer::NodeIdentity::from_key([41; 32]);
        let origin = crate::transport::hex(&identity.node_id);
        (Registry::sealed(identity), origin)
    }

    /// The served store holding a registry's records, as `seed_registry` lands them.
    fn store_of(reg: &Registry, name: &str) -> Store {
        let mut st = Store::open(fresh(name)).unwrap();
        for bytes in &reg.snapshot().records {
            st.append(Op::from_cbor(&glade_wire::cbor::decode(bytes))).unwrap();
        }
        st
    }

    fn binding(app: &str, glade_id: &str, shape: &str) -> Record {
        Record::Binding(BindingDecl {
            app: app.into(),
            glade_id: glade_id.into(),
            shape: shape.into(),
            authority: "share".into(),
            zone: "commons".into(),
            retention: "latest".into(),
        })
    }

    /// R9(a): `declared_exchange` reads `dir.bindings` through the fold, not
    /// through `any()`. A stale `exchange` declaration superseded by a newer
    /// one for the same surface no longer keeps a retired exchange routable,
    /// and a newer `exchange` declaration makes it routable again.
    #[test]
    fn a_declared_exchange_binding_is_read_through_the_fold() {
        let (mut reg, n1) = sealed();
        reg.append(binding("demo", "d.x", "exchange"), &n1).unwrap();
        assert!(declared_exchange(&store_of(&reg, "fold-1"), "d.x"));
        reg.append(binding("demo", "d.x", "value"), &n1).unwrap();
        assert!(!declared_exchange(&store_of(&reg, "fold-2"), "d.x"), "the newest declaration is not an exchange");
        reg.append(binding("demo", "d.x", "exchange"), &n1).unwrap();
        assert!(declared_exchange(&store_of(&reg, "fold-3"), "d.x"));
        let retract = BindingRetraction { app: "demo".into(), glade_id: "d.x".into() };
        reg.append(Record::Retract(retract), &n1).unwrap();
        assert!(!declared_exchange(&store_of(&reg, "fold-4"), "d.x"), "a newest retraction takes it down");
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

    /// Poll until `pred` (over the node's store) holds, or panic after ~5s.
    async fn wait_store<F: Fn(&Store) -> bool>(shared: &Arc<Shared>, pred: F, what: &str) {
        for _ in 0..500 {
            if pred(&*shared.store.lock().await) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn create_payload(workspace: &str, name: &str, target: &str) -> Vec<u8> {
        glade_wire::cbor::encode(
            &crate::sysdata::WorkspaceCreateReq { workspace: workspace.into(), name: name.into(), target: target.into() }.to_cbor(),
        )
    }

    /// Read frames until the next `ExchangeRes` — a session subscribed to
    /// directory streams legitimately interleaves fanned-out Ops with it.
    async fn next_exchange_res(r: &mut ws::WsReader, what: &str) -> ExchangeRes {
        loop {
            if let Frame::ExchangeRes(res) = next_frame(r, what).await {
                return res;
            }
        }
    }

    fn max_claim_epoch_for(st: &Store, share: &str) -> i64 {
        let mut max = 0;
        for (origin, _) in st.heads(HOME, crate::registry::G_CLAIMS, &[]) {
            for op in st.scan(HOME, crate::registry::G_CLAIMS, &[], &origin, i64::MIN) {
                let c = envelope::record(&op, ServeClaim::from_cbor).unwrap();
                if c.share == share && c.epoch > max {
                    max = c.epoch;
                }
            }
        }
        max
    }

    /// The s-create golden path (trace D1–D3 · K1 · H1, audit F2), E2E over
    /// real iroh + real websockets. A client on A asks `workspace.create`
    /// naming B as TARGET — no claim exists yet (creation PRECEDES claims;
    /// the target rides the exchange payload, the wire untouched):
    ///
    ///   (a) the request routes to B by TARGET, B mints WorkspaceEntry +
    ///       ServeClaim under its OWN origin and answers, corr preserved 1:1;
    ///   (b) the minted records replicate back (B9 push), A's LOCAL fold
    ///       routes the new share to B, and a subscribe flows THROUGH the new
    ///       claim — authority content reaches the A-side client live;
    ///   (c) re-create is idempotent: records diff away (`created:false`,
    ///       entry heads + claim epoch unchanged at B);
    ///   (d) a create naming an UNLINKED target answers `ok:false` data with
    ///       the reason, and the session stays usable;
    ///   (e) target == self performs locally, same ceremony.
    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_create_routes_to_target_end_to_end() {
        let boot_a = boot_at(fresh("cr-a-sys"), "gianni").unwrap();
        let mut boot_b = boot_at(fresh("cr-b-sys"), "gianni").unwrap();
        let (a_id, b_id) = (boot_a.node_id.clone(), boot_b.node_id.clone());
        // B's fold grants A's node id reads of the share A is about to create
        // there, so (b)'s subscribe is served (plan Step 4.3).
        let grant = CapabilityGrant {
            principal: a_id.clone(),
            share: "ws-new".into(),
            verbs: vec!["read.*".into()],
        };
        boot_b.registry.append(Record::Grant(grant), &b_id).unwrap();

        let a = Server::open(fresh("cr-a-store")).unwrap();
        let b = Server::open(fresh("cr-b-store")).unwrap();
        let (id_a, id_b) = (boot_a.identity().unwrap(), boot_b.identity().unwrap());
        a.adopt_boot(boot_a).await.unwrap();
        b.adopt_boot(boot_b).await.unwrap();

        meshed(&a, id_a).await;
        let at_b = meshed(&b, id_b).await;
        a.connect_peer(at_b).await.unwrap();

        let (a_shared, b_shared) = (a.shared.clone(), b.shared.clone());
        let lis_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let lis_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (port_a, port_b) = (lis_a.local_addr().unwrap().port(), lis_b.local_addr().unwrap().port());
        tokio::spawn(a.run(lis_a));
        tokio::spawn(b.run(lis_b));

        let (mut rc, wc) = ws::connect("127.0.0.1", port_a).await.unwrap();

        // ---- (a) create at B, asked from A ----------------------------------
        wc.send_binary(&xreq(HOME, WORKSPACE_CREATE, "cr-1", &create_payload("ws-new", "new", &b_id))).await.unwrap();
        match next_frame(&mut rc, "create response").await {
            Frame::ExchangeRes(res) => {
                assert!(res.ok, "create succeeded, corr intact: {:?}", res.error);
                assert_eq!(res.corr, "cr-1");
                let out = crate::sysdata::WorkspaceCreateRes::from_cbor(&glade_wire::cbor::decode(&res.payload.unwrap()));
                assert_eq!((out.workspace.as_str(), out.node.as_str(), out.created), ("ws-new", b_id.as_str(), true), "the TARGET performed the creation under its own origin");
            }
            other => panic!("expected ExchangeRes, got {other:?}"),
        }

        // ---- (b) the minted records reached A (B9 push): routing follows ----
        {
            let bid = b_id.clone();
            wait_store(&a_shared, move |st| crate::mesh::who_serves(st, "ws-new", now_ms()) == Some(bid.clone()), "A's fold to route ws-new to B").await;
        }
        wc.send_binary(&sub("ws-new", "ws.tree")).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "ws-new subscribe ack (routed, not absent)").await, Frame::Heads(_)));
        // B's authority session writes; the op reaches the A-side client live.
        let (_rp, wp) = ws::connect("127.0.0.1", port_b).await.unwrap();
        let op = Op {
            share: "ws-new".into(),
            glade_id: "ws.tree".into(),
            key: vec![],
            origin: "prov-b".into(),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: vec![],
            shape: Shape::Value,
            payload: b"new-tree-v0".to_vec(),
        };
        wp.send_binary(&Frame::Ops(Ops { ops: vec![op], pri: None }).to_bytes()).await.unwrap();
        loop {
            if let Frame::Ops(ops) = next_frame(&mut rc, "content through the new claim").await {
                if ops.ops.iter().any(|o| o.payload == b"new-tree-v0") {
                    break;
                }
            }
        }

        // ---- (c) re-create is idempotent: the records diff -------------------
        let (entry_heads, epoch_before) = {
            let st = b_shared.store.lock().await;
            (st.heads(HOME, crate::registry::G_WORKSPACES, &[]), max_claim_epoch_for(&st, "ws-new"))
        };
        wc.send_binary(&xreq(HOME, WORKSPACE_CREATE, "cr-2", &create_payload("ws-new", "new", &b_id))).await.unwrap();
        match next_frame(&mut rc, "re-create response").await {
            Frame::ExchangeRes(res) => {
                assert!(res.ok);
                let out = crate::sysdata::WorkspaceCreateRes::from_cbor(&glade_wire::cbor::decode(&res.payload.unwrap()));
                assert!(!out.created, "already served: nothing new minted");
            }
            other => panic!("expected ExchangeRes, got {other:?}"),
        }
        {
            let st = b_shared.store.lock().await;
            assert_eq!(st.heads(HOME, crate::registry::G_WORKSPACES, &[]), entry_heads, "no duplicate WorkspaceEntry");
            assert_eq!(max_claim_epoch_for(&st, "ws-new"), epoch_before, "no re-claim: the epoch is stable");
        }

        // ---- (d) an unlinked target fails as data ----------------------------
        wc.send_binary(&xreq(HOME, WORKSPACE_CREATE, "cr-3", &create_payload("ws-nope", "nope", "deadbeef"))).await.unwrap();
        match next_frame(&mut rc, "unlinked-target failure data").await {
            Frame::ExchangeRes(res) => {
                assert_eq!(res.corr, "cr-3");
                assert!(!res.ok);
                assert!(res.error.unwrap().contains("not self or a linked peer"), "the reason rides the response");
            }
            other => panic!("expected ExchangeRes failure data, got {other:?}"),
        }
        // failure is data, not a dead session: the next ask still answers.
        wc.send_binary(&sub(HOME, crate::registry::G_WORKSPACES)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "post-failure ack").await, Frame::Heads(_)));

        // ---- (e) target == self performs locally -----------------------------
        wc.send_binary(&xreq(HOME, WORKSPACE_CREATE, "cr-4", &create_payload("ws-mine", "mine", &a_id))).await.unwrap();
        {
            let res = next_exchange_res(&mut rc, "self-target create response").await;
            assert!(res.ok, "{:?}", res.error);
            assert_eq!(res.corr, "cr-4");
            let out = crate::sysdata::WorkspaceCreateRes::from_cbor(&glade_wire::cbor::decode(&res.payload.unwrap()));
            assert_eq!((out.node.as_str(), out.created), (a_id.as_str(), true));
        }
        {
            let st = a_shared.store.lock().await;
            assert_eq!(crate::mesh::who_serves(&st, "ws-mine", now_ms()), Some(a_id.clone()));
        }
    }

    /// Two booted nodes for the grazel-attach journeys, over real iroh and
    /// websockets. B loads grazel-app.glade as data and claims its workspace,
    /// with the lapsed `ws-attic` beside it; when `grant_a`, it also registers
    /// a file whose one line grants A's node id `read.*,gwz.*` on `ws-razel`
    /// (plan Step 4.3). B is adopted, so it checks its own fold; A is seeded
    /// and dials B. B's grazel authority session has attached as the gwz.ops
    /// provider and written two tree ops.
    struct Attach {
        a: Arc<Shared>,
        a_id: String,
        b_id: String,
        port_a: u16,
        /// The provider session on B.
        rp: ws::WsReader,
        wp: ws::WsWriter,
    }

    async fn attach_nodes(name: &str, grant_a: bool) -> Attach {
        // ---- node B: boot + LOAD grazel-app.glade + claim its workspace -----
        let boot_a = boot_at(fresh(&format!("{name}-a-sys")), "gianni").unwrap();
        let mut boot_b = boot_at(fresh(&format!("{name}-b-sys")), "gianni").unwrap();
        let (a_id, b_id) = (boot_a.node_id.clone(), boot_b.node_id.clone());
        let loaded = appdecl::register(&grazel_decl(), &mut boot_b.registry, &b_id).unwrap();
        let registered = "7 bindings + 1 service + 2 seeds + 1 revocation + 1 workspace registered";
        assert_eq!(loaded.appended, 12, "{registered}");
        if grant_a {
            // The line B's operator writes for the reading node A (plan Step 4.3).
            let text = format!("glade-app v1\napp peer-a\nseed {a_id} ws-razel read.*,gwz.*\n");
            let grant = appdecl::parse(&text).unwrap();
            appdecl::register(&grant, &mut boot_b.registry, &b_id).unwrap();
        }
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

        let (id_a, id_b) = (boot_a.identity().unwrap(), boot_b.identity().unwrap());
        let a = Server::open(fresh(&format!("{name}-a-store"))).unwrap();
        let b = Server::open(fresh(&format!("{name}-b-store"))).unwrap();
        a.seed_registry(&boot_a.registry.snapshot()).await;
        b.adopt_boot(boot_b).await.unwrap();

        meshed(&a, id_a).await;
        let at_b = meshed(&b, id_b).await;
        a.connect_peer(at_b).await.unwrap();

        let a_shared = a.shared.clone();
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
        wp.send_binary(&Frame::Ops(Ops { ops: vec![o0.clone(), o1.clone()], pri: None }).to_bytes()).await.unwrap();
        // each op is answered on the provider's session, by its hash (R1).
        for o in [&o0, &o1] {
            match next_frame(&mut rp, "the provider's op status").await {
                Frame::Error(e) => {
                    let hash: String = crate::chain::op_hash(o)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    assert_eq!(
                        (e.code, e.corr),
                        (glade_wire::generated::ErrorCode::Ok, Some(hash))
                    );
                }
                other => panic!("the provider expected its op's status, got {other:?}"),
            }
        }
        Attach {
            a: a_shared,
            a_id,
            b_id,
            port_a,
            rp,
            wp,
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
    ///       B's attached grazel provider answers on B's own correlation,
    ///       and the requester sees its own back (D, F16);
    ///   (d) an exchange against a share with no live claim answers bounded
    ///       `ok:false` data with the reason, and the session stays usable (E).
    ///
    /// B serves A in (b) and (c) because its fold grants A's node id `read.*`
    /// and `gwz.*` on `ws-razel` (plan Step 4.3); without it, B refuses both
    /// (`grazel_attach_without_a_grant_is_refused_by_its_claimed_node_id`).
    #[tokio::test(flavor = "multi_thread")]
    async fn grazel_attach_end_to_end() {
        let Attach {
            a_id,
            b_id,
            port_a,
            mut rp,
            wp,
            ..
        } = attach_nodes("e2e", true).await;

        // ---- (a) registered surfaces appear at A as ordinary records --------
        let (mut rc, wc) = ws::connect("127.0.0.1", port_a).await.unwrap();
        wc.send_binary(&sub(HOME, G_BINDINGS)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "dir.bindings ack").await, Frame::Heads(_)));
        let mut bindings = Vec::new();
        while bindings.len() < 7 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "BindingDecl records").await {
                for op in ops.ops {
                    assert_eq!(op.origin, b_id, "declarations ride the registrant's chain");
                    let binding = envelope::record(&op, BindingDecl::from_cbor).unwrap();
                    bindings.push(binding.glade_id);
                }
            }
        }
        bindings.sort();
        // the 4 workspace surfaces + the 3 pre-declared composed-supplier surfaces
        // (gwz.output, chat.msgs, chat.groups) — P1.S3.
        assert_eq!(
            bindings,
            vec!["chat.groups", "chat.msgs", "gwz.output", "term.log", "ws.diff", "ws.files", "ws.tree"]
        );
        // the ACL seeds compiled to ORDINARY grant records (s-app-register A5).
        wc.send_binary(&sub(HOME, G_GRANTS)).await.unwrap();
        assert!(matches!(next_frame(&mut rc, "dir.grants ack").await, Frame::Heads(_)));
        let mut grants = Vec::new();
        while grants.len() < 3 {
            if let Frame::Ops(ops) = next_frame(&mut rc, "seeded grant records").await {
                for op in ops.ops {
                    let g = envelope::record(&op, CapabilityGrant::from_cbor).unwrap();
                    grants.push((g.principal, g.share, g.verbs.join(",")));
                }
            }
        }
        grants.sort();
        assert_eq!(
            grants,
            vec![
                (
                    a_id.clone(),
                    "ws-razel".to_string(),
                    "read.*,gwz.*".to_string()
                ),
                (
                    "owner".to_string(),
                    "ws-razel".to_string(),
                    "gwz.*".to_string()
                ),
                (
                    "owner".to_string(),
                    "ws-razel".to_string(),
                    "read.*".to_string()
                ),
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
        let corr = match next_frame(&mut rp, "grazel receives the forwarded exchange").await {
            Frame::ExchangeReq(req) => {
                assert_ne!(req.corr, "x-42", "the handler sees B's correlation");
                assert_eq!(req.payload, b"workspace.status");
                req.corr
            }
            other => panic!("grazel expected ExchangeReq, got {other:?}"),
        };
        wp.send_binary(
            &Frame::ExchangeRes(ExchangeRes {
                corr,
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

    /// `grazel_attach_end_to_end`, turned round (plan Step 4.3): B's fold
    /// grants A's node id nothing on `ws-razel`. The forwarded `gwz.ops`
    /// exchange is refused at B by that claimed node id and answered
    /// `ok: false` with the reason, corr intact, at once, where a forwarded
    /// exchange its provider never answered would wait for the timeout. The
    /// forwarded `ws.tree` subscribe is refused too: A's forward lapses, and
    /// nothing of the zone reaches A.
    #[tokio::test(flavor = "multi_thread")]
    async fn grazel_attach_without_a_grant_is_refused_by_its_claimed_node_id() {
        let t = attach_nodes("refused", false).await;
        let (mut rc, wc) = ws::connect("127.0.0.1", t.port_a).await.unwrap();

        let exchange = xreq("ws-razel", "gwz.ops", "x-44", b"workspace.status");
        wc.send_binary(&exchange).await.unwrap();
        let res = next_exchange_res(&mut rc, "the refused exchange").await;
        assert_eq!((res.corr.as_str(), res.ok), ("x-44", false));
        let a = &t.a_id;
        let why = format!("unauthorized: node {a} holds no grant of gwz.ops on ws-razel");
        assert_eq!(res.error.as_deref(), Some(why.as_str()));

        wc.send_binary(&sub("ws-razel", "ws.tree")).await.unwrap();
        let ack = next_frame(&mut rc, "ws.tree ack").await;
        assert!(matches!(ack, Frame::Heads(_)), "{ack:?}");
        let zone = ("ws-razel".to_string(), "ws.tree".to_string(), Vec::new());
        let (share, glade_id) = (zone.0.clone(), zone.1.clone());
        crate::mesh::forward_interest(&t.a, t.b_id.clone(), share, glade_id, vec![]).await;
        let mesh = t.a.mesh.get().unwrap();
        let mut lapsed = false;
        for _ in 0..500 {
            if !mesh.forwarded.lock().await.contains(&zone) {
                lapsed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(lapsed, "A's forward of ws.tree lapsed");
        let store = t.a.store.lock().await;
        let held = store.scan("ws-razel", "ws.tree", &[], "grazel-b", i64::MIN);
        assert!(held.is_empty(), "nothing of the zone reached A: {held:?}");
    }

    /// F15b: a `workspace.create` whose payload nests 100,000 deep, which the
    /// wire codec's decode recursed on until the node's stack overflowed and
    /// the process aborted, is answered `ok: false` with the reason, and the
    /// node serves on: that session and another client are answered.
    #[tokio::test]
    async fn a_nested_create_payload_is_refused_and_the_node_serves_on() {
        let server = Server::open(fresh("nested-create")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));
        let (mut rc, wc) = ws::connect("127.0.0.1", port).await.unwrap();

        let mut nested = vec![0x81; 100_000];
        nested.push(0);
        let create = xreq(HOME, WORKSPACE_CREATE, "n-1", &nested);
        wc.send_binary(&create).await.unwrap();
        let res = next_exchange_res(&mut rc, "the nested create's answer").await;
        assert_eq!((res.corr.as_str(), res.ok), ("n-1", false));
        let why = res.error.unwrap_or_default();
        assert!(why.contains("nested deeper than 32"), "{why}");

        let (mut other, wo) = ws::connect("127.0.0.1", port).await.unwrap();
        for (r, w, corr) in [(&mut rc, &wc, "e-1"), (&mut other, &wo, "e-2")] {
            let echo = xreq("s", "e.x", corr, b"ping");
            w.send_binary(&echo).await.unwrap();
            let res = next_exchange_res(r, "an echo").await;
            let echoed = (res.corr.as_str(), res.payload.as_deref());
            assert_eq!(echoed, (corr, Some(b"ping".as_slice())));
        }
    }

    /// F15b: a claim holder's answer to a forwarded `workspace.create`
    /// carries the node's own `WorkspaceCreateRes`, which the requester
    /// decodes, so one whose payload `wellformed` refuses fails the exchange.
    /// A well-formed one passes as it came, and so does any other exchange's
    /// payload, which is its app's.
    #[test]
    fn a_nested_answer_to_a_forwarded_create_is_a_failure() {
        let mut nested = vec![0x81; 100_000];
        nested.push(0);
        let created = crate::sysdata::WorkspaceCreateRes {
            workspace: "ws".into(),
            node: "n".into(),
            created: true,
        };
        let well_formed = glade_wire::cbor::encode(&created.to_cbor());
        let answer = |payload: &[u8]| ExchangeRes {
            corr: "c".into(),
            ok: true,
            payload: Some(payload.to_vec()),
            error: None,
        };
        let failed = forwarded(WORKSPACE_CREATE, answer(&nested));
        let why = failed.unwrap_err().to_string();
        assert!(why.contains("nested deeper than 32"), "{why}");
        let passed = forwarded(WORKSPACE_CREATE, answer(&well_formed));
        assert_eq!(passed.unwrap(), answer(&well_formed));
        let app = forwarded("d.ops", answer(&nested));
        assert_eq!(app.unwrap(), answer(&nested));
    }

    /// One node serving the declared `d.ops` exchange on share `s`, with its
    /// provider attached: the node's state, its port and the provider (F16).
    async fn provided(name: &str) -> (Arc<Shared>, u16, ws::WsReader, ws::WsWriter) {
        let decl = appdecl::parse("glade-app v0\napp demo\nservice demo d.ops\n").unwrap();
        let (mut reg, n1) = sealed();
        appdecl::register(&decl, &mut reg, &n1).unwrap();
        let server = Server::open(fresh(name)).unwrap();
        server.seed_registry(&reg.snapshot()).await;
        let shared = server.shared.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));
        let (mut rp, wp) = ws::connect("127.0.0.1", port).await.unwrap();
        wp.send_binary(&sub("s", "d.ops")).await.unwrap();
        let ack = next_frame(&mut rp, "provider attach ack").await;
        assert!(matches!(ack, Frame::Heads(_)), "{ack:?}");
        (shared, port, rp, wp)
    }

    /// A caller's call on `d.ops`, as `corr`.
    async fn ask(w: &ws::WsWriter, corr: &str, payload: &[u8]) {
        let req = xreq("s", "d.ops", corr, payload);
        w.send_binary(&req).await.unwrap();
    }

    /// The provider's answer on `corr`.
    async fn answer(wp: &ws::WsWriter, corr: &str, payload: &[u8]) {
        let res = ExchangeRes {
            corr: corr.into(),
            ok: true,
            payload: Some(payload.to_vec()),
            error: None,
        };
        let bytes = Frame::ExchangeRes(res).to_bytes();
        wp.send_binary(&bytes).await.unwrap();
    }

    /// The provider's next frame, which must be a call.
    async fn next_call(rp: &mut ws::WsReader, what: &str) -> ExchangeReq {
        match next_frame(rp, what).await {
            Frame::ExchangeReq(req) => req,
            other => panic!("the provider expected {what}, got {other:?}"),
        }
    }

    /// A caller's next frame, which must be a reply: its corr and payload.
    async fn next_reply(r: &mut ws::WsReader, what: &str) -> (String, String) {
        match next_frame(r, what).await {
            Frame::ExchangeRes(res) => {
                let payload = res.payload.unwrap_or_default();
                reply(&res.corr, &String::from_utf8_lossy(&payload))
            }
            other => panic!("the caller expected {what}, got {other:?}"),
        }
    }

    fn reply(corr: &str, payload: &str) -> (String, String) {
        (corr.into(), payload.into())
    }

    /// How many calls the node holds filed, each awaiting its answer.
    async fn filed(shared: &Arc<Shared>) -> usize {
        shared.pending.lock().await.calls.len()
    }

    /// F16: two sessions call one exchange at once, each as `c1`, since every
    /// client numbers its calls from `c1`. Each receives its own reply, as
    /// `c1`. The provider answers in arrival order and the later caller is
    /// read first: filed under a shared `c1`, it got the earlier one's reply.
    #[tokio::test]
    async fn two_callers_numbering_alike_each_receive_their_own_reply() {
        let (shared, port, mut rp, wp) = provided("f16-alike").await;
        let (mut r1, w1) = ws::connect("127.0.0.1", port).await.unwrap();
        let (mut r2, w2) = ws::connect("127.0.0.1", port).await.unwrap();
        ask(&w1, "c1", b"from-1").await;
        let first = next_call(&mut rp, "the first call").await;
        ask(&w2, "c1", b"from-2").await;
        let second = next_call(&mut rp, "the second call").await;
        assert_eq!(first.payload, b"from-1");
        assert_eq!(second.payload, b"from-2");
        answer(&wp, &first.corr, b"to-1").await;
        answer(&wp, &second.corr, b"to-2").await;
        let later = next_reply(&mut r2, "the later caller's reply").await;
        assert_eq!(later, reply("c1", "to-2"));
        let earlier = next_reply(&mut r1, "the earlier caller's reply").await;
        assert_eq!(earlier, reply("c1", "to-1"));
        assert_eq!(filed(&shared).await, 0, "neither is still filed");
    }

    /// F16 across the mesh: two sessions on A call `gwz.ops`, served at B, at
    /// once, each as `c1`. A forwards each on a conversation of its own, which
    /// keys them apart there; B files each under a correlation it mints, so
    /// each caller receives its own reply, as `c1`.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_callers_across_the_mesh_each_receive_their_own_reply() {
        let t = attach_nodes("f16-mesh", true).await;
        let (mut rp, wp) = (t.rp, t.wp);
        let (mut r1, w1) = ws::connect("127.0.0.1", t.port_a).await.unwrap();
        let (mut r2, w2) = ws::connect("127.0.0.1", t.port_a).await.unwrap();
        let from_1 = xreq("ws-razel", "gwz.ops", "c1", b"from-1");
        w1.send_binary(&from_1).await.unwrap();
        let first = next_call(&mut rp, "the first forwarded call").await;
        let from_2 = xreq("ws-razel", "gwz.ops", "c1", b"from-2");
        w2.send_binary(&from_2).await.unwrap();
        let second = next_call(&mut rp, "the second forwarded call").await;
        assert_eq!(first.payload, b"from-1");
        assert_eq!(second.payload, b"from-2");
        answer(&wp, &first.corr, b"to-1").await;
        answer(&wp, &second.corr, b"to-2").await;
        let later = next_reply(&mut r2, "the later caller's reply").await;
        assert_eq!(later, reply("c1", "to-2"));
        let earlier = next_reply(&mut r1, "the earlier caller's reply").await;
        assert_eq!(earlier, reply("c1", "to-1"));
    }

    /// F16: a reply on a correlation the node holds no call under is dropped
    /// without harm: one under the caller's own `c1`, which the node never
    /// minted, and a second answer to an answered call. The filed call still
    /// gets its one reply, and the caller's next reply is its next call's.
    #[tokio::test]
    async fn a_reply_on_an_unknown_correlation_is_dropped_without_harm() {
        let (shared, port, mut rp, wp) = provided("f16-unknown").await;
        let (mut rc, wc) = ws::connect("127.0.0.1", port).await.unwrap();
        ask(&wc, "c1", b"ask").await;
        let call = next_call(&mut rp, "the call").await;
        answer(&wp, "c1", b"stray").await;
        answer(&wp, &call.corr, b"answer").await;
        answer(&wp, &call.corr, b"again").await;
        let got = next_reply(&mut rc, "the call's reply").await;
        assert_eq!(got, reply("c1", "answer"));
        ask(&wc, "c2", b"next").await;
        let next = next_call(&mut rp, "the next call").await;
        answer(&wp, &next.corr, b"answered").await;
        let got = next_reply(&mut rc, "the next call's reply").await;
        assert_eq!(got, reply("c2", "answered"));
        assert_eq!(filed(&shared).await, 0);
    }

    /// F16: a forwarded call its provider never answers stays filed until its
    /// synthetic session ends, at the timeout, and is forgotten then. The call
    /// another caller filed before it, also as `c1`, stays filed and answered.
    #[tokio::test]
    async fn a_never_answered_call_is_forgotten_when_its_session_ends() {
        let (shared, port, mut rp, wp) = provided("f16-unanswered").await;
        let (mut rc, wc) = ws::connect("127.0.0.1", port).await.unwrap();
        ask(&wc, "c1", b"kept").await;
        let kept = next_call(&mut rp, "the caller's call").await;
        let lost = ExchangeReq {
            share: "s".into(),
            glade_id: "d.ops".into(),
            corr: "c1".into(),
            payload: b"lost".to_vec(),
        };
        let ended = answer_forwarded(&shared, lost, Duration::from_millis(100)).await;
        let timeout = res_err("c1", "provider timeout at claim holder");
        assert_eq!(ended, timeout.to_bytes(), "the session ends at the timeout");
        let call = next_call(&mut rp, "the forwarded call").await;
        assert_eq!(call.payload, b"lost");
        assert_eq!(filed(&shared).await, 1, "only the caller's is kept");
        answer(&wp, &kept.corr, b"late").await;
        let got = next_reply(&mut rc, "the kept call's reply").await;
        assert_eq!(got, reply("c1", "late"));
        assert_eq!(filed(&shared).await, 0);
    }

    /// F16: the handler sees a correlation the node minted, not the caller's,
    /// and echoes it; the caller sees its own back.
    #[tokio::test]
    async fn a_handler_sees_the_nodes_correlation_and_the_caller_its_own() {
        let (_, port, mut rp, wp) = provided("f16-opaque").await;
        let (mut rc, wc) = ws::connect("127.0.0.1", port).await.unwrap();
        ask(&wc, "c1", b"ask").await;
        let call = next_call(&mut rp, "the call").await;
        assert_ne!(call.corr, "c1", "the handler sees the node's correlation");
        assert_eq!(call.payload, b"ask");
        answer(&wp, &call.corr, b"answer").await;
        let got = next_reply(&mut rc, "the reply").await;
        assert_eq!(got, reply("c1", "answer"));
    }
}
