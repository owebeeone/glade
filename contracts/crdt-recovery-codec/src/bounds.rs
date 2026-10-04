//! Allocation-free structural preflight before the wire or owned typed decoder.
use crate::{DecodeLimits, Error};
pub(crate) fn check(bytes: &[u8], limits: &DecodeLimits) -> Result<(), Error> {
    if bytes.len() > limits.bytes || limits.depth == 0 || limits.depth > 128 {
        return Err(Error::Capacity);
    }
    let mut cursor = Cursor {
        bytes,
        index: 0,
        items: 0,
        limits,
    };
    cursor.value(0)?;
    if cursor.index != bytes.len() {
        return Err(Error::Invalid);
    }
    Ok(())
}
struct Cursor<'a> {
    bytes: &'a [u8],
    index: usize,
    items: usize,
    limits: &'a DecodeLimits,
}
impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self.index.checked_add(n).ok_or(Error::Capacity)?;
        let out = self.bytes.get(self.index..end).ok_or(Error::Invalid)?;
        self.index = end;
        Ok(out)
    }
    fn value(&mut self, depth: usize) -> Result<(), Error> {
        self.items = self.items.checked_add(1).ok_or(Error::Capacity)?;
        if self.items > self.limits.items {
            return Err(Error::Capacity);
        }
        let b = self.take(1)?[0];
        let major = b >> 5;
        let additional = b & 31;
        let n = match additional {
            0..=23 => u64::from(additional),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes(
                self.take(2)?.try_into().map_err(|_| Error::Invalid)?,
            )),
            26 => u64::from(u32::from_be_bytes(
                self.take(4)?.try_into().map_err(|_| Error::Invalid)?,
            )),
            27 => u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| Error::Invalid)?),
            _ => return Err(Error::Invalid),
        };

        match major {
            0 | 1 => {}
            2 | 3 => {
                let n = usize::try_from(n).map_err(|_| Error::Capacity)?;
                if major == 3 && n > self.limits.name_bytes {
                    return Err(Error::Capacity);
                }
                self.take(n)?;
            }
            4 | 5 => {
                let n = usize::try_from(n).map_err(|_| Error::Capacity)?;
                if depth >= self.limits.depth {
                    return Err(Error::Capacity);
                }
                let count = if major == 5 {
                    n.checked_mul(2).ok_or(Error::Capacity)?
                } else {
                    n
                };
                if count > self.limits.items.saturating_sub(self.items) {
                    return Err(Error::Capacity);
                }
                for _ in 0..count {
                    self.value(depth + 1)?;
                }
            }
            7 if matches!(additional, 20..=22) => {}
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }
}
