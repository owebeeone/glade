//! Trusted injected proof and current-policy observation; peer identity is not authority.
use glade_crdt_admission_data::*;
#[derive(Clone, Debug, PartialEq)]
pub struct BoundQuery {
    pub query: Query,
    pub identity: IdentityBinding,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceObservation {
    pub instance: Instance,
    pub node: Digest,
    pub policy: Bytes,
    pub time: TimeInterval,
    pub available: bool,
    pub allowed: bool,
}
/// Required methods. Implementations authenticate the entire query/identity and
/// obtain policy/time from their own injected providers, never from Begin.current.
pub trait EvidencePort {
    fn verify(&mut self, query: BoundQuery) -> EvidenceReply;
    fn seal(&mut self, query: BoundQuery) -> EvidenceReply;
    fn observe(
        &mut self,
        instance: &Instance,
        identity: &IdentityBinding,
    ) -> Result<EvidenceObservation, Fault>;
}
