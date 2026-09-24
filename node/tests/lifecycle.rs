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
//! binary.
//!
//! A leaked handle does not appear in the report (the witness's README, "The
//! socket is the only honest witness"), so the ports are checked from outside
//! the plan. The check is shown to be able to fail: a port this file holds
//! reads as bound for the whole bound.

use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use glade_node::assembly::Settings;
use glade_node::lifecycle::{node_plan, Console, InstanceAt, NodeStart};
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

/// A booted start of the instance `name` under `dir`, dialing `peers`.
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
    NodeStart {
        settings: Settings::from_args(args),
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

/// The UDP port of a `peer <endpoint-id> <ip:port>` line.
fn peer_port(line: &str) -> u16 {
    let socket = line.rsplit(' ').next().unwrap();
    socket.rsplit(':').next().unwrap().parse().unwrap()
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

/// The done-when. B is started first; A boots, links to B with `--peer`, and
/// is stopped: a clean report with nothing incomplete, both of A's ports free
/// within the bound, and A's instance lock removed. B stops cleanly after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_links_to_a_peer_and_stops_clean_with_its_ports_free() {
    let dir = scratch("lifecycle");
    let rt = runtime();

    let b_lines = Arc::new(Lines::default());
    let mut b = node_plan().start(rt.clone(), booted(&dir, "b", &[], &b_lines));
    let steady = tokio::time::timeout(BOUND, b.ready()).await;
    assert_eq!(steady.ok(), Some(Ok(())), "B steady: {:?}", b_lines.all());
    let b_peer = b_lines.value("peer");
    let b_target = b_peer.replacen(' ', "@", 1);

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
    let stderr: Vec<String> = a_lines
        .all()
        .into_iter()
        .filter(|l| l.starts_with("stderr:"))
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
