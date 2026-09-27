//! The switch between the node's two composition roots (plan Step 3.2, the
//! owner's ruling of 2026-09-24 recorded under it). `GLADE_NODE_ASSEMBLED`
//! unset starts the hand-written root; `1` starts the assembled root, which
//! resolves its bindings from `glade_node::assembly::NodeAssembly`; any other
//! value refuses to start, exits 1, names the variable and writes nothing.
//! Either root starts a node that prints the same lines, refuses the legacy
//! form without its store directory, and boots under `GLADE_HOME`, else
//! `$HOME/.glade`. One test starts each root on an instance whose `home`
//! claim lapsed while its node was stopped, and one reads the lease each
//! root's claims carry by default.
//!
//! Each test sets or removes the variable on the node it spawns, so it reads
//! the same whichever way the suite runs (the node gate runs it both ways).
//! One test starts each root twice with an app file whose seed names a share
//! no `workspace` line declares, the second time with a `revoke` line added
//! (plan Step 4.3), and another starts each root with and without
//! `--enforce-client-grants` and subscribes from a websocket session. The last
//! tests check plan Step 4.2's endpoint key (one id across starts, and a
//! replaced key's binding revoked), and start each root on an instance written
//! before plan Step 4.1b signed its records, and on a damaged records.json.
//! Three check plan Step 4.1c on each root: the warning until a recovery key
//! is committed and the command it names, `--recovery-out` at a first boot
//! only, and a `local.json` that fails its check. Three check plan Step 4.5:
//! the network taken from a `--config` file, a bad file refused before
//! anything is written, and no line naming an endpoint id; every test that
//! needs an id reads it with `glade-node endpoint-id`, as an operator does.
//! One checks F9: a `--name` that is not an instance name is refused before
//! anything is written.
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
use glade_node::recovery::NOT_COMMITTED;
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
    ended_as(glade_node(home, root, args), root)
}

/// `ended`, for a node `command` spawns.
fn ended_as(mut command: Command, root: Root) -> (ExitStatus, String) {
    let mut node = command.spawn().expect("spawn glade-node");
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
        Running::start_as(glade_node(home, root, args), root)
    }

    /// `start`, for a node `command` spawns.
    fn start_as(mut command: Command, root: Root) -> Running {
        let mut node = command.spawn().expect("spawn glade-node");
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
    start_and_stop_as(glade_node(home, root, args), root)
}

