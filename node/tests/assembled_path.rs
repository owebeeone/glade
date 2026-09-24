//! The switch between the node's two composition roots (plan Step 3.2, the
//! owner's ruling of 2026-09-24 recorded under it). `GLADE_NODE_ASSEMBLED`
//! unset starts the hand-written root; `1` starts the assembled root, which
//! resolves its bindings from `glade_node::assembly::NodeAssembly`; any other
//! value refuses to start, exits 1, names the variable and writes nothing.
//! Either root starts a node that prints the same lines.
//!
//! Each test sets or removes the variable on the node it spawns, so it reads
//! the same whichever way the suite runs (the node gate runs it both ways).
//! The last tests start each root on an instance written before plan Step
//! 4.1a changed the node id, and check plan Step 4.2's endpoint key: one id
//! across starts, and a replaced key's binding revoked.
//! Every file goes under a fresh directory in the system temp dir, and the node
//! runs with `GLADE_HOME` and `HOME` pointed there: `~/.glade` is never touched.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use glade_node::appdecl::{load, register, AppDecl};
use glade_node::assembly::ASSEMBLED_ROOT_LINE;
use glade_node::mesh::who_serves;
use glade_node::registry::{BlobStore, Record, Registry, RegistryApi, StoreApi, HOME};
use glade_node::store::Store;
use glade_node::sysdata::{NodeRecord, ServeClaim};
use glade_node::sysdir::{boot_at, now_ms};
use glade_node::transport::Bound;
use glade_wire::cbor;
use glade_wire::generated::Op;
use sha2::{Digest, Sha256};

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
    let written: Vec<_> = std::fs::read_dir(home).unwrap().collect();
    assert!(written.is_empty(), "written under GLADE_HOME: {written:?}");
    (status, stderr)
}

/// Start a node, read its stdout up to its `listening <port>` line, then stop
/// it. Returns the stdout lines read and everything it wrote to stderr.
fn start_and_stop(home: &Path, root: Root, args: &[&str]) -> (Vec<String>, String) {
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
    let _ = node.kill();
    let _ = node.wait();
    let _ = reader.join();
    let mut stderr = String::new();
    node.stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    if let Err(why) = outcome {
        panic!("glade-node ({root:?}) {why}: stdout {lines:?}, stderr {stderr}");
    }
    (lines, stderr)
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

/// What a node at `instance` wrote before plan Step 4.1a, under the id its key
/// had then, `hex(sha256(node.key))`: its presence, `decl` registered, and
/// `decl`'s workspace `ws-x` served at epoch 3 on a live lease, in records.json
/// and in the served store. Returns that id and how many records it wrote.
fn written_before_the_step(instance: &Path, decl: &AppDecl) -> (String, usize) {
    drop(boot_at(instance.to_path_buf(), "local").unwrap());
    let seed = std::fs::read(instance.join("node.key")).unwrap();
    let old: String = Sha256::digest(&seed)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let claim = |share: &str, epoch| {
        Record::Serve(ServeClaim {
            node: old.clone(),
            share: share.into(),
            lease_expiry_ms: now_ms() + 60_000,
            epoch,
        })
    };
    let mut registry = Registry::new();
    let presence = NodeRecord {
        node_id: old.clone(),
        operator: "local".into(),
    };
    registry.append(Record::Node(presence), &old).unwrap();
    registry.append(claim(HOME, 1), &old).unwrap();
    register(decl, &mut registry, &old).unwrap();
    registry.append(claim("ws-x", 3), &old).unwrap();
    let snapshot = registry.snapshot();
    BlobStore::new(instance).save(&snapshot).unwrap();
    let mut store = Store::open(instance.join("cache").join("store")).unwrap();
    for bytes in &snapshot.records {
        store.append(Op::from_cbor(&cbor::decode(bytes))).unwrap();
    }
    (old, snapshot.records.len())
}

/// Plan Step 4.1a, on each root: an instance a node wrote before the step,
/// whose records.json and served store name it by its old id. The node
/// starts, prints its new id and the records it set aside, registers the app
/// again and serves the workspace. Afterwards records.json names only the new
/// id, and `who_serves` answers it from records.json and from the served
/// store, which holds `ws-x`'s claim at epoch 1. The old records are written
/// through the node's own registry and store code, as its boot, registration
/// and serving wrote them, and `node.key` by a boot of this build, the same
/// 32 bytes either way. It does not run the old binary.
#[test]
fn both_roots_set_an_old_instance_aside_and_serve_under_the_new_id() {
    let dir = scratch("transition");
    let home = dir.join("glade-home");
    let app = dir.join("x.glade");
    let text = "glade-app v1\napp x\n\
                binding x.one value share commons latest\n\
                workspace ws-x notes\n";
    std::fs::write(&app, text).unwrap();
    let decl = load(&app).unwrap();
    let app = app.display().to_string();
    let expected = [
        "instance",
        "node",
        "set",
        "registry",
        "app",
        "peer",
        "workspace",
        "listening",
    ];
    for (root, name) in [(Root::HandWritten, "h"), (Root::Assembled, "a")] {
        let instance = home.join("sys").join(name);
        let (old, written) = written_before_the_step(&instance, &decl);
        let args = ["--profile", "local", "--name", name, "--app", &app, "0"];
        let (lines, stderr) = start_and_stop(&home, root, &args);
        assert_eq!(kinds(&lines), expected, "{root:?}: {lines:?}, {stderr}");
        let new = lines[1].strip_prefix("node ").unwrap().to_string();
        assert_ne!(new, old);
        let aside = format!("set aside {written} record(s) of old node id {old} in ");
        assert!(lines[2].starts_with(&aside), "{root:?}: {}", lines[2]);
        assert_eq!(lines[4], "app x registered (+2 record(s), 0 unchanged)");

        let saved = BlobStore::new(&instance).load().unwrap();
        let origins: Vec<String> = saved
            .records
            .iter()
            .map(|bytes| Op::from_cbor(&cbor::decode(bytes)).origin)
            .collect();
        let only_new = origins.iter().all(|origin| *origin == new);
        assert!(only_new, "{root:?}: {origins:?}");
        let (registry, quarantined) = Registry::from_snapshot(&saved);
        assert_eq!(quarantined, 0);
        assert_eq!(registry.who_serves("ws-x", now_ms()), Some(new.clone()));
        let store = Store::open(instance.join("cache").join("store")).unwrap();
        assert_eq!(who_serves(&store, "ws-x", now_ms()), Some(new.clone()));
        let epochs: Vec<i64> = store
            .scan(HOME, "dir.claims", &[], &new, i64::MIN)
            .iter()
            .map(|op| ServeClaim::from_cbor(&cbor::decode(&op.payload)))
            .filter(|claim| claim.share == "ws-x")
            .map(|claim| claim.epoch)
            .collect();
        assert_eq!(epochs, [1], "{root:?}: ws-x's claims under the new id");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
