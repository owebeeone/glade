//! The actual glade-node binary for library witnesses: the one Cargo built
//! beside this test executable, never a desk or foreign target directory's copy.
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ports the owner's running desk instances hold; all below the witness ports.
const DESK_PORTS: [u16; 6] = [5173, 5180, 8080, 8087, 9099, 9106];

/// The glade-node binary in this test executable's own target directory
/// (`<target>/<profile>/deps/<test>` beside `<target>/<profile>/glade-node`).
/// Refuses a missing binary, or one older than any source Cargo recorded as
/// its input: a `--lib` run after a source edit without rebuilding the binary
/// fails here instead of running stale code.
pub(crate) fn glade_node() -> PathBuf {
    let test = std::env::current_exe().expect("this test executable's path");
    let profile = test
        .parent()
        .filter(|deps| deps.file_name() == Some("deps".as_ref()))
        .and_then(Path::parent)
        .unwrap_or_else(|| {
            panic!(
                "test executable {} is not under <target>/<profile>/deps",
                test.display()
            )
        });
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    match current(&profile.join("glade-node"), &sources) {
        Ok(binary) => binary,
        Err(why) => panic!(
            "{why}; build it into this test's own target directory first, e.g. \
             `cargo build --offline --locked --bin glade-node` with the same CARGO_TARGET_DIR"
        ),
    }
}

/// `binary` if it exists and no source it was built from is newer. Its inputs
/// are those its Cargo dep-info `<binary>.d` names, which `cargo build` writes
/// with the whole closure (at least one must lie under `sources`). `cargo
/// test` writes none beside the binary, so then they are the `.rs` files
/// under `sources` outside the test-only paths ([`test_only`]). Either way an
/// edit to a test-only file does not refuse a binary that cannot contain it.
fn current(binary: &Path, sources: &Path) -> Result<PathBuf, String> {
    let built = std::fs::metadata(binary)
        .and_then(|meta| meta.modified())
        .map_err(|error| {
            format!(
                "actual glade-node binary missing at {}: {error}",
                binary.display()
            )
        })?;
    let info = binary.with_extension("d");
    let inputs = match std::fs::read_to_string(&info) {
        Ok(listed) => inputs(&listed),
        Err(_) => scanned(sources)?,
    };
    if !inputs.iter().any(|input| input.starts_with(sources)) {
        return Err(format!(
            "actual glade-node inputs ({} or a scan) name no source under {}",
            info.display(),
            sources.display()
        ));
    }
    for input in inputs {
        let changed = std::fs::metadata(&input)
            .and_then(|meta| meta.modified())
            .map_err(|error| {
                format!(
                    "stale actual glade-node {}: its input {} is gone: {error}",
                    binary.display(),
                    input.display()
                )
            })?;
        if changed > built {
            return Err(format!(
                "stale actual glade-node {}: {} is newer than the binary",
                binary.display(),
                input.display()
            ));
        }
    }
    Ok(binary.to_path_buf())
}

/// Every `.rs` file under `root` that the binary can be built from: all of
/// them but the test-only paths.
fn scanned(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = vec![];
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|error| format!("cannot scan sources {}: {error}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if test_only(&path) {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    Ok(found)
}

/// The crate's test-only source paths: a `tests` directory, a `tests.rs`
/// file, or a `test_support` directory. Test modules inline in a production
/// file are not here: editing one rebuilds the binary in a full test run.
fn test_only(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "tests" || name == "tests.rs" || name == "test_support")
}

