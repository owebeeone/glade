use super::super::namespace::*;
use super::super::text_profile::valid_text_payload;
use crate::signing::public_key;
use glade_crdt_evidence_api::*;
use glade_crdt_recovery_codec::*;
use std::collections::BTreeMap;

use super::*;

impl<P: PolicyClock> GenuineEvidence<P> {
    pub fn new(config: EvidenceConfig, source: P) -> Result<Self, Fault> {
        let intent = validate_namespace(
            &config.descriptor,
            &config.identity,
            &config.trusted_root,
            &config.limits,
        )?;
        if config.session_capacity == 0
            || config.session_capacity > intent.limits.guard_history
            || config.certificates.len() > config.descriptor.bounds.max_origins
        {
            return Err(Fault::Capacity);
        }
        let mut out = Self {
            config,
            source,
            intent,
            certificates: BTreeMap::new(),
            sessions: BTreeMap::new(),
            authorized: BTreeMap::new(),
            sealable: BTreeMap::new(),
            observed_policy: None,
            observed_time_floor: 0,
        };
        let certificates = std::mem::take(&mut out.config.certificates);
        for record in certificates {
            out.register_certificate(record)?;
        }
        Ok(out)
    }
    pub fn source_mut(&mut self) -> &mut P {
        &mut self.source
    }
    pub fn register_certificate(&mut self, record: SignedRecord) -> Result<(), Fault> {
        let body: CertificateBody =
            signed_body(&record, &self.config.trusted_root, &self.config.limits)?;
        if body.instance != self.config.descriptor.instance
            || body.identity != self.config.identity
            || body.resource != self.config.descriptor.resource
            || body.creation_root != self.config.descriptor.creation_root
            || body.canonical_origin.is_empty()
            || body.epoch.is_empty()
            || body.requester.is_empty()
            || body.canonical_origin.len() > self.config.limits.name_bytes
        {
            return Err(Fault::Integrity);
        }
        if let Some((old, _)) = self.certificates.get(&body.canonical_origin) {
            return if *old == record {
                Ok(())
            } else {
                Err(Fault::Integrity)
            };
        }
        if self.certificates.len() >= self.config.descriptor.bounds.max_origins {
            return Err(Fault::Capacity);
        }
        if self.certificates.values().any(|(_, c)| {
            c.issuance == body.issuance
                || (c.writer_key == body.writer_key && c.epoch == body.epoch)
        }) {
            return Err(Fault::Integrity);
        }
        self.certificates
            .insert(body.canonical_origin.clone(), (record, body));
        Ok(())
    }
    pub(crate) fn policy(&self, record: &SignedRecord) -> Result<PolicyBody, Fault> {
        let body: PolicyBody = signed_body(record, &self.config.trusted_root, &self.config.limits)?;
        if body.instance != self.config.descriptor.instance
            || body.identity != self.config.identity
            || body.creation_root != self.config.descriptor.creation_root
            || body.window.not_before_ms > body.window.not_after_ms
            || (body.generation == 0) != body.previous.is_none()
        {
            return Err(Fault::Integrity);
        }
        Ok(body)
    }
    pub(crate) fn current(&mut self) -> Result<(TrustedCut, PolicyBody), Fault> {
        let cut = self.source.current()?;
        let policy = self.policy(&cut.policy)?;
        let time = cut.time.as_ref().ok_or(Fault::Unavailable)?;
        if time.earliest_ms > time.latest_ms
            || time.earliest_ms < cut.time_floor_ms.max(self.observed_time_floor)
        {
            return Err(Fault::Unavailable);
        }
        let digest = record_bytes(&cut.policy, &self.config.limits)?.digest();
        if let Some((generation, prior)) = self.observed_policy {
            if policy.generation < generation {
                return Err(Fault::Unavailable);
            }
            if (policy.generation == generation && digest != prior)
                || (policy.generation > generation
                    && (generation.checked_add(1) != Some(policy.generation)
                        || policy.previous != Some(prior)))
            {
                return Err(Fault::Integrity);
            }
        }
        self.observed_policy = Some((policy.generation, digest));
        self.observed_time_floor = time.earliest_ms;
        Ok((cut, policy))
    }
    pub fn issue_challenge(&mut self, request: SessionRequest) -> Result<SignedRecord, Fault> {
        let (cut, policy) = self.current()?;
        let time = cut.time.as_ref().ok_or(Fault::Unavailable)?;
        if self.sessions.len() >= self.config.session_capacity {
            return Err(Fault::Capacity);
        }
        if !fits(time, &request.window)
            || !fits(time, &policy.window)
            || request.session_nonce == [0; 32]
            || request.challenge_nonce == [0; 32]
            || self.sessions.values().any(|s| {
                decode_signed_body::<LocalChallengeBody>(&s.challenge, &self.config.limits)
                    .is_ok_and(|c| c.challenge_nonce == request.challenge_nonce)
            })
        {
            return Err(Fault::Integrity);
        }
        if !self.certificates.values().any(|(record, c)| {
            c.requester == request.requester
                && c.writer_key == request.writer_key
                && !revoked(
                    &policy,
                    &[
                        record_bytes(record, &self.config.limits)
                            .map(|b| b.digest())
                            .unwrap_or([0; 32]),
                        c.writer_key,
                    ],
                )
                && grant(&policy, &c.requester, &c.writer_key, &c.resource)
        }) {
            return Err(Fault::Ownership);
        }
        let body = LocalChallengeBody {
            instance: self.config.descriptor.instance.clone(),
            identity: self.config.identity.clone(),
            resource: self.config.descriptor.resource.clone(),
            node: public_key(&self.config.node_seed),
            requester: request.requester,
            writer_key: request.writer_key,
            operation: request.operation,
            role: "local-append".into(),
            channel: request.channel,
            session_nonce: request.session_nonce,
            challenge_nonce: request.challenge_nonce,
            window: request.window,
        };
        let challenge = sign_record(&self.config.node_seed, &body, &self.config.limits)?;
        let hash = record_bytes(&challenge, &self.config.limits)?.digest();
        self.sessions.insert(
            hash,
            SessionCustody {
                challenge: challenge.clone(),
                response: None,
                consumed: false,
            },
        );
        Ok(challenge)
    }
    pub fn authenticate(
        &mut self,
        response: SignedRecord,
        channel: Digest,
        session: Digest,
    ) -> Result<(), Fault> {
        let body: LocalResponseBody =
            signed_body(&response, &response.signer, &self.config.limits)?;
        let custody = self
            .sessions
            .get(&body.signed_challenge)
            .ok_or(Fault::Ownership)?;
        let challenge: LocalChallengeBody =
            decode_signed_body(&custody.challenge, &self.config.limits).map_err(codec_fault)?;
        if custody.consumed
            || body.instance != challenge.instance
            || body.node != challenge.node
            || body.requester != challenge.requester
            || body.writer_key != challenge.writer_key
            || response.signer != challenge.writer_key
            || body.operation != challenge.operation
            || body.channel != challenge.channel
            || body.session_nonce != challenge.session_nonce
            || body.challenge_nonce != challenge.challenge_nonce
            || body.channel != channel
            || body.session_nonce != session
        {
            return Err(Fault::Ownership);
        }
        let (cut, policy) = self.current()?;
        if !fits(
            cut.time.as_ref().ok_or(Fault::Unavailable)?,
            &challenge.window,
        ) || !grant(
            &policy,
            &challenge.requester,
            &challenge.writer_key,
            &challenge.resource,
        ) || revoked(&policy, &[challenge.writer_key])
        {
            return Err(Fault::Ownership);
        }
        let entry = self
            .sessions
            .get_mut(&body.signed_challenge)
            .ok_or(Fault::Ownership)?;
        entry.response = Some(response);
        entry.consumed = true;
        self.authorized.insert(challenge.operation, challenge);
        Ok(())
    }
    pub fn session_custody(&self) -> Vec<SessionCustody> {
        self.sessions.values().cloned().collect()
    }
    pub(crate) fn checked(
        &mut self,
        bound: &BoundQuery,
        mode: &Mode,
        for_seal: bool,
    ) -> Result<Facts, Fault> {
        let q = &bound.query;
        let descriptor = &self.config.descriptor;
        let identity = &self.config.identity;
        if q.instance != descriptor.instance
            || q.descriptor != *descriptor
            || bound.identity != *identity
        {
            return Err(Fault::Integrity);
        }
        let total = [
            Some(q.evidence.certificate.value().len()),
            Some(q.evidence.permit.value().len()),
            Some(q.evidence.policy.value().len()),
            q.evidence.admission.as_ref().map(|v| v.value().len()),
        ]
        .into_iter()
        .flatten()
        .try_fold(0usize, |n, v| n.checked_add(v))
        .ok_or(Fault::Capacity)?;
        let op = q.operation.inner();
        if q.operation.canonical().value().len() > descriptor.bounds.max_operation_bytes
            || total > descriptor.bounds.evidence_bytes
            || op.refs.len() > descriptor.bounds.max_refs
        {
            return Err(Fault::Capacity);
        }
        const SAFE: i64 = 9_007_199_254_740_991;
        if op.seq < 0
            || op.seq >= SAFE
            || op.lamport < 0
            || op.lamport > SAFE
            || (op.seq == 0 && op.prev.is_some())
            || (op.seq > 0 && op.prev.as_ref().is_none_or(|h| h.len() != 32))
            || op.refs.iter().any(|r| {
                r.origin.is_empty()
                    || r.seq < 0
                    || r.seq >= SAFE
                    || r.hash.as_ref().is_some_and(|h| h.len() != 32)
            })
            || !op.refs.windows(2).all(|w| w[0].origin < w[1].origin)
        {
            return Err(Fault::Integrity);
        }
        let certificate: CertificateBody = signed_bytes(
            &q.evidence.certificate,
            &self.config.trusted_root,
            &self.config.limits,
        )?;
        let Some((registered, known)) = self.certificates.get(&certificate.canonical_origin) else {
            return Err(Fault::Unavailable);
        };
        if certificate != *known
            || record_bytes(registered, &self.config.limits)? != q.evidence.certificate
        {
            return Err(Fault::Integrity);
        }
        let policy_record: SignedRecord =
            decode_profile(q.evidence.policy.value(), &self.config.limits).map_err(codec_fault)?;
        let policy = self.policy(&policy_record)?;
        let wrapper: PermitWrapper =
            decode_profile(q.evidence.permit.value(), &self.config.limits).map_err(codec_fault)?;
        let permit: PermitBody = signed_body(
            &wrapper.signed_permit,
            &self.config.trusted_root,
            &self.config.limits,
        )?;
        let permit_digest = record_bytes(&wrapper.signed_permit, &self.config.limits)?.digest();
        let proof: OperationProofBody = signed_body(
            &wrapper.writer_proof,
            &certificate.writer_key,
            &self.config.limits,
        )?;
        let operation = q.operation.inner();
        if operation.origin != certificate.canonical_origin
            || operation.share != descriptor.share
            || operation.glade_id != descriptor.glade_id
            || operation.key != descriptor.key
            || operation.shape != glade_wire::generated::Shape::Crdt
            || permit.instance != q.instance
            || permit.identity != *identity
            || permit.resource != descriptor.resource
            || permit.certificate != q.evidence.certificate.digest()
            || permit.requester != certificate.requester
            || permit.writer_key != certificate.writer_key
            || permit.canonical_origin != certificate.canonical_origin
            || permit.action != ResourceAction::Append
            || permit.policy != q.evidence.policy.digest()
            || permit.policy_generation != policy.generation
            || permit.first_seq < 0
            || permit.first_seq > permit.last_seq
            || permit.window.not_before_ms > permit.window.not_after_ms
            || proof.instance != q.instance
            || proof.identity != *identity
            || proof.resource != descriptor.resource
            || proof.operation != *q.operation.canonical()
            || proof.certificate != q.evidence.certificate.digest()
            || proof.signed_permit != permit_digest
            || proof.requester != certificate.requester
        {
            return Err(Fault::Integrity);
        }
        let mut facts = Facts {
            certificate: OriginCertificate {
                instance: certificate.instance.clone(),
                canonical_origin: certificate.canonical_origin.clone(),
                epoch: certificate.epoch.clone(),
                display_name: certificate.display_name.clone(),
                writer_key: certificate.writer_key,
                requester: certificate.requester.clone(),
                canonical: q.evidence.certificate.clone(),
            },
            descriptor_valid: true,
            payload_valid: valid_text_payload(&operation.payload),
            signatures_valid: true,
            issuance_unique: true,
            permit_window: permit.window.clone(),
            permit_first_seq: permit.first_seq,
            permit_last_seq: permit.last_seq,
            current_allowed: false,
            historical_admitter_authorized: false,
            historical_policy_allowed: false,
            historical_time: None,
        };
        let base_allowed = grant(
            &policy,
            &certificate.requester,
            &certificate.writer_key,
            &certificate.resource,
        ) && !revoked(
            &policy,
            &[
                certificate.writer_key,
                q.evidence.certificate.digest(),
                permit_digest,
            ],
        );
        match mode {
            Mode::Historical => {
                if let Some(bytes) = &q.evidence.admission {
                    let record: SignedRecord =
                        decode_profile(bytes.value(), &self.config.limits).map_err(codec_fault)?;
                    let admission: OriginalAdmissionBody =
                        signed_body(&record, &record.signer, &self.config.limits)?;
                    if admission.instance != q.instance
                        || admission.identity != *identity
                        || admission.resource != descriptor.resource
                        || admission.operation != q.operation.canonical().digest()
                        || admission.certificate != q.evidence.certificate.digest()
                        || admission.signed_permit != permit_digest
                        || admission.policy != q.evidence.policy.digest()
                        || admission.policy_generation != policy.generation
                        || admission.admitting_node != record.signer
                        || admission.storage != self.intent.storage
                        || admission.stable_id != q.operation.canonical().digest()
                    {
                        return Err(Fault::Integrity);
                    }
                    facts.historical_admitter_authorized =
                        permit.admitting_nodes.contains(&admission.admitting_node)
                            && policy.admitting_nodes.contains(&admission.admitting_node)
                            && !revoked(&policy, &[admission.admitting_node]);
                    facts.historical_policy_allowed = base_allowed
                        && fits(&admission.time, &permit.window)
                        && fits(&admission.time, &policy.window)
                        && operation.seq >= permit.first_seq
                        && operation.seq <= permit.last_seq;
                    facts.historical_time = Some(admission.time);
                }
            }
            Mode::Local {
                current_policy,
                time,
            } => {
                let (cut, current) = self.current()?;
                let actual = cut.time.as_ref().ok_or(Fault::Unavailable)?;
                if *current_policy != q.evidence.policy.digest()
                    || record_bytes(&cut.policy, &self.config.limits)?.digest() != *current_policy
                    || actual != time
                {
                    return Err(Fault::Unavailable);
                }
                let operation = q.operation.canonical().digest();
                let authorized = if for_seal {
                    self.sealable.get(&operation).map(|(_, _, session)| session)
                } else {
                    self.authorized.get(&operation)
                }
                .ok_or(Fault::Ownership)?;
                if authorized.requester != certificate.requester
                    || authorized.writer_key != certificate.writer_key
                {
                    return Err(Fault::Ownership);
                }
                let node = public_key(&self.config.node_seed);
                facts.current_allowed = base_allowed
                    && grant(
                        &current,
                        &certificate.requester,
                        &certificate.writer_key,
                        &certificate.resource,
                    )
                    && fits(actual, &authorized.window)
                    && fits(actual, &permit.window)
                    && fits(actual, &current.window)
                    && permit.admitting_nodes.contains(&node)
                    && current.admitting_nodes.contains(&node)
                    && !revoked(
                        &current,
                        &[
                            certificate.writer_key,
                            q.evidence.certificate.digest(),
                            permit_digest,
                            node,
                        ],
                    );
            }
            Mode::AdmissionSeal => return Err(Fault::Integrity),
        }
        Ok(facts)
    }
}
