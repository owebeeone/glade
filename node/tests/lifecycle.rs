//! Plan Step 3.3's done-when, first half: a node started on the assembled path
//! links to a peer and stops with `report.incomplete` empty, and its ports can
//! be bound again within the witness's 2 s bound
//! (`dev-docs/async-witness/real/tests/peer_release.rs`, `RELEASE_BOUND`).
//!
//! Two nodes run in this process, each as its own run of the plan the
//! assembled root starts (`glade_node::lifecycle::node_plan`), with real iroh
//! endpoints and real TCP listeners on loopback. Each boots its own instance
//! under a fresh temporary directory, passed explicitly, so neither reads
//! `GLADE_HOME` and `~/.glade` is never touched. The report is read directly.
//! `tests/stop_signal.rs` covers the same stop driven by a signal to the
//! binary. One more test (F1) starts a node on leases of its own and reads
//! the claims its instance saved.
//!
//! A leaked handle does not appear in the report (the witness's README, "The
//! socket is the only honest witness"), so the ports are checked from outside
//! the plan. The check is shown to be able to fail: a port this file holds
//! reads as bound for the whole bound.

use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use glade_node::appdecl::parse;
use glade_node::assembly::Settings;
use glade_node::cbor;
use glade_node::claims::Leases;
use glade_node::envelope;
use glade_node::lifecycle::{node_plan, Console, InstanceAt, NodeStart};
use glade_node::netconf;
use glade_node::registry::{BlobStore, StoreApi, HOME};
use glade_node::sysdata::ServeClaim;
use glade_node::sysdir::now_ms;
use glade_wire::generated::Op;
use sdax::{Outcome, Report};
use sdax_tokio::{PlanStart, TokioRuntime};

/// The witness's bound, and the node's own (`iroh_carrier.rs`'s tests).
const RELEASE_BOUND: Duration = Duration::from_secs(2);

/// How often to ask whether a port is free.
const POLL: Duration = Duration::from_millis(5);

/// How long a node may take to reach steady state or to stop.
const BOUND: Duration = Duration::from_secs(20);

/// The lines a node printed, stdout and stderr in one list, in order.
#[derive(Default)]
struct Lines(Mutex<Vec<String>>);

impl Console for Lines {
    fn out(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_owned());
    }

    fn err(&self, line: &str) {
        self.0.lock().unwrap().push(format!("stderr: {line}"));
    }
}

impl Lines {
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// The rest of the first line starting `word `, e.g. `listening 4711`.
    fn value(&self, word: &str) -> String {
        let prefix = format!("{word} ");
        self.all()
            .iter()
            .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
            .unwrap_or_else(|| panic!("no `{word}` line in {:?}", self.all()))
    }
}

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
    let name = format!("glade-node-{test}-{}-{nanos}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The endpoint id of the instance `name` under `dir`, booted once so its
/// keys exist: what `glade-node endpoint-id` prints, since no line of a
/// node's holds it (plan Step 4.5).
fn endpoint_id(dir: &Path, name: &str) -> String {
    let boot = glade_node::sysdir::boot_at(dir.join("sys").join(name), "local").unwrap();
    glade_node::transport::hex(&boot.endpoint_key().endpoint_id)
}

/// A booted start of the instance `name` under `dir`, with `peers` as its
/// `--peer` entries: dialed with an address, only admitted without one. The
/// test is the composition root here, so it loads the network as a root
/// does (plan Step 4.5).
fn booted(dir: &Path, name: &str, peers: &[String], lines: &Arc<Lines>) -> NodeStart {
    let mut args = vec![
        "--profile".to_owned(),
        "local".into(),
        "--name".into(),
        name.into(),
    ];
    for peer in peers {
        args.push("--peer".into());
        args.push(peer.clone());
    }
    args.push("0".into());
    let instance = InstanceAt {
        dir: dir.join("sys").join(name),
        operator: "local".into(),
    };
    let mut settings = Settings::from_args(args);
    settings.network = netconf::load(None, &settings.peers).unwrap();
    NodeStart {
        settings,
        decls: Vec::new(),
        instance: Some(instance),
        console: lines.clone(),
    }
}

fn runtime() -> Arc<TokioRuntime> {
    Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()))
}

