use glade_crdt_admission_data::*;
use glade_crdt_evidence_api::*;
pub struct RefusingEvidence;
impl EvidencePort for RefusingEvidence {
    fn verify(&mut self, query: BoundQuery) -> EvidenceReply {
        EvidenceReply {
            query: query.query,
            verdict: Verdict::Unavailable,
        }
    }
    fn seal(&mut self, query: BoundQuery) -> EvidenceReply {
        EvidenceReply {
            query: query.query,
            verdict: Verdict::Unavailable,
        }
    }
    fn observe(
        &mut self,
        _instance: &Instance,
        _identity: &IdentityBinding,
    ) -> Result<EvidenceObservation, Fault> {
        Err(Fault::Unavailable)
    }
}
