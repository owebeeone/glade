//! Integration: the rust client + supplier helper against a SPAWNED glade-node
//! (never the real `~/.glade` — a temp GLADE_HOME/HOME + temp store dirs). No
//! node internals: the crate depends on the wire + tokio only (P00-a); the tests
//! talk to the shipped binary exactly as any deployed supplier would. Three
//! scenarios cover the choreography the plan names:
//!
//!   1. exchange round-trip, BOTH roles rust (requester ↔ provider), + failure
//!      as data — booted with grazel-app.glade so gwz.ops is a declared exchange.
//!   2. op append + fold visible to a rust subscriber (value lww + log order).
//!   3. reattach after a NODE RESTART — kill the node, respawn on the same port
//!      + store dir, and the supplier's serving resumes on the new connection.
//!   4. op outcomes (client-writes plan Step 3.1): an accepted op, a repeat,
//!      and a refused op, whose chain then stops.
//!   5. the subscribe outcome (Step 3.2): the node's heads come back, a
//!      subscribe returns with its replay folded and resumes a refused chain,
//!      and one fails on a frame the session cannot take.
//!
//! Requires the node binary; the harness builds it once if absent.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use glade_client::hash::op_hash;
use glade_client::supplier::{Supplier, SupplierConfig, SupplierSurface};
use glade_client::ws::{self, Msg};
use glade_client::{Backoff, GladeClient, OpOutcome, SubscribeOutcome};
use glade_wire::cbor;
use glade_wire::generated::{self, ErrorCode, FrameType, Head, Op, Ops, Shape};

// ---- harness: spawn the real glade-node binary ----------------------------

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn node_bin() -> PathBuf {
    manifest().join("../node/target/debug/glade-node")
}

/// The gate normally pre-builds the binary; build it once if it is absent so the
/// suite is self-sufficient (the node has its own target dir — no lock clash).
fn ensure_node_built() {
    let bin = node_bin();
    if bin.exists() {
        return;
    }
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "--bin", "glade-node"])
        .current_dir(manifest().join("../node"))
        .status()
        .expect("build glade-node");
    assert!(status.success(), "failed to build glade-node");
    assert!(bin.exists(), "glade-node missing after build");
}

