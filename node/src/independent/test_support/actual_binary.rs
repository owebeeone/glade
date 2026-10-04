//! The actual glade-node binary for library witnesses: the one Cargo built
//! beside this test executable, never a desk or foreign target directory's copy.
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ports the owner's running desk instances hold; never offered to a witness.
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

/// `binary` if it exists and is no older than every input its Cargo dep-info
/// (`<binary>.d`) names; at least one input must lie under `sources`. Test-only
/// files are not inputs of the binary, so editing one does not refuse it.
fn current(binary: &Path, sources: &Path) -> Result<PathBuf, String> {
    let built = std::fs::metadata(binary)
        .and_then(|meta| meta.modified())
        .map_err(|error| {
            format!(
                "actual glade-node binary missing at {}: {error}",
                binary.display()
            )
        })?;
    let (info, listed) = dep_info(binary)?;
    let inputs = inputs(&listed);
    if !inputs.iter().any(|input| input.starts_with(sources)) {
        return Err(format!(
            "actual glade-node dep-info {} names no source under {}",
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

/// The dep-info Cargo recorded for `binary`, and where: `<binary>.d` beside it
/// after `cargo build`; after `cargo test`, which writes dep-info only under
/// `deps/`, that of the `deps/` executable the binary was copied or linked
/// from (the same name stem, size and modification time).
fn dep_info(binary: &Path) -> Result<(PathBuf, String), String> {
    let beside = binary.with_extension("d");
    if let Ok(listed) = std::fs::read_to_string(&beside) {
        return Ok((beside, listed));
    }
    let missing = |why: String| {
        format!(
            "actual glade-node dep-info missing at {} and under deps/: {why}",
            beside.display()
        )
    };
    let built = std::fs::metadata(binary).map_err(|error| missing(error.to_string()))?;
    let stamp = (built.len(), built.modified().ok());
    let deps = binary.with_file_name("deps");
    let stem = binary
        .file_stem()
        .map(|stem| format!("{}-", stem.to_string_lossy().replace('-', "_")))
        .unwrap_or_default();
    let entries = std::fs::read_dir(&deps).map_err(|error| missing(error.to_string()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let named = path
            .file_stem()
            .is_some_and(|name| name.to_string_lossy().starts_with(&stem));
        if !named || path.extension() != binary.extension() {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if (meta.len(), meta.modified().ok()) != stamp {
            continue;
        }
        let info = path.with_extension("d");
        if let Ok(listed) = std::fs::read_to_string(&info) {
            return Ok((info, listed));
        }
    }
    Err(missing("no deps/ executable matches the binary".into()))
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

/// A loopback port for a witness's listener and carrier, never a desk port.
pub(crate) fn port() -> u16 {
    loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        if !DESK_PORTS.contains(&port) {
            return port;
        }
    }
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
        let mut child = Command::new(glade_node())
            .env_clear()
            .args([
                "--independent-config",
                profile.to_str().unwrap(),
                &port.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
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
        assert!(Command::new("/bin/kill")
            .env_clear()
            .args(["-TERM", &self.child.id().to_string()])
            .status()
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

    /// `cargo test` writes no `<binary>.d`: the dep-info read is that of the
    /// `deps/` executable the binary was copied from, never a decoy's.
    #[test]
    fn cargo_test_layout_reads_the_dep_info_of_the_deps_executable_it_copied() {
        let root = scratch("deps");
        let (lib, binary) = (root.join("src/lib.rs"), root.join("glade-node"));
        let sources = root.join("src");
        std::fs::create_dir_all(root.join("deps")).unwrap();
        let (copied, decoy) = (
            root.join("deps/glade_node-0aa1"),
            root.join("deps/glade_node-0bb2"),
        );
        let then = SystemTime::now() - Duration::from_secs(60);
        let built = SystemTime::now();
        touch(&lib, then);
        for path in [&binary, &copied] {
            std::fs::write(path, b"same bytes").unwrap();
            touch(path, built);
        }
        std::fs::write(&decoy, b"a decoy of another size").unwrap();
        touch(&decoy, built);
        let decoy_input = root.join("src/decoy.rs");
        touch(&decoy_input, built + Duration::from_secs(60));
        info(&decoy, &[&decoy_input]);
        info(&copied, &[&lib]);
        assert_eq!(current(&binary, &sources), Ok(binary.clone()));
        touch(&lib, built + Duration::from_secs(60));
        let stale = current(&binary, &sources).unwrap_err();
        assert!(
            stale.contains("stale") && stale.contains("lib.rs"),
            "{stale}"
        );
        std::fs::remove_file(copied.with_extension("d")).unwrap();
        assert!(current(&binary, &sources)
            .unwrap_err()
            .contains("dep-info missing"));
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
        assert!(current(&binary, &sources)
            .unwrap_err()
            .contains("dep-info missing"));
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
            .contains("names no source"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
