//! Echo provider (P1.S6) — the trivial authority session that proves the
//! directed legs: `EXCHANGE` (request/response) and `CHANNEL` (live bytes),
//! neither replicated. Carrier-free: `handle` maps an inbound frame to the
//! frames to send back, honoring correlation ids and propagating channel close.

use std::collections::BTreeSet;

use glade_wire::generated::{ChannelClose, ChannelData, ExchangeRes};

use crate::frame::Frame;

/// An echo authority session. Tracks open channels so data only echoes on a
/// live channel and a close is propagated exactly once.
#[derive(Default)]
pub struct Echo {
    open: BTreeSet<String>,
}

impl Echo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle one inbound frame; return frames to send back to the peer.
    pub fn handle(&mut self, frame: &Frame) -> Vec<Frame> {
        match frame {
            Frame::ExchangeReq(req) => vec![Frame::ExchangeRes(ExchangeRes {
                corr: req.corr.clone(), // correlation id echoed back
                ok: true,
                payload: Some(req.payload.clone()),
                error: None,
            })],
            Frame::ChannelOpen(open) => {
                self.open.insert(open.channel.clone());
                vec![]
            }
            Frame::ChannelData(data) if self.open.contains(&data.channel) => {
                vec![Frame::ChannelData(ChannelData {
                    channel: data.channel.clone(),
                    data: data.data.clone(),
                })]
            }
            Frame::ChannelClose(close) if self.open.remove(&close.channel) => {
                vec![Frame::ChannelClose(ChannelClose {
                    channel: close.channel.clone(),
                    reason: close.reason.clone(),
                })]
            }
            _ => vec![], // data on a closed channel, or anything else: nothing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glade_wire::generated::{ChannelClose, ChannelData, ChannelOpen, ExchangeReq};

    #[test]
    fn exchange_echoes_payload_and_corr() {
        let mut e = Echo::new();
        let out = e.handle(&Frame::ExchangeReq(ExchangeReq {
            share: "sh".into(),
            glade_id: "echo".into(),
            corr: "x1".into(),
            payload: b"ping".to_vec(),
        }));
        match out.as_slice() {
            [Frame::ExchangeRes(res)] => {
                assert_eq!(res.corr, "x1"); // correlation honored
                assert!(res.ok);
                assert_eq!(res.payload.as_deref(), Some(b"ping".as_slice()));
            }
            other => panic!("expected one ExchangeRes, got {other:?}"),
        }
    }

    #[test]
    fn channel_echoes_only_while_open_and_close_propagates() {
        let mut e = Echo::new();
        let ch = || "ch1".to_string();
        // data before open: ignored
        assert!(e.handle(&Frame::ChannelData(ChannelData { channel: ch(), data: b"x".to_vec() })).is_empty());
        // open, then data echoes
        e.handle(&Frame::ChannelOpen(ChannelOpen {
            share: "sh".into(),
            glade_id: "pty".into(),
            channel: ch(),
            key: None,
        }));
        match e.handle(&Frame::ChannelData(ChannelData { channel: ch(), data: b"abc".to_vec() })).as_slice() {
            [Frame::ChannelData(d)] => assert_eq!(d.data, b"abc"),
            other => panic!("expected echoed ChannelData, got {other:?}"),
        }
        // close propagates once...
        match e.handle(&Frame::ChannelClose(ChannelClose { channel: ch(), reason: None })).as_slice() {
            [Frame::ChannelClose(c)] => assert_eq!(c.channel, "ch1"),
            other => panic!("expected ChannelClose, got {other:?}"),
        }
        // ...and data after close is ignored
        assert!(e.handle(&Frame::ChannelData(ChannelData { channel: ch(), data: b"y".to_vec() })).is_empty());
    }
}