/// A temp dir that removes itself on drop (never the real `~/.glade`).
struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        static N: AtomicU64 = AtomicU64::new(0);
        let uniq = format!("{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst));
        let p = std::env::temp_dir().join(format!("glade-client-rs-{tag}-{uniq}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Read the node's `listening <port>` line (bounded), then drain stdout so the
/// pipe never fills while it serves.
async fn wait_listening(child: &mut Child) -> u16 {
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    let port = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(line) = lines.next_line().await.ok().flatten() {
            if let Some(rest) = line.strip_prefix("listening ") {
                if let Ok(p) = rest.trim().parse::<u16>() {
                    return Some(p);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .expect("node printed a listening port");
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    port
}

/// Boot a node with grazel-app.glade (declares `service grazel gwz.ops` +
/// `workspace ws-razel`) under a temp GLADE_HOME/HOME — the declared-exchange
/// form.
async fn boot_grazel(tmp: &Tmp) -> (Child, u16) {
    ensure_node_built();
    let mut child = Command::new(node_bin())
        .args(["--profile", "local", "--name", "seamrs", "--app"])
        .arg(manifest().join("../apps/grazel-app.glade"))
        .arg("0")
        .arg(tmp.path().join("store"))
        .env("GLADE_HOME", tmp.path().join("gh"))
        .env("HOME", tmp.path().join("h"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn booted glade-node");
    let port = wait_listening(&mut child).await;
    (child, port)
}

/// Spawn the legacy serve form on an explicit port + store dir (0 = OS-assigned).
/// The legacy node has no declared surfaces (every subscribe is Local, allow-all)
/// and persists its store to `store_dir` — so a restart on the same dir resumes.
async fn spawn_legacy(store_dir: &Path, port: u16) -> (Child, u16) {
    ensure_node_built();
    let mut child = Command::new(node_bin())
        .arg(port.to_string())
        .arg(store_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn legacy glade-node");
    let port = wait_listening(&mut child).await;
    (child, port)
}

async fn poll<F, Fut>(mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

// ---- 1. exchange round-trip, both roles rust ------------------------------

/// A rust requester and a rust provider (the supplier helper) round-trip a
/// declared exchange through the node: the Subscribe attaches the supplier as
/// THE provider, the request routes to it corr-preserved, its handler answers,
/// and the answer routes back. A handler `Err` is failure-as-DATA (`ok:false`),
/// never a hang — the session stays usable.
#[tokio::test(flavor = "multi_thread")]
async fn exchange_round_trip_both_roles_rust() {
    let tmp = Tmp::new("exch");
    let (mut node, port) = boot_grazel(&tmp).await;
    let url = format!("ws://127.0.0.1:{port}");

    let requester = GladeClient::new("requester");
    let provider = GladeClient::new("supplier");
    requester.connect(&url).await.unwrap();
    provider.connect(&url).await.unwrap();

    let sup = Supplier::attach(provider.clone(), SupplierConfig { principal: Some("gianni".into()), ..Default::default() });
    // handler: `boom` fails as data; anything else answers `pong:<payload>`.
    sup.serve_exchange(SupplierSurface::new("ws-razel", "gwz.ops", "exchange"), |req| {
        let s = String::from_utf8_lossy(&req.payload).to_string();
        if s == "boom" {
            Err("handler said boom".into())
        } else {
            Ok(format!("pong:{s}").into_bytes())
        }
    })
    .await
    .unwrap();

    // serve_exchange resolves on the attach ack, so the provider is registered;
    // poll defensively against any residual ordering.
    let requester_ok = requester.clone();
    let ok = poll(|| {
        let r = requester_ok.clone();
        async move { r.exchange("ws-razel", "gwz.ops", b"gwz.status".to_vec()).await.map(|o| o.ok).unwrap_or(false) }
    })
    .await;
    assert!(ok, "the supplier attached and answered");

    let res = requester.exchange("ws-razel", "gwz.ops", b"gwz.status".to_vec()).await.unwrap();
    assert!(res.ok);
    assert_eq!(res.payload.as_deref(), Some(b"pong:gwz.status".as_slice()), "the SUPPLIER answered, corr routed back");

    // failure as data: the handler's Err rides an ok:false response, corr intact.
    let boom = requester.exchange("ws-razel", "gwz.ops", b"boom".to_vec()).await.unwrap();
    assert!(!boom.ok);
    assert_eq!(boom.error.as_deref(), Some("handler said boom"));

    // the session stays usable after a failure answer.
    let again = requester.exchange("ws-razel", "gwz.ops", b"after".to_vec()).await.unwrap();
    assert_eq!(again.payload.as_deref(), Some(b"pong:after".as_slice()));

    sup.detach_all().await;
    requester.close().await;
    node.kill().await.ok();
}

// ---- 2. op append + fold visible to a rust subscriber ---------------------

/// The supplier serves value + log surfaces by APPENDING ops (the value/log
/// serve act — no provider attach); a separate rust subscriber converges them
/// through the real node and folds them (lww winner; log order). Wrong-shape
/// controller use errors.
#[tokio::test(flavor = "multi_thread")]
async fn share_serve_and_fold_visible_to_subscriber() {
    let tmp = Tmp::new("share");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let url = format!("ws://127.0.0.1:{port}");

    let supplier_client = GladeClient::new("sup");
    supplier_client.connect(&url).await.unwrap();
    let sup = Supplier::attach(supplier_client.clone(), SupplierConfig::default());

    let title = sup.serve_share(SupplierSurface::new("ws-app", "ws.title", "value"), |_| {}).await.unwrap();
    let lines = sup.serve_share(SupplierSurface::new("ws-app", "chat.lines", "log"), |_| {}).await.unwrap();

    // wrong-shape controller use is rejected (value ⇄ log).
    assert!(title.append(b"x".to_vec()).await.is_err(), "append() on a value surface errors");
    assert!(lines.set(b"x".to_vec()).await.is_err(), "set() on a log surface errors");

    let subscriber = GladeClient::new("cli");
    subscriber.connect(&url).await.unwrap();
    subscriber.subscribe("ws-app", "ws.title", None).await.unwrap();
    subscriber.subscribe("ws-app", "chat.lines", None).await.unwrap();

    // value (lww): the subscriber converges the latest title.
    title.set(b"first".to_vec()).await.unwrap();
    title.set(b"second".to_vec()).await.unwrap();
    let s = subscriber.clone();
    assert!(poll(|| { let s = s.clone(); async move { s.fold_value("ws-app", "ws.title", None).await.as_deref() == Some(b"second".as_slice()) } }).await, "value folds to the last write");

    // log: the subscriber converges the entries in order.
    lines.append(b"- hi".to_vec()).await.unwrap();
    lines.append(b"- there".to_vec()).await.unwrap();
    let s = subscriber.clone();
    assert!(poll(|| { let s = s.clone(); async move { s.fold_log("ws-app", "chat.lines", None).await.len() == 2 } }).await, "both log entries converge");
    assert_eq!(subscriber.fold_log("ws-app", "chat.lines", None).await, vec![b"- hi".to_vec(), b"- there".to_vec()], "log order preserved");

    sup.detach_all().await;
    subscriber.close().await;
    node.kill().await.ok();
}

// ---- 3. reattach after a node restart -------------------------------------

/// The supplier reattaches after the NODE goes down and comes back on the same
/// endpoint (records persist under the store dir + re-fold on boot; §6). The
/// supplier's serving resumes: a post-restart write flows through the restarted
/// node to a fresh subscriber, and the pre-restart value persisted.
#[tokio::test(flavor = "multi_thread")]
async fn reattaches_after_node_restart() {
    let tmp = Tmp::new("reattach");
    let store = tmp.path().join("store");
    let (mut node1, port) = spawn_legacy(&store, 0).await;
    let url = format!("ws://127.0.0.1:{port}");

    let supplier_client = GladeClient::new("sup");
    supplier_client.connect(&url).await.unwrap();
    // fast backoff so the reattach lands quickly after the node returns.
    let sup = Supplier::attach(supplier_client.clone(), SupplierConfig { backoff: Backoff { initial_ms: 100, factor: 2, max_ms: 1500 }, ..Default::default() });
    let state = sup.serve_share(SupplierSurface::new("ws-app", "ws.state", "value"), |_| {}).await.unwrap();

    // pre-restart: a subscriber sees the served value.
    let sub1 = GladeClient::new("sub1");
    sub1.connect(&url).await.unwrap();
    sub1.subscribe("ws-app", "ws.state", None).await.unwrap();
    state.set(b"v1".to_vec()).await.unwrap();
    let s = sub1.clone();
    assert!(poll(|| { let s = s.clone(); async move { s.fold_value("ws-app", "ws.state", None).await.as_deref() == Some(b"v1".as_slice()) } }).await, "pre-restart serving works");

    // ---- kill the node; the supplier's link drops -> reattach loop starts ----
    node1.kill().await.ok();
    node1.wait().await.ok();
    // let the OS free the listen port, then bring the node back on the SAME port
    // + store dir (persisted v1 re-folds on boot).
    tokio::time::sleep(Duration::from_millis(400)).await;
    let mut node2 = None;
    for _ in 0..10 {
        match Command::new(node_bin())
            .arg(port.to_string())
            .arg(&store)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(mut child) => {
                // if it fails to bind the port it exits fast; wait_listening times
                // out -> retry. A successful bind prints `listening <port>`.
                if let Ok(p) = tokio::time::timeout(Duration::from_secs(3), wait_listening(&mut child)).await {
                    assert_eq!(p, port, "node2 rebound the same port");
                    node2 = Some(child);
                    break;
                }
                let _ = child.kill().await;
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut node2 = node2.expect("node2 came back on the same port");

    // ---- the supplier reattaches: a post-restart write serves again ----------
    // `set` needs a live connection; polling it until Ok waits out the reattach.
    let st = state.clone();
    assert!(poll(|| { let st = st.clone(); async move { st.set(b"v2".to_vec()).await.is_ok() } }).await, "supplier reconnected and served again");

    // a FRESH subscriber on the restarted node converges v2 (v1 persisted; v2 wins lww).
    let sub2 = GladeClient::new("sub2");
    sub2.connect(&url).await.unwrap();
    sub2.subscribe("ws-app", "ws.state", None).await.unwrap();
    let s = sub2.clone();
    assert!(poll(|| { let s = s.clone(); async move { s.fold_value("ws-app", "ws.state", None).await.as_deref() == Some(b"v2".as_slice()) } }).await, "post-reattach serving converges at a fresh subscriber");

    sup.detach_all().await;
    sub1.close().await;
    sub2.close().await;
    node2.kill().await.ok();
}

// ---- 4. op outcomes (client-writes plan Step 3.1) --------------------------

/// A status is awaited for 5 s at most: R1 says one may never come.
async fn within<T>(answer: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), answer).await.expect("no answer from the node within 5 s")
}

/// R1: the node answers an op it appends `Ok`, and the chain goes on.
#[tokio::test(flavor = "multi_thread")]
async fn an_accepted_append_is_ok() {
    let tmp = Tmp::new("accepted");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let client = GladeClient::new("w");
    client.connect(&format!("ws://127.0.0.1:{port}")).await.unwrap();

    let (op, outcome) = within(client.append_outcome("ws-app", "ws.state", "value", b"v0".to_vec(), None)).await.unwrap();
    assert_eq!((op.seq, outcome), (0, OpOutcome::Accepted));
    let (op, outcome) = within(client.append_outcome("ws-app", "ws.state", "value", b"v1".to_vec(), None)).await.unwrap();
    assert_eq!((op.seq, outcome), (1, OpOutcome::Accepted));

    client.close().await;
    node.kill().await.ok();
}

/// R1: a repeat the node holds byte for byte is `Ok`, not a refusal, whether a
/// second session of the origin makes it or the op is sent twice in a frame.
#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_append_is_ok() {
    let tmp = Tmp::new("repeated");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let url = format!("ws://127.0.0.1:{port}");
    let first = GladeClient::new("w");
    first.connect(&url).await.unwrap();
    let (op, outcome) = within(first.append_outcome("ws-app", "ws.state", "value", b"same".to_vec(), None)).await.unwrap();
    assert_eq!(outcome, OpOutcome::Accepted);

    let second = GladeClient::new("w");
    second.connect(&url).await.unwrap();
    let (again, outcome) = within(second.append_outcome("ws-app", "ws.state", "value", b"same".to_vec(), None)).await.unwrap();
    assert_eq!(again, op, "the same op, byte for byte");
    assert_eq!(outcome, OpOutcome::Accepted, "a repeat the node holds is Ok");
    let outcomes = within(second.send_ops_outcome(vec![op.clone(), op.clone()])).await.unwrap();
    assert_eq!(outcomes, vec![OpOutcome::Accepted, OpOutcome::Accepted], "one status per send");
    let (next, outcome) = within(second.append_outcome("ws-app", "ws.state", "value", b"next".to_vec(), None)).await.unwrap();
    assert_eq!((next.seq, outcome), (1, OpOutcome::Accepted), "the chain goes on");

    first.close().await;
    second.close().await;
    node.kill().await.ok();
}

/// Answer 4: a client never builds on an op the node refused. Two clients
/// write under one origin; the second one's seq 0 is refused, and its next
/// append on that chain fails.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_append_stops_its_chain() {
    let tmp = Tmp::new("refused");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let url = format!("ws://127.0.0.1:{port}");
    let first = GladeClient::new("w");
    first.connect(&url).await.unwrap();
    let (_, outcome) = within(first.append_outcome("ws-app", "ws.state", "value", b"first".to_vec(), None)).await.unwrap();
    assert_eq!(outcome, OpOutcome::Accepted, "the node holds the first client's seq 0");

    let second = GladeClient::new("w");
    second.connect(&url).await.unwrap();
    let mut refusals = second.on_refused().await;
    second.append("ws-app", "ws.state", "value", b"second".to_vec(), None).await.unwrap();
    let refusal = within(refusals.recv()).await.expect("a refusal");
    assert_eq!((refusal.code, refusal.op.seq), (ErrorCode::Equivocation, 0), "{refusal:?}");
    assert_eq!(second.fold_value("ws-app", "ws.state", None).await, None, "the session does not fold a refused op");
    let next = second.append("ws-app", "ws.state", "value", b"third".to_vec(), None).await;
    assert!(next.is_err(), "the next append on a refused chain must fail, got {next:?}");

    first.close().await;
    second.close().await;
    node.kill().await.ok();
}

// ---- 5. the subscribe outcome (client-writes plan Step 3.2) ----------------

/// R7: `subscribe` returns once its replay is folded. A writer puts 2,000 ops
/// on one chain, and a fresh client's subscribe returns with all of them.
#[tokio::test(flavor = "multi_thread")]
async fn subscribe_returns_after_the_replay_is_folded() {
    let tmp = Tmp::new("replay");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let url = format!("ws://127.0.0.1:{port}");
    let writer = GladeClient::new("writer");
    writer.connect(&url).await.unwrap();
    for i in 0..1999 {
        writer.append("ws-app", "chat.lines", "log", format!("line {i}").into_bytes(), None).await.unwrap();
    }
    let (_, last) = within(writer.append_outcome("ws-app", "chat.lines", "log", b"line 1999".to_vec(), None)).await.unwrap();
    assert_eq!(last, OpOutcome::Accepted, "the node holds all 2,000");

    let reader = GladeClient::new("reader");
    reader.connect(&url).await.unwrap();
    within(reader.subscribe("ws-app", "chat.lines", None)).await.unwrap();
    let folded = reader.fold_log("ws-app", "chat.lines", None).await.len();
    assert_eq!(folded, 2000, "subscribe returned before its replay was folded");

    writer.close().await;
    reader.close().await;
    node.kill().await.ok();
}

/// R5: `subscribe_outcome` returns each origin's head in the zone, by seq and
/// hash. An empty zone's ack names no origin.
#[tokio::test(flavor = "multi_thread")]
async fn subscribe_outcome_returns_the_nodes_heads() {
    let tmp = Tmp::new("heads");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let url = format!("ws://127.0.0.1:{port}");
    let (a, b) = (GladeClient::new("a"), GladeClient::new("b"));
    a.connect(&url).await.unwrap();
    b.connect(&url).await.unwrap();
    within(a.append_outcome("ws-app", "ws.state", "value", b"a0".to_vec(), None)).await.unwrap();
    let (a1, _) = within(a.append_outcome("ws-app", "ws.state", "value", b"a1".to_vec(), None)).await.unwrap();
    let (b0, _) = within(b.append_outcome("ws-app", "ws.state", "value", b"b0".to_vec(), None)).await.unwrap();

    let reader = GladeClient::new("reader");
    reader.connect(&url).await.unwrap();
    let outcome = within(reader.subscribe_outcome("ws-app", "ws.state", None)).await.unwrap();
    let head = |op: &Op| Head { origin: op.origin.clone(), seq: op.seq, hash: Some(op_hash(op).to_vec()) };
    assert_eq!(outcome, SubscribeOutcome::Accepted { heads: vec![head(&a1), head(&b0)] }, "each origin's head, by seq and hash");
    let empty = within(reader.subscribe_outcome("ws-app", "ws.empty", None)).await.unwrap();
    assert_eq!(empty, SubscribeOutcome::Accepted { heads: vec![] }, "an empty zone's ack names no origin");

    a.close().await;
    b.close().await;
    reader.close().await;
    node.kill().await.ok();
}

/// Answer 4 and R7: a subscribe catches a refused chain up, so the refused
/// client's next op lands on the node's op, at seq 1.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_chain_resumes_after_a_subscribe() {
    let tmp = Tmp::new("resumes");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let url = format!("ws://127.0.0.1:{port}");
    let first = GladeClient::new("w");
    first.connect(&url).await.unwrap();
    let (held, _) = within(first.append_outcome("ws-app", "ws.state", "value", b"first".to_vec(), None)).await.unwrap();
    let second = GladeClient::new("w");
    second.connect(&url).await.unwrap();
    let (_, refused) = within(second.append_outcome("ws-app", "ws.state", "value", b"second".to_vec(), None)).await.unwrap();
    assert!(matches!(refused, OpOutcome::Refused { code: ErrorCode::Equivocation, .. }), "{refused:?}");

    within(second.subscribe("ws-app", "ws.state", None)).await.unwrap();
    let (op, outcome) = within(second.append_outcome("ws-app", "ws.state", "value", b"third".to_vec(), None)).await.unwrap();
    let on_held = Some(op_hash(&held).to_vec());
    assert_eq!((op.seq, &op.prev, outcome), (1, &on_held, OpOutcome::Accepted), "the refused client lands at seq 1, on the node's op");

    first.close().await;
    second.close().await;
    node.kill().await.ok();
}

/// A frame the session cannot take fails the subscribe waiting on its zone. A
/// raw writer puts a `stream` op, which no client folds, where the replay
/// carries it.
#[tokio::test(flavor = "multi_thread")]
async fn a_replay_the_session_cannot_take_fails_its_subscribe() {
    let tmp = Tmp::new("untakeable");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let (mut raw_in, raw) = ws::connect("127.0.0.1", port).await.unwrap();
    let op = Op { share: "ws-app".into(), glade_id: "ws.feed".into(), key: vec![], origin: "raw".into(), seq: 0, prev: None, lamport: 1, refs: vec![], shape: Shape::Stream, payload: b"x".to_vec() };
    let mut frame = vec![FrameType::Ops.wire() as u8];
    frame.extend(cbor::encode(&Ops { ops: vec![op], pri: None }.to_cbor()));
    raw.send_binary(&frame).await.unwrap();
    let Ok(Msg::Binary(answer)) = within(raw_in.read()).await else {
        panic!("no status for the raw op");
    };
    let status = generated::Error::from_cbor(&cbor::decode(&answer[1..]));
    assert_eq!(status.code, ErrorCode::Ok, "the node holds the raw op: {status:?}");

    let reader = GladeClient::new("reader");
    reader.connect(&format!("ws://127.0.0.1:{port}")).await.unwrap();
    let subscribed = within(reader.subscribe("ws-app", "ws.feed", None)).await;
    assert!(subscribed.is_err(), "a replay the session cannot take fails its subscribe, got {subscribed:?}");
    assert_eq!(subscribed.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);

    reader.close().await;
    node.kill().await.ok();
}
