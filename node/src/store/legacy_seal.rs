//! Q4-A local Store retirement. This is not a protected-resource enrollment.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

const LOCK: &str = "legacy-store.lock";
const MARKER: &str = "legacy-store.sealed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cut {
    BeforeCreate,
    AfterCreate,
    AfterFileSync,
    AfterDirectorySync,
}

fn locked(root: &Path) -> io::Result<File> {
    fs::create_dir_all(root)?;
    let path = root.join(LOCK);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy Store lock is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e);
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

/// Returned guard remains alive through the caller's complete mutation/replay.
pub(super) fn unsealed(root: &Path) -> io::Result<File> {
    let guard = locked(root)?;
    match fs::symlink_metadata(root.join(MARKER)) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "legacy Store {} is sealed for migration; legacy writes and replay are disabled",
                root.display()
            ),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(guard),
        Err(e) => Err(e),
    }
}

pub(super) fn seal_with(root: &Path, at: impl FnMut(Cut) -> io::Result<()>) -> io::Result<()> {
    platform::seal(root, at)
}

// The complete platform branches have explicit enclosing modules. Publication
// never substitutes a successful no-op for an unqualified directory sync.
#[cfg(unix)]
mod platform {
    use super::*;

    pub(super) fn seal(root: &Path, mut at: impl FnMut(Cut) -> io::Result<()>) -> io::Result<()> {
        let _guard = locked(root)?;
        at(Cut::BeforeCreate)?;
        let marker = root.join(MARKER);
        let file = match fs::symlink_metadata(&marker) {
            Ok(meta) if meta.file_type().is_file() => {
                OpenOptions::new().read(true).write(true).open(&marker)?
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "legacy Store seal is not a regular file; admission remains closed",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&marker)?,
            Err(e) => {
                return Err(e);
            }
        };
        at(Cut::AfterCreate)?;
        file.sync_all()?;
        at(Cut::AfterFileSync)?;
        File::open(root)?.sync_all()?;
        at(Cut::AfterDirectorySync)?;
        Ok(())
    }
}

#[cfg(not(unix))]
mod platform {
    use super::{io, Cut, Path};

