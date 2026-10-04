//! Semantic admission: trusted scoped facts, finite continuation retention,
//! separate local and historical predicates, and immutable staging before I/O.
use crate::staging::{acceptance, stage};
use crate::*;
use crate::{lifecycle, projection};
use glade_wire::generated::Shape;
use std::collections::BTreeSet;
const SAFE: i64 = 9_007_199_254_740_991;

pub(crate) fn candidate_size(c: &Candidate) -> Option<usize> {
    let e = &c.evidence;
    c.operation
        .canonical()
        .value()
        .len()
        .checked_add(e.certificate.value().len())?
        .checked_add(e.permit.value().len())?
        .checked_add(e.policy.value().len())?
        .checked_add(e.admission.as_ref().map_or(0, |a| a.value().len()))
}
fn supported(d: &Descriptor) -> bool {
    d.mode == AdmissionMode::Independent
        && d.engine == "crdt.oracle/v1"
        && d.payload_profile == "text_crdt.profile/v1"
        && d.admission_profile == "glade.crdt.independent-admission/v1"
        && d.authorization_profile == "bounded-offline-permit/v1"
}
fn structure(d: &Descriptor, c: &Candidate) -> bool {
    let o = c.operation.inner();
    c.instance == d.instance
        && d.canonical.digest() == d.instance.descriptor
        && o.share == d.share
        && o.glade_id == d.glade_id
        && o.key == d.key
        && o.shape == Shape::Crdt
        && !o.origin.is_empty()
        && o.seq >= 0
        && o.seq < SAFE
        && o.lamport >= 0
        && o.lamport <= SAFE
        && c.operation.canonical().value().len() <= d.bounds.max_operation_bytes
        && ((o.seq == 0 && o.prev.is_none())
            || (o.seq > 0 && o.prev.as_ref().is_some_and(|p| p.len() == 32)))
        && o.refs.len() <= d.bounds.max_refs
        && o.refs.iter().all(|h| {
            !h.origin.is_empty()
                && h.seq >= 0
                && h.seq < SAFE
                && h.hash.as_ref().is_none_or(|h| h.len() == 32)
        })
        && o.refs.windows(2).all(|w| w[0].origin < w[1].origin)
}
pub(crate) fn offer(s: &mut State, c: Candidate, local: bool, effects: &mut Vec<Effect>) {
    let Some(v) = s.instances.get(&c.instance) else {
        effects.push(Effect::Report(Outcome::Invalid));
        return;
    };
    if let Some(a) = v
        .accepted
        .iter()
        .find(|a| a.candidate.operation == c.operation)
    {
        effects.push(Effect::Report(Outcome::ExactRetry(a.receipt.clone())));
        return;
    }
    if v.storage_integrity_failed {
        effects.push(Effect::Report(Outcome::IntegrityConflict));
        return;
    }
    if !supported(&v.descriptor) {
        effects.push(Effect::Report(Outcome::Unsupported));
        return;
    }
    if !structure(&v.descriptor, &c) {
        effects.push(Effect::Report(Outcome::Invalid));
        return;
    }
    if s.commits
        .values()
        .chain(s.unknown_commits.values())
        .any(|p| p.instance == c.instance)
        || s.preparing.keys().any(|p| p.instance == c.instance)
    {
        lifecycle::resume(s, &c.instance, effects);
        return;
    }
    let Some(z) = s.storage_limits.as_ref() else {
        effects.push(Effect::Report(Outcome::Unavailable));
        return;
    };
    let bytes = candidate_size(&c);
    let pending: Vec<_> = s
        .queries
        .values()
        .filter(|q| q.candidate.instance == c.instance)
        .collect();
    let retained = pending.iter().try_fold(v.used.pending, |n, q| {
        n.checked_add(candidate_size(&q.candidate)?)
    });
    let origins: BTreeSet<_> = v
        .canonical_origins
        .keys()
        .chain(v.candidates.iter().map(|c| &c.operation.inner().origin))
        .chain(
            pending
                .iter()
                .map(|q| &q.candidate.operation.inner().origin),
        )
        .collect();
    let slots = projection::pending_slots(s, &c.instance, &[]);
    if c.operation.inner().origin.len() > z.max_name_bytes
        || c.operation
            .inner()
            .refs
            .iter()
            .any(|h| h.origin.len() > z.max_name_bytes)
        || bytes.is_none_or(|n| n > z.max_batch_bytes || n > v.descriptor.bounds.pending_bytes)
        || retained
            .and_then(|n| n.checked_add(bytes?))
            .is_none_or(|n| n > v.descriptor.bounds.pending_bytes)
        || pending
            .len()
            .checked_add(v.candidates.len())
            .is_none_or(|n| n >= v.descriptor.bounds.pending_items)
        || (!origins.contains(&c.operation.inner().origin)
            && origins.len() >= v.descriptor.bounds.max_origins)
        || (local && v.used.edit >= v.descriptor.bounds.edit_bytes)
        || bytes.is_none_or(|n| !projection::pending_fits(s, &c.instance, &slots, n))
    {
        if !local {
            s.instances
                .get_mut(&c.instance)
                .expect("selected instance")
                .recovery_incomplete = true;
        }
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    }
    if pending.iter().any(|q| q.candidate == c) {
        effects.push(Effect::Report(Outcome::PolicyPending));
        return;
    }
    let descriptor = v.descriptor.clone();
    let mode = if local {
        Mode::Local {
            current_policy: v.policy.digest(),
            time: v.policy_time.clone(),
        }
    } else {
        Mode::Historical
    };
    let Some(id) = lifecycle::allocate(s) else {
        if !local {
            s.instances
                .get_mut(&c.instance)
                .expect("selected instance")
                .recovery_incomplete = true;
        }
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    };
    let q = Query {
        id,
        instance: c.instance.clone(),
        descriptor,
        operation: c.operation.clone(),
        evidence: c.evidence.clone(),
        mode,
        verified_facts: None,
        validated_under: None,
    };
    s.queries.insert(
        id,
        PendingVerification {
            query: q.clone(),
            candidate: c,
            verified: None,
            verified_query: None,
        },
    );
    effects.push(Effect::Verify(q));
}
fn attributable(q: &Query, f: &Facts) -> bool {
    f.descriptor_valid
        && f.signatures_valid
        && f.issuance_unique
        && f.certificate.instance == q.instance
        && f.certificate.canonical_origin == q.operation.inner().origin
        && f.certificate.canonical == q.evidence.certificate
}
fn interval(w: &Window, t: &TimeInterval) -> bool {
    w.not_before_ms < w.not_after_ms
        && t.earliest_ms <= t.latest_ms
        && t.earliest_ms >= w.not_before_ms
        && t.latest_ms <= w.not_after_ms
}
fn local_current(s: &State, q: &Query) -> bool {
    if let Mode::Local {
        current_policy,
        time,
    } = &q.mode
    {
        lifecycle::current_matches(
            s,
            &q.instance,
            &LocalPrecondition {
                policy: *current_policy,
                time: time.clone(),
            },
        )
    } else {
        true
    }
}
pub(crate) fn evidence(s: &mut State, r: EvidenceReply, effects: &mut Vec<Effect>) {
    let Some(p) = s
        .queries
        .get(&r.query.id)
        .cloned()
        .filter(|p| p.query == r.query)
    else {
        effects.push(Effect::Report(Outcome::CallbackMismatch));
        return;
    };
    if s.instances[&p.candidate.instance].storage_integrity_failed {
        effects.push(Effect::Report(Outcome::IntegrityConflict));
        return;
    }
    if matches!(r.verdict, Verdict::Unavailable) {
        effects.push(Effect::Report(Outcome::Unavailable));
        return;
    }
    s.queries.remove(&r.query.id);
    if !local_current(s, p.verified_query.as_ref().unwrap_or(&p.query)) {
        effects.push(Effect::Report(Outcome::PolicyPending));
        return;
    }
    match r.verdict {
        Verdict::Verified(f) if p.query.mode != Mode::AdmissionSeal => {
            let v = &s.instances[&p.candidate.instance];
            if f.certificate.instance != p.query.instance {
                effects.push(Effect::Report(Outcome::Denied));
                return;
            }
            if !attributable(&p.query, &f)
                || v.canonical_origins
                    .get(&f.certificate.canonical_origin)
                    .is_some_and(|old| old != &f.certificate)
                || s.storage_limits.as_ref().is_none_or(|z| {
                    [
                        &f.certificate.canonical_origin,
                        &f.certificate.epoch,
                        &f.certificate.display_name,
                        &f.certificate.requester,
                    ]
                    .iter()
                    .any(|n| n.len() > z.max_name_bytes)
                })
            {
                effects.push(Effect::Report(Outcome::Invalid));
                return;
            }
            let local = matches!(p.query.mode, Mode::Local { .. });
            let sequence_permitted = f.permit_first_seq >= 0
                && f.permit_last_seq >= f.permit_first_seq
                && f.permit_last_seq < SAFE
                && p.candidate.operation.inner().seq >= f.permit_first_seq
                && p.candidate.operation.inner().seq <= f.permit_last_seq;
            if !f.payload_valid || !sequence_permitted {
                if local {
                    effects.push(Effect::Report(Outcome::Invalid));
                } else {
                    stage(
                        s,
                        CommitKind::SecurityEvidence,
                        vec![p.candidate],
                        vec![],
                        vec![],
                        None,
                        effects,
                    );
                }
                return;
            }
            if local {
                if !f.current_allowed {
                    effects.push(Effect::Report(Outcome::Denied));
                    return;
                }
                let Mode::Local { time, .. } = &p.query.mode else {
                    unreachable!()
                };
                if !interval(&f.permit_window, time) {
                    effects.push(Effect::Report(Outcome::Denied));
                    return;
                }
            } else if p.candidate.evidence.admission.is_none()
                || !f.historical_admitter_authorized
                || !f.historical_policy_allowed
                || f.historical_time
                    .as_ref()
                    .is_none_or(|t| !interval(&f.permit_window, t))
            {
                stage(
                    s,
                    CommitKind::SecurityEvidence,
                    vec![p.candidate],
                    vec![],
                    vec![],
                    None,
                    effects,
                );
                return;
            }
            let missing = projection::missing(v, &p.candidate);
            if !missing.is_empty() {
                let slots = projection::pending_slots(s, &p.candidate.instance, &missing);
                if !projection::pending_fits(s, &p.candidate.instance, &slots, 0) {
                    s.instances
                        .get_mut(&p.candidate.instance)
                        .expect("selected instance")
                        .recovery_incomplete = true;
                    effects.push(Effect::Report(Outcome::Capacity));
                    return;
                }
                effects.push(Effect::RequestMissing(missing.clone()));
                effects.push(Effect::Report(Outcome::MissingHistory(missing)));
                if !local {
                    stage(
                        s,
                        CommitKind::Candidate,
                        vec![p.candidate],
                        vec![],
                        vec![],
                        None,
                        effects,
                    );
                }
                return;
            }
            if local
                && (v
                    .accepted
                    .iter()
                    .any(|a| projection::slot(&a.candidate) == projection::slot(&p.candidate))
                    || v.quarantine
                        .get(&p.candidate.operation.inner().origin)
                        .is_some_and(|floor| p.candidate.operation.inner().seq >= *floor)
                    || projection::dependencies(&p.candidate)
                        .iter()
                        .any(|(slot, hash)| {
                            !v.accepted.iter().any(|a| {
                                projection::slot(&a.candidate) == *slot
                                    && hash.is_none_or(|h| {
                                        a.candidate.operation.canonical().digest() == h
                                    })
                                    && projection::eligible(v)
                                        .contains(&a.candidate.operation.canonical().digest())
                            })
                        }))
            {
                effects.push(Effect::Report(Outcome::IntegrityConflict));
                return;
            }
            if local {
                let Some(id) = lifecycle::allocate(s) else {
                    effects.push(Effect::Report(Outcome::Capacity));
                    return;
                };
                let q = Query {
                    id,
                    mode: Mode::AdmissionSeal,
                    verified_facts: Some(f.clone()),
                    validated_under: Some(p.query.mode.clone()),
                    ..p.query.clone()
                };
                s.queries.insert(
                    id,
                    PendingVerification {
                        query: q.clone(),
                        candidate: p.candidate,
                        verified: Some(*f),
                        verified_query: Some(p.query),
                    },
                );
                effects.push(Effect::Seal(q));
            } else {
                let seal = p
                    .candidate
                    .evidence
                    .admission
                    .clone()
                    .expect("historical admission checked");
                acceptance(s, p.candidate, *f, p.query, None, seal, effects);
            }
        }
        Verdict::Sealed(seal)
            if p.query.mode == Mode::AdmissionSeal
                && p.verified.is_some()
                && p.verified_query.is_some() =>
        {
            let max = s.storage_limits.as_ref().map_or(0, |z| z.max_batch_bytes);
            if seal.value().is_empty() || seal.value().len() > max {
                effects.push(Effect::Report(Outcome::Capacity));
                return;
            }
            acceptance(
                s,
                p.candidate,
                p.verified.expect("checked facts"),
                p.verified_query.expect("checked query"),
                Some(p.query),
                seal,
                effects,
            );
        }
        _ => {
            effects.push(Effect::Report(Outcome::Invalid));
        }
    }
}
pub(crate) fn fork(s: &mut State, p: ForkEvidence, effects: &mut Vec<Effect>) {
    if projection::slot(&p.first) != projection::slot(&p.second)
        || p.first.operation == p.second.operation
    {
        effects.push(Effect::Report(Outcome::Invalid));
        return;
    }
    let Some(v) = s.instances.get(&p.first.instance) else {
        effects.push(Effect::Report(Outcome::Invalid));
        return;
    };
    if v.storage_integrity_failed {
        effects.push(Effect::Report(Outcome::IntegrityConflict));
        return;
    }
    let first = v.accepted.iter().any(|a| a.candidate == p.first);
    let second = v.accepted.iter().any(|a| a.candidate == p.second);
    if first && second {
        stage(
            s,
            CommitKind::QualifiedFork,
            vec![p.first.clone(), p.second.clone()],
            vec![],
            vec![p],
            None,
            effects,
        );
    } else {
        if !first {
            offer(s, p.first, false, effects);
        }
        if !second {
            offer(s, p.second, false, effects);
        }
    }
}
