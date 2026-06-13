// GENERATED from taut/ir + corpus by taut/src/taut/gen/rust.py — do not edit.
#![allow(dead_code)]
use crate::cbor::Cbor;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum FrameType {
    #[default] Hello,
    Welcome,
    Subscribe,
    Unsubscribe,
    Ops,
    Heads,
    ExchangeReq,
    ExchangeRes,
    ChannelOpen,
    ChannelData,
    ChannelClose,
    Chunk,
    Error,
}
impl FrameType {
    pub fn wire(self) -> i64 { match self {
        Self::Hello => 0,
        Self::Welcome => 1,
        Self::Subscribe => 2,
        Self::Unsubscribe => 3,
        Self::Ops => 4,
        Self::Heads => 5,
        Self::ExchangeReq => 6,
        Self::ExchangeRes => 7,
        Self::ChannelOpen => 8,
        Self::ChannelData => 9,
        Self::ChannelClose => 10,
        Self::Chunk => 11,
        Self::Error => 12,
    } }
    pub fn from_wire(v: i64) -> Self { match v {
        0 => Self::Hello,
        1 => Self::Welcome,
        2 => Self::Subscribe,
        3 => Self::Unsubscribe,
        4 => Self::Ops,
        5 => Self::Heads,
        6 => Self::ExchangeReq,
        7 => Self::ExchangeRes,
        8 => Self::ChannelOpen,
        9 => Self::ChannelData,
        10 => Self::ChannelClose,
        11 => Self::Chunk,
        12 => Self::Error,
        _ => panic!("bad FrameType wire value {}", v),
    } }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Priority {
    #[default] Control,
    Interactive,
    Bulk,
}
impl Priority {
    pub fn wire(self) -> i64 { match self {
        Self::Control => 0,
        Self::Interactive => 1,
        Self::Bulk => 2,
    } }
    pub fn from_wire(v: i64) -> Self { match v {
        0 => Self::Control,
        1 => Self::Interactive,
        2 => Self::Bulk,
        _ => panic!("bad Priority wire value {}", v),
    } }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ErrorCode {
    #[default] Ok,
    Equivocation,
    UnknownShare,
    Unauthorized,
    Protocol,
    Retention,
    Internal,
}
impl ErrorCode {
    pub fn wire(self) -> i64 { match self {
        Self::Ok => 0,
        Self::Equivocation => 1,
        Self::UnknownShare => 2,
        Self::Unauthorized => 3,
        Self::Protocol => 4,
        Self::Retention => 5,
        Self::Internal => 6,
    } }
    pub fn from_wire(v: i64) -> Self { match v {
        0 => Self::Ok,
        1 => Self::Equivocation,
        2 => Self::UnknownShare,
        3 => Self::Unauthorized,
        4 => Self::Protocol,
        5 => Self::Retention,
        6 => Self::Internal,
        _ => panic!("bad ErrorCode wire value {}", v),
    } }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Shape {
    #[default] Value,
    Log,
    Stream,
}
impl Shape {
    pub fn wire(self) -> i64 { match self {
        Self::Value => 0,
        Self::Log => 1,
        Self::Stream => 2,
    } }
    pub fn from_wire(v: i64) -> Self { match v {
        0 => Self::Value,
        1 => Self::Log,
        2 => Self::Stream,
        _ => panic!("bad Shape wire value {}", v),
    } }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Head {
    pub origin: String,
    pub seq: i64,
    pub hash: Option<Vec<u8>>,
}
impl Head {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.origin.clone())),
            (2, Cbor::Int(self.seq)),
            (3, match &self.hash { Some(v) => Cbor::Bytes(v.clone()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            origin: c.get(1).text(),
            seq: c.get(2).int(),
            hash: { let v = c.get(3); if v.is_null() { None } else { Some(v.bytes()) } },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct StreamHeads {
    pub share: String,
    pub glade_id: String,
    pub key: Vec<u8>,
    pub heads: Vec<Head>,
}
impl StreamHeads {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.share.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
            (3, Cbor::Bytes(self.key.clone())),
            (4, Cbor::Array(self.heads.iter().map(|x| x.to_cbor()).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            share: c.get(1).text(),
            glade_id: c.get(2).text(),
            key: c.get(3).bytes(),
            heads: c.get(4).array().iter().map(|x| Head::from_cbor(x)).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Op {
    pub share: String,
    pub glade_id: String,
    pub key: Vec<u8>,
    pub origin: String,
    pub seq: i64,
    pub prev: Option<Vec<u8>>,
    pub lamport: i64,
    pub refs: Vec<Head>,
    pub shape: Shape,
    pub payload: Vec<u8>,
}
impl Op {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.share.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
            (3, Cbor::Bytes(self.key.clone())),
            (4, Cbor::Text(self.origin.clone())),
            (5, Cbor::Int(self.seq)),
            (6, match &self.prev { Some(v) => Cbor::Bytes(v.clone()), None => Cbor::Null }),
            (7, Cbor::Int(self.lamport)),
            (8, Cbor::Array(self.refs.iter().map(|x| x.to_cbor()).collect())),
            (9, Cbor::Int(self.shape.wire())),
            (10, Cbor::Bytes(self.payload.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            share: c.get(1).text(),
            glade_id: c.get(2).text(),
            key: c.get(3).bytes(),
            origin: c.get(4).text(),
            seq: c.get(5).int(),
            prev: { let v = c.get(6); if v.is_null() { None } else { Some(v.bytes()) } },
            lamport: c.get(7).int(),
            refs: c.get(8).array().iter().map(|x| Head::from_cbor(x)).collect(),
            shape: Shape::from_wire(c.get(9).int()),
            payload: c.get(10).bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Hello {
    pub session: String,
    pub protocol: i64,
    pub principal: Option<String>,
    pub capability: Option<Vec<u8>>,
    pub heads: Vec<StreamHeads>,
}
impl Hello {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.session.clone())),
            (2, Cbor::Int(self.protocol)),
            (3, match &self.principal { Some(v) => Cbor::Text(v.clone()), None => Cbor::Null }),
            (4, match &self.capability { Some(v) => Cbor::Bytes(v.clone()), None => Cbor::Null }),
            (5, Cbor::Array(self.heads.iter().map(|x| x.to_cbor()).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            session: c.get(1).text(),
            protocol: c.get(2).int(),
            principal: { let v = c.get(3); if v.is_null() { None } else { Some(v.text()) } },
            capability: { let v = c.get(4); if v.is_null() { None } else { Some(v.bytes()) } },
            heads: c.get(5).array().iter().map(|x| StreamHeads::from_cbor(x)).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Welcome {
    pub session: String,
    pub protocol: i64,
    pub heads: Vec<StreamHeads>,
}
impl Welcome {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.session.clone())),
            (2, Cbor::Int(self.protocol)),
            (3, Cbor::Array(self.heads.iter().map(|x| x.to_cbor()).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            session: c.get(1).text(),
            protocol: c.get(2).int(),
            heads: c.get(3).array().iter().map(|x| StreamHeads::from_cbor(x)).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Subscribe {
    pub share: String,
    pub glade_id: String,
    pub key: Option<Vec<u8>>,
    pub from: Option<Vec<Head>>,
}
impl Subscribe {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.share.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
            (3, match &self.key { Some(v) => Cbor::Bytes(v.clone()), None => Cbor::Null }),
            (4, match &self.from { Some(v) => Cbor::Array(v.iter().map(|x| x.to_cbor()).collect()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            share: c.get(1).text(),
            glade_id: c.get(2).text(),
            key: { let v = c.get(3); if v.is_null() { None } else { Some(v.bytes()) } },
            from: { let v = c.get(4); if v.is_null() { None } else { Some(v.array().iter().map(|x| Head::from_cbor(x)).collect()) } },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Unsubscribe {
    pub share: String,
    pub glade_id: String,
    pub key: Option<Vec<u8>>,
}
impl Unsubscribe {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.share.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
            (3, match &self.key { Some(v) => Cbor::Bytes(v.clone()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            share: c.get(1).text(),
            glade_id: c.get(2).text(),
            key: { let v = c.get(3); if v.is_null() { None } else { Some(v.bytes()) } },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Ops {
    pub ops: Vec<Op>,
    pub pri: Option<Priority>,
}
impl Ops {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Array(self.ops.iter().map(|x| x.to_cbor()).collect())),
            (2, match &self.pri { Some(v) => Cbor::Int(v.wire()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            ops: c.get(1).array().iter().map(|x| Op::from_cbor(x)).collect(),
            pri: { let v = c.get(2); if v.is_null() { None } else { Some(Priority::from_wire(v.int())) } },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Heads {
    pub streams: Vec<StreamHeads>,
}
impl Heads {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Array(self.streams.iter().map(|x| x.to_cbor()).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            streams: c.get(1).array().iter().map(|x| StreamHeads::from_cbor(x)).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ExchangeReq {
    pub share: String,
    pub glade_id: String,
    pub corr: String,
    pub payload: Vec<u8>,
}
impl ExchangeReq {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.share.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
            (3, Cbor::Text(self.corr.clone())),
            (4, Cbor::Bytes(self.payload.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            share: c.get(1).text(),
            glade_id: c.get(2).text(),
            corr: c.get(3).text(),
            payload: c.get(4).bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ExchangeRes {
    pub corr: String,
    pub ok: bool,
    pub payload: Option<Vec<u8>>,
    pub error: Option<String>,
}
impl ExchangeRes {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.corr.clone())),
            (2, Cbor::Bool(self.ok)),
            (3, match &self.payload { Some(v) => Cbor::Bytes(v.clone()), None => Cbor::Null }),
            (4, match &self.error { Some(v) => Cbor::Text(v.clone()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            corr: c.get(1).text(),
            ok: c.get(2).boolean(),
            payload: { let v = c.get(3); if v.is_null() { None } else { Some(v.bytes()) } },
            error: { let v = c.get(4); if v.is_null() { None } else { Some(v.text()) } },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChannelOpen {
    pub share: String,
    pub glade_id: String,
    pub channel: String,
    pub key: Option<Vec<u8>>,
}
impl ChannelOpen {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.share.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
            (3, Cbor::Text(self.channel.clone())),
            (4, match &self.key { Some(v) => Cbor::Bytes(v.clone()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            share: c.get(1).text(),
            glade_id: c.get(2).text(),
            channel: c.get(3).text(),
            key: { let v = c.get(4); if v.is_null() { None } else { Some(v.bytes()) } },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChannelData {
    pub channel: String,
    pub data: Vec<u8>,
}
impl ChannelData {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.channel.clone())),
            (2, Cbor::Bytes(self.data.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            channel: c.get(1).text(),
            data: c.get(2).bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChannelClose {
    pub channel: String,
    pub reason: Option<String>,
}
impl ChannelClose {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.channel.clone())),
            (2, match &self.reason { Some(v) => Cbor::Text(v.clone()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            channel: c.get(1).text(),
            reason: { let v = c.get(2); if v.is_null() { None } else { Some(v.text()) } },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Chunk {
    pub corr: String,
    pub index: i64,
    pub total: i64,
    pub data: Vec<u8>,
}
impl Chunk {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.corr.clone())),
            (2, Cbor::Int(self.index)),
            (3, Cbor::Int(self.total)),
            (4, Cbor::Bytes(self.data.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            corr: c.get(1).text(),
            index: c.get(2).int(),
            total: c.get(3).int(),
            data: c.get(4).bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    pub share: Option<String>,
    pub glade_id: Option<String>,
    pub corr: Option<String>,
}
impl Error {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Int(self.code.wire())),
            (2, Cbor::Text(self.message.clone())),
            (3, match &self.share { Some(v) => Cbor::Text(v.clone()), None => Cbor::Null }),
            (4, match &self.glade_id { Some(v) => Cbor::Text(v.clone()), None => Cbor::Null }),
            (5, match &self.corr { Some(v) => Cbor::Text(v.clone()), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            code: ErrorCode::from_wire(c.get(1).int()),
            message: c.get(2).text(),
            share: { let v = c.get(3); if v.is_null() { None } else { Some(v.text()) } },
            glade_id: { let v = c.get(4); if v.is_null() { None } else { Some(v.text()) } },
            corr: { let v = c.get(5); if v.is_null() { None } else { Some(v.text()) } },
        }
    }
}

pub fn roundtrip(message: &str, bytes: &[u8]) -> Vec<u8> {
    let c = crate::cbor::decode(bytes);
    match message {
        "Head" => crate::cbor::encode(&Head::from_cbor(&c).to_cbor()),
        "StreamHeads" => crate::cbor::encode(&StreamHeads::from_cbor(&c).to_cbor()),
        "Op" => crate::cbor::encode(&Op::from_cbor(&c).to_cbor()),
        "Hello" => crate::cbor::encode(&Hello::from_cbor(&c).to_cbor()),
        "Welcome" => crate::cbor::encode(&Welcome::from_cbor(&c).to_cbor()),
        "Subscribe" => crate::cbor::encode(&Subscribe::from_cbor(&c).to_cbor()),
        "Unsubscribe" => crate::cbor::encode(&Unsubscribe::from_cbor(&c).to_cbor()),
        "Ops" => crate::cbor::encode(&Ops::from_cbor(&c).to_cbor()),
        "Heads" => crate::cbor::encode(&Heads::from_cbor(&c).to_cbor()),
        "ExchangeReq" => crate::cbor::encode(&ExchangeReq::from_cbor(&c).to_cbor()),
        "ExchangeRes" => crate::cbor::encode(&ExchangeRes::from_cbor(&c).to_cbor()),
        "ChannelOpen" => crate::cbor::encode(&ChannelOpen::from_cbor(&c).to_cbor()),
        "ChannelData" => crate::cbor::encode(&ChannelData::from_cbor(&c).to_cbor()),
        "ChannelClose" => crate::cbor::encode(&ChannelClose::from_cbor(&c).to_cbor()),
        "Chunk" => crate::cbor::encode(&Chunk::from_cbor(&c).to_cbor()),
        "Error" => crate::cbor::encode(&Error::from_cbor(&c).to_cbor()),
        _ => panic!("unknown message {}", message),
    }
}

pub static VECTORS: &[(&str, &str, &str)] = &[
    ("ChannelClose", "ChannelClose", "a20162733102627332"),
    ("ChannelData", "ChannelData", "a2016273310243020102"),
    ("ChannelOpen", "ChannelOpen", "a40162733102627332036273330443040102"),
    ("Chunk", "Chunk", "a4016273310219012c03000443040102"),
    ("Error", "Error", "a5010102627332036273330462733405627335"),
    ("ExchangeReq", "ExchangeReq", "a40162733102627332036273330443040102"),
    ("ExchangeRes", "ExchangeRes", "a40162733102f5034303010204627334"),
    ("Head", "Head", "a3016273310219012c0343030102"),
    ("Heads", "Heads", "a10181a4016273310262733203430301020481a3016273310219012c0343030102"),
    ("Hello", "Hello", "a5016273310219012c0362733304430401020581a4016273310262733203430301020481a3016273310219012c0343030102"),
    ("Op", "Op", "aa01627331026273320343030102046273340526064306010207000881a3016273310219012c034303010209000a430a0102"),
    ("Ops", "Ops", "a20181aa01627331026273320343030102046273340526064306010207000881a3016273310219012c034303010209000a430a01020202"),
    ("StreamHeads", "StreamHeads", "a4016273310262733203430301020481a3016273310219012c0343030102"),
    ("Subscribe", "Subscribe", "a4016273310262733203430301020481a3016273310219012c0343030102"),
    ("Unsubscribe", "Unsubscribe", "a301627331026273320343030102"),
    ("Welcome", "Welcome", "a3016273310219012c0381a4016273310262733203430301020481a3016273310219012c0343030102"),
    ("edge/channel-close", "ChannelClose", "a2016363683102f6"),
    ("edge/channel-data", "ChannelData", "a2016363683102496b65797374726f6b65"),
    ("edge/channel-open", "ChannelOpen", "a401627368026370747903636368310440"),
    ("edge/chunk", "Chunk", "a4016263310200030404590100000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff"),
    ("edge/error-equivocation", "Error", "a501010275666f726b656420636861696e20617420286f2c35290362736804616705f6"),
    ("edge/error-unicode", "Error", "a50104027819c3a9e28094e4b8ade28094f09f988020626164206672616d6503657368c3a4720467676ce29390646505f6"),
    ("edge/exchange-err", "ExchangeRes", "a40162783202f403f60464626f6f6d"),
    ("edge/exchange-ok", "ExchangeRes", "a40162783102f503436f757404f6"),
    ("edge/exchange-req", "ExchangeReq", "a401627368026372756e03627831044461726776"),
    ("edge/heads-frame", "Heads", "a10181a40162736802616703400482a301616102070358200101010101010101010101010101010101010101010101010101010101010101a301616202000358200202020202020202020202020202020202020202020202020202020202020202"),
    ("edge/hello-anon", "Hello", "a50166736573732d32020103f604f60580"),
    ("edge/hello-resume", "Hello", "a50166736573732d3102010368757365723a616e6e044200010581a40162736802616703400482a301616102070358200101010101010101010101010101010101010101010101010101010101010101a301616202000358200202020202020202020202020202020202020202020202020202020202020202"),
    ("edge/op-chain", "Op", "aa016273680261670342010204616f0505065820abababababababababababababababababababababababababababababababab07090882a301626f3202030340a301626f33020103f609010a4568656c6c6f"),
    ("edge/op-min", "Op", "aa01627368026167034004616f050006f60700088009000a40"),
    ("edge/ops-bulk", "Ops", "a20182aa01627368026167034004616f050006f60700088009000a40aa016273680261670342010204616f0505065820abababababababababababababababababababababababababababababababab07090882a301626f3202030340a301626f33020103f609010a4568656c6c6f0202"),
    ("edge/ops-nopri", "Ops", "a20181aa01627368026167034004616f050006f60700088009000a4002f6"),
    ("edge/streamheads-multi", "StreamHeads", "a40162736802616703400482a301616102070358200101010101010101010101010101010101010101010101010101010101010101a301616202000358200202020202020202020202020202020202020202020202020202020202020202"),
    ("edge/subscribe-allkeys", "Subscribe", "a40162736802616703f604f6"),
    ("edge/subscribe-key", "Subscribe", "a401627368026167034201020481a3016161020703f6"),
    ("edge/unsubscribe", "Unsubscribe", "a30162736802616703f6"),
    ("edge/welcome", "Welcome", "a30166736573732d3102010381a40162736802616703400482a301616102070358200101010101010101010101010101010101010101010101010101010101010101a301616202000358200202020202020202020202020202020202020202020202020202020202020202"),
];