    pub(super) fn seal(_root: &Path, _at: impl FnMut(Cut) -> io::Result<()>) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "legacy Store seal publication is not qualified on this platform",
        ))
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::store::{Append, Store, StoreError};
    use glade_wire::generated::{Op, Shape};
    use std::io::{BufRead, Read, Write};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("glade-q4-seal-unit-{}-{label}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn store(&self) -> Store {
            Store::open(&self.0).unwrap()
        }
        fn journal(&self) -> Vec<u8> {
            fs::read(self.0.join("77732d72617a656c").join("7461622d61.log")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn op(seq: i64) -> Op {
        Op {
            share: "ws-razel".into(),
            glade_id: "gyld.appearance".into(),
            key: b"self:alice".to_vec(),
            origin: "tab-a".into(),
            seq,
            prev: None,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: b"complete appearance value".to_vec(),
        }
    }
    fn fenced(result: Result<Append, StoreError>) {
        assert!(
            matches!(result, Err(StoreError::Io(ref e)) if e.kind() == io::ErrorKind::PermissionDenied),
            "{result:?}"
        );
    }

    #[test]
    fn ls003_append_retains_root_lock_through_mutation_and_seal_waits() {
        let f = Fixture::new("race");
        let mut writer = f.store();
        let mut closer = f.store();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (sealed_tx, sealed_rx) = mpsc::channel();
        let root = f.0.clone();
        let append = std::thread::spawn(move || {
            writer
                .append_with(op(0), || {
                    let rival = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(root.join(LOCK))
                        .unwrap();
                    assert!(
                        matches!(rival.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                        "append's lock was released before mutation"
                    );
                    entered_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                })
                .unwrap()
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let seal = std::thread::spawn(move || {
            closer.seal_legacy().unwrap();
            sealed_tx.send(()).unwrap();
        });
        assert!(
            sealed_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "seal returned across an in-progress append"
        );
        resume_tx.send(()).unwrap();
        assert_eq!(append.join().unwrap(), Append::Appended);
        sealed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        seal.join().unwrap();
        assert!(f.0.join(MARKER).exists());
        assert!(!f.journal().is_empty(), "pre-cut mutation was retained");
        assert!(Store::open(&f.0).is_err());
    }

    #[test]
    fn ls006_each_failed_publication_cut_preserves_data_and_closes_after_create() {
        for cut in [
            Cut::BeforeCreate,
            Cut::AfterCreate,
            Cut::AfterFileSync,
            Cut::AfterDirectorySync,
        ] {
            let f = Fixture::new(&format!("fault-{cut:?}"));
            let mut store = f.store();
            store.append(op(0)).unwrap();
            let before = f.journal();
            let mut reached = false;
            let result = seal_with(&f.0, |at| {
                if at == cut {
                    reached = true;
                    return Err(io::Error::other("injected interrupted publication"));
                }
                Ok(())
            });
            assert!(reached, "the requested real I/O boundary did not execute");
            assert!(result.is_err());
            assert_eq!(f.journal(), before);
            if cut == Cut::BeforeCreate {
                assert!(!f.0.join(MARKER).exists());
                assert_eq!(store.append(op(1)).unwrap(), Append::Appended);
            } else {
                assert!(f.0.join(MARKER).exists());
                fenced(store.append(op(1)));
                assert!(Store::open(&f.0).is_err());
            }
            store.seal_legacy().unwrap();
            fenced(store.append(op(2)));
        }
    }

    #[test]
    fn ls006_retry_preserves_existing_marker_bytes_and_refuses_nonregular_marker() {
        let f = Fixture::new("retry");
        let mut store = f.store();
        fs::write(f.0.join(MARKER), b"unknown prior publication").unwrap();
        store.seal_legacy().unwrap();
        assert_eq!(
            fs::read(f.0.join(MARKER)).unwrap(),
            b"unknown prior publication"
        );
        fenced(store.append(op(0)));
        let odd = Fixture::new("odd-marker");
        let mut store = odd.store();
        fs::create_dir(odd.0.join(MARKER)).unwrap();
        assert!(store.seal_legacy().is_err());
        fenced(store.append(op(0)));
    }

    // Exact ignored subprocess fixture; normal tests only execute the parent.
    #[test]
    #[ignore = "invoked only by ls006_real_process_interruptions"]
    fn process_worker() {
        let root = PathBuf::from(std::env::var_os("GLADE_SEAL_WORKER_ROOT").unwrap());
        let target = std::env::var("GLADE_SEAL_WORKER_CUT").unwrap();
        seal_with(&root, |at| {
            if format!("{at:?}") == target {
                println!("seal-cut:{at:?}");
                io::stdout().flush().unwrap();
                // Parent kills this worker. No sleeps or inherited environment.
                let mut byte = [0u8];
                io::stdin().read_exact(&mut byte).unwrap();
                panic!("parent should kill the worker at the named boundary");
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn ls006_real_process_interruptions() {
        for cut in [
            Cut::BeforeCreate,
            Cut::AfterCreate,
            Cut::AfterFileSync,
            Cut::AfterDirectorySync,
        ] {
            let f = Fixture::new(&format!("kill-{cut:?}"));
            let mut store = f.store();
            store.append(op(0)).unwrap();
            let before = f.journal();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .env_clear()
                .env("GLADE_SEAL_WORKER_ROOT", &f.0)
                .env("GLADE_SEAL_WORKER_CUT", format!("{cut:?}"))
                .args([
                    "--exact",
                    "store::legacy_seal::tests::process_worker",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (tx, rx) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in io::BufReader::new(stdout).lines() {
                    let line = line.unwrap();
                    if let Some(start) = line.find("seal-cut:") {
                        tx.send(line[start..].to_owned()).unwrap();
                        break;
                    }
                }
            });
            let observed = rx.recv_timeout(Duration::from_secs(5));
            // Always terminate our child even if its boundary was never reached.
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            reader.join().unwrap();
            assert_eq!(observed.unwrap(), format!("seal-cut:{cut:?}"));
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(output.status.signal(), Some(9), "actual SIGKILL required");
            println!("LS-006 actual SIGKILL at {cut:?}");
            assert_eq!(f.journal(), before);
            if cut == Cut::BeforeCreate {
                assert!(!f.0.join(MARKER).exists());
                assert_eq!(store.append(op(1)).unwrap(), Append::Appended);
            } else {
                assert!(f.0.join(MARKER).exists());
                fenced(store.append(op(1)));
                assert!(Store::open(&f.0).is_err());
            }
            store.seal_legacy().unwrap();
            fenced(store.append(op(2)));
        }
    }
}
