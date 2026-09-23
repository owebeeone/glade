//! An app is declared by one file (the amendment review's COD-P2-1 =
//! STA-P2-1). `register` takes a file as its app's whole binding set, so two
//! `--app` files naming one app would retract each other's bindings on every
//! start, and the store would grow for ever. `appdecl::load_all` refuses such
//! a start, and `glade-node` calls it before it opens its instance, so a
//! refused start writes nothing. Files naming different apps still converge.
//!
//! Every file these tests write goes under a fresh directory in the system
//! temp dir, and the node runs with `GLADE_HOME` and `HOME` pointed there:
//! the real `~/.glade` is never touched.

use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use glade_node::appdecl::{load_all, register, Registered};
use glade_node::registry::{BlobStore, Registry, RegistryApi, StoreApi};
use glade_node::sysdata::BindingDecl;

/// The registrant, as in the other integration tests.
const ORIGIN: &str = "node-1";

/// Two files naming the app `x`, each with a binding of its own.
const X_ONE: &str = "glade-app v1\napp x\nbinding x.one value share commons latest\n";
const X_TWO: &str = "glade-app v1\napp x\nbinding x.two value share commons latest\n";

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
    let name = format!("glade-node-{test}-{}-{nanos}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `text` to `dir/name`.
fn app_file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// `load_all` refuses two files naming one app, in either order, with one
/// line prefixed with the later file's path, as `load` prefixes its errors,
/// naming the app and the earlier file.
#[test]
fn load_all_refuses_two_files_naming_one_app_in_either_order() {
    let dir = scratch("load-all");
    let a1 = app_file(&dir, "a1.glade", X_ONE);
    let a2 = app_file(&dir, "a2.glade", X_TWO);
    for (earlier, later) in [(&a1, &a2), (&a2, &a1)] {
        let err = load_all(&[earlier, later]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        let (earlier, later) = (earlier.display(), later.display());
        assert_eq!(
            err.to_string(),
            format!("{later}: app `x` is already declared by {earlier} (an app is declared by one file)")
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `glade-node --profile local --name t --app a1.glade --app a2.glade 0`, the
/// two files naming one app: the node exits non-zero, its stderr names both
/// files, and nothing is written under its `GLADE_HOME`. A node still running
/// after 20 s was not refused: it is killed and the test fails.
#[test]
fn a_start_with_two_files_naming_one_app_is_refused_and_writes_nothing() {
    let dir = scratch("refused-start");
    let a1 = app_file(&dir, "a1.glade", X_ONE);
    let a2 = app_file(&dir, "a2.glade", X_TWO);
    let home = dir.join("glade-home");
    std::fs::create_dir(&home).unwrap();

    let mut node = Command::new(env!("CARGO_BIN_EXE_glade-node"))
        .args(["--profile", "local", "--name", "t", "--app"])
        .arg(&a1)
        .arg("--app")
        .arg(&a2)
        .arg("0")
        .env("GLADE_HOME", &home)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn glade-node");
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = node.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = node.kill();
            let _ = node.wait();
            panic!("glade-node still ran after 20 s: the start was not refused");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    let mut pipe = node.stderr.take().unwrap();
    pipe.read_to_string(&mut stderr).unwrap();

    assert!(!status.success(), "the start is refused: {status}");
    for file in [&a1, &a2] {
        let file = file.display().to_string();
        assert!(stderr.contains(&file), "stderr names {file}: {stderr}");
    }
    assert!(stderr.contains("app `x`"), "stderr names the app: {stderr}");
    let written: Vec<_> = std::fs::read_dir(&home).unwrap().collect();
    assert!(written.is_empty(), "written under GLADE_HOME: {written:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The control: two files naming different apps, loaded and registered on
/// three starts through a `BlobStore`, as `glade-node` does (register, then
/// save, file by file). The first start appends every record, and the next
/// two append nothing: the files converge.
#[test]
fn files_naming_different_apps_converge_over_three_starts() {
    let dir = scratch("three-starts");
    let x = "glade-app v1\napp x\n\
             binding x.one value share commons latest\n\
             binding x.two log   share commons from-cursor\n";
    let y = "glade-app v1\napp y\nbinding y.one value share commons latest\n";
    let files = [app_file(&dir, "x.glade", x), app_file(&dir, "y.glade", y)];
    let records = dir.join("records");

    let mut appended = Vec::new();
    for _ in 0..3 {
        let mut store = BlobStore::new(&records);
        let (mut reg, rejected) = Registry::from_snapshot(&store.load().unwrap());
        assert_eq!(rejected, 0);
        let mut this_start = 0;
        for decl in load_all(&files).unwrap() {
            let out: Registered = register(&decl, &mut reg, ORIGIN).unwrap();
            store.save(&reg.snapshot()).unwrap();
            this_start += out.appended;
        }
        appended.push(this_start);
    }
    assert_eq!(appended, [3, 0, 0]);

    let (reg, _) = Registry::from_snapshot(&BlobStore::new(&records).load().unwrap());
    assert_eq!(reg.snapshot().records.len(), 3);
    let row = |b: BindingDecl| format!("{}/{}", b.app, b.glade_id);
    let live: Vec<String> = reg.bindings_of().into_iter().map(row).collect();
    assert_eq!(live, ["x/x.one", "x/x.two", "y/y.one"]);
    std::fs::remove_dir_all(&dir).unwrap();
}
