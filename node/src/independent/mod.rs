//! IC3A production composition seams. Deliberately refuse until IC3B qualification.
//! No claim-holder, disk, proof or peer path is activated by constructing these.
mod evidence;
mod recovery;
pub use evidence::RefusingEvidence;
pub use recovery::{RefusingRecovery, RefusingSession};
pub struct IndependentAdapters {
    pub evidence: RefusingEvidence,
    pub recovery: RefusingRecovery,
}
impl IndependentAdapters {
    pub(crate) fn refusing() -> Self {
        Self {
            evidence: RefusingEvidence,
            recovery: RefusingRecovery,
        }
    }
}

mod auth;
mod namespace;
mod text_profile;
pub use auth::{EvidenceConfig, GenuineEvidence, PolicyClock, SessionRequest, TrustedCut};
pub use namespace::{record_bytes, sign_record, validate_namespace};
