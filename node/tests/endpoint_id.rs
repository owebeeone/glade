//! Plan Step 4.5: `glade-node endpoint-id --name <name>` prints a node's
//! endpoint id without serving, so each machine's peer entry can be written
//! before its first start (the owner's ruling of 2026-09-25 on 4.2b's
//! question 3). It is chosen by its first argument and starts no node, so
//! `GLADE_NODE_ASSEMBLED` does not apply to it, and it reads the instance
//! root the binary reads at its entry point.
//!
//! Every file goes under a fresh directory in the system temp dir, and the
//! command runs with `GLADE_HOME` and `HOME` pointed there: `~/.glade` is
//! never touched.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use glade_node::sysdir::{boot_at, now_ms};
use glade_node::transport::{hex, Bound};

/// How long the command may run before the test fails.
const BOUND: Duration = Duration::from_secs(20);

/// A fresh directory for one test, with an empty `glade-home` inside it.
fn scratch(test: &str) -> PathBuf {
    let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
    let name = format!("glade-node-{test}-{}-{nanos}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    std::fs::create_dir_all(dir.join("glade-home")).unwrap();
    dir
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

/// `glade-node endpoint-id --name <name>` under `home`, run to its end within
/// the bound: its exit status, its stdout lines and its stderr. A process
/// still running at the bound is killed, and the test fails with what it
/// printed.
fn identity_id(home: &Path, name: &str, kind: &str) -> (ExitStatus, Vec<String>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_glade-node"));
    command
        .args([kind, "--name", name])
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
        if Instant::now() >= deadline {
            let _ = node.kill();
            let _ = node.wait();
            let mut stdout = String::new();
            let _ = node.stdout.take().unwrap().read_to_string(&mut stdout);
            panic!("glade-node endpoint-id still ran after {BOUND:?}: {stdout}");
        }
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
    (status, stdout.lines().map(str::to_owned).collect(), stderr)
}

fn endpoint_id(home: &Path, name: &str) -> (ExitStatus, Vec<String>, String) {
    identity_id(home, name, "endpoint-id")
}

#[test]
fn node_id_command_is_stable_key_only_and_matches_a_booted_identity() {
    let dir = scratch("node-id-command");
    let home = dir.join("glade-home");
    let (status, lines, error) = identity_id(&home, "n", "node-id");
    assert!(status.success(), "{error}");
    assert_eq!(lines.len(), 1);
    let first = lines[0].clone();
    assert_eq!(first.len(), 64);
    let instance = home.join("sys/n");
    assert!(instance.join("node.key").is_file());
    assert!(!instance.join("endpoint.key").exists());
    assert!(!instance.join("records.json").exists());
    assert!(!instance.join("instance.lock").exists());
    let before = files(&home);
    let (status, again, error) = identity_id(&home, "n", "node-id");
    assert!(status.success(), "{error}");
    assert_eq!(again, lines);
    assert_eq!(files(&home), before);
    let boot = boot_at(instance, "owner").unwrap();
    assert_eq!(boot.node_id, first);
    let (status, running, error) = identity_id(&home, "n", "node-id");
    assert!(status.success(), "{error}");
    assert_eq!(running, lines);
    drop(boot);
    let (status, _, _) = identity_id(&home, "../escape", "node-id");
    assert!(!status.success());
    std::fs::remove_dir_all(dir).unwrap();
}

/// Every file under `dir`, with its length and when it was last written.
fn files(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut held = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            held.extend(files(&path));
        } else {
            let meta = std::fs::metadata(&path).unwrap();
            held.insert(path, (meta.len(), meta.modified().unwrap()));
        }
    }
    held
}

/// The 32 bytes 64 hex digits write.
fn raw(hex: &str) -> [u8; 32] {
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap();
    std::array::from_fn(|i| byte(2 * i))
}

