//! Frame codec (P1.S2). The session multiplexes one connection; each frame is
//! `[FrameType tag byte][glade-wire CBOR of the frame message]`. The type tag
//! is the transport discriminator (the frozen `FrameType` enum); the bodies are
//! the frozen frame messages from `glade-wire`. Carrier-agnostic: the same
//! bytes ride a websocket (M-LIMP) or iroh (post-LIMP).

use glade_wire::generated::{
    ChannelClose, ChannelData, ChannelOpen, Error, ExchangeReq, ExchangeRes, FrameType, Heads,
    Hello, NodeHello, NodeWelcome, Ops, Subscribe, Unsubscribe, Welcome,
};
use glade_wire::{cbor, checked};

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

    /// Decode one frame, or refuse it: an empty frame, a chunk, and (F12) a
    /// frame whose tag, op shape, priority or error code names no value of
    /// its enum, which the generated decode panics on (`glade_wire::checked`).
    pub fn from_bytes(bytes: &[u8]) -> Result<Frame, String> {
        let (&tag, rest) = bytes.split_first().ok_or("empty frame")?;
        let ty = checked::frame_type(tag).map_err(|e| e.to_string())?;
        let c = cbor::decode(rest);
        checked::frame_body(ty, &c).map_err(|e| e.to_string())?;
        Ok(match ty {
            FrameType::Hello => Frame::Hello(Hello::from_cbor(&c)),
            FrameType::Welcome => Frame::Welcome(Welcome::from_cbor(&c)),
            FrameType::NodeHello => Frame::NodeHello(NodeHello::from_cbor(&c)),
            FrameType::NodeWelcome => Frame::NodeWelcome(NodeWelcome::from_cbor(&c)),
            FrameType::Subscribe => Frame::Subscribe(Subscribe::from_cbor(&c)),
            FrameType::Unsubscribe => Frame::Unsubscribe(Unsubscribe::from_cbor(&c)),
            FrameType::Ops => Frame::Ops(Ops::from_cbor(&c)),
            FrameType::Heads => Frame::Heads(Heads::from_cbor(&c)),
            FrameType::ExchangeReq => Frame::ExchangeReq(ExchangeReq::from_cbor(&c)),
            FrameType::ExchangeRes => Frame::ExchangeRes(ExchangeRes::from_cbor(&c)),
            FrameType::ChannelOpen => Frame::ChannelOpen(ChannelOpen::from_cbor(&c)),
            FrameType::ChannelData => Frame::ChannelData(ChannelData::from_cbor(&c)),
            FrameType::ChannelClose => Frame::ChannelClose(ChannelClose::from_cbor(&c)),
            FrameType::Chunk => return Err("chunk is carrier-level, not a Frame".into()),
            FrameType::Error => Frame::Error(Error::from_cbor(&c)),
        })
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
    /// value of its enum is refused, naming the value, where decoding it
    /// panicked (`Shape::from_wire` and the others) and ended the task that
    /// read it. Its neighbours, a crdt op at the highest priority and the
    /// last error code, decode.
    #[test]
    fn a_frame_with_an_unknown_enum_value_is_refused_not_a_panic() {
        let op = |shape| with(Op::default().to_cbor(), 9, shape);
        let ops = |shape, pri| Cbor::Map(vec![(1, Cbor::Array(vec![op(shape)])), (2, pri)]);
        let error = |code| with(Error::default().to_cbor(), 1, code);
        let type_15 = [&[15][..], &cbor::encode(&Cbor::Map(vec![]))].concat();
        let refused = [
            (framed(FrameType::Ops, &ops(9, Cbor::Null)), "shape 9"),
            (framed(FrameType::Ops, &ops(0, Cbor::Int(3))), "priority 3"),
            (framed(FrameType::Error, &error(7)), "error code 7"),
            (type_15, "frame type 15"),
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
}
