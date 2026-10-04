//! Complete trusted recovery and bounded pre-consumption ownership. No disk proof here.
use glade_crdt_admission_data::*;
mod ingress;
pub use ingress::{GuardedReceive, IngressAuthority, IngressPermit};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistResult {
    Committed { generation: u64, digest: Digest },
    Unknown,
    Refused(Fault),
}
#[derive(Clone, Debug, PartialEq)]
pub enum LoadResult {
    Validated(Box<ReplicaRecovery>),
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconstructResult {
    Cut(Box<ReplicaCut>),
    Unavailable,
    Refused(Fault),
}
/// Opening verifies exclusive root/floor custody and the complete identity first.
pub trait ReplicaRecoveryHost {
    type Session: ReplicaRecoverySession;
    fn open_replica(&mut self, request: ReplicaOpen) -> Result<Self::Session, Fault>;
}
/// Issued capabilities are session-owned. Unknown writes keep guard/history,
/// Never-started abandon requires issuer-owned gate proof, not caller claims.
pub trait ReplicaRecoverySession: StorageAttemptSession {
    fn load(&mut self) -> LoadResult;
    fn checkpoint(&mut self, request: Checkpoint) -> PersistResult;
    fn begin_ingress(&mut self, guard: ReceiveGuard) -> Result<IngressPermit, Fault>;
    fn receive_ingress<F: std::future::Future<Output = Result<Bytes, Fault>>>(
        &mut self,
        permit: IngressPermit,
        make: impl FnOnce() -> F,
    ) -> Result<GuardedReceive<F>, Fault>;
    fn settle_ingress(
        &mut self,
        permit: IngressPermit,
        observation: Observation,
        checkpoint: Checkpoint,
    ) -> PersistResult;
    fn abandon_ingress(&mut self, permit: IngressPermit) -> PersistResult;
    fn record_observation(
        &mut self,
        observation: Observation,
        checkpoint: Checkpoint,
    ) -> PersistResult;
    fn reconstruct_cut(&mut self, request: ReconstructionRequest) -> ReconstructResult;
    fn close(&mut self) -> CloseResult;
}
