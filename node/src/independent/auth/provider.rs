use super::super::namespace::*;
use crate::signing::public_key;
use glade_crdt_evidence_api::*;
use glade_crdt_recovery_codec::*;
use std::collections::BTreeMap;

/// Injected trusted current source, independent of Query and Begin.current.
pub trait PolicyClock {
    fn current(&mut self) -> Result<TrustedCut, Fault>;
}
#[derive(Clone)]
pub struct TrustedCut {
    pub policy: SignedRecord,
    pub time: Option<TimeInterval>,
    pub time_floor_ms: u64,
}
pub struct EvidenceConfig {
    pub descriptor: Descriptor,
    pub identity: IdentityBinding,
    pub trusted_root: Digest,
    pub node_seed: Digest,
    pub limits: DecodeLimits,
    pub certificates: Vec<SignedRecord>,
    pub session_capacity: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRequest {
    pub requester: String,
    pub writer_key: Digest,
    pub operation: Digest,
    pub channel: Digest,
    pub session_nonce: Digest,
    pub challenge_nonce: Digest,
    pub window: Window,
}
pub struct GenuineEvidence<P: PolicyClock> {
    pub(crate) config: EvidenceConfig,
    pub(crate) source: P,
    pub(crate) intent: CreationIntent,
    pub(crate) certificates: BTreeMap<String, (SignedRecord, CertificateBody)>,
    pub(crate) sessions: BTreeMap<Digest, SessionCustody>,
    pub(crate) authorized: BTreeMap<Digest, LocalChallengeBody>,
    pub(crate) sealable: BTreeMap<Digest, (BoundQuery, Facts, LocalChallengeBody)>,
    pub(crate) observed_policy: Option<(u64, Digest)>,
    pub(crate) observed_time_floor: u64,
}
pub(crate) fn fits(time: &TimeInterval, window: &Window) -> bool {
    time.earliest_ms <= time.latest_ms
        && window.not_before_ms <= window.not_after_ms
        && time.earliest_ms >= window.not_before_ms
        && time.latest_ms <= window.not_after_ms
}
pub(crate) fn revoked(policy: &PolicyBody, ids: &[Digest]) -> bool {
    ids.iter().any(|id| policy.revoked.contains(id))
}
pub(crate) fn grant(policy: &PolicyBody, requester: &str, key: &Digest, resource: &str) -> bool {
    policy.grants.iter().any(|g| {
        g.requester == requester
            && g.key == *key
            && g.resource == resource
            && g.action == ResourceAction::Append
    })
}
impl<P: PolicyClock> EvidencePort for GenuineEvidence<P> {
    fn verify(&mut self, query: BoundQuery) -> EvidenceReply {
        let verdict = match self.checked(&query, &query.query.mode, false) {
            Ok(facts) => {
                if matches!(query.query.mode, Mode::Local { .. }) {
                    let operation = query.query.operation.canonical().digest();
                    if let Some(session) = self.authorized.remove(&operation) {
                        if facts.current_allowed && facts.payload_valid {
                            self.sealable
                                .insert(operation, (query.clone(), facts.clone(), session));
                        }
                    }
                }
                Verdict::Verified(Box::new(facts))
            }
            Err(Fault::Unavailable | Fault::Capacity) => Verdict::Unavailable,
            Err(_) => Verdict::Invalid,
        };
        EvidenceReply {
            query: query.query,
            verdict,
        }
    }
    fn seal(&mut self, bound: BoundQuery) -> EvidenceReply {
        let q = &bound.query;
        let result = (|| {
            if q.mode != Mode::AdmissionSeal {
                return Err(Fault::Integrity);
            }
            let mode = q.validated_under.as_ref().ok_or(Fault::Integrity)?;
            let Mode::Local {
                current_policy,
                time,
            } = mode
            else {
                return Err(Fault::Integrity);
            };
            let retained = self
                .sealable
                .get(&q.operation.canonical().digest())
                .ok_or(Fault::Ownership)?;
            let mut original = q.clone();
            original.id = retained.0.query.id;
            original.mode = retained.0.query.mode.clone();
            original.verified_facts = None;
            original.validated_under = None;
            if original != retained.0.query || bound.identity != retained.0.identity {
                return Err(Fault::Integrity);
            }
            let facts = self.checked(&bound, mode, true)?;
            if q.verified_facts.as_deref() != Some(&facts)
                || !facts.current_allowed
                || !facts.payload_valid
                || q.operation.inner().seq < facts.permit_first_seq
                || q.operation.inner().seq > facts.permit_last_seq
            {
                return Err(Fault::Integrity);
            }
            let wrapper: PermitWrapper =
                decode_profile(q.evidence.permit.value(), &self.config.limits)
                    .map_err(codec_fault)?;
            let policy: PolicyBody = signed_bytes(
                &q.evidence.policy,
                &self.config.trusted_root,
                &self.config.limits,
            )?;
            let body = OriginalAdmissionBody {
                instance: q.instance.clone(),
                identity: bound.identity.clone(),
                resource: q.descriptor.resource.clone(),
                operation: q.operation.canonical().digest(),
                certificate: q.evidence.certificate.digest(),
                signed_permit: record_bytes(&wrapper.signed_permit, &self.config.limits)?.digest(),
                policy: *current_policy,
                policy_generation: policy.generation,
                time: time.clone(),
                admitting_node: public_key(&self.config.node_seed),
                storage: self.intent.storage.clone(),
                stable_id: q.operation.canonical().digest(),
            };
            record_bytes(
                &sign_record(&self.config.node_seed, &body, &self.config.limits)?,
                &self.config.limits,
            )
        })();
        let verdict = match result {
            Ok(bytes) => Verdict::Sealed(bytes),
            Err(Fault::Unavailable | Fault::Capacity) => Verdict::Unavailable,
            Err(_) => Verdict::Invalid,
        };
        EvidenceReply {
            query: bound.query,
            verdict,
        }
    }
    fn observe(
        &mut self,
        instance: &Instance,
        identity: &IdentityBinding,
    ) -> Result<EvidenceObservation, Fault> {
        if *instance != self.config.descriptor.instance || *identity != self.config.identity {
            return Err(Fault::Integrity);
        }
        let (cut, policy) = self.current()?;
        let time = cut.time.ok_or(Fault::Unavailable)?;
        let node = public_key(&self.config.node_seed);
        let allowed = policy.admitting_nodes.contains(&node)
            && !revoked(&policy, &[node])
            && fits(&time, &policy.window);
        Ok(EvidenceObservation {
            instance: instance.clone(),
            node,
            policy: record_bytes(&cut.policy, &self.config.limits)?,
            time,
            available: true,
            allowed,
        })
    }
}
