// taut v0.10.0 wrote this file: the command in glade/node/ir/sysdata.taut.py
// regenerates it. Do not edit it by hand.
// GENERATED native Rust types + codec — do not edit.
#![allow(dead_code)]
use crate::cbor::{Cbor, DecodeError};

// The file's bounds, for a decode rooted at a type that is not a message:
// `cbor::try_decode_with(bytes, MAX_DEPTH, MAX_ENCODED_LEN)`.
pub const MAX_DEPTH: usize = 32;
pub const MAX_ENCODED_LEN: Option<usize> = None;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NodeRecord {
    pub node_id: String,
    pub operator: String,
}
impl NodeRecord {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node_id.clone())),
            (2, Cbor::Text(self.operator.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            node_id: c.try_get(1)?.try_text()?,
            operator: c.try_get(2)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WorkspaceEntry {
    pub workspace: String,
    pub name: String,
    pub eligible_hosts: Vec<String>,
}
impl WorkspaceEntry {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.workspace.clone())),
            (2, Cbor::Text(self.name.clone())),
            (3, Cbor::Array(self.eligible_hosts.iter().map(|x| Cbor::Text(x.clone())).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            workspace: c.try_get(1)?.try_text()?,
            name: c.try_get(2)?.try_text()?,
            eligible_hosts: c.try_get(3)?.try_array()?.iter().map(|x| Ok(x.try_text()?)).collect::<Result<Vec<_>, DecodeError>>()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ServeClaim {
    pub node: String,
    pub share: String,
    pub lease_expiry_ms: i64,
    pub epoch: i64,
}
impl ServeClaim {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node.clone())),
            (2, Cbor::Text(self.share.clone())),
            (3, Cbor::Int(self.lease_expiry_ms)),
            (4, Cbor::Int(self.epoch)),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            node: c.try_get(1)?.try_text()?,
            share: c.try_get(2)?.try_text()?,
            lease_expiry_ms: c.try_get(3)?.try_int()?,
            epoch: c.try_get(4)?.try_int()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CapabilityGrant {
    pub principal: String,
    pub share: String,
    pub verbs: Vec<String>,
}
impl CapabilityGrant {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.principal.clone())),
            (2, Cbor::Text(self.share.clone())),
            (3, Cbor::Array(self.verbs.iter().map(|x| Cbor::Text(x.clone())).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            principal: c.try_get(1)?.try_text()?,
            share: c.try_get(2)?.try_text()?,
            verbs: c.try_get(3)?.try_array()?.iter().map(|x| Ok(x.try_text()?)).collect::<Result<Vec<_>, DecodeError>>()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CapabilityRevocation {
    pub principal: String,
    pub share: String,
}
impl CapabilityRevocation {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.principal.clone())),
            (2, Cbor::Text(self.share.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            principal: c.try_get(1)?.try_text()?,
            share: c.try_get(2)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct BindingDecl {
    pub app: String,
    pub glade_id: String,
    pub shape: String,
    pub authority: String,
    pub zone: String,
    pub retention: String,
}
impl BindingDecl {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.app.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
            (3, Cbor::Text(self.shape.clone())),
            (4, Cbor::Text(self.authority.clone())),
            (5, Cbor::Text(self.zone.clone())),
            (6, Cbor::Text(self.retention.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            app: c.try_get(1)?.try_text()?,
            glade_id: c.try_get(2)?.try_text()?,
            shape: c.try_get(3)?.try_text()?,
            authority: c.try_get(4)?.try_text()?,
            zone: c.try_get(5)?.try_text()?,
            retention: c.try_get(6)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct BindingRetraction {
    pub app: String,
    pub glade_id: String,
}
impl BindingRetraction {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.app.clone())),
            (2, Cbor::Text(self.glade_id.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            app: c.try_get(1)?.try_text()?,
            glade_id: c.try_get(2)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ServiceDefinition {
    pub app: String,
    pub name: String,
    pub glade_id: String,
}
impl ServiceDefinition {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.app.clone())),
            (2, Cbor::Text(self.name.clone())),
            (3, Cbor::Text(self.glade_id.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            app: c.try_get(1)?.try_text()?,
            name: c.try_get(2)?.try_text()?,
            glade_id: c.try_get(3)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PrincipalRecord {
    pub principal: String,
}
impl PrincipalRecord {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.principal.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            principal: c.try_get(1)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NodeTransportBinding {
    pub node: String,
    pub endpoint_id: String,
    pub valid_from: i64,
    pub sig: Vec<u8>,
}
impl NodeTransportBinding {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node.clone())),
            (2, Cbor::Text(self.endpoint_id.clone())),
            (3, Cbor::Int(self.valid_from)),
            (4, Cbor::Bytes(self.sig.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            node: c.try_get(1)?.try_text()?,
            endpoint_id: c.try_get(2)?.try_text()?,
            valid_from: c.try_get(3)?.try_int()?,
            sig: c.try_get(4)?.try_bytes()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NodeTransportRevocation {
    pub node: String,
    pub endpoint_id: String,
    pub sig: Vec<u8>,
}
impl NodeTransportRevocation {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node.clone())),
            (2, Cbor::Text(self.endpoint_id.clone())),
            (3, Cbor::Bytes(self.sig.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            node: c.try_get(1)?.try_text()?,
            endpoint_id: c.try_get(2)?.try_text()?,
            sig: c.try_get(3)?.try_bytes()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NodeRecoveryKey {
    pub node: String,
    pub recovery_key: String,
}
impl NodeRecoveryKey {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node.clone())),
            (2, Cbor::Text(self.recovery_key.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            node: c.try_get(1)?.try_text()?,
            recovery_key: c.try_get(2)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChainCheckpoint {
    pub node: String,
    pub stream: String,
    pub seq: i64,
    pub hash: Vec<u8>,
}
impl ChainCheckpoint {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node.clone())),
            (2, Cbor::Text(self.stream.clone())),
            (3, Cbor::Int(self.seq)),
            (4, Cbor::Bytes(self.hash.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            node: c.try_get(1)?.try_text()?,
            stream: c.try_get(2)?.try_text()?,
            seq: c.try_get(3)?.try_int()?,
            hash: c.try_get(4)?.try_bytes()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SignedRecord {
    pub record: Vec<u8>,
    pub sig: Vec<u8>,
}
impl SignedRecord {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Bytes(self.record.clone())),
            (2, Cbor::Bytes(self.sig.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            record: c.try_get(1)?.try_bytes()?,
            sig: c.try_get(2)?.try_bytes()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WorkspaceCreateReq {
    pub workspace: String,
    pub name: String,
    pub target: String,
}
impl WorkspaceCreateReq {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.workspace.clone())),
            (2, Cbor::Text(self.name.clone())),
            (3, Cbor::Text(self.target.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            workspace: c.try_get(1)?.try_text()?,
            name: c.try_get(2)?.try_text()?,
            target: c.try_get(3)?.try_text()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WorkspaceCreateRes {
    pub workspace: String,
    pub node: String,
    pub created: bool,
}
impl WorkspaceCreateRes {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.workspace.clone())),
            (2, Cbor::Text(self.node.clone())),
            (3, Cbor::Bool(self.created)),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            workspace: c.try_get(1)?.try_text()?,
            node: c.try_get(2)?.try_text()?,
            created: c.try_get(3)?.try_bool()?,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SystemSnapshot {
    pub records: Vec<Vec<u8>>,
    pub heads: Vec<Vec<u8>>,
    pub revision: Option<i64>,
}
impl SystemSnapshot {
    pub const MAX_DEPTH: usize = 32;
    pub const MAX_ENCODED_LEN: Option<usize> = None;
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Array(self.records.iter().map(|x| Cbor::Bytes(x.clone())).collect())),
            (2, Cbor::Array(self.heads.iter().map(|x| Cbor::Bytes(x.clone())).collect())),
            (3, match &self.revision { Some(v) => Cbor::Int(*v), None => Cbor::Null }),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Result<Self, DecodeError> {
        Ok(Self {
            records: c.try_get(1)?.try_array()?.iter().map(|x| Ok(x.try_bytes()?)).collect::<Result<Vec<_>, DecodeError>>()?,
            heads: c.try_get(2)?.try_array()?.iter().map(|x| Ok(x.try_bytes()?)).collect::<Result<Vec<_>, DecodeError>>()?,
            revision: { let v = c.try_get(3)?; if v.is_null() { None } else { Some(v.try_int()?) } },
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_cbor(&crate::cbor::try_decode_with(bytes, Self::MAX_DEPTH, Self::MAX_ENCODED_LEN)?)
    }
}
