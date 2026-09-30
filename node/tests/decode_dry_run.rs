//! TautCheckedDecode.md §7 (CD-G3 item 2): `glade-node decode-dry-run
//! DIR...` decodes every op and `home` record a node's data holds with taut's
//! fail-closed codec, and counts what it refuses, before a node's binary is
//! replaced by one on that codec. It is chosen by its first argument, starts
//! no node, and reads only under the directories it is given.
//!
//! Every file goes under a fresh directory in the system temp dir, and the
//! command runs with `GLADE_HOME` and `HOME` pointed there: `~/.glade` is
//! never touched.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use glade_node::cbor::{self, Cbor};
use glade_wire::generated::Op;

/// How long the command may run before the test fails.
const BOUND: Duration = Duration::from_secs(20);

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
    let name = format!("glade-node-{test}-{}-{nanos}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A spawned `glade-node`, killed when dropped while it still runs (F10).
struct Spawned(Child);

impl Drop for Spawned {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// `glade-node decode-dry-run` with `args`, `GLADE_HOME` and `HOME` at
/// `home`, run to its end within the bound: its exit status, its stdout
/// lines and its stderr.
fn dry_run(home: &Path, args: &[&Path]) -> (ExitStatus, Vec<String>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
    command
        .arg("decode-dry-run")
        .args(args)
        .env("GLADE_HOME", home)
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut held = Spawned(command.spawn().expect("spawn glade-node"));
    let node = &mut held.0;
    let deadline = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = node.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "glade-node decode-dry-run still ran after {BOUND:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    node.stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    node.stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    (status, stdout.lines().map(String::from).collect(), stderr)
}

/// An app's op, encoded.
fn op() -> Vec<u8> {
    let op = Op {
        share: "ws-a".into(),
        glade_id: "a.log".into(),
        origin: "o".into(),
        ..Op::default()
    };
    cbor::encode(&op.to_cbor())
}

/// An instance's data under `dir`: records.json holding `records`, and one
/// journal of the store holding `op()`.
fn instance(dir: &Path, records: Vec<Vec<u8>>) {
    let bytes = |items: Vec<Vec<u8>>| Cbor::Array(items.into_iter().map(Cbor::Bytes).collect());
    let snapshot = Cbor::Map(vec![(1, bytes(records)), (2, bytes(vec![]))]);
    std::fs::write(dir.join("records.json"), cbor::encode(&snapshot)).unwrap();
    let share = dir.join("cache").join("store").join("77732d61");
    std::fs::create_dir_all(&share).unwrap();
    let framed = [&(op().len() as u32).to_le_bytes()[..], &op()].concat();
    std::fs::write(share.join("6f.log"), framed).unwrap();
}

/// A clean instance exits 0 and says what it read, file by file, then in
/// all; one whose records.json holds an op the codec refuses exits 1 and
/// names it; none named is the usage, exit 1, nothing on stdout.
#[test]
fn the_dry_run_counts_refusals_and_exits_1_on_any() {
    let dir = scratch("dry-run");
    let (home, data) = (dir.join("home"), dir.join("data"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data).unwrap();

    instance(&data, vec![op()]);
    let (status, lines, stderr) = dry_run(&home, &[&data]);
    assert_eq!(status.code(), Some(0), "{lines:?} {stderr}");
    let total = "decode dry run: 2 files, 2 ops and 0 home records read, none refused";
    assert_eq!(lines.last().map(String::as_str), Some(total), "{stderr}");
    assert_eq!(lines.len(), 3, "{lines:?}");

    instance(&data, vec![op(), vec![0x18, 0x01]]);
    let (status, lines, stderr) = dry_run(&home, &[&data]);
    assert_eq!(status.code(), Some(1), "{lines:?} {stderr}");
    let refused = "  record 2: op refused (quarantined at load; the grant fold is then unreadable): non-canonical integer encoding of 1 (NonCanonicalInt)";
    assert!(lines.iter().any(|line| line == refused), "{lines:?}");
    let total = "decode dry run: 2 files, 3 ops and 0 home records read, 1 refused";
    assert_eq!(lines.last().map(String::as_str), Some(total), "{stderr}");

    let (status, lines, stderr) = dry_run(&home, &[]);
    assert_eq!(status.code(), Some(1), "{lines:?}");
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(stderr.trim_end(), "usage: glade-node decode-dry-run DIR...");
    std::fs::remove_dir_all(&dir).unwrap();
}
