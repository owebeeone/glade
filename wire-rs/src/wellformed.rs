//! Bytes `cbor::decode` panics on or recurses too deep for, refused before it
//! reads them (F15, the owner's ruling of 2026-09-27). `cbor.rs` is
//! regenerated from taut, and its decode recurses once for each level of
//! nesting, with no limit, so a frame nested a few thousand deep overflowed
//! the stack of the thread decoding it and aborted the node. It also indexes
//! past the end of truncated bytes, and panics on CBOR outside the wire's
//! subset. This module is written by hand: [`decode`] walks the bytes once,
//! without recursion, before `cbor::decode` does, and refuses them with a
//! [`Malformed`] where `cbor::decode` would fail.
//!
//! It checks the CBOR only. A body of another shape (a missing field, a value
//! of the wrong type) still panics in the generated decode, as before, and an
//! enum value its enum does not name is [`crate::checked`]'s to refuse.

use std::fmt;

use crate::cbor::{self, Cbor};

/// The deepest arrays and maps may nest. The wire's types nest five deep: an
/// `Ops` frame's map, its `ops`, an `Op`, its `refs` and a `Head`; `Hello`,
/// `Welcome` and `Heads` as deep through `StreamHeads`. 32 leaves room for
/// fields a later version adds, which a reader keeps as residual, and holds
/// `cbor::decode` to 33 calls deep, where a debug build's 2 MiB worker stack
/// overflowed at about 1,000.
pub const MAX_DEPTH: usize = 32;

/// Why [`decode`] refused bytes: each is a case `cbor::decode` panics on, or
/// overflows its stack on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// Arrays and maps nested deeper than [`MAX_DEPTH`].
    TooDeep,
    /// The bytes end inside an item, or claim more of them than remain.
    Truncated,
    /// Bytes after the one item.
    Trailing,
    /// An item, begun by this byte, that the wire's subset does not take: a
    /// tag, an indefinite or reserved length, or a simple value other than
    /// false, true, null and the floats.
    Unsupported(u8),
    /// A map key that is not an int.
    MapKey,
    /// Text that is not UTF-8.
    NotUtf8,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Malformed::TooDeep => write!(f, "bad frame: nested deeper than {MAX_DEPTH}"),
            Malformed::Truncated => write!(f, "bad frame: truncated"),
            Malformed::Trailing => write!(f, "bad frame: bytes after its message"),
            Malformed::Unsupported(initial) => {
                write!(f, "bad frame: CBOR the wire does not take ({initial:#04x})")
            }
            Malformed::MapKey => write!(f, "bad frame: a map key that is not an int"),
            Malformed::NotUtf8 => write!(f, "bad frame: text that is not UTF-8"),
        }
    }
}

/// `bytes` decoded as `cbor::decode` decodes them, or refused where it would
/// panic or recurse deeper than [`MAX_DEPTH`].
pub fn decode(bytes: &[u8]) -> Result<Cbor, Malformed> {
    scan(bytes)?;
    Ok(cbor::decode(bytes))
}

