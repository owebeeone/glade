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
//!
//! Requires the node binary; the harness builds it once if absent.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use glade_client::supplier::{Supplier, SupplierConfig, SupplierSurface};
use glade_client::{Backoff, GladeClient};

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
