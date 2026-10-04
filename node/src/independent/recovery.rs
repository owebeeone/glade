use glade_crdt_admission_data::*;
use glade_crdt_recovery_api::*;
pub struct RefusingRecovery;
pub struct RefusingSession;
impl ReplicaRecoveryHost for RefusingRecovery {
    type Session = RefusingSession;
    fn open_replica(&mut self, _request: ReplicaOpen) -> Result<Self::Session, Fault> {
        Err(Fault::Unavailable)
    }
}
impl ReplicaRecoverySession for RefusingSession {
    fn load(&mut self) -> LoadResult {
        LoadResult::Unavailable
    }
    fn checkpoint(&mut self, _request: Checkpoint) -> PersistResult {
        PersistResult::Refused(Fault::Unavailable)
    }
    fn begin_ingress(&mut self, _guard: ReceiveGuard) -> Result<IngressPermit, Fault> {
        Err(Fault::Unavailable)
    }
    fn receive_ingress<F: std::future::Future<Output = Result<Bytes, Fault>>>(
        &mut self,
        _permit: IngressPermit,
        _make: impl FnOnce() -> F,
    ) -> Result<GuardedReceive<F>, Fault> {
        Err(Fault::Unavailable)
    }
    fn settle_ingress(
        &mut self,
        _permit: IngressPermit,
        _observation: Observation,
        _checkpoint: Checkpoint,
    ) -> PersistResult {
        PersistResult::Refused(Fault::Unavailable)
    }
    fn abandon_ingress(&mut self, _permit: IngressPermit) -> PersistResult {
        PersistResult::Refused(Fault::Unavailable)
    }
    fn record_observation(
        &mut self,
        _observation: Observation,
        _checkpoint: Checkpoint,
    ) -> PersistResult {
        PersistResult::Refused(Fault::Unavailable)
    }
    fn reconstruct_cut(&mut self, _request: ReconstructionRequest) -> ReconstructResult {
        ReconstructResult::Unavailable
    }
    fn close(&mut self) -> CloseResult {
        CloseResult::Closed
    }
}

impl StorageAttemptSession for RefusingSession {
    fn recover(&mut self) -> RecoveryResult {
        RecoveryResult::Unavailable
    }
    fn prepare(&mut self, request: PrepareRequest) -> PrepareReply {
        PrepareReply {
            request,
            result: PrepareResult::Refused(Fault::Unavailable),
        }
    }
    fn recover_plan(&mut self, request: PrepareRequest) -> PrepareReply {
        PrepareReply {
            request,
            result: PrepareResult::Refused(Fault::Unavailable),
        }
    }
    fn begin(&mut self, request: BeginRequest) -> ResolveReply {
        ResolveReply {
            request: request.request,
            result: ResolveResult::Refused(Fault::Unavailable),
        }
    }
    fn inspect(&mut self, request: ResolveRequest) -> ResolveReply {
        ResolveReply {
            request,
            result: ResolveResult::Refused(Fault::Unavailable),
        }
    }
    fn request_fence(&mut self, request: ResolveRequest) -> ResolveReply {
        ResolveReply {
            request,
            result: ResolveResult::Refused(Fault::Unavailable),
        }
    }
    fn close(&mut self) -> CloseResult {
        CloseResult::Closed
    }
}