/// Wait for `port` to be bindable again, bounded, and say how long it took.
/// `None` means it was still bound at the bound.
async fn free_after(port: u16, bind: fn(u16) -> bool) -> Option<Duration> {
    let started = Instant::now();
    loop {
        if bind(port) {
            return Some(started.elapsed());
        }
        if started.elapsed() >= RELEASE_BOUND {
            return None;
        }
        tokio::time::sleep(POLL).await;
    }
}

fn udp(port: u16) -> bool {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

fn tcp(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// The address of a `peer <tag> <ip:port>` line (plan Step 4.5).
fn peer_address(line: &str) -> &str {
    line.rsplit(' ').next().unwrap()
}

/// The UDP port of a `peer <tag> <ip:port>` line.
fn peer_port(line: &str) -> u16 {
    let port = peer_address(line).rsplit(':').next().unwrap();
    port.parse().unwrap()
}

fn shown(report: &Report) -> String {
    format!("{report}")
}

/// The check can answer "still bound", and takes the whole bound to say so.
/// Without this, a `Some(..)` below could mean the check is vacuous rather
/// than that the port is free.
#[tokio::test(flavor = "multi_thread")]
async fn the_release_check_can_answer_still_bound() {
    let udp_held = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let tcp_held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (u, t) = (
        udp_held.local_addr().unwrap().port(),
        tcp_held.local_addr().unwrap().port(),
    );
    let started = Instant::now();
    let (u_free, t_free) = tokio::join!(free_after(u, udp), free_after(t, tcp));
    assert_eq!(
        (u_free, t_free),
        (None, None),
        "a held port never reads as free"
    );
    assert!(started.elapsed() >= RELEASE_BOUND);
    drop((udp_held, tcp_held));
    assert!(free_after(u, udp).await.is_some());
    assert!(free_after(t, tcp).await.is_some());
}

/// The done-when. B is started first, admitting A's endpoint key (plan Step
/// 4.2b); A boots, links to B with `--peer`, and is stopped: a clean report
/// with nothing incomplete, both of A's ports free within the bound, and A's
/// instance lock removed. B stops cleanly after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_links_to_a_peer_and_stops_clean_with_its_ports_free() {
    let dir = scratch("lifecycle");
    let rt = runtime();

    let b_lines = Arc::new(Lines::default());
    let (a_key, b_key) = (endpoint_id(&dir, "a"), endpoint_id(&dir, "b"));
    let mut b = node_plan().start(rt.clone(), booted(&dir, "b", &[a_key], &b_lines));
    let steady = tokio::time::timeout(BOUND, b.ready()).await;
    assert_eq!(steady.ok(), Some(Ok(())), "B steady: {:?}", b_lines.all());
    let b_peer = b_lines.value("peer");
    let b_target = format!("{b_key}@{}", peer_address(&b_peer));

    let a_lines = Arc::new(Lines::default());
    let mut a = node_plan().start(rt.clone(), booted(&dir, "a", &[b_target], &a_lines));
    let steady = tokio::time::timeout(BOUND, a.ready()).await;
    assert_eq!(steady.ok(), Some(Ok(())), "A steady: {:?}", a_lines.all());

    // A linked to B: the HELLO completed and the home share was pulled.
    assert_eq!(a_lines.value("peer-connected"), b_lines.value("node"));
    let udp_port = peer_port(&a_lines.value("peer"));
    let tcp_port: u16 = a_lines.value("listening").parse().unwrap();
    let lock = dir.join("sys").join("a").join("instance.lock");
    assert!(lock.exists(), "A holds its instance while it runs");
    assert!(
        !udp(udp_port) && !tcp(tcp_port),
        "A holds its ports while it runs"
    );

    let stopping = Instant::now();
    a.handle().shutdown();
    let report = tokio::time::timeout(BOUND, a)
        .await
        .expect("A stops in time");
    println!(
        "A's report came back {:?} after the stop",
        stopping.elapsed()
    );
    assert_eq!(report.outcome, Outcome::Ok, "{}", shown(&report));
    assert!(report.incomplete.is_empty(), "{}", shown(&report));
    assert!(report.is_clean(), "{}", shown(&report));

    let (udp_free, tcp_free) = tokio::join!(free_after(udp_port, udp), free_after(tcp_port, tcp));
    let udp_free =
        udp_free.unwrap_or_else(|| panic!("UDP {udp_port} still bound at {RELEASE_BOUND:?}"));
    let tcp_free =
        tcp_free.unwrap_or_else(|| panic!("TCP {tcp_port} still bound at {RELEASE_BOUND:?}"));
    println!(
        "A's UDP port {udp_port} free after {udp_free:?}, TCP port {tcp_port} after {tcp_free:?}"
    );
    assert!(
        !lock.exists(),
        "A's instance lock is released with its storage"
    );
    // Nothing on stderr but plan Step 4.1c's warning of a node with no
    // recovery key committed.
    let warned = format!("stderr: {}", glade_node::recovery::NOT_COMMITTED);
    let stderr: Vec<String> = a_lines
        .all()
        .into_iter()
        .filter(|l| l.starts_with("stderr:") && !l.starts_with(&warned))
        .collect();
    assert_eq!(stderr, Vec::<String>::new());

    b.handle().shutdown();
    let report = tokio::time::timeout(BOUND, b)
        .await
        .expect("B stops in time");
    assert!(report.is_clean(), "{}", shown(&report));
    let b_udp = peer_port(&b_peer);
    assert!(
        free_after(b_udp, udp).await.is_some(),
        "B's UDP port {b_udp}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.2b on the assembled root: B admits no key, so A's dial is
/// refused at accept. B reports it on its console's stderr, naming A's
/// endpoint key; A's own report of the failed dial carries no reason. Both
/// still reach steady state and stop clean.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialer_its_peer_does_not_know_is_refused_and_reported() {
    let dir = scratch("lifecycle-door");
    let rt = runtime();
    let b_lines = Arc::new(Lines::default());
    let b_key = endpoint_id(&dir, "b");
    let mut b = node_plan().start(rt.clone(), booted(&dir, "b", &[], &b_lines));
    let steady = tokio::time::timeout(BOUND, b.ready()).await;
    assert_eq!(steady.ok(), Some(Ok(())), "B steady: {:?}", b_lines.all());
    let b_address = peer_address(&b_lines.value("peer")).to_owned();
    let target = format!("{b_key}@{b_address}");

    let a_lines = Arc::new(Lines::default());
    let a_start = booted(&dir, "a", std::slice::from_ref(&target), &a_lines);
    let mut a = node_plan().start(rt.clone(), a_start);
    let steady = tokio::time::timeout(BOUND, a.ready()).await;
    assert_eq!(steady.ok(), Some(Ok(())), "A steady: {:?}", a_lines.all());
    // Lines name an endpoint by its tag (plan Step 4.5).
    let a_tag = a_lines.value("peer").split(' ').next().unwrap().to_owned();
    let refused = format!("stderr: peer refused: endpoint {a_tag}: unknown endpoint key");
    assert!(b_lines.all().contains(&refused), "{:?}", b_lines.all());
    let failed = a_lines.value(&format!("stderr: peer {}@{b_address}:", &b_key[..10]));
    assert!(!failed.contains("unknown"), "a reason crossed: {failed}");
    assert!(!a_lines
        .all()
        .iter()
        .any(|l| l.starts_with("peer-connected")));

    for mut node in [a, b] {
        node.handle().shutdown();
        let report = tokio::time::timeout(BOUND, &mut node)
            .await
            .expect("stops in time");
        assert!(report.is_clean(), "{}", shown(&report));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The claims records.json at `instance` holds, in chain order: each one's
/// share and the instant its lease ends.
fn claims_held(instance: &Path) -> Vec<(String, i64)> {
    let saved = BlobStore::new(instance).load().unwrap();
    let ops = saved.records.iter();
    let ops = ops.map(|bytes| Op::from_cbor(&cbor::decode(bytes)));
    let claims = ops.filter(|op| op.glade_id == "dir.claims");
    let claims = claims.map(|op| envelope::record(&op, ServeClaim::from_cbor));
    let held = claims.map(|claim| (claim.share, claim.lease_expiry_ms));
    held.collect()
}

/// F1 (question 32 (a), the owner's ruling of 2026-09-27): the assembled
/// root leases and renews as its settings say. A node whose settings give a
/// one-minute lease renewed every 200 ms, with an app declaring a workspace,
/// runs for a second past steady state. Every claim it minted, on `home`
/// (its first boot's included) and on the workspace, ends a minute after it
/// was minted, and `home` was renewed at least three times, never more often
/// than every 200 ms. The binary's settings are the defaults, five minutes
/// renewed every 100 s (`tests/assembled_path.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn the_assembled_root_leases_and_renews_as_its_settings_say() {
    const LEASE: i64 = 60_000;
    const RENEW: u64 = 200;
    let dir = scratch("lifecycle-leases");
    let lines = Arc::new(Lines::default());
    let mut start = booted(&dir, "l", &[], &lines);
    start.settings.leases = Leases {
        lease_ms: LEASE,
        renew_ms: RENEW,
    };
    let text = "glade-app v1\napp x\n\
                binding x.one value share commons latest\n\
                workspace ws-x notes\n";
    start.settings.apps = vec!["x.glade".into()];
    start.decls = vec![parse(text).unwrap()];
    let started = now_ms();
    let mut run = node_plan().start(runtime(), start);
    let steady = tokio::time::timeout(BOUND, run.ready()).await;
    assert_eq!(steady.ok(), Some(Ok(())), "{:?}", lines.all());
    tokio::time::sleep(Duration::from_secs(1)).await;
    run.handle().shutdown();
    let report = tokio::time::timeout(BOUND, run)
        .await
        .expect("stops in time");
    assert!(report.is_clean(), "{}", shown(&report));
    let ran = now_ms() - started;

    let held = claims_held(&dir.join("sys").join("l"));
    let ends: Vec<(String, i64)> = held
        .into_iter()
        .map(|(share, expiry)| (share, expiry - started))
        .collect();
    for (share, end) in &ends {
        let leased = (LEASE..=LEASE + ran).contains(end);
        let said = format!("a claim on {share} ends {end} ms after the start: {ends:?}");
        assert!(leased, "the settings lease for {LEASE} ms: {said}");
    }
    assert!(ends.iter().any(|(share, _)| share == "ws-x"), "{ends:?}");
    // The first boot's claim on `home` and adoption's renewal, then the ticks.
    let ticks = ends.iter().filter(|(share, _)| share == HOME).count();
    let ticks = ticks.saturating_sub(2);
    let most = ran as u64 / RENEW;
    assert!(
        (3..=most).contains(&(ticks as u64)),
        "{ticks} renewals in {ran} ms, renewing every {RENEW} ms: {ends:?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The legacy form requires its store directory (the owner's ruling of
/// 2026-09-26). A plan started without one fails at `Storage`, before it
/// opens a store or binds a port, and its fault says why; it once served from
/// the temp dir.
#[tokio::test(flavor = "multi_thread")]
async fn the_legacy_form_without_its_store_directory_fails_at_storage() {
    let lines = Arc::new(Lines::default());
    let start = NodeStart {
        settings: Settings::from_args(["0".to_owned()]),
        decls: Vec::new(),
        instance: None,
        console: lines.clone(),
    };
    let mut run = node_plan().start(runtime(), start);
    let steady = tokio::time::timeout(BOUND, run.ready()).await;
    let started = steady.expect("the plan settles in time").is_ok();
    assert!(!started, "the plan started: {:?}", lines.all());
    let report = tokio::time::timeout(BOUND, run)
        .await
        .expect("ends in time");
    let faults: Vec<String> = report.faults.iter().map(|f| f.kind.to_string()).collect();
    let why = "the legacy form requires its store directory".to_owned();
    assert_eq!(faults, [why], "{}", shown(&report));
    assert_eq!(lines.all(), Vec::<String>::new());
}

/// The legacy form (no instance, no mesh) runs the same plan: it binds only
/// its listener, prints only `listening`, and stops clean with the port free.
#[tokio::test(flavor = "multi_thread")]
async fn the_legacy_form_stops_clean_with_its_port_free() {
    let dir = scratch("lifecycle-legacy");
    let lines = Arc::new(Lines::default());
    let store = dir.join("store").display().to_string();
    let start = NodeStart {
        settings: Settings::from_args(["0".to_owned(), store]),
        decls: Vec::new(),
        instance: None,
        console: lines.clone(),
    };
    let mut run = node_plan().start(runtime(), start);
    let steady = tokio::time::timeout(BOUND, run.ready()).await;
    assert_eq!(steady.ok(), Some(Ok(())), "{:?}", lines.all());
    let kinds: Vec<String> = lines
        .all()
        .iter()
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["listening"]);
    let port: u16 = lines.value("listening").parse().unwrap();

    run.handle().shutdown();
    let report = tokio::time::timeout(BOUND, run)
        .await
        .expect("stops in time");
    assert!(report.is_clean(), "{}", shown(&report));
    assert!(free_after(port, tcp).await.is_some(), "TCP {port}");
    std::fs::remove_dir_all(&dir).unwrap();
}
