//! Glade's exact SWMR op-payload adapter (`glade.swmr.adapter/v1`).
//!
//! The authenticated `Op.origin` is the canonical writer id. These two bytes
//! select the canonical `swmr.oracle/v1` input; the remaining bytes stay
//! opaque to the Glade transport and node.

pub const SWMR_ADAPTER_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwmrAction {
    Snapshot,
    Delta,
    Reset,
}

impl SwmrAction {
    fn tag(self) -> u8 {
        match self {
            Self::Snapshot => 0,
            Self::Delta => 1,
            Self::Reset => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwmrPayload<'a> {
    pub action: SwmrAction,
    pub body: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwmrPayloadError {
    TooShort,
    UnsupportedVersion(u8),
    UnsupportedAction(u8),
}

pub fn encode_swmr(action: SwmrAction, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + body.len());
    out.extend_from_slice(&[SWMR_ADAPTER_VERSION, action.tag()]);
    out.extend_from_slice(body);
    out
}

pub fn decode_swmr(payload: &[u8]) -> Result<SwmrPayload<'_>, SwmrPayloadError> {
    if payload.len() < 2 {
        return Err(SwmrPayloadError::TooShort);
    }
    if payload[0] != SWMR_ADAPTER_VERSION {
        return Err(SwmrPayloadError::UnsupportedVersion(payload[0]));
    }
    let action = match payload[1] {
        0 => SwmrAction::Snapshot,
        1 => SwmrAction::Delta,
        2 => SwmrAction::Reset,
        tag => return Err(SwmrPayloadError::UnsupportedAction(tag)),
    };
    Ok(SwmrPayload { action, body: &payload[2..] })
}