/// On a new instance the command mints `endpoint.key` and prints one line,
/// its id: 64 lower-case hex digits, the same when asked again. It mints
/// nothing else, no `node.key` and no records.json, and leaves no lock. The
/// first boot binds the key it finds, so the id is the one the node's
/// records bind to it.
#[test]
fn the_command_mints_a_key_that_the_first_start_binds() {
    let dir = scratch("endpoint-id-mint");
    let home = dir.join("glade-home");
    let (status, lines, stderr) = endpoint_id(&home, "n");
    assert_eq!(status.code(), Some(0), "{lines:?}, {stderr}");
    let [id] = lines.as_slice() else {
        panic!("one line: {lines:?}");
    };
    let digits = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    assert!(id.len() == 64 && id.bytes().all(digits), "{id}");
    let instance = home.join("sys").join("n");
    let held: Vec<PathBuf> = files(&instance).into_keys().collect();
    assert_eq!(held, [instance.join("endpoint.key")], "nothing else");
    let mode = platform::mode(&instance.join("endpoint.key"));
    assert!(mode.is_none_or(|mode| mode == 0o600), "{mode:?}");
    let (_, again, _) = endpoint_id(&home, "n");
    assert_eq!(again, lines, "the same id");

    let boot = boot_at(instance, "local").unwrap();
    let key = boot.endpoint_key().endpoint_id;
    assert_eq!(hex(&key), *id, "the first boot took the key it found");
    let bound = boot
        .registry
        .transport()
        .binds(&raw(&boot.node_id), &key, now_ms());
    assert_eq!(bound, Bound::Live, "and bound it to the node");
    drop(boot);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// While a node holds its instance, the command reads the node's key and
/// prints its id, taking no lock, which the node holds, and writing nothing:
/// every file of the instance is as it was.
#[test]
fn the_command_reads_a_running_nodes_key_and_writes_nothing() {
    let dir = scratch("endpoint-id-running");
    let home = dir.join("glade-home");
    let instance = home.join("sys").join("n");
    let boot = boot_at(instance.clone(), "local").unwrap();
    let before = files(&instance);
    let (status, lines, stderr) = endpoint_id(&home, "n");
    assert_eq!(status.code(), Some(0), "{stderr}");
    assert_eq!(lines, [hex(&boot.endpoint_key().endpoint_id)]);
    assert_eq!(files(&instance), before, "nothing written");
    drop(boot);
    std::fs::remove_dir_all(&dir).unwrap();
}

// File modes are a Unix notion. Each platform's branch is one braced module,
// so the condition encloses the whole section.
#[cfg(unix)]
mod platform {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// The file's mode.
    pub fn mode(path: &Path) -> Option<u32> {
        let meta = std::fs::metadata(path).unwrap();
        Some(meta.permissions().mode() & 0o777)
    }

    /// A key group or others can read is refused, as a boot refuses it: exit
    /// 1, the reason on stderr, and nothing on stdout.
    #[test]
    fn a_group_readable_key_is_refused() {
        let dir = scratch("endpoint-id-mode");
        let home = dir.join("glade-home");
        let (status, _, stderr) = endpoint_id(&home, "n");
        assert_eq!(status.code(), Some(0), "{stderr}");
        let key = home.join("sys").join("n").join("endpoint.key");
        let open = std::fs::Permissions::from_mode(0o640);
        std::fs::set_permissions(&key, open).unwrap();
        let (status, lines, stderr) = endpoint_id(&home, "n");
        assert_eq!(status.code(), Some(1), "{lines:?}, {stderr}");
        let said = "endpoint.key is group/world-accessible (mode 640) — refusing";
        assert_eq!(stderr.trim_end(), said);
        assert_eq!(lines, Vec::<String>::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(not(unix))]
mod platform {
    use std::path::Path;

    /// Off Unix a file has no mode that the node checks (F5).
    pub fn mode(_path: &Path) -> Option<u32> {
        None
    }
}
