//! Bounded atomic floor-file replacement; typed validation belongs to the host.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloorCut {
    BeforeTemp,
    TempCreated,
    PartialWritten,
    Written,
    FileSynced,
    Renamed,
    DirectorySynced,
}
pub(super) fn replace(
    path: &Path,
    bytes: &Bytes,
    limit: usize,
    mut at: impl FnMut(FloorCut) -> Result<(), Fault>,
) -> Result<(), Fault> {
    if limit == 0 || bytes.value().len() > limit {
        return Err(Fault::Capacity);
    }
    if path.exists() {
        regular(path)?;
    }
    at(FloorCut::BeforeTemp)?;
    let temporary = path.with_extension("next");
    let mut file = if temporary.exists() {
        regular(&temporary)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(io_fault)?
    } else {
        platform::create(&temporary).map_err(io_fault)?
    };
    if platform::identity(&temporary).map_err(io_fault)?
        != platform::file_identity(&file).map_err(io_fault)?
    {
        return Err(Fault::Integrity);
    }
    at(FloorCut::TempCreated)?;
    file.set_len(0).map_err(io_fault)?;
    let halfway = bytes.value().len() / 2;
    file.write_all(&bytes.value()[..halfway])
        .map_err(io_fault)?;
    at(FloorCut::PartialWritten)?;
    file.write_all(&bytes.value()[halfway..])
        .map_err(io_fault)?;
    at(FloorCut::Written)?;
    file.sync_all().map_err(io_fault)?;
    at(FloorCut::FileSynced)?;
    regular(&temporary)?;
    if platform::identity(&temporary).map_err(io_fault)?
        != platform::file_identity(&file).map_err(io_fault)?
    {
        return Err(Fault::Integrity);
    }
    fs::rename(&temporary, path).map_err(io_fault)?;
    at(FloorCut::Renamed)?;
    sync_directory(path.parent().ok_or(Fault::Integrity)?)?;
    at(FloorCut::DirectorySynced)
}
pub(super) fn read(path: &Path, limit: usize) -> Result<Bytes, Fault> {
    regular(path)?;
    let mut file = File::open(path).map_err(io_fault)?;
    let bound = u64::try_from(limit).map_err(|_| Fault::Capacity)?;
    if file.metadata().map_err(io_fault)?.len() > bound {
        return Err(Fault::Capacity);
    }
    if platform::identity(path).map_err(io_fault)?
        != platform::file_identity(&file).map_err(io_fault)?
    {
        return Err(Fault::Integrity);
    }
    let mut bytes = vec![];
    Read::by_ref(&mut file)
        .take(bound.checked_add(1).ok_or(Fault::Capacity)?)
        .read_to_end(&mut bytes)
        .map_err(io_fault)?;
    if bytes.len() > limit {
        return Err(Fault::Capacity);
    }
    if platform::identity(path).map_err(io_fault)?
        != platform::file_identity(&file).map_err(io_fault)?
    {
        return Err(Fault::Integrity);
    }
    Ok(Bytes::new(bytes))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    #[test]
    fn bounded_floor_replacement_retains_installed_bytes_after_lost_rename_reply() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-floor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("instance.floor");
        let old = Bytes::new(b"old exact floor metadata".to_vec());
        replace(&path, &old, 4096, |_| Ok(()))
            .expect("floor file+directory synchronized before acknowledgment");
        assert_eq!(read(&path, 4096).unwrap(), old);
        let next = Bytes::new(b"next exact floor metadata".to_vec());
        assert!(
            replace(&path, &next, 4096, |cut| if cut == FloorCut::Renamed {
                Err(Fault::Unavailable)
            } else {
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            read(&path, 4096).unwrap(),
            next,
            "lost replacement reply cannot infer that next floor was absent"
        );
        assert!(replace(&path, &Bytes::new(vec![0; 4097]), 4096, |_| Ok(())).is_err());
        assert_eq!(read(&path, 4096).unwrap(), next);
        assert!(read(&path, 8).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn atomic_floor_exposes_each_exact_creation_partial_write_and_sync_cut() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-floor-cuts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let mut observed = vec![];
        replace(
            &root.join("instance.floor"),
            &Bytes::new(b"actual floor metadata".to_vec()),
            4096,
            |cut| {
                observed.push(cut);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            observed,
            vec![
                FloorCut::BeforeTemp,
                FloorCut::TempCreated,
                FloorCut::PartialWritten,
                FloorCut::Written,
                FloorCut::FileSynced,
                FloorCut::Renamed,
                FloorCut::DirectorySynced
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "owned helper process invoked only by its kill-cut parent"]
    fn floor_cut_child() {
        let path =
            PathBuf::from(std::env::var_os("IC3_B2_FLOOR_PATH").expect("explicit child path"));
        let target = std::env::var("IC3_B2_FLOOR_CUT").expect("explicit child cut");
        replace(
            &path,
            &Bytes::new(b"new exact floor".to_vec()),
            4096,
            |cut| {
                if format!("{cut:?}") == target {
                    println!("IC3-CUT-READY");
                    std::io::stdout().flush().unwrap();
                    loop {
                        std::thread::park();
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        panic!("selected cut was never reached");
    }
    #[test]
    fn actual_process_kill_at_every_floor_cut_reopens_exact_old_or_installed_bytes() {
        let base = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-floor-kill-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        for cut in [
            FloorCut::BeforeTemp,
            FloorCut::TempCreated,
            FloorCut::PartialWritten,
            FloorCut::Written,
            FloorCut::FileSynced,
            FloorCut::Renamed,
            FloorCut::DirectorySynced,
        ] {
            let root = base.join(format!("{cut:?}"));
            fs::create_dir(&root).unwrap();
            let path = root.join("instance.floor");
            let old = Bytes::new(b"old exact floor".to_vec());
            replace(&path, &old, 4096, |_| Ok(())).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .env_clear()
                .env("IC3_B2_FLOOR_PATH", &path)
                .env("IC3_B2_FLOOR_CUT", format!("{cut:?}"))
                .args([
                    "--exact",
                    "independent::disk::floors::tests::floor_cut_child",
                    "--ignored",
                    "--nocapture",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines() {
                    let line = line.unwrap();
                    if line == "IC3-CUT-READY" {
                        let _ = tx.send(());
                        break;
                    }
                }
            });
            let reached = rx.recv_timeout(std::time::Duration::from_secs(5));
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            reader.join().unwrap();
            assert!(
                reached.is_ok(),
                "cut {cut:?} not reached: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !output.status.success(),
                "actual process termination required"
            );
            let expected = if matches!(cut, FloorCut::Renamed | FloorCut::DirectorySynced) {
                Bytes::new(b"new exact floor".to_vec())
            } else {
                old
            };
            assert_eq!(read(&path, 4096).unwrap(), expected, "cut {cut:?}");
        }
        fs::remove_dir_all(base).unwrap();
    }
}
