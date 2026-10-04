//! Frozen IC3 direct-root proof bodies. Values are untrusted until EvidencePort verifies.
use crate::*;
pub const CERTIFICATE_DOMAIN: &str = "glade/ic3/v1/origin-certificate\0";
pub const PERMIT_DOMAIN: &str = "glade/ic3/v1/offline-permit\0";
pub const OPERATION_DOMAIN: &str = "glade/ic3/v1/application-op\0";
pub const POLICY_DOMAIN: &str = "glade/ic3/v1/policy-cut\0";
pub const CHALLENGE_DOMAIN: &str = "glade/ic3/v1/local-session\0";
pub const RESPONSE_DOMAIN: &str = "glade/ic3/v1/local-session\0";
pub const ADMISSION_DOMAIN: &str = "glade/ic3/v1/admission\0";
pub const HELLO_DOMAIN: &str = "glade/ic3/v1/inventory\0";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateBody {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub resource: String,
    pub canonical_origin: String,
    pub epoch: String,
    pub display_name: String,
    pub writer_key: Digest,
    pub requester: String,
    pub creation_root: Digest,
    pub issuance: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceAction {
    Append,
    Read,
    ReplicaHold,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedGrant {
    pub requester: String,
    pub key: Digest,
    pub action: ResourceAction,
    pub resource: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyBody {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub creation_root: Digest,
    pub generation: u64,
    pub previous: Option<Digest>,
    pub window: Window,
    pub grants: Vec<ScopedGrant>,
    pub revoked: Vec<Digest>,
    pub admitting_nodes: Vec<Digest>,
    pub replica_holders: Vec<Digest>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermitBody {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub resource: String,
    pub certificate: Digest,
    pub requester: String,
    pub writer_key: Digest,
    pub canonical_origin: String,
    pub action: ResourceAction,
    pub admitting_nodes: Vec<Digest>,
    pub policy: Digest,
    pub policy_generation: u64,
    pub window: Window,
    pub first_seq: i64,
    pub last_seq: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationProofBody {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub resource: String,
    pub operation: Bytes,
    pub certificate: Digest,
    pub signed_permit: Digest,
    pub requester: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermitWrapper {
    pub signed_permit: SignedRecord,
    pub writer_proof: SignedRecord,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalChallengeBody {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub resource: String,
    pub node: Digest,
    pub requester: String,
    pub writer_key: Digest,
    pub operation: Digest,
    pub role: String,
    pub channel: Digest,
    pub session_nonce: Digest,
    pub challenge_nonce: Digest,
    pub window: Window,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalResponseBody {
    pub instance: Instance,
    pub node: Digest,
    pub requester: String,
    pub writer_key: Digest,
    pub signed_challenge: Digest,
    pub operation: Digest,
    pub channel: Digest,
    pub session_nonce: Digest,
    pub challenge_nonce: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginalAdmissionBody {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub resource: String,
    pub operation: Digest,
    pub certificate: Digest,
    pub signed_permit: Digest,
    pub policy: Digest,
    pub policy_generation: u64,
    pub time: TimeInterval,
    pub admitting_node: Digest,
    pub storage: StorageClass,
    pub stable_id: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginFloor {
    pub certificate: Digest,
    pub next_seq: i64,
    pub lamport: i64,
    pub prev: Option<Digest>,
    pub reserved_intents: Vec<Bytes>,
    pub frontier: Vec<RecordRef>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuanceFloor {
    pub time_floor_ms: u64,
    pub policy_generation: u64,
    pub policy_digest: Digest,
    pub plan_namespace: Digest,
    pub invocation_namespace: Digest,
    pub next_effect_id: u64,
    pub origins: std::collections::BTreeMap<String, OriginFloor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCustody {
    pub challenge: SignedRecord,
    pub response: Option<SignedRecord>,
    pub consumed: bool,
}

pub const DESCRIPTOR_DOMAIN: &str = "glade/ic3/v1/descriptor\0";
pub const INVENTORY_DOMAIN: &str = "glade/ic3/v1/inventory\0";
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeclarationShape {
    Crdt,
}
/// Unsigned genesis preimage. No own digest or signed descriptor digest inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationIntent {
    pub root_key: Digest,
    pub nonce: Digest,
    pub incarnation: String,
    pub resource: String,
    pub private_owner: String,
    pub zone: String,
    pub share: String,
    pub glade_id: String,
    pub key: Bytes,
    pub declaration_hash: Digest,
    pub declaration_version: String,
    pub shape: DeclarationShape,
    pub schema_identity: String,
    pub schema_hash: Digest,
    pub schema_version: String,
    pub parameters: Bytes,
    pub engine: String,
    pub payload_profile: String,
    pub admission_profile: String,
    pub authorization_profile: String,
    pub proof_profile: String,
    pub recovery_profile: String,
    pub retention_policy: String,
    pub rejoin_policy: String,
    pub corpus_identity: String,
    pub corpus_version: String,
    pub corpus_digest: Digest,
    pub mode: AdmissionMode,
    pub storage: StorageClass,
    pub bounds: Bounds,
    pub limits: ReplicaLimits,
    pub initial_chain_base: i64,
    pub migration_generation: u64,
}
/// Root signature binds exact retained intent bytes and its computed digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedDescriptorBody {
    pub intent: Bytes,
    pub creation_root: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryHeaderBody {
    pub instance: Instance,
    pub identity: IdentityBinding,
    pub source: Digest,
    pub target: Digest,
    pub channel: Digest,
    pub nonce: Digest,
    pub number: u64,
    pub pages: usize,
    pub entries: usize,
    pub total_bytes: usize,
    pub manifest: Digest,
}
