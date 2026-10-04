//! Physical layout foundation; decoding an image does not validate runtime recovery.
use glade_crdt_recovery_codec::*;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
pub(crate) mod floors;
mod metadata;
mod paired;
pub(crate) use metadata::{DiskOperation, FloorEnvelope};
pub(crate) use paired::{PairCut, PairStore};
pub(crate) mod slots;

pub(crate) struct DiskLayout {
    data: PathBuf,
    floor: PathBuf,
    _locks: [File; 2],
    registration: Digest,
}
impl DiskLayout {
    pub(crate) fn read_floor(&self, bounds: &DecodeLimits) -> Result<FloorEnvelope, Fault> {
        self.owned()?;
        let bytes = floors::read(&self.floor.join("replica.floor"), bounds.bytes)?;
        decode(bytes.value(), bounds).map_err(|error| match error {
            Error::Capacity => Fault::Capacity,
            Error::Invalid => Fault::Integrity,
            Error::Unavailable => Fault::Unavailable,
        })
    }

    pub(crate) fn path_binding(&self) -> Result<Digest, Fault> {
        self.owned()?;
        Ok(Bytes::new(marker(&self.data, &self.floor, self.registration)?).digest())
    }
    pub(crate) fn floor_digest(&self, bound: usize) -> Result<Digest, Fault> {
        self.owned()?;
        Ok(floors::read(&self.floor.join("replica.floor"), bound)?.digest())
    }

