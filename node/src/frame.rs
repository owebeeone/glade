//! Frame codec (P1.S2). The session multiplexes one connection; each frame is
//! `[FrameType tag byte][glade-wire CBOR of the frame message]`. The type tag
//! is the transport discriminator (the frozen `FrameType` enum); the bodies are
//! the frozen frame messages from `glade-wire`. Carrier-agnostic: the same
//! bytes ride a websocket (M-LIMP) or iroh (post-LIMP).

use glade_wire::cbor;
use glade_wire::generated::{
    ChannelClose, ChannelData, ChannelOpen, Error, ExchangeReq, ExchangeRes, FrameType, Heads,
    Hello, NodeHello, NodeWelcome, Ops, Subscribe, Unsubscribe, Welcome,
};

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

    pub fn from_bytes(bytes: &[u8]) -> Result<Frame, String> {
        let (&tag, rest) = bytes.split_first().ok_or("empty frame")?;
        let c = cbor::decode(rest);
        Ok(match FrameType::from_wire(tag as i64) {
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
    use glade_wire::generated::{Head, Op, Shape, StreamHeads};

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
}
