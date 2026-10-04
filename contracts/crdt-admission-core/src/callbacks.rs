//! Authenticate full issued requests before interpreting any callback payload.
//! Original terminals and first contrary assertions remain bounded by attempts.
use crate::*;
use crate::{lifecycle, projection};

fn terminal_valid(t: &Terminal, a: &AttemptBinding) -> bool {
    if &t.attempt != a {
        return false;
    }
    let b = &a.binding;
    match &t.outcome {
        TerminalOutcome::NonCommit { revision, storage } => {
            *revision == b.expected_revision && storage == &b.storage
        }
        TerminalOutcome::Committed {
            revision,
            storage,
            receipts,
            start_cut,
        } => {
            b.expected_revision.checked_add(1) == Some(*revision)
                && storage == &b.storage
                && receipts == &b.receipts
                && start_cut == &b.prestart.as_ref().map(|p| p.cut.clone())
        }
    }
}
pub(crate) fn resolve(
    s: &mut State,
    r: ResolveReply,
    operation: OperationKind,
    effects: &mut Vec<Effect>,
) {
    let issued = StorageInvocation::Resolve(r.request.clone());
    let live = s.storage_invocations.get(&r.request.invocation) == Some(&issued);
    let retired = s.retired_storage_invocations.get(&r.request.invocation) == Some(&issued);
    if r.request.operation != operation
        || (!live && !retired)
        || s.storage_owner.as_ref() != Some(&r.request.owner)
        || r.request.invocation.namespace != s.invocation_namespace
    {
        effects.push(Effect::Report(Outcome::CallbackMismatch));
        return;
    }
    if let ResolveResult::Terminal(t) = &r.result {
        if !terminal_valid(t, &r.request.attempt) {
            effects.push(Effect::Report(Outcome::CallbackMismatch));
            return;
        }
        if let Some(original) = s.terminal_results.get(&t.attempt.id) {
            if original != t.as_ref() {
                s.terminal_conflicts
                    .entry(t.attempt.id.clone())
                    .or_insert((**t).clone());
                if let Some(v) = s.instances.get_mut(&t.attempt.id.instance) {
                    v.storage_integrity_failed = true;
                }
                effects.push(Effect::Report(Outcome::IntegrityConflict));
                return;
            }
        }
    }
    if !live {
        effects.push(Effect::Report(Outcome::CallbackMismatch));
        return;
    }
    let key = r.request.attempt.binding.plan.clone();
    if s.attempts
        .get(&key)
        .is_none_or(|a| a.attempt != r.request.attempt)
    {
        effects.push(Effect::Report(Outcome::CallbackMismatch));
        return;
    }
    let plan = s
        .commits
        .get(&key.number)
        .or_else(|| s.unknown_commits.get(&key.number))
        .cloned();
    if let Some(p) = &plan {
        if lifecycle::binding(s, p).as_ref() != Some(&r.request.attempt.binding) {
            effects.push(Effect::Report(Outcome::CallbackMismatch));
            return;
        }
    }
    if let ResolveResult::Started { cut } = &r.result {
        if cut
            != &r
                .request
                .attempt
                .binding
                .prestart
                .as_ref()
                .map(|p| p.cut.clone())
        {
            effects.push(Effect::Report(Outcome::CallbackMismatch));
            return;
        }
    }
    // Check all installation arithmetic before consuming authenticated work.
    if matches!(&r.result,ResolveResult::Terminal(t) if matches!(t.outcome,TerminalOutcome::Committed{..}))
        && !s.terminal_results.contains_key(&r.request.attempt.id)
    {
        let Some(p) = &plan else {
            effects.push(Effect::Report(Outcome::CallbackMismatch));
            return;
        };
        let Some(v) = s.instances.get(&p.instance) else {
            effects.push(Effect::Report(Outcome::CallbackMismatch));
            return;
        };
        if v.revision != p.expected_revision || add(&v.used, &p.charges).is_none() {
            effects.push(Effect::Report(Outcome::CallbackMismatch));
            return;
        }
    }
    let request = s
        .storage_invocations
        .remove(&r.request.invocation)
        .expect("matched issuance");
    s.retired_storage_invocations
        .insert(r.request.invocation.clone(), request);
    s.lookups.remove(&r.request.invocation.number);
    match r.result {
        ResolveResult::Pending | ResolveResult::Refused(_) | ResolveResult::Started { .. } => {
            if let ResolveResult::Started { cut } = r.result {
                s.attempts.get_mut(&key).expect("matched attempt").phase =
                    AttemptPhase::Started { cut };
            }
            if let Some(p) = s.commits.remove(&key.number) {
                s.unknown_commits.insert(p.id, p);
            }
            effects.push(Effect::Report(Outcome::OutcomeUnknown(
                r.request.attempt.binding.batch.digest(),
            )));
        }
        ResolveResult::Terminal(t) => {
            if s.terminal_results.contains_key(&t.attempt.id) {
                effects.push(Effect::Report(Outcome::CallbackMismatch));
                return;
            }
            s.terminal_results
                .insert(t.attempt.id.clone(), (*t).clone());
            s.attempts.get_mut(&key).expect("matched attempt").phase =
                AttemptPhase::Terminal(t.outcome.clone());
            let recovering =
                s.unknown_commits.contains_key(&key.number) || operation == OperationKind::Inspect;
            s.commits.remove(&key.number);
            s.unknown_commits.remove(&key.number);
            s.preparing.remove(&key);
            if let TerminalOutcome::Committed {
                revision, storage, ..
            } = &t.outcome
            {
                let p = plan.expect("validated complete semantic plan");
                let v = s
                    .instances
                    .get_mut(&p.instance)
                    .expect("validated instance");
                v.used = add(&v.used, &p.charges).expect("checked installation");
                v.revision = *revision;
                for a in &p.acceptances {
                    if !v
                        .accepted
                        .iter()
                        .any(|old| old.candidate.operation == a.candidate.operation)
                    {
                        v.canonical_origins.insert(
                            a.facts.certificate.canonical_origin.clone(),
                            a.facts.certificate.clone(),
                        );
                        v.accepted.push(Accepted {
                            candidate: a.candidate.clone(),
                            facts: a.facts.clone(),
                            receipt: a.receipt.clone(),
                        });
                    }
                    v.candidates
                        .retain(|c| c.operation != a.candidate.operation);
                }
                match p.kind {
                    CommitKind::Candidate => {
                        for c in &p.records {
                            if !v.candidates.contains(c) {
                                v.candidates.push(c.clone());
                            }
                        }
                    }
                    CommitKind::SecurityEvidence => {
                        for c in &p.records {
                            if !v.security_evidence.contains(c) {
                                v.security_evidence.push(c.clone());
                            }
                        }
                    }
                    CommitKind::AcceptedBatch | CommitKind::QualifiedFork => {}
                }
                for f in &p.qualified_forks {
                    if !v.qualified_forks.contains(f) {
                        v.qualified_forks.push(f.clone());
                    }
                }
                projection::refresh(v);
                if p.kind == CommitKind::AcceptedBatch {
                    for a in &p.acceptances {
                        effects.push(Effect::Report(if recovering {
                            Outcome::ExactRetry(a.receipt.clone())
                        } else {
                            Outcome::AcceptedLocal(a.receipt.clone())
                        }));
                    }
                } else {
                    effects.push(Effect::Report(Outcome::BatchRetained {
                        instance: p.instance.clone(),
                        kind: p.kind.clone(),
                        batch_digest: p.batch.digest(),
                        revision: *revision,
                        storage: storage.clone(),
                    }));
                }
                if let Some(cut) = projection::cut(s, &p.instance) {
                    effects.push(Effect::ProjectionChanged(cut));
                }
                // Replay retained candidates only after publication releases X.
                let candidates = s.instances[&p.instance].candidates.clone();
                for c in candidates {
                    if projection::missing(&s.instances[&p.instance], &c).is_empty() {
                        crate::admission::offer(s, c, false, effects);
                        break;
                    }
                }
            }
        }
    }
}
pub(crate) fn add(a: &Charges, b: &Charges) -> Option<Charges> {
    Some(Charges {
        edit: a.edit.checked_add(b.edit)?,
        recovery: a.recovery.checked_add(b.recovery)?,
        evidence: a.evidence.checked_add(b.evidence)?,
        pending: a.pending.checked_add(b.pending)?,
    })
}
