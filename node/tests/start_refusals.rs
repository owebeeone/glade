//! What a refused start prints (the amendment review's SUR-P3-10 and
//! SUR-P3-11). `glade-node` loads every `--app` file before it opens its
//! instance, so a file it cannot read, or one that breaks a rule, stops it
//! before it writes anything. The refusal is printed as its message, prefixed
//! with the file's path as the warnings are, not as a Rust `Debug` dump, and
//! the node exits 1.
//!
//! Every file these tests write goes under a fresh directory in the system
//! temp dir, and the node runs with `GLADE_HOME` and `HOME` pointed there:
//! the real `~/.glade` is never touched.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// A file that loads: the app `x`, with one binding.
const X: &str = "glade-app v1\napp x\nbinding x.one value share commons latest\n";

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

/// `glade-node --profile local --name t --app <app>... 0`, with `GLADE_HOME`
/// and `HOME` at a fresh `dir/glade-home`, required to be refused: the node
/// exits 1, and nothing is written under its `GLADE_HOME`. A node still
/// running after 20 s was not refused: it is killed and the test fails.
/// Returns the node's stderr.
fn refused_start(dir: &Path, apps: &[&Path]) -> String {
    let home = dir.join("glade-home");
    std::fs::create_dir(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
    command.args(["--profile", "local", "--name", "t"]);
    for app in apps {
        command.arg("--app").arg(app);
    }
    let mut node = command
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

    assert_eq!(status.code(), Some(1), "the start is refused: {stderr}");
    let written: Vec<_> = std::fs::read_dir(&home).unwrap().collect();
    assert!(written.is_empty(), "written under GLADE_HOME: {written:?}");
    stderr
}

/// SUR-P3-10: the second of two `--app` paths does not exist, as when a path
/// is mistyped. The start is refused before anything is written, and stderr
/// names the missing path, as it names a file that breaks a rule.
#[test]
fn a_missing_app_file_is_refused_naming_its_path() {
    let dir = scratch("missing-app");
    let x = app_file(&dir, "x.glade", X);
    let missing = dir.join("mistyped.glade");
    let stderr = refused_start(&dir, &[&x, &missing]);
    let named = format!("{}: ", missing.display());
    assert!(
        stderr.lines().any(|line| line.starts_with(&named)),
        "stderr names the missing path: {stderr}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// SUR-P3-11: a file that breaks a rule is refused with its message as the
/// author reads it, on a line starting `<file>: line N:`, and not as
/// `io::Error`'s `Debug` form, `Error: Custom { kind: InvalidData, ... }`.
#[test]
fn a_refused_file_prints_its_message_not_a_debug_dump() {
    let dir = scratch("refusal-text");
    let text = "glade-app v1\napp x\nbinding x.one value shared commons latest\n";
    let bad = app_file(&dir, "bad.glade", text);
    let stderr = refused_start(&dir, &[&bad]);
    let at = format!("{}: line 3: ", bad.display());
    assert!(
        stderr.lines().any(|line| line.starts_with(&at)),
        "no stderr line starts `{at}`: {stderr}"
    );
    assert!(!stderr.contains("Custom {"), "a Debug dump: {stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}
