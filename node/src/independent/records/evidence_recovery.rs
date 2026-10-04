//! Recovery authority is scoped to an exact selected pre-crash query, never a new link.
use super::*;
use glade_crdt_evidence_api::{BoundQuery, EvidencePort};
pub(super) fn queries<P: PolicyClock>(session: &mut DiskSession<P>) -> Result<(), Fault> {
    let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
    genesis::validate(&mut session.evidence, &session.request, pair)?;
    let retained: Vec<_> = pair.image.state.queries.values().cloned().collect();
    for pending in retained {
        let query = pending.query.clone();
        let operation = query.operation.canonical().digest();
        let bound = BoundQuery {
            query: query.clone(),
            identity: session.request.identity.clone(),
        };
        let reply = if let Some(original) = &pending.verified_query {
            let facts = bindings::local_facts(&mut session.evidence, &session.request, original)?;
            if pending.verified.as_ref() != Some(&facts) {
                return Err(Fault::Integrity);
            }
            let challenge = challenge(session, original, &facts)?;
            session.evidence.sealable.insert(
                operation,
                (
                    BoundQuery {
                        query: original.clone(),
                        identity: session.request.identity.clone(),
                    },
                    facts,
                    challenge,
                ),
            );
            session.evidence.seal(bound)
        } else {
            if matches!(query.mode, Mode::Local { .. }) {
                let facts = bindings::local_facts(&mut session.evidence, &session.request, &query)?;
                let challenge = challenge(session, &query, &facts)?;
                session.evidence.authorized.insert(operation, challenge);
            }
            session.evidence.verify(bound)
        };
        // The exact selected query owns this one invocation. Unavailable must
        // not leave generic authority usable by a different connection/request.
        session.evidence.authorized.remove(&operation);
        runtime::retain_observed(session)?;
        let effects = runtime::event(session, Event::EvidenceResolved(Box::new(reply)))?;
        runtime::drain(session, effects)?;
        session.evidence.sealable.remove(&operation);
    }
    Ok(())
}
fn challenge<P: PolicyClock>(
    session: &DiskSession<P>,
    query: &Query,
    facts: &Facts,
) -> Result<LocalChallengeBody, Fault> {
    let Mode::Local { time, .. } = &query.mode else {
        return Err(Fault::Integrity);
    };
    session
        .evidence
        .sessions
        .values()
        .find_map(|custody| {
            if !custody.consumed {
                return None;
            }
            let body: LocalChallengeBody =
                decode_signed_body(&custody.challenge, &session.evidence.config.limits).ok()?;
            if body.operation == query.operation.canonical().digest()
                && body.requester == facts.certificate.requester
                && body.writer_key == facts.certificate.writer_key
                && super::super::auth::fits(time, &body.window)
            {
                Some(body)
            } else {
                None
            }
        })
        .ok_or(Fault::Integrity)
}
