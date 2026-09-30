//! Frame codec (P1.S2). The session multiplexes one connection; each frame is
//! `[FrameType tag byte][glade-wire CBOR of the frame message]`. The type tag
//! is the transport discriminator (the frozen `FrameType` enum); the bodies are
//! the frozen frame messages from `glade-wire`. Carrier-agnostic: the same
//! bytes ride a websocket (M-LIMP) or iroh (post-LIMP).

use glade_wire::cbor::{self, DecodeError};
use glade_wire::generated::{
    ChannelClose, ChannelData, ChannelOpen, Error, ExchangeReq, ExchangeRes, FrameType, Heads,
    Hello, NodeHello, NodeWelcome, Ops, Subscribe, Unsubscribe, Welcome,
};

/// A frame's size limit, glade-wire's since the move off the legacy codec
/// (TautCheckedDecode.md CD-G3), so client-rs applies the same one: the
/// websocket (`ws.rs`) and the peer stream (`peer::read_frame`) refuse a
/// header that claims more than [`MAX_FRAME_BYTES`] (F15).
pub use glade_wire::frame::{frame_len, MAX_FRAME_BYTES};

/// One decoded frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Hello(Hello),
    Welcome(Welcome),
    // node<->node handshake seam (Lane R step 2): peer identity, not a session.
    NodeHello(NodeHello),
    NodeWelcome(NodeWelcome),
    Subscribe(Subscribe),
    Unsubscribe(Unsubscribe),
    Ops(Ops),
    Heads(Heads),
    ExchangeReq(ExchangeReq),
    ExchangeRes(ExchangeRes),
    ChannelOpen(ChannelOpen),
    ChannelData(ChannelData),
    ChannelClose(ChannelClose),
    // Chunk reassembly is handled by the carrier, not surfaced as a Frame here.
    Error(Error),
}

impl Frame {
    pub fn to_bytes(&self) -> Vec<u8> {
        let (ty, body) = match self {
            Frame::Hello(m) => (FrameType::Hello, m.to_cbor()),
            Frame::Welcome(m) => (FrameType::Welcome, m.to_cbor()),
            Frame::NodeHello(m) => (FrameType::NodeHello, m.to_cbor()),
            Frame::NodeWelcome(m) => (FrameType::NodeWelcome, m.to_cbor()),
            Frame::Subscribe(m) => (FrameType::Subscribe, m.to_cbor()),
            Frame::Unsubscribe(m) => (FrameType::Unsubscribe, m.to_cbor()),
            Frame::Ops(m) => (FrameType::Ops, m.to_cbor()),
            Frame::Heads(m) => (FrameType::Heads, m.to_cbor()),
            Frame::ExchangeReq(m) => (FrameType::ExchangeReq, m.to_cbor()),
            Frame::ExchangeRes(m) => (FrameType::ExchangeRes, m.to_cbor()),
            Frame::ChannelOpen(m) => (FrameType::ChannelOpen, m.to_cbor()),
            Frame::ChannelData(m) => (FrameType::ChannelData, m.to_cbor()),
            Frame::ChannelClose(m) => (FrameType::ChannelClose, m.to_cbor()),
            Frame::Error(m) => (FrameType::Error, m.to_cbor()),
        };
        let mut out = Vec::with_capacity(1 + 16);
        out.push(ty.wire() as u8);
        out.extend_from_slice(&cbor::encode(&body));
        out
    }

    /// Decode one frame, or refuse it as `bad frame: <why>`, where `why` is
    /// taut's `DecodeError` for it: an empty frame, a tag, op shape, priority
    /// or error code its enum does not name (F12), a message that is
    /// truncated, malformed or nested deeper than 32 (F15), and a message of
    /// another shape, a field missing or of another type, which the legacy
    /// codec panicked on (TautCheckedDecode.md CD-G3). A chunk is refused
    /// too: it is the carrier's.
    pub fn from_bytes(bytes: &[u8]) -> Result<Frame, String> {
        match Frame::decode(bytes) {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => Err("chunk is carrier-level, not a Frame".into()),
            Err(why) => Err(format!("bad frame: {why}")),
        }
    }

