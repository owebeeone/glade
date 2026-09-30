//! A frame from the node, decoded whole before the client touches any state
//! (TautCheckedDecode.md CD-G4): its tag, its message's CBOR and its message,
//! each by taut's fail-closed codec. A frame the codec refuses is refused
//! whole, with its `DecodeError`, where the legacy decode panicked the read
//! loop's task.

use glade_wire::cbor::{self, DecodeError};
use glade_wire::generated::{Error, ExchangeReq, ExchangeRes, FrameType, Heads, Op, Ops, Welcome};

/// One frame a client takes, typed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Inbound {
    Ops(Vec<Op>),
    Heads(Heads),
    Error(Error),
    Welcome(Welcome),
    ExchangeReq(ExchangeReq),
    ExchangeRes(ExchangeRes),
    /// A frame no client takes: a channel's (echo and channels are P3), or
    /// one only a node takes.
    Ignored,
}

impl Inbound {
    /// `bytes`, a whole frame, `[FrameType tag][CBOR of its message]`,
    /// decoded, or the codec's refusal; an empty frame is `Truncated`.
    /// glade's schema declares neither `max_depth` nor `max_encoded_len`,
    /// and the websocket applied the frame limit before it read the frame,
    /// so every frame type resolves to the same bounds, and one raw
    /// `try_decode`, at the default depth, serves every arm (CD-B3).
    pub(crate) fn decode(bytes: &[u8]) -> Result<Inbound, DecodeError> {
        let (&tag, body) = bytes.split_first().ok_or(DecodeError::Truncated)?;
        let ty = FrameType::from_wire(i64::from(tag))?;
        let c = cbor::try_decode(body)?;
        Ok(match ty {
            FrameType::Ops => Inbound::Ops(Ops::from_cbor(&c)?.ops),
            FrameType::Heads => Inbound::Heads(Heads::from_cbor(&c)?),
            FrameType::Error => Inbound::Error(Error::from_cbor(&c)?),
            FrameType::Welcome => Inbound::Welcome(Welcome::from_cbor(&c)?),
            FrameType::ExchangeReq => Inbound::ExchangeReq(ExchangeReq::from_cbor(&c)?),
            FrameType::ExchangeRes => Inbound::ExchangeRes(ExchangeRes::from_cbor(&c)?),
            _ => Inbound::Ignored,
        })
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::cbor::Cbor;
    use glade_wire::generated::{ChannelData, ErrorCode, Head, Hello, Shape, StreamHeads};

    fn framed(ty: FrameType, body: &Cbor) -> Vec<u8> {
        [&[ty.wire() as u8][..], &cbor::encode(body)].concat()
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

    /// CD-G4: each frame a client takes decodes whole into its arm; a frame
    /// no client takes (a channel's, a node's) is `Ignored`, as before.
    #[test]
    fn each_frame_a_client_takes_decodes_into_its_arm() {
        let op = Op {
            share: "s".into(),
            glade_id: "g".into(),
            origin: "o".into(),
            seq: 1,
            shape: Shape::Log,
            ..Op::default()
        };
        let ops = Ops {
            ops: vec![op.clone()],
            pri: None,
        };
        let head = Head {
            origin: "o".into(),
            seq: 1,
            hash: None,
        };
        let heads = Heads {
            streams: vec![StreamHeads {
                share: "s".into(),
                glade_id: "g".into(),
                key: vec![],
                heads: vec![head],
            }],
        };
        let error = Error {
            code: ErrorCode::Unauthorized,
            message: "no".into(),
            ..Error::default()
        };
        let welcome = Welcome {
            session: "w".into(),
            protocol: 1,
            heads: vec![],
        };
        let req = ExchangeReq {
            share: "s".into(),
            glade_id: "x".into(),
            corr: "c1".into(),
            payload: vec![1],
        };
        let res = ExchangeRes {
            corr: "c1".into(),
            ok: true,
            payload: Some(vec![2]),
            error: None,
        };
        let taken = [
            (
                framed(FrameType::Ops, &ops.to_cbor()),
                Inbound::Ops(vec![op]),
            ),
            (
                framed(FrameType::Heads, &heads.to_cbor()),
                Inbound::Heads(heads),
            ),
            (
                framed(FrameType::Error, &error.to_cbor()),
                Inbound::Error(error),
            ),
            (
                framed(FrameType::Welcome, &welcome.to_cbor()),
                Inbound::Welcome(welcome),
            ),
            (
                framed(FrameType::ExchangeReq, &req.to_cbor()),
                Inbound::ExchangeReq(req),
            ),
            (
                framed(FrameType::ExchangeRes, &res.to_cbor()),
                Inbound::ExchangeRes(res),
            ),
            (
                framed(FrameType::ChannelData, &ChannelData::default().to_cbor()),
                Inbound::Ignored,
            ),
            (
                framed(FrameType::Hello, &Hello::default().to_cbor()),
                Inbound::Ignored,
            ),
        ];
        for (bytes, inbound) in taken {
            assert_eq!(Inbound::decode(&bytes), Ok(inbound));
        }
    }

    /// CD-G4: a frame the codec refuses is refused whole, with its
    /// `DecodeError`: an empty frame, where the client dropped it silently;
    /// an unknown tag, a value its enum does not name and a message of
    /// another shape, where decoding it panicked the read loop's task; a
    /// truncated or malformed message and one nested deeper than 32. A frame
    /// no client takes is refused too when its bytes are not CBOR the codec
    /// takes.
    #[test]
    fn a_frame_the_codec_refuses_is_refused_whole() {
        let op = with(Op::default().to_cbor(), 9, 9);
        let ops = Cbor::Map(vec![(1, Cbor::Array(vec![op])), (2, Cbor::Null)]);
        let welcome = framed(FrameType::Welcome, &Welcome::default().to_cbor());
        let nested = [
            &[FrameType::Welcome.wire() as u8][..],
            &[0x81].repeat(33),
            &[0x80],
        ]
        .concat();
        let refused = [
            (vec![], DecodeError::Truncated),
            (
                vec![15, 0xa0],
                DecodeError::UnknownEnum {
                    enum_name: "FrameType",
                    value: 15,
                },
            ),
            (
                framed(FrameType::Ops, &ops),
                DecodeError::UnknownEnum {
                    enum_name: "Shape",
                    value: 9,
                },
            ),
            (
                framed(FrameType::Heads, &Cbor::Map(vec![])),
                DecodeError::MissingKey(1),
            ),
            (
                framed(FrameType::Error, &Cbor::Int(3)),
                DecodeError::WrongType { expected: "map" },
            ),
            (
                welcome[..welcome.len() - 1].to_vec(),
                DecodeError::Truncated,
            ),
            (nested, DecodeError::TooDeep { limit: 32 }),
            (
                vec![FrameType::ChannelData.wire() as u8, 0x18, 0x01],
                DecodeError::NonCanonicalInt(1),
            ),
        ];
        for (bytes, why) in refused {
            assert_eq!(Inbound::decode(&bytes), Err(why), "{bytes:02x?}");
        }
    }
}
