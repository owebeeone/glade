//! Durable signed session reconstruction. It never recreates fresh connection authority.
use super::super::namespace::{record_bytes, signed_body};
use super::provider::*;
use crate::signing::public_key;
use glade_crdt_recovery_codec::*;
use std::collections::{BTreeMap, BTreeSet};
impl<P: PolicyClock> GenuineEvidence<P> {
    pub(crate) fn validate_sessions(
        &self,
        entries: &[SessionCustody],
    ) -> Result<BTreeMap<Digest, SessionCustody>, Fault> {
        if entries.len() > self.config.session_capacity {
            return Err(Fault::Capacity);
        }
        let mut result = BTreeMap::new();
        let mut nonces = BTreeSet::new();
        let mut prior = None;
        for custody in entries {
            let challenge: LocalChallengeBody = signed_body(
                &custody.challenge,
                &public_key(&self.config.node_seed),
                &self.config.limits,
            )?;
            let hash = record_bytes(&custody.challenge, &self.config.limits)?.digest();
            if prior.is_some_and(|previous| previous >= hash)
                || !nonces.insert(challenge.challenge_nonce)
                || challenge.instance != self.config.descriptor.instance
                || challenge.identity != self.config.identity
                || challenge.resource != self.config.descriptor.resource
                || challenge.node != public_key(&self.config.node_seed)
                || challenge.role != "local-append"
                || challenge.session_nonce == [0; 32]
                || challenge.challenge_nonce == [0; 32]
                || challenge.window.not_before_ms > challenge.window.not_after_ms
                || !self.certificates.values().any(|(_, certificate)| {
                    certificate.requester == challenge.requester
                        && certificate.writer_key == challenge.writer_key
                        && certificate.resource == challenge.resource
                })
                || custody.consumed != custody.response.is_some()
            {
                return Err(Fault::Integrity);
            }
            if let Some(record) = &custody.response {
                let response: LocalResponseBody =
                    signed_body(record, &challenge.writer_key, &self.config.limits)?;
                if response.instance != challenge.instance
                    || response.node != challenge.node
                    || response.requester != challenge.requester
                    || response.writer_key != challenge.writer_key
                    || response.signed_challenge != hash
                    || response.operation != challenge.operation
                    || response.channel != challenge.channel
                    || response.session_nonce != challenge.session_nonce
                    || response.challenge_nonce != challenge.challenge_nonce
                {
                    return Err(Fault::Integrity);
                }
            }
            result.insert(hash, custody.clone());
            prior = Some(hash);
        }
        Ok(result)
    }
    pub(crate) fn restore_sessions(&mut self, entries: &[SessionCustody]) -> Result<(), Fault> {
        self.sessions = self.validate_sessions(entries)?;
        // A durable response is proof of old consumption, not a new link's
        // possession. Exact pending kernel continuations are restored separately.
        self.authorized.clear();
        self.sealable.clear();
        Ok(())
    }
}