/// The inputs a Makefile-style dep-info line names after `<target>: `, with
/// `\ ` an escaped space inside a path.
fn inputs(listed: &str) -> Vec<PathBuf> {
    let Some((_, names)) = listed.lines().next().and_then(|line| line.split_once(": ")) else {
        return vec![];
    };
    let mut paths = vec![];
    let mut current = String::new();
    let mut escaped = false;
    for c in names.chars() {
        if escaped {
            current.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == ' ' {
            if !current.is_empty() {
                paths.push(PathBuf::from(std::mem::take(&mut current)));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        paths.push(PathBuf::from(current));
    }
    paths
}

/// The first witness port. Every port handed out lies below 32768, under
/// every default OS ephemeral range (Linux 32768, macOS 49152), and above
/// every desk port.
const FIRST_PORT: u16 = 20000;
/// Ports in each test process's own block; 32 blocks end below 32768.
const BLOCK_PORTS: u16 = 384;
const BLOCKS: u32 = 32;
const _: () = assert!(above_desk() && FIRST_PORT as u32 + BLOCKS * BLOCK_PORTS as u32 <= 32768);
const fn above_desk() -> bool {
    let mut at = 0;
    while at < DESK_PORTS.len() {
        if DESK_PORTS[at] >= FIRST_PORT {
            return false;
        }
        at += 1;
    }
    true
}
/// The next offset into this test process's block. Test-only.
static NEXT_PORT: AtomicU16 = AtomicU16::new(0);

/// A loopback port for a witness's listener and carrier (TCP and UDP), free
/// when handed out. A port released by `bind(0)` was unsafe: the node binds
/// it some time later, and meanwhile any socket's own `bind(0)`, a parallel
/// witness's carrier for one, could take it ("Address already in use"). No
/// `bind(0)` hands out these: they are below the ephemeral ranges, drawn
/// from this process's own block (by process id) and never repeated in it.
pub(crate) fn port() -> u16 {
    let block = FIRST_PORT + u16::try_from(std::process::id() % BLOCKS).unwrap() * BLOCK_PORTS;
    loop {
        let offset = NEXT_PORT.fetch_add(1, Ordering::SeqCst);
        assert!(
            offset < BLOCK_PORTS,
            "this test process used all {BLOCK_PORTS} witness ports of its block"
        );
        let port = block + offset;
        if free(port) {
            return port;
        }
    }
}

/// Whether nothing holds `port` on loopback, over TCP or UDP.
fn free(port: u16) -> bool {
    let at = (std::net::Ipv4Addr::LOCALHOST, port);
    std::net::TcpListener::bind(at).is_ok() && std::net::UdpSocket::bind(at).is_ok()
}

/// One actual glade-node process started `env_clear` on an independent
/// profile, its stdout/stderr lines collected.
pub(crate) struct Process {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
    readers: Vec<std::thread::JoinHandle<()>>,
}
impl Process {
    pub(crate) fn start(profile: &Path, port: u16) -> Self {
        let mut command = Command::new(glade_node());
        command
            .env_clear()
            .args([
                "--independent-config",
                profile.to_str().unwrap(),
                &port.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = crate::test_support::children::spawn(&mut command).unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let mut readers = Vec::new();
        let outputs: Vec<Box<dyn std::io::Read + Send>> = vec![
            Box::new(child.stdout.take().unwrap()),
            Box::new(child.stderr.take().unwrap()),
        ];
        for output in outputs {
            let owned = lines.clone();
            readers.push(std::thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    match line {
                        Ok(line) => owned.lock().unwrap().push(line),
                        Err(_) => {
                            break;
                        }
                    }
                }
            }));
        }
        println!(
            "actual node pid={} profile={}",
            child.id(),
            profile.display()
        );
        Self {
            child,
            lines,
            readers,
        }
    }
    /// Every line collected so far, in arrival order per stream.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
    pub(crate) async fn wait(&mut self, prefix: &str) {
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if self
                    .lines
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.starts_with(prefix))
                {
                    break;
                }
                if let Some(status) = self.child.try_wait().unwrap() {
                    panic!(
                        "actual node pid {} exited {status}: {:?}",
                        self.child.id(),
                        self.lines.lock().unwrap()
                    );
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "actual node missing {prefix}: {:?}",
            self.lines.lock().unwrap()
        );
    }
    /// SIGTERM, then a successful exit within the bound.
    pub(crate) async fn stop(&mut self) {
        let mut kill = Command::new("/bin/kill");
        kill.env_clear()
            .args(["-TERM", &self.child.id().to_string()]);
        assert!(crate::test_support::children::spawn(&mut kill)
            .and_then(|mut kill| kill.wait())
            .unwrap()
            .success());
        let status = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        assert!(
            status.success(),
            "actual node close: {:?}",
            self.lines.lock().unwrap()
        );
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("glade-actual-binary-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deeper")).unwrap();
        root
    }
    fn touch(path: &Path, at: SystemTime) {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .unwrap();
        file.set_modified(at).unwrap();
    }
    fn info(binary: &Path, inputs: &[&Path]) {
        let names: Vec<String> = inputs
            .iter()
            .map(|input| input.display().to_string().replace(' ', "\\ "))
            .collect();
        std::fs::write(
            binary.with_extension("d"),
            format!("{}: {}\n", binary.display(), names.join(" ")),
        )
        .unwrap();
    }

    #[test]
    fn current_binary_newer_than_every_recorded_input_is_used() {
        let root = scratch("current");
        let (lib, binary) = (root.join("src/deeper/lib file.rs"), root.join("glade-node"));
        let then = SystemTime::now() - Duration::from_secs(60);
        touch(&lib, then);
        touch(&binary, SystemTime::now());
        info(&binary, &[&lib]);
        assert_eq!(current(&binary, &root.join("src")), Ok(binary.clone()));
        // A test-only file is not a recorded input: editing it keeps the binary.
        touch(&root.join("src/tests_only.rs"), SystemTime::now());
        assert_eq!(current(&binary, &root.join("src")), Ok(binary));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// `cargo test` writes no `<binary>.d`: the inputs are then the `.rs`
    /// files under the sources outside the test-only paths.
    #[test]
    fn without_dep_info_the_non_test_sources_are_the_inputs() {
        let root = scratch("scan");
        let (lib, binary) = (root.join("src/deeper/lib.rs"), root.join("glade-node"));
        let sources = root.join("src");
        let built = SystemTime::now();
        let later = built + Duration::from_secs(60);
        touch(&lib, built - Duration::from_secs(60));
        touch(&binary, built);
        assert_eq!(current(&binary, &sources), Ok(binary.clone()));
        // Test-only paths are not inputs: edits there keep the binary.
        for test_only in [
            "src/tests.rs",
            "src/deeper/tests/witness.rs",
            "src/test_support/harness.rs",
        ] {
            let path = root.join(test_only);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            touch(&path, later);
        }
        assert_eq!(current(&binary, &sources), Ok(binary.clone()));
        touch(&lib, later);
        let stale = current(&binary, &sources).unwrap_err();
        assert!(
            stale.contains("stale") && stale.contains("lib.rs"),
            "{stale}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_stale_or_unrecorded_binary_is_refused() {
        let root = scratch("refused");
        let (lib, binary) = (root.join("src/lib.rs"), root.join("glade-node"));
        let sources = root.join("src");
        assert!(current(&binary, &sources)
            .unwrap_err()
            .contains("binary missing"));
        let then = SystemTime::now() - Duration::from_secs(60);
        touch(&binary, then);
        // No dep-info and no source to scan: nothing proves it current.
        assert!(current(&binary, &sources)
            .unwrap_err()
            .contains("name no source"));
        touch(&lib, SystemTime::now());
        info(&binary, &[&lib]);
        let stale = current(&binary, &sources).unwrap_err();
        assert!(
            stale.contains("stale") && stale.contains("lib.rs"),
            "{stale}"
        );
        info(&binary, &[&root.join("src/removed.rs")]);
        assert!(current(&binary, &sources).unwrap_err().contains("is gone"));
        info(&binary, &[&root.join("glade-node")]);
        assert!(current(&binary, &sources)
            .unwrap_err()
            .contains("name no source"));
        std::fs::remove_dir_all(root).unwrap();
    }
    /// A witness's port is one no `bind(0)` hands out, in any process: below
    /// every default OS ephemeral range (Linux 32768, macOS 49152), and never
    /// the same twice in this process, nor a desk port.
    #[test]
    fn witness_ports_are_below_the_ephemeral_ranges_distinct_and_not_desk_ports() {
        let ports: Vec<u16> = (0..8).map(|_| port()).collect();
        for port in &ports {
            assert!(
                *port < 32768,
                "{port} is in an OS ephemeral range: {ports:?}"
            );
            assert!(!DESK_PORTS.contains(port), "{port} is a desk port");
        }
        let mut distinct = ports.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), ports.len(), "{ports:?}");
    }
    #[test]
    fn a_port_held_over_tcp_or_udp_is_not_free() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(!free(tcp.local_addr().unwrap().port()));
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(!free(udp.local_addr().unwrap().port()));
    }
}
