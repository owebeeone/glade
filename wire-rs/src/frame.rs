//! A frame's size limit, for every carrier that reads frames: the node's
//! websocket and peer stream, and client-rs's websocket (CD-G3 item 3,
//! CD-G4). A frame is its `FrameType` tag byte, then one frame message; the
//! limit is taken from the schema's file-level `max_encoded_len`
//! (`taut/ir/glade.taut.py`), so the node and its clients cannot drift apart.
//! A carrier checks a frame's claimed length with [`frame_len`] before it
//! allocates anything for it (F15).

use std::io;

use crate::generated::MAX_ENCODED_LEN;

/// The most bytes a frame may hold, its tag byte included: the tag, then a
/// frame message of at most the schema's `max_encoded_len`, 16 MiB less one.
/// So 16 MiB, the frame limit the owner ruled for the carrier port (plan
/// Step 4.5b, question 6).
pub const MAX_FRAME_BYTES: usize = match MAX_ENCODED_LEN {
    Some(message) => 1 + message,
    None => panic!("glade's schema declares no max_encoded_len"),
};

/// The length a frame's header claims, if it is at most [`MAX_FRAME_BYTES`],
/// checked before anything is allocated for it. A longer one is refused as
/// `InvalidData`, which ends its connection: the stream cannot be read past
/// a body left unread.
pub fn frame_len(claimed: u64) -> io::Result<usize> {
    match usize::try_from(claimed) {
        Ok(len) if len <= MAX_FRAME_BYTES => Ok(len),
        _ => {
            let said = format!("bad frame: {claimed} bytes, over the limit of {MAX_FRAME_BYTES}");
            Err(io::Error::new(io::ErrorKind::InvalidData, said))
        }
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::{self, FrameType};

    /// CD-G3 item 3: the frame limit is the tag byte plus the schema's
    /// `max_encoded_len`, which glade's schema declares at file level so that
    /// the limit stays 16 MiB. Every frame message resolves to that bound and
    /// to the default depth, so one raw decode serves every frame type
    /// (CD-G4).
    #[test]
    fn the_frame_limit_is_the_tag_byte_and_the_schemas_bound() {
        assert_eq!(MAX_FRAME_BYTES, 16 << 20);
        assert_eq!(MAX_ENCODED_LEN, Some(MAX_FRAME_BYTES - 1));
        assert_eq!(generated::MAX_DEPTH, crate::cbor::DEFAULT_MAX_DEPTH);
        let bounds = [
            (
                generated::Hello::MAX_DEPTH,
                generated::Hello::MAX_ENCODED_LEN,
            ),
            (
                generated::Welcome::MAX_DEPTH,
                generated::Welcome::MAX_ENCODED_LEN,
            ),
            (
                generated::NodeHello::MAX_DEPTH,
                generated::NodeHello::MAX_ENCODED_LEN,
            ),
            (
                generated::NodeWelcome::MAX_DEPTH,
                generated::NodeWelcome::MAX_ENCODED_LEN,
            ),
            (
                generated::Subscribe::MAX_DEPTH,
                generated::Subscribe::MAX_ENCODED_LEN,
            ),
            (
                generated::Unsubscribe::MAX_DEPTH,
                generated::Unsubscribe::MAX_ENCODED_LEN,
            ),
            (generated::Ops::MAX_DEPTH, generated::Ops::MAX_ENCODED_LEN),
            (
                generated::Heads::MAX_DEPTH,
                generated::Heads::MAX_ENCODED_LEN,
            ),
            (
                generated::ExchangeReq::MAX_DEPTH,
                generated::ExchangeReq::MAX_ENCODED_LEN,
            ),
            (
                generated::ExchangeRes::MAX_DEPTH,
                generated::ExchangeRes::MAX_ENCODED_LEN,
            ),
            (
                generated::ChannelOpen::MAX_DEPTH,
                generated::ChannelOpen::MAX_ENCODED_LEN,
            ),
            (
                generated::ChannelData::MAX_DEPTH,
                generated::ChannelData::MAX_ENCODED_LEN,
            ),
            (
                generated::ChannelClose::MAX_DEPTH,
                generated::ChannelClose::MAX_ENCODED_LEN,
            ),
            (
                generated::Chunk::MAX_DEPTH,
                generated::Chunk::MAX_ENCODED_LEN,
            ),
            (
                generated::Error::MAX_DEPTH,
                generated::Error::MAX_ENCODED_LEN,
            ),
        ];
        assert_eq!(bounds.len(), FrameType::NodeWelcome.wire() as usize + 1);
        for (depth, len) in bounds {
            assert_eq!((depth, len), (generated::MAX_DEPTH, MAX_ENCODED_LEN));
        }
    }

    /// F15: a claimed length up to the limit is taken, exactly the limit
    /// included; one byte more, and every length up to `u64::MAX`, is
    /// refused as `InvalidData`, naming the length and the limit.
    #[test]
    fn a_claimed_length_over_the_limit_is_refused() {
        for taken in [0, 1, MAX_FRAME_BYTES as u64 - 1, MAX_FRAME_BYTES as u64] {
            assert_eq!(frame_len(taken).unwrap() as u64, taken);
        }
        for claimed in [MAX_FRAME_BYTES as u64 + 1, u64::from(u32::MAX), u64::MAX] {
            let refused = frame_len(claimed).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{claimed}");
            let said = format!("bad frame: {claimed} bytes, over the limit of 16777216");
            assert_eq!(refused.to_string(), said);
        }
    }
}
