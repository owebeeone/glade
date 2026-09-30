//! What a refused start prints (the amendment review's SUR-P3-10 and
//! SUR-P3-11). `glade-node` loads every `--app` file before it opens its
//! instance, so a file it cannot read, or one that breaks a rule, stops it
//! before it writes anything. The refusal is printed as its message, prefixed
//! with the file's path as the warnings are, not as a Rust `Debug` dump, and
//! the node exits 1.
//!
//! Two tests check plan Step 4.6's `--lease-ms`: a value outside 3,000 to
//! 3,600,000 ms is refused the same way, naming the range, and one inside it
//! is the node's lease, said after `node`, and never a store directory.
//!
//! Every file these tests write goes under a fresh directory in the system
//! temp dir, and the node runs there, with `GLADE_HOME` and `HOME` pointed
//! there too: the real `~/.glade` is never touched.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

/// A spawned `glade-node`, killed when dropped while it still runs (F10), as
/// `assembled_path.rs`'s `Running` is: a test that fails while its process
/// runs leaves none behind. Nothing here can panic.
struct Spawned(Child);

impl Drop for Spawned {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// `glade-node --profile local --name t --app <app>... 0 <flags>`, run in
/// `dir` with `GLADE_HOME` and `HOME` at a fresh `dir/glade-home`, required
/// to be refused: the node exits 1, and nothing is written under its
/// `GLADE_HOME`. A node still running after 20 s was not refused: it is
/// killed and the test fails. Returns the node's stderr.
fn refused_start(dir: &Path, apps: &[&Path], flags: &[&str]) -> String {
    let home = dir.join("glade-home");
    std::fs::create_dir(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
    command.args(["--profile", "local", "--name", "t"]);
    for app in apps {
        command.arg("--app").arg(app);
    }
    let spawned = command
        .arg("0")
        .args(flags)
        .current_dir(dir)
        .env("GLADE_HOME", &home)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn glade-node");
    let mut held = Spawned(spawned);
    let node = &mut held.0;
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
    let stderr = refused_start(&dir, &[&x, &missing], &[]);
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
    let stderr = refused_start(&dir, &[&bad], &[]);
    let at = format!("{}: line 3: ", bad.display());
    assert!(
        stderr.lines().any(|line| line.starts_with(&at)),
        "no stderr line starts `{at}`: {stderr}"
    );
    assert!(!stderr.contains("Custom {"), "a Debug dump: {stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The stdout lines a spawned node prints up to `listening <port>`, read for
/// at most 20 s. The node is then killed, as `node` is dropped.
fn started(mut node: Spawned) -> Vec<String> {
    let stdout = node.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut lines = Vec::new();
    while let Ok(line) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        let last = line.starts_with("listening ");
        lines.push(line);
        if last {
            break;
        }
    }
    lines
}

/// Plan Step 4.6: `--lease-ms 2999`, below the range, refuses the start
/// before anything is written, and stderr names the flag and the range.
#[test]
fn a_lease_out_of_range_is_refused_naming_the_range() {
    let dir = scratch("lease-refused");
    let stderr = refused_start(&dir, &[], &["--lease-ms", "2999"]);
    let named = |line: &str| line.starts_with("--lease-ms ") && line.contains("3000 to 3600000");
    let named = stderr.lines().any(named);
    assert!(named, "no line names the range: {stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Plan Step 4.6: `--lease-ms 12000` is the node's lease. The node starts,
/// says `leases 12000 ms, renewed every 4000 ms` right after its `node` line,
/// and makes no `12000` store directory where it runs, as a root reading the
/// value as positional would, the port first and 0 when it does not parse.
#[test]
fn a_lease_in_range_is_taken_and_said_after_the_node_line() {
    let dir = scratch("lease-taken");
    let home = dir.join("glade-home");
    std::fs::create_dir(&home).unwrap();
    let spawned = Command::new(env!("CARGO_BIN_EXE_glade-node"))
        .args(["--profile", "local", "--name", "t", "--lease-ms", "12000"])
        .current_dir(&dir)
        .env("GLADE_HOME", &home)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn glade-node");
    let lines = started(Spawned(spawned));
    let listening = lines.last().is_some_and(|l| l.starts_with("listening "));
    assert!(listening, "the node did not start: {lines:?}");
    let stray = dir.join("12000").exists();
    assert!(!stray, "12000 was taken as the store directory: {lines:?}");
    let node = lines.iter().position(|line| line.starts_with("node "));
    let next = node.and_then(|at| lines.get(at + 1)).map(String::as_str);
    let said = Some("leases 12000 ms, renewed every 4000 ms");
    assert_eq!(next, said, "the line after `node`: {lines:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
