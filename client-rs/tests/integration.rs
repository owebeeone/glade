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
//!      and a raw `stream` op is refused (F3), its zone left empty.
//!   6. the follow-ups ruled 2026-09-27: a `ShareController` whose chain a
//!      refusal stopped subscribes its surface again by itself (F7).
//!   7. a zone refused after its ack is reported, and no longer live (F13):
//!      two linked nodes, the claim holder refusing a forwarded read (F5).
//!   8. the route probe (plan Step 4.6 part 4), `examples/route_probe.rs`,
//!      driven a line at a time against a node that enforces client grants.
//!
//! Requires the node binary; the harness builds it once if absent. The route
//! probe is built into this suite's own target each time it runs.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use glade_client::hash::op_hash;
use glade_client::supplier::{Supplier, SupplierConfig, SupplierSurface};
use glade_client::ws::{self, Msg};
use glade_client::{Backoff, GladeClient, OpOutcome, SubscribeOutcome, ZoneRefusal};
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
    listening(child).await.1
}

/// `wait_listening`, with the lines the node printed before that one.
async fn listening(child: &mut Child) -> (Vec<String>, u16) {
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    let mut before = Vec::new();
    let port = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(line) = lines.next_line().await.ok().flatten() {
            if let Some(rest) = line.strip_prefix("listening ") {
                if let Ok(p) = rest.trim().parse::<u16>() {
                    return Some(p);
                }
            }
            before.push(line);
        }
        None
    })
    .await
    .ok()
    .flatten();
    let port = port.unwrap_or_else(|| panic!("node printed no listening port, after {before:?}"));
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    (before, port)
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

/// F3 (the owner's ruling of 2026-09-27): a `stream` op has no op path, so the
/// node refuses one from any client `Protocol` and keeps none of it. A raw
/// writer sends one, as no client can (`require_op` refuses it before a send):
/// its status is `Protocol`, and a fresh reader's subscribe to its zone then
/// completes with an empty zone. This test once had the node hold such an op,
/// so that a replay the session cannot take failed the reader's subscribe. No
/// node now takes one from a client, and a `stream` op was the only op a node
/// took that `apply_remote` refuses (the node's store refuses a malformed SWMR
/// envelope too), so that logic is covered by the pure test
/// `answers.rs::a_frame_the_session_cannot_take_fails_its_zones_subscribes`.
#[tokio::test(flavor = "multi_thread")]
async fn a_raw_stream_op_is_refused_and_its_zone_stays_empty() {
    let tmp = Tmp::new("stream-refused");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let (mut raw_in, raw) = ws::connect("127.0.0.1", port).await.unwrap();
    let op = Op { share: "ws-app".into(), glade_id: "ws.feed".into(), key: vec![], origin: "raw".into(), seq: 0, prev: None, lamport: 1, refs: vec![], shape: Shape::Stream, payload: b"x".to_vec() };
    let mut frame = vec![FrameType::Ops.wire() as u8];
    frame.extend(cbor::encode(&Ops { ops: vec![op], pri: None }.to_cbor()));
    raw.send_binary(&frame).await.unwrap();
    let Ok(Msg::Binary(answer)) = within(raw_in.read()).await else {
        panic!("no status for the raw op");
    };
    let status = generated::Error::from_cbor(&cbor::try_decode(&answer[1..]).unwrap()).unwrap();
    assert_eq!(status.code, ErrorCode::Protocol, "the node refuses the raw stream op: {status:?}");

    let reader = GladeClient::new("reader");
    reader.connect(&format!("ws://127.0.0.1:{port}")).await.unwrap();
    let outcome = within(reader.subscribe_outcome("ws-app", "ws.feed", None)).await.unwrap();
    assert_eq!(outcome, SubscribeOutcome::Accepted { heads: vec![] }, "the node kept none of the op: its zone is empty");

    reader.close().await;
    node.kill().await.ok();
}

// ---- 6. follow-ups ruled 2026-09-27 ----------------------------------------

