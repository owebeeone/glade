//! Bounded file primitives; no successful ReplicaRecoverySession is asserted here.
use super::*;
use std::io::{Seek, SeekFrom};

pub(super) struct SlotFile {
    file: File,
    path: PathBuf,
    capacity: usize,
}
const MAGIC: &[u8] = b"glade.ic3.image\0\x01";
const HEADER: usize = MAGIC.len() + 8 + 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotCut {
    BeforeWrite,
    HeaderWritten,
    PartialWritten,
    Written,
    FileSynced,
}
impl SlotFile {
    pub(super) fn open(path: &Path, capacity: usize) -> Result<Self, Fault> {
        regular(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(io_fault)?;
        let slot = Self {
            file,
            path: path.to_path_buf(),
            capacity,
        };
        slot.owned()?;
        if capacity == 0
            || slot.file.metadata().map_err(io_fault)?.len()
                != u64::try_from(capacity.checked_add(HEADER).ok_or(Fault::Capacity)?)
                    .map_err(|_| Fault::Capacity)?
        {
            return Err(Fault::Integrity);
        }
        Ok(slot)
    }
    pub(super) fn reserve(path: &Path, capacity: usize) -> Result<Self, Fault> {
        if capacity == 0 {
            return Err(Fault::Capacity);
        }
        let total = capacity.checked_add(HEADER).ok_or(Fault::Capacity)?;
        u64::try_from(total).map_err(|_| Fault::Capacity)?;
        let mut file = platform::create(path).map_err(io_fault)?;
        zeroes(&mut file, total)?;
        file.sync_all().map_err(io_fault)?;
        sync_directory(path.parent().ok_or(Fault::Integrity)?)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            capacity,
        })
    }
    fn owned(&self) -> Result<(), Fault> {
        regular(&self.path)?;
        if platform::identity(&self.path).map_err(io_fault)?
            != platform::file_identity(&self.file).map_err(io_fault)?
        {
            return Err(Fault::Integrity);
        }
        Ok(())
    }
    pub(super) fn resync(&self) -> Result<(), Fault> {
        self.owned()?;
        self.file.sync_all().map_err(io_fault)?;
        self.owned()
    }
    pub(super) fn write_image(&mut self, bytes: &Bytes) -> Result<(), Fault> {
        self.write_image_with(bytes, |_| Ok(()))
    }
    pub(super) fn write_image_with(
        &mut self,
        bytes: &Bytes,
        mut at: impl FnMut(SlotCut) -> Result<(), Fault>,
    ) -> Result<(), Fault> {
        self.owned()?;
        if bytes.value().len() > self.capacity {
            return Err(Fault::Capacity);
        }
        if self.file.metadata().map_err(io_fault)?.len()
            != u64::try_from(self.capacity.checked_add(HEADER).ok_or(Fault::Capacity)?)
                .map_err(|_| Fault::Capacity)?
        {
            return Err(Fault::Integrity);
        }
        at(SlotCut::BeforeWrite)?;
        self.file.seek(SeekFrom::Start(0)).map_err(io_fault)?;
        self.file.write_all(MAGIC).map_err(io_fault)?;
        self.file
            .write_all(
                &u64::try_from(bytes.value().len())
                    .map_err(|_| Fault::Capacity)?
                    .to_be_bytes(),
            )
            .map_err(io_fault)?;
        self.file.write_all(&bytes.digest()).map_err(io_fault)?;
        at(SlotCut::HeaderWritten)?;
        let halfway = bytes.value().len() / 2;
        self.file
            .write_all(&bytes.value()[..halfway])
            .map_err(io_fault)?;
        at(SlotCut::PartialWritten)?;
        self.file
            .write_all(&bytes.value()[halfway..])
            .map_err(io_fault)?;
        zeroes(&mut self.file, self.capacity - bytes.value().len())?;
        at(SlotCut::Written)?;
        self.file.sync_all().map_err(io_fault)?;
        at(SlotCut::FileSynced)?;
        self.owned()
    }
    pub(super) fn read_image(&mut self, digest: Digest) -> Result<Bytes, Fault> {
        self.owned()?;
        if self.file.metadata().map_err(io_fault)?.len()
            != u64::try_from(self.capacity.checked_add(HEADER).ok_or(Fault::Capacity)?)
                .map_err(|_| Fault::Capacity)?
        {
            return Err(Fault::Integrity);
        }
        self.file.seek(SeekFrom::Start(0)).map_err(io_fault)?;
        let mut header = [0u8; HEADER];
        self.file.read_exact(&mut header).map_err(io_fault)?;
        if &header[..MAGIC.len()] != MAGIC || header[MAGIC.len() + 8..] != digest {
            return Err(Fault::Integrity);
        }
        let length = u64::from_be_bytes(
            header[MAGIC.len()..MAGIC.len() + 8]
                .try_into()
                .map_err(|_| Fault::Integrity)?,
        );
        let length = usize::try_from(length).map_err(|_| Fault::Capacity)?;
        if length > self.capacity {
            return Err(Fault::Capacity);
        }
        let mut bytes = vec![0; length];
        self.file.read_exact(&mut bytes).map_err(io_fault)?;
        let bytes = Bytes::new(bytes);
        if bytes.digest() != digest {
            return Err(Fault::Integrity);
        }
        let mut remaining = self.capacity - length;
        let mut buffer = [0; 8192];
        while remaining > 0 {
            let take = remaining.min(buffer.len());
            self.file
                .read_exact(&mut buffer[..take])
                .map_err(io_fault)?;
            if buffer[..take].iter().any(|v| *v != 0) {
                return Err(Fault::Integrity);
            }
            remaining -= take;
        }
        self.owned()?;
        Ok(bytes)
    }
}
fn zeroes(file: &mut File, mut count: usize) -> Result<(), Fault> {
    let buffer = [0; 8192];
    while count > 0 {
        let take = count.min(buffer.len());
        file.write_all(&buffer[..take]).map_err(io_fault)?;
        count -= take;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_reserved_slot_roundtrips_exact_bounded_bytes_and_rejects_torn_or_extra_data() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-slot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("image.0");
        let mut slot =
            SlotFile::reserve(&path, 4096).expect("real bounded reservation writes actual bytes");
        let bytes = Bytes::new(vec![7; 3000]);
        slot.write_image(&bytes)
            .expect("whole image with file synchronization");
        assert_eq!(slot.read_image(bytes.digest()).unwrap(), bytes);
        assert!(slot.write_image(&Bytes::new(vec![8; 4097])).is_err());
        let short = Bytes::new(vec![9; 80]);
        slot.write_image(&short).unwrap();
        assert_eq!(slot.read_image(short.digest()).unwrap(), short);
        slot.file.seek(SeekFrom::Start(25)).unwrap();
        slot.file.write_all(&[1; 2]).unwrap();
        assert!(slot.read_image(short.digest()).is_err());
        slot.write_image(&short).unwrap();
        slot.file.seek(SeekFrom::End(0)).unwrap();
        slot.file.write_all(&[0]).unwrap();
        assert!(slot.read_image(short.digest()).is_err());
        assert!(
            slot.write_image(&short).is_err(),
            "corrupt slot length cannot be silently repaired or resized"
        );
        drop(slot);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn slot_reopens_and_exposes_exact_header_partial_body_and_sync_cuts() {
        let root = fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("glade-ic3-b2-slot-open-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("image.0");
        let bytes = Bytes::new(vec![13; 3000]);
        let mut slot = SlotFile::reserve(&path, 4096).unwrap();
        let mut cuts = vec![];
        slot.write_image_with(&bytes, |cut| {
            cuts.push(cut);
            Ok(())
        })
        .expect("actual image cuts are owned");
        assert_eq!(
            cuts,
            vec![
                SlotCut::BeforeWrite,
                SlotCut::HeaderWritten,
                SlotCut::PartialWritten,
                SlotCut::Written,
                SlotCut::FileSynced
            ]
        );
        drop(slot);
        let mut reopened =
            SlotFile::open(&path, 4096).expect("existing slot is never recreated/resized");
        assert_eq!(reopened.read_image(bytes.digest()).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }
}
