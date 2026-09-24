//! The switch between the node's two composition roots (plan Step 3.2, the
//! owner's ruling of 2026-09-24 recorded under it). `GLADE_NODE_ASSEMBLED`
//! unset starts the hand-written root; `1` starts the assembled root, which
//! resolves its bindings from `glade_node::assembly::NodeAssembly`; any other
//! value refuses to start, exits 1, names the variable and writes nothing.
//! Either root starts a node that prints the same lines.
//!
//! Each test sets or removes the variable on the node it spawns, so it reads
//! the same whichever way the suite runs (the node gate runs it both ways).
//! One test starts each root twice with an app file whose seed names a share
//! no `workspace` line declares, the second time with a `revoke` line added
//! (plan Step 4.3), and another starts each root with and without
//! `--enforce-client-grants` and subscribes from a websocket session. The last
//! tests check plan Step 4.2's endpoint key (one id across starts, and a
//! replaced key's binding revoked), and start each root on an instance written
//! before plan Step 4.1b signed its records.
//! Every file goes under a fresh directory in the system temp dir, and the node
//! runs with `GLADE_HOME` and `HOME` pointed there: `~/.glade` is never touched.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use glade_node::appdecl::{load, register, AppDecl};
use glade_node::assembly::ASSEMBLED_ROOT_LINE;
use glade_node::envelope;
use glade_node::frame::Frame;
use glade_node::grants::CLIENT_GRANTS_ENFORCED;
use glade_node::mesh::who_serves;
use glade_node::registry::{BlobStore, Record, Registry, RegistryApi, StoreApi, HOME};
use glade_node::store::Store;
use glade_node::sysdata::{NodeRecord, ServeClaim};
use glade_node::sysdir::{boot_at, now_ms};
use glade_node::transport::Bound;
use glade_node::ws;
use glade_wire::cbor::{self, Cbor};
use glade_wire::generated::{Hello, Op, Shape, Subscribe};

const VARIABLE: &str = "GLADE_NODE_ASSEMBLED";

/// How long a node may take to start, or to be refused, before the test fails.
const BOUND: Duration = Duration::from_secs(20);

/// Which root a spawned node is asked for.
#[derive(Clone, Copy, Debug)]
enum Root {
    HandWritten,
    Assembled,
    Value(&'static str),
}

/// A fresh directory for one test, with an empty `glade-home` inside it.
fn scratch(test: &str) -> PathBuf {
    let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
    let name = format!("glade-node-{test}-{}-{nanos}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    std::fs::create_dir_all(dir.join("glade-home")).unwrap();
    dir
}

fn glade_node(home: &Path, root: Root, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
    command
        .args(args)
        .env("GLADE_HOME", home)
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match root {
        Root::HandWritten => {
            command.env_remove(VARIABLE);
        }
        Root::Assembled => {
            command.env(VARIABLE, "1");
        }
        Root::Value(value) => {
            command.env(VARIABLE, value);
        }
    }
    command
}

/// A node asked for `root` must be refused: it exits within the bound, and
/// nothing is written under `home`. Returns its exit status and stderr.
fn refused(home: &Path, root: Root, args: &[&str]) -> (ExitStatus, String) {
    let (status, stderr) = ended(home, root, args);
    let written: Vec<_> = std::fs::read_dir(home).unwrap().collect();
    assert!(written.is_empty(), "written under GLADE_HOME: {written:?}");
    (status, stderr)
}

/// A node asked for `root` must end by itself, within the bound. Returns its
/// exit status and stderr.
fn ended(home: &Path, root: Root, args: &[&str]) -> (ExitStatus, String) {
    let mut node = glade_node(home, root, args)
        .spawn()
        .expect("spawn glade-node");
    let deadline = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = node.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = node.kill();
            let _ = node.wait();
            panic!("glade-node ({root:?}) still ran after {BOUND:?}: the start was not refused");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    node.stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    (status, stderr)
}

/// A node started from `root`, its stdout read up to its `listening <port>`
/// line, and left running until `stop`.
struct Running {
    node: Child,
    lines: Vec<String>,
}

impl Running {
    fn start(home: &Path, root: Root, args: &[&str]) -> Running {
        let mut node = glade_node(home, root, args)
            .spawn()
            .expect("spawn glade-node");
        let stdout = node.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let last = line.starts_with("listening ");
                if tx.send(line).is_err() || last {
                    break;
                }
            }
        });
        let deadline = Instant::now() + BOUND;
        let mut lines: Vec<String> = Vec::new();
        let outcome = loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) => {
                    let last = line.starts_with("listening ");
                    lines.push(line);
                    if last {
                        break Ok(());
                    }
                }
                Err(RecvTimeoutError::Timeout) => break Err("did not print `listening` in time"),
                Err(RecvTimeoutError::Disconnected) => break Err("stopped before `listening`"),
            }
        };
        let running = Running { node, lines };
        if let Err(why) = outcome {
            let lines = running.lines.clone();
            let stderr = running.stop();
            let _ = reader.join();
            panic!("glade-node ({root:?}) {why}: stdout {lines:?}, stderr {stderr}");
        }
        let _ = reader.join();
        running
    }

    /// Kill the node; what it wrote to stderr.
    fn stop(mut self) -> String {
        let _ = self.node.kill();
        let _ = self.node.wait();
        let mut stderr = String::new();
        let pipe = self.node.stderr.take();
        pipe.unwrap().read_to_string(&mut stderr).unwrap();
        stderr
    }
}