/// F7: a `ShareController` surface whose chain a refusal stopped subscribes
/// again by itself, so its next write goes to the node instead of failing on
/// the stopped chain. Another client makes the zone a `crdt` one, so the node
/// refuses each of the supplier's value ops as a shape conflict.
#[tokio::test(flavor = "multi_thread")]
async fn a_share_controller_resubscribes_a_chain_a_refusal_stopped() {
    let tmp = Tmp::new("resubscribes");
    let (mut node, port) = spawn_legacy(&tmp.path().join("store"), 0).await;
    let url = format!("ws://127.0.0.1:{port}");
    let other = GladeClient::new("other");
    other.connect(&url).await.unwrap();
    let crdt = other.append_outcome("ws-app", "ws.state", "crdt", b"c".to_vec(), None);
    assert_eq!(
        within(crdt).await.unwrap().1,
        OpOutcome::Accepted,
        "the zone is a crdt one"
    );

    let client = GladeClient::new("sup");
    client.connect(&url).await.unwrap();
    let mut refusals = client.on_refused().await;
    let sup = Supplier::attach(client.clone(), SupplierConfig::default());
    let surface = SupplierSurface::new("ws-app", "ws.state", "value");
    let state = sup.serve_share(surface, |_| {}).await.unwrap();
    state.set(b"v0".to_vec()).await.unwrap();
    let first = within(refusals.recv()).await.expect("a refusal");
    assert_eq!(
        (first.code, first.op.payload),
        (ErrorCode::Protocol, b"v0".to_vec())
    );

    // The refusal stopped the chain. The controller subscribes again, and the
    // next write goes to the node, which answers it on its merits.
    let next = within(state.set(b"v1".to_vec())).await;
    assert!(
        next.is_ok(),
        "the next write must not be lost to the stopped chain, got {next:?}"
    );
    let second = within(refusals.recv())
        .await
        .expect("the node's answer to the next write");
    assert_eq!(
        (second.code, second.op.payload),
        (ErrorCode::Protocol, b"v1".to_vec())
    );

    sup.detach_all().await;
    other.close().await;
    node.kill().await.ok();
}

// ---- 7. a zone refused after its ack (F13) ---------------------------------

