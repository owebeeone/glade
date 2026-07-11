// DEPRECATED (--legacy-codec): fail-OPEN legacy codec (from_cbor -> Self, panics on malformed input). Fail-closed decode is the taut v0.8.0 default; this opt-out is removed at v0.10.0 — regenerate without --legacy-codec to migrate. See dev-docs/RustFailClosed.md.
// GENERATED native Rust types + codec — do not edit.
#![allow(dead_code)]
use crate::cbor::Cbor;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NodeRecord {
    pub node_id: String,
    pub operator: String,
}
impl NodeRecord {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node_id.clone())),
            (2, Cbor::Text(self.operator.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            node_id: c.get(1).text(),
            operator: c.get(2).text(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WorkspaceEntry {
    pub workspace: String,
    pub name: String,
    pub eligible_hosts: Vec<String>,
}
impl WorkspaceEntry {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.workspace.clone())),
            (2, Cbor::Text(self.name.clone())),
            (3, Cbor::Array(self.eligible_hosts.iter().map(|x| Cbor::Text(x.clone())).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            workspace: c.get(1).text(),
            name: c.get(2).text(),
            eligible_hosts: c.get(3).array().iter().map(|x| x.text()).collect(),
        }
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
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.node.clone())),
            (2, Cbor::Text(self.share.clone())),
            (3, Cbor::Int(self.lease_expiry_ms)),
            (4, Cbor::Int(self.epoch)),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            node: c.get(1).text(),
            share: c.get(2).text(),
            lease_expiry_ms: c.get(3).int(),
            epoch: c.get(4).int(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CapabilityGrant {
    pub principal: String,
    pub share: String,
    pub verbs: Vec<String>,
}
impl CapabilityGrant {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.principal.clone())),
            (2, Cbor::Text(self.share.clone())),
            (3, Cbor::Array(self.verbs.iter().map(|x| Cbor::Text(x.clone())).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            principal: c.get(1).text(),
            share: c.get(2).text(),
            verbs: c.get(3).array().iter().map(|x| x.text()).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CapabilityRevocation {
    pub principal: String,
    pub share: String,
}
impl CapabilityRevocation {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.principal.clone())),
            (2, Cbor::Text(self.share.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            principal: c.get(1).text(),
            share: c.get(2).text(),
        }
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
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            app: c.get(1).text(),
            glade_id: c.get(2).text(),
            shape: c.get(3).text(),
            authority: c.get(4).text(),
            zone: c.get(5).text(),
            retention: c.get(6).text(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ServiceDefinition {
    pub app: String,
    pub name: String,
    pub glade_id: String,
}
impl ServiceDefinition {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Text(self.app.clone())),
            (2, Cbor::Text(self.name.clone())),
            (3, Cbor::Text(self.glade_id.clone())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            app: c.get(1).text(),
            name: c.get(2).text(),
            glade_id: c.get(3).text(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SystemSnapshot {
    pub records: Vec<Vec<u8>>,
    pub heads: Vec<Vec<u8>>,
}
impl SystemSnapshot {
    pub fn to_cbor(&self) -> Cbor {
        Cbor::Map(vec![
            (1, Cbor::Array(self.records.iter().map(|x| Cbor::Bytes(x.clone())).collect())),
            (2, Cbor::Array(self.heads.iter().map(|x| Cbor::Bytes(x.clone())).collect())),
        ])
    }
    pub fn from_cbor(c: &Cbor) -> Self {
        Self {
            records: c.get(1).array().iter().map(|x| x.bytes()).collect(),
            heads: c.get(2).array().iter().map(|x| x.bytes()).collect(),
        }
    }
}