/// Start a node, read its stdout up to its `listening <port>` line, then stop
/// it. Returns the stdout lines read and everything it wrote to stderr.
fn start_and_stop(home: &Path, root: Root, args: &[&str]) -> (Vec<String>, String) {
    let node = Running::start(home, root, args);
    let lines = node.lines.clone();
    (lines, node.stop())
}

/// A line's first word: what the line reports, without the values (a port,
/// a node id, a path) that differ from one start to the next.
fn kinds(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line.split(' ').next().unwrap_or(""))
        .collect()
}

fn names_the_assembled_root(stderr: &str) -> bool {
    stderr.lines().any(|line| line == ASSEMBLED_ROOT_LINE)
}

/// Any value but `1`, the empty string included, refuses the start before the
/// node reads a file or writes anything: exit 1, and a stderr line that names
/// the variable.
#[test]
fn a_value_other_than_1_refuses_to_start_naming_the_variable() {
    for value in ["0", "true", "", "1 "] {
        let dir = scratch("assembled-bad-value");
        let home = dir.join("glade-home");
        let args = ["--profile", "local", "--name", "t", "0"];
        let (status, stderr) = refused(&home, Root::Value(value), &args);
        assert_eq!(status.code(), Some(1), "{VARIABLE}={value:?}: {stderr}");
        assert!(
            stderr.lines().any(|line| line.starts_with(VARIABLE)),
            "{VARIABLE}={value:?}: no stderr line names the variable: {stderr}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// The legacy serve form (`glade-node <port> <store_dir>`, no instance) starts
/// the same way from either root. Only the assembled root names itself.
#[test]
fn both_roots_start_the_legacy_form_alike() {
    let dir = scratch("assembled-legacy");
    let home = dir.join("glade-home");
    for (root, store) in [(Root::HandWritten, "store-h"), (Root::Assembled, "store-a")] {
        let store = dir.join(store).display().to_string();
        let (lines, stderr) = start_and_stop(&home, root, &["0", &store]);
        assert_eq!(kinds(&lines), ["listening"], "{root:?}");
        let assembled = matches!(root, Root::Assembled);
        assert_eq!(
            names_the_assembled_root(&stderr),
            assembled,
            "{root:?}: {stderr}"
        );
    }
    let written: Vec<_> = std::fs::read_dir(&home).unwrap().collect();
    assert!(
        written.is_empty(),
        "the legacy form wrote under GLADE_HOME: {written:?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The booted form, with an app file declaring a workspace, starts the same
/// way from either root: the same lines in the same order, the registration
/// and serving lines word for word. Each root boots its own instance.
#[test]
fn both_roots_boot_register_and_serve_alike() {
    let dir = scratch("assembled-booted");
    let app = dir.join("x.glade");
    let text = "glade-app v1\napp x\n\
                binding x.one value share commons latest\n\
                workspace ws-x notes\n";
    std::fs::write(&app, text).unwrap();
    let app = app.display().to_string();
    let mut starts = Vec::new();
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "--app", &app, "0"];
        let (lines, stderr) = start_and_stop(&dir.join("glade-home"), root, &args);
        let assembled = matches!(root, Root::Assembled);
        assert_eq!(
            names_the_assembled_root(&stderr),
            assembled,
            "{root:?}: {stderr}"
        );
        starts.push(lines);
    }
    let (hand, assembled) = (&starts[0], &starts[1]);
    let expected = [
        "instance",
        "node",
        "registry",
        "app",
        "peer",
        "workspace",
        "listening",
    ];
    assert_eq!(kinds(hand), expected, "{hand:?}");
    assert_eq!(kinds(assembled), expected, "{assembled:?}");
    for at in [2, 3, 5] {
        assert_eq!(hand[at], assembled[at], "line {at} differs");
    }
    assert_eq!(assembled[2], "registry ready (home served: true)");
    assert_eq!(assembled[3], "app x registered (+2 record(s), 0 unchanged)");
    assert_eq!(assembled[5], "workspace ws-x serving");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.3's part 1, on each root, over two starts of one instance. The
/// first start warns on stderr, on its line, of the seed whose share no loaded
/// `workspace` line declares, and registers it. For the second start the file
/// gains `revoke owner x`: the one revocation registers, and records.json's
/// fold then grants `owner` nothing on `x` and keeps its grant on `ws-x`.
#[test]
fn both_roots_warn_of_a_seeds_undeclared_share_and_register_a_revoke_line() {
    let dir = scratch("revoke-line");
    let home = dir.join("glade-home");
    let app = dir.join("x.glade");
    let seeds = "glade-app v1\napp x\n\
                 seed owner ws-x read.*\n\
                 seed owner x read.*,gwz.*\n\
                 workspace ws-x notes\n";
    let path = app.display().to_string();
    let warned = format!(
        "{path}: warning: line 4: no loaded `workspace` line declares the share `x`; the grant registers, \
         but a seed names a workspace share (expected on a node that reads a share another node serves)"
    );
    let warnings = |stderr: &str| -> Vec<String> {
        stderr
            .lines()
            .filter(|line| line.contains(": warning: "))
            .map(str::to_owned)
            .collect()
    };
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "--app", &path, "0"];
        std::fs::write(&app, seeds).unwrap();
        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(warnings(&stderr), [warned.as_str()], "{root:?}");
        let registered = "app x registered (+3 record(s), 0 unchanged)".to_string();
        assert!(lines.contains(&registered), "{root:?}: {lines:?}");

        std::fs::write(&app, format!("{seeds}revoke owner x\n")).unwrap();
        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(
            warnings(&stderr),
            [warned.as_str()],
            "{root:?}: the seed line stays"
        );
        let registered = "app x registered (+1 record(s), 3 unchanged)".to_string();
        assert!(lines.contains(&registered), "{root:?}: {lines:?}, {stderr}");
        let saved = BlobStore::new(home.join("sys").join(name)).load().unwrap();
        let (registry, quarantined) = Registry::from_snapshot(&saved);
        assert_eq!(quarantined, 0);
        assert_eq!(
            registry.grants_for("owner", "x"),
            Vec::<String>::new(),
            "{root:?}"
        );
        assert_eq!(registry.grants_for("owner", "ws-x"), ["read.*"], "{root:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The next frame a websocket session reads, within 5 s.
async fn ws_next(r: &mut ws::WsReader) -> Frame {
    let read = tokio::time::timeout(Duration::from_secs(5), r.read());
    match read.await.expect("the node answered").unwrap() {
        ws::Msg::Binary(bytes) => Frame::from_bytes(&bytes).unwrap(),
        ws::Msg::Close => panic!("the node closed the session"),
    }
}

/// A websocket session on the node at `port`, whose Hello claims
/// `principal`, if any, then subscribes `ws-x/x.one`: the number of zones an
/// accepted ack names, or the reason a refusal gives after its ack that names
/// no zone.
async fn ws_subscribe(port: u16, principal: Option<&str>) -> Result<usize, String> {
    let (mut r, w) = ws::connect("127.0.0.1", port).await.unwrap();
    if let Some(principal) = principal {
        let hello = Hello {
            session: "s".into(),
            protocol: 1,
            principal: Some(principal.into()),
            capability: None,
            heads: vec![],
        };
        w.send_binary(&Frame::Hello(hello).to_bytes())
            .await
            .unwrap();
        assert!(matches!(ws_next(&mut r).await, Frame::Welcome(_)));
    }
    let subscribe = Subscribe {
        share: "ws-x".into(),
        glade_id: "x.one".into(),
        key: None,
        from: None,
    };
    w.send_binary(&Frame::Subscribe(subscribe).to_bytes())
        .await
        .unwrap();
    match ws_next(&mut r).await {
        Frame::Heads(h) if h.streams.is_empty() => match ws_next(&mut r).await {
            Frame::Error(e) => Err(e.message),
            other => panic!("expected the reason, got {other:?}"),
        },
        Frame::Heads(h) => Ok(h.streams.len()),
        other => panic!("expected an ack, got {other:?}"),
    }
}

/// Plan Step 4.3's websocket switch, on each root. Without
/// `--enforce-client-grants`, the default, a session that names no principal
/// is served, as ever. With it, the node says so before it serves, refuses
/// that session's subscribe, and serves a session whose Hello claims `owner`,
/// whom the app file seeds `read.*` on the share.
#[test]
fn both_roots_check_client_grants_only_when_switched_on() {
    let dir = scratch("client-grants");
    let home = dir.join("glade-home");
    let app = dir.join("x.glade");
    let text = "glade-app v1\napp x\n\
                binding x.one value share commons latest\n\
                seed owner ws-x read.*\n\
                workspace ws-x notes\n";
    std::fs::write(&app, text).unwrap();
    let path = app.display().to_string();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let why =
        "unauthorized: a session that names no principal holds no grant of read.subscribe on ws-x";
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        for checked in [false, true] {
            let mut args = vec!["--profile", "local", "--name", name, "--app", &path];
            if checked {
                args.push("--enforce-client-grants");
            }
            args.push("0");
            let node = Running::start(&home, root, &args);
            let lines = node.lines.clone();
            let port = lines
                .iter()
                .find_map(|line| line.strip_prefix("listening "));
            let port: u16 = port.unwrap().parse().unwrap();
            let asked = async {
                [
                    ws_subscribe(port, None).await,
                    ws_subscribe(port, Some("owner")).await,
                ]
            };
            let answers = runtime.block_on(asked);
            let stderr = node.stop();
            let said = lines.iter().any(|line| line == CLIENT_GRANTS_ENFORCED);
            assert_eq!(said, checked, "{root:?}: {lines:?}");
            let expected = match checked {
                true => [Err(why.to_string()), Ok(1)],
                false => [Ok(1), Ok(1)],
            };
            assert_eq!(answers, expected, "{root:?}, checked {checked}: {stderr}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The endpoint id of a `peer <endpoint-id> <ip:port>` line among `lines`.
fn endpoint_id(lines: &[String]) -> String {
    let peer = lines.iter().find_map(|line| line.strip_prefix("peer "));
    let peer = peer.unwrap_or_else(|| panic!("no `peer` line in {lines:?}"));
    peer.split(' ').next().unwrap().to_string()
}

/// Plan Step 4.2 (the signing note's F1), on each root: a booted node's
/// endpoint id, which its `peer` line prints, is the same at every start of
/// one instance, and it is not the node id. Before, iroh drew a new endpoint
/// key at every bind. It does not dial the endpoint.
#[test]
fn both_roots_keep_one_endpoint_id_across_restarts() {
    let dir = scratch("endpoint-key");
    let home = dir.join("glade-home");
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "0"];
        let (first, _) = start_and_stop(&home, root, &args);
        let (second, stderr) = start_and_stop(&home, root, &args);
        let id = endpoint_id(&first);
        assert_eq!(endpoint_id(&second), id, "{root:?}: {second:?}, {stderr}");
        let node = second[1].strip_prefix("node ").unwrap();
        assert_ne!(id, node, "{root:?}: the endpoint key is not the node key");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.2, on each root: an operator replaces the endpoint key by
/// moving `endpoint.key` aside. The next start prints a new endpoint id and,
/// after `node`, that it revoked the old key's binding; records.json then
/// binds the new key and revokes the old one, both under the node's id.
#[test]
fn both_roots_revoke_a_replaced_endpoint_keys_binding() {
    let dir = scratch("endpoint-replaced");
    let home = dir.join("glade-home");
    let expected = [
        "instance",
        "node",
        "revoked",
        "registry",
        "peer",
        "listening",
    ];
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "0"];
        let (first, _) = start_and_stop(&home, root, &args);
        let instance = home.join("sys").join(name);
        let key = instance.join("endpoint.key");
        std::fs::rename(&key, instance.join("endpoint.key.old")).unwrap();
        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(kinds(&lines), expected, "{root:?}: {lines:?}, {stderr}");
        assert_eq!(lines[2], "revoked 1 binding(s) of replaced endpoint key(s)");
        let (old, new) = (endpoint_id(&first), endpoint_id(&lines));
        assert_ne!(old, new, "{root:?}");

        let node = lines[1].strip_prefix("node ").unwrap();
        let saved = BlobStore::new(&instance).load().unwrap();
        let (registry, quarantined) = Registry::from_snapshot(&saved);
        assert_eq!(quarantined, 0);
        let fold = registry.transport();
        let raw = |hex: &str| -> [u8; 32] {
            let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap();
            std::array::from_fn(|i| byte(2 * i))
        };
        let (node, old, new) = (raw(node), raw(&old), raw(&new));
        assert_eq!(
            fold.binds(&node, &old, now_ms()),
            Bound::Revoked,
            "{root:?}"
        );
        assert_eq!(fold.binds(&node, &new, now_ms()), Bound::Live, "{root:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.2b, on each root: B's door refuses A, whose key it does not
/// know. B says so on stderr, naming A's endpoint id, and A's line about
/// the failed dial carries no reason. B started again with `--peer <A's
/// endpoint id>` admits A, which links.
#[test]
fn both_roots_refuse_an_unknown_dialer_and_admit_a_configured_one() {
    let dir = scratch("door");
    let home = dir.join("glade-home");
    for (root, a, b) in [
        (Root::HandWritten, "ha", "hb"),
        (Root::Assembled, "aa", "ab"),
    ] {
        let start = |name: &str, peer: Option<&str>| {
            let peer = peer.map(|peer| ["--peer", peer]);
            let args = ["--profile", "local", "--name", name].into_iter();
            let args: Vec<&str> = args
                .chain(peer.into_iter().flatten())
                .chain(["0"])
                .collect();
            Running::start(&home, root, &args)
        };
        let dial = |b: &Running| format!("{}@{}", endpoint_id(&b.lines), port_line(&b.lines));
        let b_node = start(b, None);
        let target = dial(&b_node);
        let a_node = start(a, Some(&target));
        let a_key = endpoint_id(&a_node.lines);
        let linked = a_node.lines.iter().any(|l| l.starts_with("peer-connected"));
        assert!(!linked, "{root:?}: {:?}", a_node.lines);
        let (a_err, b_err) = (a_node.stop(), b_node.stop());
        let refused = format!("peer refused: endpoint {a_key}: unknown endpoint key");
        assert!(b_err.lines().any(|l| l == refused), "{root:?}: {b_err}");
        let failed = a_err
            .lines()
            .find(|l| l.starts_with(&format!("peer {target}: ")));
        assert!(
            failed.is_some_and(|l| !l.contains("unknown")),
            "{root:?}: {a_err}"
        );

        let b_node = start(b, Some(&a_key));
        let a_node = start(a, Some(&dial(&b_node)));
        let b_id = b_node.lines[1].strip_prefix("node ").unwrap().to_string();
        let linked = a_node
            .lines
            .iter()
            .find_map(|l| l.strip_prefix("peer-connected "));
        assert_eq!(linked, Some(b_id.as_str()), "{root:?}: {:?}", a_node.lines);
        drop((a_node.stop(), b_node.stop()));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The `<ip:port>` of a `peer <endpoint-id> <ip:port>` line among `lines`.
fn port_line(lines: &[String]) -> String {
    let peer = lines.iter().find_map(|line| line.strip_prefix("peer "));
    peer.unwrap().split(' ').nth(1).unwrap().to_string()
}

/// A key or a name as the store names its journals: lower-case hex.
fn hexed(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

/// What a node at `instance` wrote before plan Step 4.1b, unsigned, as every
/// build from 4.1a to 4.3 wrote it: its presence and home claim, `decl`
/// registered, and `decl`'s workspace `ws-x` served at epoch 3 on a live
/// lease, in records.json and in the served store's `home` journal, as the
/// store frames it. Returns the node's id and how many records it wrote.
fn written_before_the_step(instance: &Path, decl: &AppDecl) -> (String, usize) {
    let node = boot_at(instance.to_path_buf(), "local").unwrap().node_id;
    let claim = |share: &str, epoch| {
        Record::Serve(ServeClaim {
            node: node.clone(),
            share: share.into(),
            lease_expiry_ms: now_ms() + 60_000,
            epoch,
        })
    };
    let mut registry = Registry::new();
    let presence = NodeRecord {
        node_id: node.clone(),
        operator: "local".into(),
    };
    registry.append(Record::Node(presence), &node).unwrap();
    registry.append(claim(HOME, 1), &node).unwrap();
    register(decl, &mut registry, &node).unwrap();
    registry.append(claim("ws-x", 3), &node).unwrap();
    let snapshot = registry.snapshot();
    BlobStore::new(instance).save(&snapshot).unwrap();
    let home = instance.join("cache").join("store").join(hexed(HOME));
    std::fs::create_dir_all(&home).unwrap();
    let mut journal = Vec::new();
    for bytes in &snapshot.records {
        journal.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        journal.extend_from_slice(bytes);
    }
    std::fs::write(home.join(format!("{}.log", hexed(&node))), journal).unwrap();
    (node, snapshot.records.len())
}

/// Plan Step 4.1b, on each root: an instance a node wrote before the step,
/// whose records.json and served store hold its records unsigned. The node
/// starts under the same id, prints the unsigned records it set aside, then,
/// once the app has registered again, the served store's `home` journal it
/// set aside, and serves the workspace. Afterwards every record in
/// records.json and in the served store verifies, and `who_serves` answers
/// the node from each, the served store holding `ws-x`'s claim at epoch 1. A
/// second start sets nothing aside and registers nothing. The old records
/// are written through the node's registry code, unsealed, and the journal
/// as the store frames it; it does not run the old binary, which the step's
/// replay does.
#[test]
fn both_roots_set_an_unsigned_instance_aside_and_serve_signed() {
    let dir = scratch("unsigned");
    let home = dir.join("glade-home");
    let app = dir.join("x.glade");
    let text = "glade-app v1\napp x\n\
                binding x.one value share commons latest\n\
                workspace ws-x notes\n";
    std::fs::write(&app, text).unwrap();
    let decl = load(&app).unwrap();
    let app = app.display().to_string();
    let first = [
        "instance",
        "node",
        "set",
        "registry",
        "app",
        "set",
        "peer",
        "workspace",
        "listening",
    ];
    let again = [
        "instance",
        "node",
        "registry",
        "app",
        "peer",
        "workspace",
        "listening",
    ];
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let instance = home.join("sys").join(name);
        let (node, written) = written_before_the_step(&instance, &decl);
        let args = ["--profile", "local", "--name", name, "--app", &app, "0"];
        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(kinds(&lines), first, "{root:?}: {lines:?}, {stderr}");
        assert_eq!(lines[1], format!("node {node}"), "{root:?}: the same id");
        let aside = format!("set aside {written} unsigned record(s) in records.legacy-");
        assert!(lines[2].starts_with(&aside), "{root:?}: {}", lines[2]);
        assert_eq!(lines[4], "app x registered (+2 record(s), 0 unchanged)");
        let journal = format!(
            "set aside 1 journal(s) of the served store's home share ({written} record(s)) that do not verify, renamed *.legacy-"
        );
        assert!(lines[5].starts_with(&journal), "{root:?}: {}", lines[5]);

        let saved = BlobStore::new(&instance).load().unwrap();
        let ops: Vec<Op> = saved
            .records
            .iter()
            .map(|bytes| Op::from_cbor(&cbor::decode(bytes)))
            .collect();
        assert!(ops.iter().all(|op| op.origin == node), "{root:?}");
        let signed = ops.iter().all(|op| envelope::verify(op).is_ok());
        assert!(signed, "{root:?}: records.json signed");
        let (registry, quarantined) = Registry::from_snapshot(&saved);
        assert_eq!(quarantined, 0);
        assert_eq!(registry.who_serves("ws-x", now_ms()), Some(node.clone()));
        let store = Store::open(instance.join("cache").join("store")).unwrap();
        assert!(
            store.set_aside().is_none(),
            "{root:?}: the served store verifies"
        );
        assert_eq!(who_serves(&store, "ws-x", now_ms()), Some(node.clone()));
        let epochs: Vec<i64> = store
            .scan(HOME, "dir.claims", &[], &node, i64::MIN)
            .iter()
            .map(|op| envelope::record(op, ServeClaim::from_cbor))
            .filter(|claim| claim.share == "ws-x")
            .map(|claim| claim.epoch)
            .collect();
        assert_eq!(epochs, [1], "{root:?}: ws-x's claims");
        drop(store);

        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(kinds(&lines), again, "{root:?}: {lines:?}, {stderr}");
        assert_eq!(lines[3], "app x registered (+0 record(s), 2 unchanged)");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.1b's part 2's hardening, on each root: an instance whose
/// records.json holds a record a newer build might write (this build's
/// envelope, signed by the node, on a stream this build does not know). The
/// start is refused: the node exits 1 and says on stderr which record it
/// cannot read and what to do, with no panic, and records.json is as it was.
#[test]
fn both_roots_refuse_a_store_in_a_newer_format_with_a_clear_message() {
    let dir = scratch("newer");
    let home = dir.join("glade-home");
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let instance = home.join("sys").join(name);
        let boot = boot_at(instance.clone(), "local").unwrap();
        let (identity, node) = (boot.identity().unwrap(), boot.node_id.clone());
        let mut snapshot = boot.registry.snapshot();
        drop(boot);
        let record = Cbor::Map(vec![(1, Cbor::Text("recovery".into()))]);
        let op = Op {
            share: HOME.into(),
            glade_id: "dir.recovery-keys".into(),
            origin: node.clone(),
            shape: Shape::Log,
            payload: cbor::encode(&record),
            ..Op::default()
        };
        let op = Op {
            payload: envelope::seal(&identity, &op),
            ..op
        };
        snapshot.records.push(cbor::encode(&op.to_cbor()));
        BlobStore::new(&instance).save(&snapshot).unwrap();
        let written = std::fs::read(instance.join("records.json")).unwrap();

        let args = ["--profile", "local", "--name", name, "0"];
        let (status, stderr) = ended(&home, root, &args);
        assert_eq!(status.code(), Some(1), "{root:?}: {stderr}");
        let named = format!(
            "holds a home record this build cannot read (dir.recovery-keys of node {node} at seq 0)"
        );
        assert!(stderr.contains(&named), "{root:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{root:?}: {stderr}");
        let now = std::fs::read(instance.join("records.json")).unwrap();
        assert_eq!(now, written, "{root:?}: records.json as it was");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