/// `start_and_stop`, for a node `command` spawns.
fn start_and_stop_as(command: Command, root: Root) -> (Vec<String>, String) {
    let node = Running::start_as(command, root);
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

/// The legacy form requires its store directory (the owner's ruling of
/// 2026-09-26). Started with only a port, each root prints the usage line,
/// exits 1 as every refused start does, and writes nothing: not under
/// `GLADE_HOME`, and not in its temp dir, where the form once stored.
#[test]
fn both_roots_refuse_the_legacy_form_without_its_store_directory() {
    let dir = scratch("legacy-no-store");
    let (home, tmp) = (dir.join("glade-home"), dir.join("tmp"));
    std::fs::create_dir_all(&tmp).unwrap();
    let usage =
        "usage: glade-node <port> <store_dir> (the legacy form requires its store directory)";
    for root in [Root::HandWritten, Root::Assembled] {
        let mut command = glade_node(&home, root, &["0"]);
        command.env("TMPDIR", &tmp);
        let (status, stderr) = ended_as(command, root);
        assert_eq!(status.code(), Some(1), "{root:?}: {stderr}");
        let said = stderr.lines().any(|line| line.starts_with(usage));
        assert!(said, "{root:?}: no usage line: {stderr}");
        for place in [&home, &tmp] {
            let written: Vec<_> = std::fs::read_dir(place).unwrap().collect();
            assert!(written.is_empty(), "{root:?} wrote {written:?}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Each root reads the instance root as it starts, as before: `GLADE_HOME`
/// when it is set, whatever `HOME` says, and `$HOME/.glade` when it is not.
/// The other tests give the two one directory, so they cannot tell.
#[test]
fn both_roots_boot_under_glade_home_else_home_dot_glade() {
    let dir = scratch("instance-root");
    let (glade_home, home) = (dir.join("glade-home"), dir.join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let instance = |lines: &[String]| lines[0].strip_prefix("instance ").map(PathBuf::from);
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "0"];
        let mut command = glade_node(&glade_home, root, &args);
        command.env("HOME", &home);
        let (lines, stderr) = start_and_stop_as(command, root);
        let expected = glade_home.join("sys").join(name);
        assert_eq!(instance(&lines), Some(expected), "{root:?}: {stderr}");
        let written: Vec<_> = std::fs::read_dir(&home).unwrap().collect();
        assert!(written.is_empty(), "{root:?} wrote under HOME: {written:?}");

        let mut command = glade_node(&glade_home, root, &args);
        command.env_remove("GLADE_HOME").env("HOME", &home);
        let (lines, stderr) = start_and_stop_as(command, root);
        let expected = home.join(".glade").join("sys").join(name);
        assert_eq!(instance(&lines), Some(expected), "{root:?}: {stderr}");
        std::fs::remove_dir_all(home.join(".glade")).unwrap();
    }
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
        "app",
        "registry",
        "peer",
        "workspace",
        "listening",
    ];
    assert_eq!(kinds(hand), expected, "{hand:?}");
    assert_eq!(kinds(assembled), expected, "{assembled:?}");
    for at in [2, 3, 5] {
        assert_eq!(hand[at], assembled[at], "line {at} differs");
    }
    assert_eq!(assembled[2], "app x registered (+2 record(s), 0 unchanged)");
    assert_eq!(assembled[3], "registry ready (home served: true)");
    assert_eq!(assembled[5], "workspace ws-x serving");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The `home` claim renewed like any served share (the lane owner's ruling
/// of 2026-09-25), on each root: an instance whose `home` claim lapsed while
/// its node was stopped, written as the node writes it, signed. The start
/// renews the claim at adoption and prints `registry ready (home served:
/// true)`, and records.json then holds the claim live. Before, the line read
/// the lapsed claim, and nothing renewed it.
#[test]
fn both_roots_report_home_served_on_a_start_after_the_claim_lapsed() {
    let dir = scratch("home-lapsed");
    let home = dir.join("glade-home");
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let instance = home.join("sys").join(name);
        let boot = boot_at(instance.clone(), "local").unwrap();
        let (identity, node) = (boot.identity().unwrap(), boot.node_id.clone());
        drop(boot);
        let mut registry = Registry::sealed(identity);
        let presence = NodeRecord {
            node_id: node.clone(),
            operator: "local".into(),
        };
        registry.append(Record::Node(presence), &node).unwrap();
        let lapsed = ServeClaim {
            node: node.clone(),
            share: HOME.into(),
            lease_expiry_ms: now_ms() - 1,
            epoch: 1,
        };
        registry.append(Record::Serve(lapsed), &node).unwrap();
        BlobStore::new(&instance)
            .save(&registry.snapshot())
            .unwrap();

        let args = ["--profile", "local", "--name", name, "0"];
        let (lines, stderr) = start_and_stop(&home, root, &args);
        let expected = ["instance", "node", "registry", "peer", "listening"];
        assert_eq!(kinds(&lines), expected, "{root:?}: {lines:?}, {stderr}");
        let line = "registry ready (home served: true)";
        assert_eq!(lines[2], line, "{root:?}");
        let saved = BlobStore::new(&instance).load().unwrap();
        let serves = Registry::from_snapshot(&saved).0.who_serves(HOME, now_ms());
        assert_eq!(serves, Some(node), "{root:?}: records.json holds it live");
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

/// F1 (question 32 (a), the owner's ruling of 2026-09-27), on each root: by
/// default a node leases what it serves for five minutes. Every claim a start
/// mints, on `home` (its first boot's and adoption's renewal) and on the
/// workspace its app file declares, ends five minutes after it was minted.
/// Before, each ended after 30 s. The renewal every 100 s is the same
/// settings' other half (`claims.rs`), and `tests/lifecycle.rs` starts the
/// assembled root on settings of its own.
#[test]
fn both_roots_lease_their_claims_for_five_minutes_by_default() {
    const FIVE_MINUTES: i64 = 300_000;
    let dir = scratch("five-minutes");
    let home = dir.join("glade-home");
    let app = dir.join("x.glade");
    let text = "glade-app v1\napp x\n\
                binding x.one value share commons latest\n\
                workspace ws-x notes\n";
    std::fs::write(&app, text).unwrap();
    let app = app.display().to_string();
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "--app", &app, "0"];
        let started = now_ms();
        let (lines, stderr) = start_and_stop(&home, root, &args);
        let ran = now_ms() - started;
        let held = claims_held(&home.join("sys").join(name));
        let ends: Vec<(String, i64)> = held
            .into_iter()
            .map(|(share, expiry)| (share, expiry - started))
            .collect();
        let shares: Vec<&str> = ends.iter().map(|(share, _)| share.as_str()).collect();
        let minted = ["home", "home", "ws-x"];
        assert_eq!(shares, minted, "{root:?}: {lines:?}, {stderr}");
        for (share, end) in &ends {
            let leased = (FIVE_MINUTES..=FIVE_MINUTES + ran).contains(end);
            let said = format!("a claim on {share} ends {end} ms after the start: {ends:?}");
            assert!(leased, "{root:?}: {said}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.3's part 1, on each root, over two starts of one instance. The
/// first start warns on stderr, on its line, of the seed whose share no loaded
/// `workspace` line declares, and registers it. For the second start the file
/// gains `revoke owner x`: the one revocation registers, and records.json's
/// fold then grants `owner` nothing on `x` and keeps its grant on `ws-x`. That
/// start also warns, on the seed's line, that the `revoke` line cancels it
/// (F4).
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
        let cancelled = format!(
            "{path}: warning: line 4: `revoke owner x` on line 6 withdraws every grant of the pair, \
             for good; the grant registers, and allows nothing"
        );
        assert_eq!(
            warnings(&stderr),
            [warned.as_str(), cancelled.as_str()],
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

/// F4 (the owner's ruling of 2026-09-27), on each root: the odd spellings of
/// a grant, a verb with a `*` that does not end a pattern `p.*` (F14, here
/// `*` and `read*`) and a node's id written with capitals, and a seed that
/// another loaded file's `revoke` line cancels, are each warned on stderr, on
/// the seed's file and line, in that order; the start goes on, and the
/// revoking file is warned of nothing.
#[test]
fn both_roots_warn_of_odd_grant_spellings_and_a_seed_another_file_revokes() {
    let dir = scratch("odd-grants");
    let home = dir.join("glade-home");
    let capitals = "0123456789ABCDEF".repeat(4);
    let (seeds, revokes) = (dir.join("a.glade"), dir.join("b.glade"));
    let text = format!(
        "glade-app v1\napp a\nseed owner ws-x read.*,*,read*\nseed {capitals} ws-x read.*\nworkspace ws-x notes\n"
    );
    std::fs::write(&seeds, text).unwrap();
    std::fs::write(&revokes, "glade-app v1\napp b\nrevoke owner ws-x\n").unwrap();
    let (a, b) = (seeds.display().to_string(), revokes.display().to_string());
    let told = [
        format!(
            "{a}: warning: line 3: a `*` in the verb `*` matches only a `*`, for only a `*` that \
             ends a pattern `p.*` stands for more, as `read.*` allows every verb that begins \
             `read.`; the grant registers"
        ),
        format!(
            "{a}: warning: line 3: a `*` in the verb `read*` matches only a `*`, for only a `*` \
             that ends a pattern `p.*` stands for more, as `read.*` allows every verb that begins \
             `read.`; the grant registers"
        ),
        format!(
            "{a}: warning: line 4: the principal `{capitals}` is a node's id written with capitals, \
             which names no node: a node's id is lower-case hex, and a client may claim this name; \
             the line registers"
        ),
        format!(
            "{a}: warning: line 3: `revoke owner ws-x` on line 3 of {b} withdraws every grant of the \
             pair, for good; the grant registers, and allows nothing"
        ),
    ];
    let warnings = |stderr: &str| -> Vec<String> {
        let warned = stderr.lines().filter(|line| line.contains(": warning: "));
        warned.map(str::to_owned).collect()
    };
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let named = ["--profile", "local", "--name", name];
        let apps = ["--app", &a, "--app", &b, "0"];
        let args: Vec<&str> = named.into_iter().chain(apps).collect();
        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(warnings(&stderr), told, "{root:?}");
        let registered = "app b registered (+1 record(s), 0 unchanged)".to_string();
        assert!(lines.contains(&registered), "{root:?}: {lines:?}");
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

/// Run `glade-node` under `root` to its end, within the bound: its exit
/// status, its stdout lines and its stderr. A process still running at the
/// bound is killed, and the test fails with what it printed.
fn bounded(home: &Path, root: Root, args: &[&str]) -> (ExitStatus, Vec<String>, String) {
    let mut command = glade_node(home, root, args);
    let mut node = command.spawn().expect("spawn glade-node");
    let deadline = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = node.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = node.kill();
            let _ = node.wait();
            let stdout = drained(node.stdout.take());
            panic!("glade-node {args:?} ({root:?}) still ran after {BOUND:?}: {stdout}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = (drained(node.stdout.take()), drained(node.stderr.take()));
    (status, stdout.lines().map(str::to_owned).collect(), stderr)
}

/// What a process that has ended wrote to `pipe`.
fn drained(pipe: Option<impl Read>) -> String {
    let mut pipe = pipe.expect("a piped stream");
    let mut text = String::new();
    pipe.read_to_string(&mut text).unwrap();
    text
}

/// The endpoint id of the instance `name` under `home`, as `glade-node
/// endpoint-id` prints it (plan Step 4.5), its key minted first if it has
/// none. No line of a node's holds it.
fn endpoint_id(home: &Path, name: &str) -> String {
    let asked = ["endpoint-id", "--name", name];
    let (status, lines, stderr) = bounded(home, Root::HandWritten, &asked);
    assert_eq!(status.code(), Some(0), "{stderr}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    lines[0].clone()
}

/// The tag a line names the endpoint `id` by (plan Step 4.5): its first 10
/// hex digits.
fn tag(id: &str) -> &str {
    &id[..10]
}

/// The line a node's door refuses the endpoint `id` with, not knowing it.
fn refusal(id: &str) -> String {
    let tag = tag(id);
    format!("peer refused: endpoint {tag}: unknown endpoint key")
}

/// The value of the `peer <tag> <ip:port>` line among `lines`.
fn peer_line(lines: &[String]) -> &str {
    let peer = lines.iter().find_map(|line| line.strip_prefix("peer "));
    peer.unwrap_or_else(|| panic!("no `peer` line in {lines:?}"))
}

/// The tag of the `peer <tag> <ip:port>` line among `lines`.
fn peer_tag(lines: &[String]) -> String {
    peer_line(lines).split(' ').next().unwrap().to_string()
}

/// Plan Step 4.2 (the signing note's F1), on each root: a booted node's
/// endpoint id is the same at every start of one instance, and it is not the
/// node id. Before, iroh drew a new endpoint key at every bind. The `peer`
/// line names it by its tag, and `glade-node endpoint-id` prints it whole
/// (plan Step 4.5). It does not dial the endpoint.
#[test]
fn both_roots_keep_one_endpoint_id_across_restarts() {
    let dir = scratch("endpoint-key");
    let home = dir.join("glade-home");
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "0"];
        let (first, _) = start_and_stop(&home, root, &args);
        let (second, stderr) = start_and_stop(&home, root, &args);
        let named = peer_tag(&first);
        assert_eq!(peer_tag(&second), named, "{root:?}: {second:?}, {stderr}");
        let id = endpoint_id(&home, name);
        assert_eq!(tag(&id), named, "{root:?}: named by its tag");
        let node = second[1].strip_prefix("node ").unwrap();
        assert_ne!(id, node, "{root:?}: the endpoint key is not the node key");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.2, on each root: an operator replaces the endpoint key by
/// moving `endpoint.key` aside. The next start names a new endpoint and,
/// after `node`, says that it revoked the old key's binding; records.json
/// then binds the new key and revokes the old one, both under the node's
/// id. The ids come from `glade-node endpoint-id` (plan Step 4.5).
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
        start_and_stop(&home, root, &args);
        let old = endpoint_id(&home, name);
        let instance = home.join("sys").join(name);
        let key = instance.join("endpoint.key");
        std::fs::rename(&key, instance.join("endpoint.key.old")).unwrap();
        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(kinds(&lines), expected, "{root:?}: {lines:?}, {stderr}");
        assert_eq!(lines[2], "revoked 1 binding(s) of replaced endpoint key(s)");
        let new = endpoint_id(&home, name);
        assert_ne!(old, new, "{root:?}");
        assert_eq!(peer_tag(&lines), tag(&new), "{root:?}");

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
/// know. B says so on stderr, naming A's endpoint by its tag, and A's line
/// about the failed dial, which names B by its tag and the address it
/// dialed, carries no reason. B started again with `--peer <A's endpoint
/// id>` admits A, which links, and A notes on stdout its path to B and its
/// `home` round before `peer-connected`. Each id is minted and read with
/// `glade-node endpoint-id` before its node's first start (plan Step 4.5).
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
        let (a_key, b_key) = (endpoint_id(&home, a), endpoint_id(&home, b));
        let dial = |b: &Running| format!("{b_key}@{}", port_line(&b.lines));
        let b_node = start(b, None);
        let at_b = port_line(&b_node.lines);
        let a_node = start(a, Some(&dial(&b_node)));
        let linked = a_node.lines.iter().any(|l| l.starts_with("peer-connected"));
        assert!(!linked, "{root:?}: {:?}", a_node.lines);
        let (a_err, b_err) = (a_node.stop(), b_node.stop());
        let refused = refusal(&a_key);
        assert!(b_err.lines().any(|l| l == refused), "{root:?}: {b_err}");
        let failed = format!("peer {}@{at_b}: ", tag(&b_key));
        let failed = a_err.lines().find(|l| l.starts_with(&failed));
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
        // The notes on stdout (plan Step 4.5): the link at HELLO and the
        // round, then `peer-connected`.
        let at = |head: &str| a_node.lines.iter().position(|l| l.starts_with(head));
        let link = format!("link {b_id} via direct {}, rtt ", port_line(&b_node.lines));
        let round = format!("home round with node {b_id}: ");
        let order = [at(&link), at(&round), at("peer-connected ")];
        let ordered = order.iter().all(Option::is_some) && order.is_sorted();
        assert!(ordered, "{root:?}: {:?}", a_node.lines);
        drop((a_node.stop(), b_node.stop()));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The `<ip:port>` of the `peer <tag> <ip:port>` line among `lines`.
fn port_line(lines: &[String]) -> String {
    peer_line(lines).split(' ').nth(1).unwrap().to_string()
}

// A file's mode is a Unix notion. Each platform's branch is one braced
// module, so the condition encloses the whole section.
#[cfg(unix)]
mod files {
    use std::fs::{self, OpenOptions, Permissions};
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    /// Write `text` to a new file at `path`, mode `mode` whatever the umask.
    pub fn write(path: &Path, text: &str, mode: u32) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(mode);
        let mut file = options.open(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }
}

#[cfg(not(unix))]
mod files {
    use std::path::Path;

    /// Write `text` to a new file at `path`. Off Unix no mode is set, and the
    /// node checks none (F5).
    pub fn write(path: &Path, text: &str, _mode: u32) {
        std::fs::write(path, text).unwrap();
    }
}

/// Plan Step 4.5, on each root: a node takes its network from its
/// `--config` file. B, with no file, refuses A, whose file dials B at its
/// loopback address, and says so. B's file then admits A, and they link.
/// Each file is 0600, and each id comes from `glade-node endpoint-id`. Every
/// socket is on loopback, and no file names a relay.
#[test]
fn both_roots_take_their_network_from_the_config_file() {
    let dir = scratch("config");
    let home = dir.join("glade-home");
    for (root, a, b) in [
        (Root::HandWritten, "ha", "hb"),
        (Root::Assembled, "aa", "ab"),
    ] {
        let config = |name: String, text: String| {
            let path = dir.join(name);
            files::write(&path, &text, 0o600);
            path.display().to_string()
        };
        let start = |name: &str, config: Option<&str>| {
            let config = config.map(|path| ["--config", path]);
            let args = ["--profile", "local", "--name", name].into_iter();
            let args: Vec<&str> = args
                .chain(config.into_iter().flatten())
                .chain(["0"])
                .collect();
            Running::start(&home, root, &args)
        };
        let (a_key, b_key) = (endpoint_id(&home, a), endpoint_id(&home, b));
        let dials_b = |b: &Running, n: u32| {
            let text = format!("# A dials B\npeer {b_key}@{}\n", port_line(&b.lines));
            config(format!("{a}-{n}.conf"), text)
        };

        let b_node = start(b, None);
        let a_node = start(a, Some(&dials_b(&b_node, 1)));
        let linked = a_node.lines.iter().any(|l| l.starts_with("peer-connected"));
        assert!(!linked, "{root:?}: {:?}", a_node.lines);
        let (_, b_err) = (a_node.stop(), b_node.stop());
        let refused = refusal(&a_key);
        assert!(b_err.lines().any(|l| l == refused), "{root:?}: {b_err}");

        let text = format!("relay off\nbind 127.0.0.1:0\n\npeer {a_key} # A\n");
        let b_conf = config(format!("{b}.conf"), text);
        let b_node = start(b, Some(&b_conf));
        let a_node = start(a, Some(&dials_b(&b_node, 2)));
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

/// Plan Step 4.5, on each root: a `--config` file with one bad line, and on
/// Unix one that others can read, refuses the start before anything is
/// written: exit 1, the message naming the file, and no instance directory.
#[test]
fn both_roots_refuse_a_bad_config_file_before_writing() {
    let dir = scratch("bad-config");
    let home = dir.join("glade-home");
    let file = |name: &str, text: &str, mode: u32| {
        let path = dir.join(name);
        files::write(&path, text, mode);
        path.display().to_string()
    };
    let bad = file("bad.conf", "relay off\n# again\nrelay n0\n", 0o600);
    let mut cases = vec![(bad.clone(), format!("{bad}: line 3: a second relay line"))];
    if cfg!(unix) {
        let open = file("open.conf", "relay off\n", 0o644);
        let said = format!("{open} is group/world-accessible (mode 644) — refusing");
        cases.push((open, said));
    }
    for root in [Root::HandWritten, Root::Assembled] {
        for (path, said) in &cases {
            let args = ["--profile", "local", "--name", "n", "--config", path, "0"];
            let (status, stderr) = refused(&home, root, &args);
            assert_eq!(status.code(), Some(1), "{root:?}: {stderr}");
            assert!(stderr.lines().any(|l| l == said), "{root:?}: {stderr}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// F9 (the owner's ruling of 2026-09-27), on each root: an instance name
/// must match `[A-Za-z0-9._-]{1,63}` and not end in `.`. A start named
/// `../../outside`, which climbs out of `GLADE_HOME`, `..`, `.`, a name with
/// a `/`, the empty name, one of 64 characters or (F9 (b)) `n.`, which
/// Windows would take for `n`, is refused before anything is written: exit
/// 1, a stderr line quoting the name, and nothing under `GLADE_HOME` or
/// beside it. Before the check, `../../outside` booted and served from a
/// directory beside `GLADE_HOME`, and before F9 (b), `n.` served.
#[test]
fn both_roots_refuse_a_name_outside_sys_before_writing() {
    let dir = scratch("bad-name");
    let home = dir.join("glade-home");
    let too_long = "n".repeat(64);
    let names = [
        "../../outside",
        "..",
        ".",
        "a/b",
        "",
        too_long.as_str(),
        "n.",
    ];
    for root in [Root::HandWritten, Root::Assembled] {
        for name in names {
            let args = ["--profile", "local", "--name", name, "0"];
            let (status, stderr) = refused(&home, root, &args);
            assert_eq!(status.code(), Some(1), "{root:?} {name:?}: {stderr}");
            let said = format!(
                "--name {name:?}: an instance name must match [A-Za-z0-9._-]{{1,63}} and not end in a dot"
            );
            assert!(stderr.lines().any(|l| l == said), "{root:?}: {stderr}");
        }
    }
    let beside: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert_eq!(beside.len(), 1, "only GLADE_HOME: {beside:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.5, on each root: no line a node prints holds an endpoint id.
/// A links to B, and C, whose key B's door does not know, is refused. None
/// of the three ids is in any line any of them printed, on stdout or
/// stderr; where a line names an endpoint, it names it by its tag: each
/// node's `peer` line, B's refusal of C, and C's failed dial of B. No node
/// panicked, though B noted its link after this test stopped reading its
/// stdout.
#[test]
fn no_line_names_an_endpoint_id() {
    let dir = scratch("no-ids");
    let home = dir.join("glade-home");
    for (root, names) in [
        (Root::HandWritten, ["ha", "hb", "hc"]),
        (Root::Assembled, ["aa", "ab", "ac"]),
    ] {
        let ids = names.map(|name| endpoint_id(&home, name));
        let start = |name: &str, peer: &str| {
            let args = ["--profile", "local", "--name", name, "--peer", peer, "0"];
            Running::start(&home, root, &args)
        };
        let b_node = start(names[1], &ids[0]);
        let at_b = format!("{}@{}", ids[1], port_line(&b_node.lines));
        let a_node = start(names[0], &at_b);
        let c_node = start(names[2], &at_b);
        let said = [a_node, b_node, c_node].map(|node| (node.lines.clone(), node.stop()));
        // B noted its link to A, and its round, after its `listening`, when
        // this test no longer read its stdout: a note it could not write
        // stopped nothing (plan Step 4.5).
        for (_, err) in &said {
            assert!(!err.contains("panicked"), "{root:?}: {err}");
        }
        for id in &ids {
            let lines = said.iter().flat_map(|(out, err)| {
                let out = out.iter().map(String::as_str);
                out.chain(err.lines())
            });
            let holding: Vec<&str> = lines.filter(|line| line.contains(id.as_str())).collect();
            assert_eq!(holding, Vec::<&str>::new(), "{root:?}");
        }
        let [(a_out, _), (b_out, b_err), (c_out, c_err)] = &said;
        for (out, id) in [a_out, b_out, c_out].into_iter().zip(&ids) {
            assert_eq!(peer_tag(out), tag(id), "{root:?}: {out:?}");
        }
        let b_id = b_out[1].strip_prefix("node ").unwrap();
        let linked = a_out.iter().find_map(|l| l.strip_prefix("peer-connected "));
        assert_eq!(linked, Some(b_id), "{root:?}: {a_out:?}");
        let refused = refusal(&ids[2]);
        assert!(b_err.lines().any(|l| l == refused), "{root:?}: {b_err}");
        let failed = format!("peer {}@", tag(&ids[1]));
        let reported = c_err.lines().any(|l| l.starts_with(&failed));
        assert!(reported, "{root:?}: {c_err}");
        assert!(c_out.iter().all(|l| !l.starts_with("peer-connected")));
    }
    std::fs::remove_dir_all(&dir).unwrap();
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
        "app",
        "set",
        "registry",
        "peer",
        "workspace",
        "listening",
    ];
    let again = [
        "instance",
        "node",
        "app",
        "registry",
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
        assert_eq!(lines[3], "app x registered (+2 record(s), 0 unchanged)");
        let journal = format!(
            "set aside 1 journal(s) of the served store's home share ({written} record(s)) that do not verify, renamed *.legacy-"
        );
        assert!(lines[4].starts_with(&journal), "{root:?}: {}", lines[4]);

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
        assert_eq!(lines[2], "app x registered (+0 record(s), 2 unchanged)");
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
            glade_id: "dir.key-rotations".into(),
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
            "holds a home record this build cannot read (dir.key-rotations of node {node} at seq 0)"
        );
        assert!(stderr.contains(&named), "{root:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{root:?}: {stderr}");
        let now = std::fs::read(instance.join("records.json")).unwrap();
        assert_eq!(now, written, "{root:?}: records.json as it was");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The persistence suite on records.json (owner, 2026-09-24), on each root:
/// an instance whose records.json is damaged, here cut in half. The start is
/// refused: the node exits 1 and says on stderr which file it cannot read,
/// what is wrong and what to do, with no panic, and records.json is as it
/// was. The wire codec panicked on it.
#[test]
fn both_roots_refuse_a_damaged_records_json_with_a_clear_message() {
    let dir = scratch("damaged");
    let home = dir.join("glade-home");
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let instance = home.join("sys").join(name);
        drop(boot_at(instance.clone(), "local").unwrap());
        let records = instance.join("records.json");
        let whole = std::fs::read(&records).unwrap();
        let torn = &whole[..whole.len() / 2];
        std::fs::write(&records, torn).unwrap();

        let args = ["--profile", "local", "--name", name, "0"];
        let (status, stderr) = ended(&home, root, &args);
        assert_eq!(status.code(), Some(1), "{root:?}: {stderr}");
        let named = format!(
            "{} cannot be read as a snapshot (a torn or unreadable item): it is damaged, or not a records.json; move it aside to start without it",
            records.display()
        );
        assert!(stderr.contains(&named), "{root:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{root:?}: {stderr}");
        let now = std::fs::read(&records).unwrap();
        assert_eq!(now, torn, "{root:?}: records.json as it was");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A command asked of `glade-node` under `root`, run to its end: its exit
/// status, its stdout lines, and its stderr.
fn command(home: &Path, root: Root, args: &[&str]) -> (ExitStatus, Vec<String>, String) {
    let ran = glade_node(home, root, args)
        .output()
        .expect("run glade-node");
    let stdout = String::from_utf8(ran.stdout).unwrap();
    let lines = stdout.lines().map(str::to_owned).collect();
    (ran.status, lines, String::from_utf8(ran.stderr).unwrap())
}

/// The recovery key the node `node` of the instance at `instance` has
/// committed, as records.json holds it.
fn committed_key(instance: &Path, node: &str) -> Option<String> {
    let saved = BlobStore::new(instance).load().unwrap();
    Registry::from_snapshot(&saved).0.recovery_key(node)
}

/// Plan Step 4.1c (`GladeNodeSigning.md` D10 (a)), on each root: a booted
/// node that has committed no recovery key says on stderr exactly what to
/// run, under the instance root the entry point read, and starts. The
/// command it names, run on the stopped instance, commits the key and writes
/// its secret where it was told, and the next start says nothing of it.
#[test]
fn both_roots_warn_until_a_recovery_key_is_committed() {
    let dir = scratch("recovery");
    let home = dir.join("glade-home");
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_glade-node")).unwrap();
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let args = ["--profile", "local", "--name", name, "0"];
        let (lines, stderr) = start_and_stop(&home, root, &args);
        let warning = format!(
            "{NOT_COMMITTED}: stop it, then run GLADE_HOME={} {} recovery --name {name} --out <an absolute path outside GLADE_HOME>",
            home.display(),
            program.display()
        );
        assert!(stderr.lines().any(|l| l == warning), "{root:?}: {stderr}");

        let out = dir.join(format!("{name}.recovery"));
        let asked = ["recovery", "--name", name, "--out", out.to_str().unwrap()];
        let (status, said, stderr) = command(&home, root, &asked);
        assert_eq!(status.code(), Some(0), "{root:?}: {stderr}");
        let node = lines[1].strip_prefix("node ").unwrap();
        let instance = home.join("sys").join(name);
        let key = committed_key(&instance, node).expect("committed");
        let file = std::fs::canonicalize(&out).unwrap();
        let committed = format!(
            "recovery key {key} committed; wrote its secret to {}; this node keeps no copy: move the file offline now",
            file.display()
        );
        assert_eq!(said, [format!("node {node}"), committed], "{root:?}");
        assert_eq!(std::fs::metadata(&out).unwrap().len(), 32, "{root:?}");

        let (_, stderr) = start_and_stop(&home, root, &args);
        assert!(!stderr.contains(NOT_COMMITTED), "{root:?}: {stderr}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.1c, on each root: a first boot given `--recovery-out` commits
/// the key in the save that writes its presence, says so after `node`,
/// writes the secret there, and does not warn. A later start given the flag
/// is refused, and writes no file.
#[test]
fn both_roots_take_recovery_out_at_a_first_boot_only() {
    let dir = scratch("recovery-out");
    let home = dir.join("glade-home");
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let out = dir.join(format!("{name}.recovery"));
        let out = out.to_str().unwrap();
        let args = [
            "--profile",
            "local",
            "--name",
            name,
            "--recovery-out",
            out,
            "0",
        ];
        let (lines, stderr) = start_and_stop(&home, root, &args);
        let expected = [
            "instance",
            "node",
            "recovery",
            "registry",
            "peer",
            "listening",
        ];
        assert_eq!(kinds(&lines), expected, "{root:?}: {lines:?}, {stderr}");
        let node = lines[1].strip_prefix("node ").unwrap();
        let instance = home.join("sys").join(name);
        let key = committed_key(&instance, node).expect("committed");
        let file = std::fs::canonicalize(out).unwrap();
        let committed = format!(
            "recovery key {key} committed; wrote its secret to {}; this node keeps no copy: move the file offline now",
            file.display()
        );
        assert_eq!(lines[2], committed, "{root:?}");
        assert!(!stderr.contains(NOT_COMMITTED), "{root:?}: {stderr}");

        let again = dir.join(format!("{name}.again"));
        let again = again.to_str().unwrap();
        let args = [
            "--profile",
            "local",
            "--name",
            name,
            "--recovery-out",
            again,
            "0",
        ];
        let (status, stderr) = ended(&home, root, &args);
        assert_eq!(status.code(), Some(1), "{root:?}: {stderr}");
        let first_only = "--recovery-out is taken at a node's first boot only";
        assert!(stderr.contains(first_only), "{root:?}: {stderr}");
        assert!(!Path::new(again).exists(), "{root:?}: no file written");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.1c (D7), on each root: a `local.json` that fails its check,
/// here one that is not a signed overlay, is discarded to its fail-closed
/// defaults. The start says so on stderr, naming the file, and goes on. Until
/// the step the file was read and never checked, and nothing was said.
#[test]
fn both_roots_discard_a_local_json_that_fails_its_check() {
    let dir = scratch("local-json");
    let home = dir.join("glade-home");
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let instance = home.join("sys").join(name);
        std::fs::create_dir_all(&instance).unwrap();
        let local = instance.join("local.json");
        std::fs::write(&local, "{}").unwrap();
        let args = ["--profile", "local", "--name", name, "0"];
        let (lines, stderr) = start_and_stop(&home, root, &args);
        let discarded = format!(
            "{}: not a signed overlay; its assertions are discarded to their fail-closed defaults",
            local.display()
        );
        assert!(stderr.lines().any(|l| l == discarded), "{root:?}: {stderr}");
        assert_eq!(kinds(&lines).last(), Some(&"listening"), "{root:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