    pub(crate) fn owned(&self) -> Result<(), Fault> {
        let expected = marker(&self.data, &self.floor, self.registration)?;
        for (index, directory) in [&self.data, &self.floor].into_iter().enumerate() {
            if fs::canonicalize(directory).map_err(io_fault)? != *directory
                || !fs::symlink_metadata(directory).map_err(io_fault)?.is_dir()
                || platform::file_identity(&self._locks[index]).map_err(io_fault)?
                    != platform::identity(&directory.join("root.lock")).map_err(io_fault)?
            {
                return Err(Fault::Integrity);
            }
            let path = directory.join("ic3-records.v1");
            regular(&path)?;
            let mut bytes = vec![];
            File::open(path)
                .map_err(io_fault)?
                .take(expected.len() as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(io_fault)?;
            if bytes != expected {
                return Err(Fault::Integrity);
            }
        }
        Ok(())
    }
    pub(crate) fn open(data: &Path, floor: &Path, registration: Digest) -> Result<Self, Fault> {
        for directory in [data, floor] {
            if fs::canonicalize(directory).map_err(io_fault)? != directory
                || !fs::symlink_metadata(directory).map_err(io_fault)?.is_dir()
            {
                return Err(Fault::Integrity);
            }
        }
        if data.starts_with(floor) || floor.starts_with(data) || registration == [0; 32] {
            return Err(Fault::Integrity);
        }
        let locks = [lock_existing(data)?, lock_existing(floor)?];
        let expected = marker(data, floor, registration)?;
        for directory in [data, floor] {
            let path = directory.join("ic3-records.v1");
            let mut bytes = vec![];
            regular(&path)?;
            File::open(&path)
                .map_err(io_fault)?
                .take(expected.len() as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(io_fault)?;
            if bytes != expected {
                return Err(Fault::Integrity);
            }
        }
        regular(&data.join("legacy-store.sealed"))?;
        Ok(Self {
            data: data.to_path_buf(),
            floor: floor.to_path_buf(),
            _locks: locks,
            registration,
        })
    }
    pub(crate) fn provision(
        data: &Path,
        floor: &Path,
        registration: Digest,
    ) -> Result<Self, Fault> {
        let data = fresh_path(data)?;
        let floor = fresh_path(floor)?;
        if data.starts_with(&floor) || floor.starts_with(&data) || registration == [0; 32] {
            return Err(Fault::Integrity);
        }
        fs::create_dir(&data).map_err(io_fault)?;
        fs::create_dir(&floor).map_err(io_fault)?;
        let locks = [lock_new(&data)?, lock_new(&floor)?];
        let marker = marker(&data, &floor, registration)?;
        write_new(&data.join("ic3-records.v1"), &marker)?;
        write_new(&floor.join("ic3-records.v1"), &marker)?;
        // Only a disposable new root is provisioned. Never seal an existing Store.
        write_new(
            &data.join("legacy-store.sealed"),
            b"ic3 fresh-root compatibility refusal\n",
        )?;
        sync_directory(&data)?;
        sync_directory(&floor)?;
        sync_directory(data.parent().ok_or(Fault::Integrity)?)?;
        sync_directory(floor.parent().ok_or(Fault::Integrity)?)?;
        Ok(Self {
            data,
            floor,
            _locks: locks,
            registration,
        })
    }
}
fn marker(data: &Path, floor: &Path, registration: Digest) -> Result<Vec<u8>, Fault> {
    let mut bytes = b"glade.ic3.records\0\x01".to_vec();
    bytes.extend(registration);
    bytes.extend(Bytes::new(data.as_os_str().as_encoded_bytes().to_vec()).digest());
    bytes.extend(Bytes::new(floor.as_os_str().as_encoded_bytes().to_vec()).digest());
    for path in [
        data.to_path_buf(),
        floor.to_path_buf(),
        data.join("root.lock"),
        floor.join("root.lock"),
    ] {
        bytes.extend(platform::identity(&path).map_err(io_fault)?);
    }
    Ok(bytes)
}
fn regular(path: &Path) -> Result<(), Fault> {
    if !fs::symlink_metadata(path).map_err(io_fault)?.is_file() {
        return Err(Fault::Integrity);
    }
    Ok(())
}
fn lock_existing(directory: &Path) -> Result<File, Fault> {
    let path = directory.join("root.lock");
    regular(&path)?;
    let before = platform::identity(&path).map_err(io_fault)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(io_fault)?;
    file.try_lock().map_err(|_| Fault::Ownership)?;
    regular(&path)?;
    if before != platform::identity(&path).map_err(io_fault)?
        || before != platform::file_identity(&file).map_err(io_fault)?
    {
        return Err(Fault::Integrity);
    }
    Ok(file)
}
fn io_fault(_: io::Error) -> Fault {
    Fault::Unavailable
}
fn fresh_path(path: &Path) -> Result<PathBuf, Fault> {
    if !path.is_absolute() || path.exists() {
        return Err(Fault::Integrity);
    }
    let parent = path.parent().ok_or(Fault::Integrity)?;
    let canonical = fs::canonicalize(parent).map_err(io_fault)?;
    if canonical != parent {
        return Err(Fault::Integrity);
    }
    Ok(canonical.join(path.file_name().ok_or(Fault::Integrity)?))
}
fn lock_new(directory: &Path) -> Result<File, Fault> {
    let file = platform::create(&directory.join("root.lock")).map_err(io_fault)?;
    file.try_lock().map_err(|_| Fault::Ownership)?;
    file.sync_all().map_err(io_fault)?;
    Ok(file)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Fault> {
    let mut file = platform::create(path).map_err(io_fault)?;
    file.write_all(bytes).map_err(io_fault)?;
    file.sync_all().map_err(io_fault)
}
fn sync_directory(path: &Path) -> Result<(), Fault> {
    platform::sync_directory(path).map_err(io_fault)
}
#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    fn object(meta: &fs::Metadata) -> [u8; 16] {
        let mut out = [0; 16];
        out[..8].copy_from_slice(&meta.dev().to_be_bytes());
        out[8..].copy_from_slice(&meta.ino().to_be_bytes());
        out
    }
    pub(super) fn identity(path: &Path) -> io::Result<[u8; 16]> {
        Ok(object(&fs::symlink_metadata(path)?))
    }
    pub(super) fn file_identity(file: &File) -> io::Result<[u8; 16]> {
        Ok(object(&file.metadata()?))
    }
    pub(super) fn create(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }
    pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
        File::open(path)?.sync_all()
    }
}
#[cfg(not(unix))]
mod platform {
    use super::*;
    pub(super) fn create(_: &Path) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "IC3 physical profile is Unix-qualified only",
        ))
    }
    pub(super) fn sync_directory(_: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "IC3 directory sync is unqualified",
        ))
    }
    pub(super) fn identity(_: &Path) -> io::Result<[u8; 16]> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "IC3 object identity unqualified",
        ))
    }
    pub(super) fn file_identity(_: &File) -> io::Result<[u8; 16]> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "IC3 file identity unqualified",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_layout_has_stable_exclusive_custody_and_legacy_refusal_marker() {
        let base = std::fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let data = base.join("data");
        let floor = base.join("trusted-floor");
        let layout = DiskLayout::provision(&data, &floor, [75; 32])
            .expect("fresh physical layout obtains actual lifetime root locks");
        assert!(data.join("ic3-records.v1").is_file());
        assert!(data.join("legacy-store.sealed").is_file());
        assert!(data.join("root.lock").is_file());
        assert!(floor.join("root.lock").is_file());
        assert!(glade_crdt_recovery_codec::Bytes::new(
            std::fs::read(data.join("ic3-records.v1")).unwrap()
        )
        .value()
        .starts_with(b"glade.ic3.records\0\x01"));
        assert_eq!(layout.data, data);
        assert_eq!(layout.floor, floor);
        assert!(crate::store::Store::open(&data).is_err());
        drop(layout);
        assert!(
            data.join("root.lock").is_file(),
            "stable lock path survives clean close"
        );
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn registered_roots_reopen_but_live_lock_wrong_registration_and_data_clone_refuse() {
        let base = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-reopen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let data = base.join("data");
        let floor = base.join("trusted-floor");
        let layout = DiskLayout::provision(&data, &floor, [75; 32]).unwrap();
        assert!(matches!(
            DiskLayout::open(&data, &floor, [75; 32]),
            Err(Fault::Ownership)
        ));
        drop(layout);
        crate::test_support::children::settled();
        assert!(matches!(
            DiskLayout::open(&data, &floor, [74; 32]),
            Err(Fault::Integrity)
        ));
        // That refused open took both locks before it checked the marker, and
        // released them: a release in this process like any other.
        crate::test_support::children::settled();
        let reopened = DiskLayout::open(&data, &floor, [75; 32])
            .expect("registered roots reopen through the unchanged stable lock inode");
        drop(reopened);
        crate::test_support::children::settled();
        let old = base.join("old-data");
        fs::rename(&data, &old).unwrap();
        fs::create_dir(&data).unwrap();
        for name in ["ic3-records.v1", "root.lock", "legacy-store.sealed"] {
            fs::copy(old.join(name), data.join(name)).unwrap();
        }
        assert!(
            matches!(
                DiskLayout::open(&data, &floor, [75; 32]),
                Err(Fault::Integrity)
            ),
            "same pathname copied data cannot replace the registered directory/lock inode"
        );
        fs::remove_dir_all(base).unwrap();
    }
    /// A root released while other threads of this process spawn children
    /// reopens, every time, once the spawns in flight have settled
    /// (`crate::test_support::children`): the new process of a spawn holds a
    /// copy of the released lock until its exec completes, and a reopen that
    /// meets the copy is refused `Ownership`.
    #[test]
    fn released_root_reopens_after_in_flight_child_spawns_settle() {
        use crate::test_support::children;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let base = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-spawning-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let data = base.join("data");
        let floor = base.join("trusted-floor");
        let mut layout = Some(DiskLayout::provision(&data, &floor, [75; 32]).unwrap());
        let (done, spawned) = (AtomicBool::new(false), AtomicUsize::new(0));
        let mut reopened = 0_usize;
        let refused = std::thread::scope(|scope| {
            let spawners: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        while !done.load(Ordering::SeqCst) {
                            let child = children::spawn(
                                std::process::Command::new(std::env::current_exe()?)
                                    .env_clear()
                                    .args(["--exact", "no-such-test"])
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null()),
                            );
                            let waited = child.and_then(|mut child| child.wait());
                            if let Err(error) = waited {
                                done.store(true, Ordering::SeqCst);
                                return Err(error);
                            }
                            spawned.fetch_add(1, Ordering::SeqCst);
                        }
                        Ok(())
                    })
                })
                .collect();
            let mut refused = None;
            while spawned.load(Ordering::SeqCst) < 200 && !done.load(Ordering::SeqCst) {
                drop(layout.take());
                children::settled();
                match DiskLayout::open(&data, &floor, [75; 32]) {
                    Ok(value) => {
                        layout = Some(value);
                        reopened += 1;
                    }
                    Err(fault) => {
                        refused = Some(fault);
                        break;
                    }
                }
            }
            done.store(true, Ordering::SeqCst);
            for spawner in spawners {
                spawner
                    .join()
                    .unwrap()
                    .expect("children of this test spawn and exit");
            }
            refused
        });
        assert_eq!(
            refused,
            None,
            "a released root reopens after spawns settle ({reopened} reopens, {} children)",
            spawned.load(Ordering::SeqCst)
        );
        drop(layout);
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    #[ignore = "B2 qualification requires the preserved exact committed legacy executable fixture"]
    fn actual_committed_legacy_executable_refuses_the_disposable_ic3_root() {
        let base = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-legacy-exe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let data = base.join("data");
        let layout = DiskLayout::provision(&data, &base.join("trusted-floor"), [75; 32]).unwrap();
        let mut command =
            std::process::Command::new("/private/tmp/ic3-b2-legacy-e8097861/glade-node-legacy");
        command
            .env_clear()
            .arg("0")
            .arg(&data)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = crate::test_support::children::spawn(&mut command).unwrap();
        let mut exited = false;
        for _ in 0..100 {
            if child.try_wait().unwrap().is_some() {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !exited {
            child.kill().unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(exited && !output.status.success());
        assert!(String::from_utf8(output.stderr).unwrap().contains("sealed"));
        drop(layout);
        fs::remove_dir_all(base).unwrap();
    }
}