    /// One frame through taut's fail-closed codec: its tag, its message's
    /// CBOR, bounded 32 deep, and its message, each refused with its
    /// `DecodeError`; `None` for a chunk.
    fn decode(bytes: &[u8]) -> Result<Option<Frame>, DecodeError> {
        let (&tag, rest) = bytes.split_first().ok_or(DecodeError::Truncated)?;
        let ty = FrameType::from_wire(tag.into())?;
        let c = cbor::try_decode(rest)?;
        Ok(Some(match ty {
            FrameType::Hello => Frame::Hello(Hello::from_cbor(&c)?),
            FrameType::Welcome => Frame::Welcome(Welcome::from_cbor(&c)?),
            FrameType::NodeHello => Frame::NodeHello(NodeHello::from_cbor(&c)?),
            FrameType::NodeWelcome => Frame::NodeWelcome(NodeWelcome::from_cbor(&c)?),
            FrameType::Subscribe => Frame::Subscribe(Subscribe::from_cbor(&c)?),
            FrameType::Unsubscribe => Frame::Unsubscribe(Unsubscribe::from_cbor(&c)?),
            FrameType::Ops => Frame::Ops(Ops::from_cbor(&c)?),
            FrameType::Heads => Frame::Heads(Heads::from_cbor(&c)?),
            FrameType::ExchangeReq => Frame::ExchangeReq(ExchangeReq::from_cbor(&c)?),
            FrameType::ExchangeRes => Frame::ExchangeRes(ExchangeRes::from_cbor(&c)?),
            FrameType::ChannelOpen => Frame::ChannelOpen(ChannelOpen::from_cbor(&c)?),
            FrameType::ChannelData => Frame::ChannelData(ChannelData::from_cbor(&c)?),
            FrameType::ChannelClose => Frame::ChannelClose(ChannelClose::from_cbor(&c)?),
            FrameType::Chunk => return Ok(None),
            FrameType::Error => Frame::Error(Error::from_cbor(&c)?),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::cbor::Cbor;
    use glade_wire::generated::{ErrorCode, Head, Op, Priority, Shape, StreamHeads};

    #[test]
    fn frames_round_trip_through_bytes() {
        let op = Op {
            share: "sh".into(),
            glade_id: "g".into(),
            key: vec![],
            origin: "a".into(),
            seq: 1,
            prev: None,
            lamport: 1,
            refs: vec![Head { origin: "b".into(), seq: 2, hash: None }],
            shape: Shape::Value,
            payload: b"hi".to_vec(),
        };
        let frames = vec![
            Frame::Hello(Hello {
                session: "s1".into(),
                protocol: 1,
                principal: None,
                capability: None,
                heads: vec![StreamHeads {
                    share: "sh".into(),
                    glade_id: "".into(),
                    key: vec![],
                    heads: vec![Head { origin: "a".into(), seq: 1, hash: None }],
                }],
            }),
            Frame::Ops(Ops { ops: vec![op], pri: None }),
            Frame::Welcome(Welcome { session: "s1".into(), protocol: 1, heads: vec![] }),
        ];
        for f in frames {
            let round = Frame::from_bytes(&f.to_bytes()).unwrap();
            assert_eq!(round, f);
        }
    }

    /// A frame of type `ty` whose message is `body`.
    fn framed(ty: FrameType, body: &Cbor) -> Vec<u8> {
        let mut bytes = vec![ty.wire() as u8];
        bytes.extend(cbor::encode(body));
        bytes
    }

    /// `message`'s map, with the int `value` at `key`.
    fn with(mut message: Cbor, key: i64, value: i64) -> Cbor {
        if let Cbor::Map(entries) = &mut message {
            for (k, v) in entries.iter_mut() {
                if *k == key {
                    *v = Cbor::Int(value);
                }
            }
        }
        message
    }

    /// F12: a frame whose op shape, priority, error code or tag names no
    /// value of its enum is refused, naming the enum and the value, where
    /// decoding it panicked (`Shape::from_wire` and the others) and ended the
    /// task that read it. Its neighbours, a crdt op at bulk priority and the
    /// last error code, decode.
    #[test]
    fn a_frame_with_an_unknown_enum_value_is_refused_not_a_panic() {
        let op = |shape| with(Op::default().to_cbor(), 9, shape);
        let ops = |shape, pri| Cbor::Map(vec![(1, Cbor::Array(vec![op(shape)])), (2, pri)]);
        let error = |code| with(Error::default().to_cbor(), 1, code);
        let type_15 = [&[15][..], &cbor::encode(&Cbor::Map(vec![]))].concat();
        let refused = [
            (
                framed(FrameType::Ops, &ops(9, Cbor::Null)),
                "Shape wire value 9",
            ),
            (
                framed(FrameType::Ops, &ops(0, Cbor::Int(3))),
                "Priority wire value 3",
            ),
            (
                framed(FrameType::Error, &error(7)),
                "ErrorCode wire value 7",
            ),
            (type_15, "FrameType wire value 15"),
        ];
        for (bytes, value) in refused {
            let said = Frame::from_bytes(&bytes).unwrap_err();
            assert_eq!(said, format!("bad frame: unknown {value}"));
        }
        let crdt = Frame::from_bytes(&framed(FrameType::Ops, &ops(4, Cbor::Int(2))));
        let Ok(Frame::Ops(crdt)) = crdt else {
            panic!("{crdt:?}");
        };
        assert_eq!(crdt.ops[0].shape, Shape::Crdt);
        assert_eq!(crdt.pri, Some(Priority::Bulk));
        let internal = Frame::from_bytes(&framed(FrameType::Error, &error(6)));
        assert!(matches!(internal, Ok(Frame::Error(e)) if e.code == ErrorCode::Internal));
    }

    /// TautCheckedDecode.md CD-G3: a frame whose message is CBOR of another
    /// shape, a field missing or of another type, or no map, panicked the
    /// generated decode, which ended the session's task and left its
    /// connection open. Each is refused like any bad frame, naming why, as
    /// are an empty frame, a truncated one, a message nested deeper than 32
    /// and bytes the strict codec refuses; a chunk is the carrier's.
    #[test]
    fn a_frame_of_another_shape_is_refused_not_a_panic() {
        let hello = Frame::Hello(Hello::default()).to_bytes();
        let mut session_an_int = Hello::default().to_cbor();
        if let Cbor::Map(entries) = &mut session_an_int {
            entries[0].1 = Cbor::Int(1);
        }
        let ops_an_int = Cbor::Map(vec![(1, Cbor::Int(1)), (2, Cbor::Null)]);
        let nested = [&[0x81].repeat(33)[..], &[0x80]].concat();
        let refused = [
            (
                framed(FrameType::Hello, &Cbor::Map(vec![])),
                "missing map key 1",
            ),
            (
                framed(FrameType::Hello, &session_an_int),
                "expected CBOR text",
            ),
            (
                framed(FrameType::Subscribe, &Cbor::Int(3)),
                "expected CBOR map",
            ),
            (framed(FrameType::Ops, &ops_an_int), "expected CBOR array"),
            (vec![], "truncated CBOR input"),
            (hello[..hello.len() - 1].to_vec(), "truncated CBOR input"),
            (
                [&hello[..1], &nested].concat(),
                "CBOR nested deeper than 32",
            ),
            (
                [&hello[..1], &[0x18, 0x01]].concat(),
                "non-canonical integer encoding of 1",
            ),
            (
                [&hello[..1], &[0xc0, 0x00]].concat(),
                "unsupported major type 6",
            ),
        ];
        for (bytes, why) in refused {
            let said = Frame::from_bytes(&bytes).unwrap_err();
            assert_eq!(said, format!("bad frame: {why}"), "{bytes:02x?}");
        }
        let chunk = glade_wire::generated::Chunk::default().to_cbor();
        let chunk = framed(FrameType::Chunk, &chunk);
        let said = Frame::from_bytes(&chunk).unwrap_err();
        assert_eq!(said, "chunk is carrier-level, not a Frame");
    }
}