/// `glade-node endpoint-id --name <name>` under `tmp`'s GLADE_HOME, as an
/// operator reads it (plan Step 4.5): the instance's endpoint id, its key
/// minted first.
async fn endpoint_id(tmp: &Tmp, name: &str) -> String {
    ensure_node_built();
    let run = Command::new(node_bin())
        .args(["endpoint-id", "--name", name])
        .env("GLADE_HOME", tmp.path().join("gh"))
        .env("HOME", tmp.path().join("h"))
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let ran = tokio::time::timeout(Duration::from_secs(15), run).await;
    let out = ran.expect("endpoint-id in time").expect("endpoint-id ran");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// The instance `name` booted under `tmp`'s GLADE_HOME and HOME, with `args`
/// before its port, 0: the node, the lines it printed before `listening
/// <port>`, and the port.
async fn boot_as(tmp: &Tmp, name: &str, args: &[&str]) -> (Child, Vec<String>, u16) {
    ensure_node_built();
    let mut child = Command::new(node_bin())
        .args(["--profile", "local", "--name", name])
        .args(args)
        .arg("0")
        .env("GLADE_HOME", tmp.path().join("gh"))
        .env("HOME", tmp.path().join("h"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn booted glade-node");
    let (lines, port) = listening(&mut child).await;
    (child, lines, port)
}

/// The rest of the first of `lines` that starts `word `.
fn said<'a>(lines: &'a [String], word: &str) -> &'a str {
    let prefix = format!("{word} ");
    let found = lines.iter().find_map(|line| line.strip_prefix(&prefix));
    found.unwrap_or_else(|| panic!("no `{word}` line in {lines:?}"))
}

/// F13 (the owner's answer (a) to F5's question): a zone refused after its
/// subscribe was acked is reported to `on_zone_refused`, and is no longer
/// live. Two nodes on loopback, relays off (`--peer`): B loads
/// grazel-app.glade, so it serves ws-razel, and admits A's endpoint but
/// grants A's node nothing; A dials B. A acks a subscribe to ws-razel/ws.tree
/// from its empty replica, and B's refusal of the forwarded read then reaches
/// the client as an `Error` naming the zone and no op (F5). A later subscribe
/// asks again: acked, forwarded again, and refused again.
#[tokio::test(flavor = "multi_thread")]
async fn a_zone_refused_after_its_ack_is_reported_and_no_longer_live() {
    let tmp = Tmp::new("refused-after-ack");
    let (a_id, b_id) = (endpoint_id(&tmp, "a").await, endpoint_id(&tmp, "b").await);
    let app = manifest().join("../apps/grazel-app.glade");
    let app = app.to_str().unwrap();
    let (mut b, b_lines, _) = boot_as(&tmp, "b", &["--app", app, "--peer", &a_id]).await;
    let b_node = said(&b_lines, "node");
    // `peer <tag> <ip:port>`: where B's endpoint listens.
    let b_at = said(&b_lines, "peer").split(' ').nth(1).unwrap();
    let dial = format!("{b_id}@{b_at}");
    let (mut a, a_lines, a_port) = boot_as(&tmp, "a", &["--peer", &dial]).await;
    let round = format!("home round with node {b_node}");
    let linked = a_lines.iter().any(|line| line.starts_with(&round));
    assert!(linked, "A took no home round with B: {a_lines:?}");

    let client = GladeClient::new("reader");
    let at_a = format!("ws://127.0.0.1:{a_port}");
    client.connect(&at_a).await.unwrap();
    let mut refusals = client.on_zone_refused().await;
    let acked = within(client.subscribe_outcome("ws-razel", "ws.tree", None)).await;
    let empty = SubscribeOutcome::Accepted { heads: vec![] };
    assert_eq!(acked.unwrap(), empty, "A acks from its empty replica");

    let refused = within(refusals.recv()).await.expect("a zone refusal");
    let expected = ZoneRefusal {
        share: "ws-razel".into(),
        glade_id: "ws.tree".into(),
        key: vec![],
        code: ErrorCode::Unauthorized,
        message: refused.message.clone(),
    };
    assert_eq!(refused, expected);
    let by_b = format!("refused by node {b_node}, which serves ws-razel: ");
    assert!(refused.message.starts_with(&by_b), "{refused:?}");
    let live = client.live("ws-razel", "ws.tree", None).await;
    assert!(!live, "a zone refused after its ack is no longer live");

    // The refusal is not kept: a later subscribe asks A again, which acks it,
    // forwards the read again, and relays B's refusal again.
    let again = within(client.subscribe_outcome("ws-razel", "ws.tree", None)).await;
    assert_eq!(again.unwrap(), empty);
    let second = within(refusals.recv()).await.expect("a second refusal");
    assert_eq!(second, refused);
    assert!(!client.live("ws-razel", "ws.tree", None).await);

    client.close().await;
    a.kill().await.ok();
    b.kill().await.ok();
}

// ---- 8. the route probe (plan Step 4.6 part 4) ------------------------------

/// The route probe, `examples/route_probe.rs`, built into this suite's own
/// target: a run of this suite alone builds no example, and a fresh one is
/// not built again.
fn probe_built() -> PathBuf {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).parent().unwrap();
    let built = std::process::Command::new(env!("CARGO"))
        .args(["build", "--offline", "--example", "route_probe"])
        .arg("--manifest-path")
        .arg(manifest().join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .output()
        .expect("run cargo build");
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(built.status.success(), "the probe's build: {stderr}");
    target.join("debug/examples/route_probe")
}

/// A route probe's process: its commands in, its lines out, and the events it
/// printed before each answer.
struct Probe {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    events: Vec<String>,
}

impl Probe {
    /// `principal`'s probe on the node at `port`, once it prints its welcome.
    async fn start(bin: &Path, port: u16, principal: &str) -> Probe {
        let mut child = Command::new(bin)
            .arg(format!("ws://127.0.0.1:{port}"))
            .arg(principal)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the route probe");
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut probe = Probe {
            child,
            stdin,
            lines,
            events: vec![],
        };
        assert_eq!(probe.line().await, format!("welcome {principal}"));
        probe
    }

    /// The next line it prints, within 5 s.
    async fn line(&mut self) -> String {
        let line = within(self.lines.next_line()).await.unwrap();
        line.expect("the probe printed no more")
    }

    /// Send `command`, and return its answer; the events printed first are kept.
    async fn ask(&mut self, command: &str) -> String {
        let command = format!("{command}\n");
        self.stdin.write_all(command.as_bytes()).await.unwrap();
        loop {
            let line = self.line().await;
            let event = line.starts_with("op ") || line.starts_with("zone-refused ");
            if !event && line != "dropped" {
                return line;
            }
            self.events.push(line);
        }
    }
}

/// The route probe (plan Step 4.6 part 4), driven a line at a time as the route
/// journey drives it, against a node that enforces client grants and whose app
/// file seeds two: `alice` may read ws-route, and `writer` write there, which
/// the node checks since cross-node writes plan X4.1. The writer's appends and
/// its `resend-last` are `ok`; alice's subscribe is acked at the writer's head, its
/// replay arrives as events, each once, and her `log` holds it in order. Her
/// subscribe to a share she holds no grant on is refused, as is `mallory`'s,
/// who holds none, and no op reaches him. A command that cannot run is `error`,
/// and `quit` is `bye`, after which each probe exits 0.
#[tokio::test(flavor = "multi_thread")]
async fn the_route_probe_answers_each_command_on_a_line() {
    let bin = probe_built();
    let tmp = Tmp::new("route-probe");
    let app = tmp.path().join("route.glade");
    let text = "glade-app v1\napp route\nbinding route.notes log share commons from-cursor\n\
                seed alice ws-route read.subscribe\nseed writer ws-route write.append\n\
                workspace ws-route notes\n";
    std::fs::write(&app, text).unwrap();
    let args = ["--app", app.to_str().unwrap(), "--enforce-client-grants"];
    let (mut node, _, port) = boot_as(&tmp, "route", &args).await;

    let mut writer = Probe::start(&bin, port, "writer").await;
    let e1 = writer.ask("append ws-route/route.notes log e1").await;
    assert_eq!(e1, "ok ws-route/route.notes writer:0 e1");
    let e2 = writer.ask("append ws-route/route.notes log e2").await;
    assert_eq!(e2, "ok ws-route/route.notes writer:1 e2");
    assert_eq!(writer.ask("resend-last").await, e2, "held byte for byte");

    let mut alice = Probe::start(&bin, port, "alice").await;
    let acked = alice.ask("subscribe ws-route/route.notes").await;
    assert_eq!(acked, "acked ws-route/route.notes [writer:1]");
    let log = alice.ask("log ws-route/route.notes").await;
    assert_eq!(log, "log ws-route/route.notes [e1 e2]");
    let arrived = [
        "op ws-route/route.notes writer:0 e1",
        "op ws-route/route.notes writer:1 e2",
    ];
    assert_eq!(alice.events, arrived, "each op of the replay, once");

    let refused = |who: &str, share: &str| {
        format!(
            "refused {share}/route.notes Unauthorized: unauthorized: \
             principal {who} holds no grant of read.subscribe on {share}"
        )
    };
    let other = alice.ask("subscribe ws-other/route.notes").await;
    assert_eq!(other, refused("alice", "ws-other"));
    let mut mallory = Probe::start(&bin, port, "mallory").await;
    let theirs = mallory.ask("subscribe ws-route/route.notes").await;
    assert_eq!(theirs, refused("mallory", "ws-route"));
    let none = mallory.ask("log ws-route/route.notes").await;
    assert_eq!(none, "log ws-route/route.notes []");
    assert!(mallory.events.is_empty(), "{:?}", mallory.events);
    let bad = mallory.ask("subscribe ws-route").await;
    assert_eq!(bad, "error subscribe: a zone is <share>/<glade_id>");

    for mut probe in [writer, alice, mallory] {
        assert_eq!(probe.ask("quit").await, "bye");
        let status = within(probe.child.wait()).await.unwrap();
        assert!(status.success(), "{status}");
    }
    node.kill().await.ok();
}