/// Walk `bytes` as one item, as `cbor::decode` reads it, without recursion:
/// each step reads one item's head, so the walk is linear in the bytes, and
/// a count is checked against the bytes that remain before it is believed.
fn scan(bytes: &[u8]) -> Result<(), Malformed> {
    // Each array or map open around the next item, innermost last: the items
    // it has still to hold (a map counts its keys and its values), and
    // whether it is a map.
    let mut open = [(0_u64, false); MAX_DEPTH];
    let mut depth = 0;
    let mut at = 0;
    loop {
        let key = match open[..depth].last_mut() {
            Some((left, map)) => {
                let key = *map && *left % 2 == 0;
                *left -= 1;
                key
            }
            None => false,
        };
        let initial = *bytes.get(at).ok_or(Malformed::Truncated)?;
        let (major, info) = (initial >> 5, initial & 0x1f);
        let width = match info {
            0..=23 => 0,
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err(Malformed::Unsupported(initial)),
        };
        let (head, end) = (at + 1, at + 1 + width);
        let arg = bytes.get(head..end).ok_or(Malformed::Truncated)?;
        let arg = match width {
            0 => u64::from(info),
            _ => arg.iter().fold(0, |n, b| (n << 8) | u64::from(*b)),
        };
        at = end;
        let remain = (bytes.len() - at) as u64;
        if key && major > 1 {
            return Err(Malformed::MapKey);
        }
        match major {
            0 | 1 => {}
            2 | 3 => {
                if arg > remain {
                    return Err(Malformed::Truncated);
                }
                let item = &bytes[at..at + arg as usize];
                if major == 3 && std::str::from_utf8(item).is_err() {
                    return Err(Malformed::NotUtf8);
                }
                at += item.len();
            }
            4 | 5 => {
                if depth == MAX_DEPTH {
                    return Err(Malformed::TooDeep);
                }
                // Each item takes a byte at least, and so a map's entry two.
                let per = if major == 5 { 2 } else { 1 };
                if arg > remain / per {
                    return Err(Malformed::Truncated);
                }
                if arg > 0 {
                    open[depth] = (arg * per, major == 5);
                    depth += 1;
                    continue;
                }
            }
            6 => return Err(Malformed::Unsupported(initial)),
            _ => {
                if !matches!(info, 20..=22 | 25..=27) {
                    return Err(Malformed::Unsupported(initial));
                }
            }
        }
        // The item is whole: close each array or map it completes.
        while open[..depth].last().is_some_and(|(left, _)| *left == 0) {
            depth -= 1;
        }
        if depth == 0 {
            break;
        }
    }
    if at == bytes.len() {
        Ok(())
    } else {
        Err(Malformed::Trailing)
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::VECTORS;

    fn hex(h: &str) -> Vec<u8> {
        let byte = |i| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
        (0..h.len()).step_by(2).map(byte).collect()
    }

    /// `levels` arrays, or maps, each the one item of the one around it, the
    /// innermost empty.
    fn nested(levels: usize, map: bool) -> Vec<u8> {
        let (open, empty): (&[u8], u8) = if map {
            (&[0xa1, 0x00], 0xa0)
        } else {
            (&[0x81], 0x80)
        };
        let mut bytes = open.repeat(levels - 1);
        bytes.push(empty);
        bytes
    }

    /// F15: `cbor::decode` recursed once a level, with no limit, so a frame
    /// nested 100,000 deep, 100 KB, overflowed the stack of the thread that
    /// decoded it, which aborts the process. Arrays or maps nested deeper
    /// than `MAX_DEPTH` are refused; nested `MAX_DEPTH` deep, they decode as
    /// `cbor::decode` decodes them.
    #[test]
    fn nesting_deeper_than_max_depth_is_refused_not_an_abort() {
        for map in [false, true] {
            let too_deep = Err(Malformed::TooDeep);
            assert_eq!(decode(&nested(100_000, map)), too_deep, "map: {map}");
            assert_eq!(decode(&nested(MAX_DEPTH + 1, map)), too_deep, "map: {map}");
            let deepest = nested(MAX_DEPTH, map);
            assert_eq!(decode(&deepest), Ok(cbor::decode(&deepest)), "map: {map}");
        }
    }

    /// F15: `cbor::decode` indexed past the end of truncated bytes, and
    /// panicked. Each golden vector decodes as `cbor::decode` decodes it, and
    /// each proper prefix of one, the empty one too, is refused as truncated.
    /// So is a length or a count that claims more bytes than remain, up to
    /// all of `u64`, and a float short of its bytes.
    #[test]
    fn truncated_bytes_are_refused_not_a_panic() {
        for (name, _, vector) in VECTORS {
            let bytes = hex(vector);
            assert_eq!(decode(&bytes), Ok(cbor::decode(&bytes)), "{name}");
            for cut in 0..bytes.len() {
                let refused = decode(&bytes[..cut]);
                assert_eq!(refused, Err(Malformed::Truncated), "{name} cut at {cut}");
            }
        }
        // Bytes, text, an array and a map, each claiming 2^64 - 1.
        for initial in [0x5b, 0x7b, 0x9b, 0xbb] {
            let claim = [&[initial][..], &[0xff; 8]].concat();
            let refused = decode(&claim);
            assert_eq!(refused, Err(Malformed::Truncated), "{initial:#04x}");
        }
        let half_a_double = [0xfb, 0x3f, 0xf8, 0x00, 0x00];
        assert_eq!(decode(&half_a_double), Err(Malformed::Truncated));
    }

    /// F15: `cbor::decode` panicked on CBOR outside the wire's subset. Each
    /// such item is refused, at the top or inside a message: a tag, an
    /// indefinite or reserved length, a break, a simple value other than
    /// false, true, null and the three floats, text that is not UTF-8, and a
    /// map key that is not an int; and so are bytes after the item. Each kind
    /// of item the subset takes decodes as `cbor::decode` decodes it.
    #[test]
    fn malformed_bytes_are_refused_not_a_panic() {
        use Malformed::{MapKey, NotUtf8, TooDeep, Trailing, Truncated, Unsupported};
        let refused: &[(&[u8], Malformed)] = &[
            (&[0xc0, 0x00], Unsupported(0xc0)),
            (&[0xa1, 0x01, 0x81, 0xc1, 0x00], Unsupported(0xc1)),
            (&[0x9f, 0xff], Unsupported(0x9f)),
            (&[0x5f, 0xff], Unsupported(0x5f)),
            (&[0x1c], Unsupported(0x1c)),
            (&[0xff], Unsupported(0xff)),
            (&[0xe0], Unsupported(0xe0)),
            (&[0xf7], Unsupported(0xf7)),
            (&[0xf8, 0x20], Unsupported(0xf8)),
            (&[0x62, 0xc3, 0x28], NotUtf8),
            (&[0xa1, 0x61, 0x61, 0x00], MapKey),
            (&[0xa1, 0xf6, 0x00], MapKey),
            (&[0xa1, 0x80, 0x00], MapKey),
            (&[0x00, 0x00], Trailing),
        ];
        for (bytes, why) in refused {
            assert_eq!(decode(bytes), Err(*why), "{bytes:02x?}");
        }
        let taken: &[&[u8]] = &[
            &[0xf4],
            &[0xf5],
            &[0xf6],
            &[0xf9, 0x3e, 0x00],
            &[0xfa, 0x3f, 0xc0, 0x00, 0x00],
            &[0xfb, 0x3f, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01],
            &[0x3b, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            &[0x40],
            &[0x60],
            &[0x80],
            &[0xa2, 0x20, 0x61, 0x61, 0x19, 0x01, 0x00, 0x42, 0x00, 0x01],
        ];
        for bytes in taken {
            assert_eq!(decode(bytes), Ok(cbor::decode(bytes)), "{bytes:02x?}");
        }
        let said = [
            (TooDeep, "bad frame: nested deeper than 32"),
            (Truncated, "bad frame: truncated"),
            (Trailing, "bad frame: bytes after its message"),
            (
                Unsupported(0xc0),
                "bad frame: CBOR the wire does not take (0xc0)",
            ),
            (MapKey, "bad frame: a map key that is not an int"),
            (NotUtf8, "bad frame: text that is not UTF-8"),
        ];
        for (why, words) in said {
            assert_eq!(why.to_string(), words);
        }
    }
}
