//! Complete original batch bindings and source-derived cryptographic facts.
use super::super::namespace::{codec_fault, signed_bytes};
use super::*;
use glade_crdt_evidence_api::BoundQuery;
pub(super) fn check<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    binding: &Binding,
    next_effect: u64,
) -> Result<CommitPlan, Fault> {
    let limits = &request.limits.storage;
    if binding.plan.namespace != request.plan_namespace
        || binding.plan.instance != request.instance
        || binding.storage != StorageClass::LocalProcessRestart
        || binding.batch.value().len() > limits.max_batch_bytes
        || binding.receipts.len() > limits.max_receipts
        || binding.plan.number >= next_effect
        || binding
            .prestart
            .as_ref()
            .is_some_and(|start| start.windows.len() > limits.max_windows)
    {
        return Err(Fault::BindingMismatch);
    }
    let decoded =
        decode_batch(binding.batch.value(), &request.limits.decode).map_err(codec_fault)?;
    let plan = &decoded.plan;
    if decoded.owner.as_ref() != Some(&request.owner)
        || decoded.storage != Some(binding.storage.clone())
        || decoded.plan_namespace != request.plan_namespace
        || decoded.invocation_namespace != request.invocation_namespace
        || decoded.descriptor != request.descriptor
        || decoded.next_effect_id > next_effect
        || decoded.next_effect_id <= plan.id
        || plan.id != binding.plan.number
        || plan.instance != request.instance
        || plan.expected_revision != binding.expected_revision
        || plan.kind != binding.kind
        || plan.charges != binding.charges
        || plan
            .acceptances
            .iter()
            .map(|acceptance| acceptance.receipt.clone())
            .collect::<Vec<_>>()
            != binding.receipts
        || plan
            .local_precondition
            .clone()
            .map(|cut| StartPrecondition {
                cut,
                windows: plan
                    .acceptances
                    .iter()
                    .map(|acceptance| acceptance.facts.permit_window.clone())
                    .collect(),
            })
            != binding.prestart
        || plan.records.is_empty()
    {
        return Err(Fault::Integrity);
    }
    for candidate in &plan.records {
        record(evidence, request, candidate)?;
    }
    // The frozen core charges the complete converged batch, including the
    // original verification/seal continuations, not just payload evidence.
    let size = binding.batch.value().len();
    let charges = Charges {
        edit: if plan.local_precondition.is_some() {
            size
        } else {
            0
        },
        recovery: if plan.local_precondition.is_none() {
            size
        } else {
            0
        },
        evidence: if matches!(
            plan.kind,
            CommitKind::SecurityEvidence | CommitKind::QualifiedFork
        ) {
            size
        } else {
            0
        },
        pending: if plan.kind == CommitKind::Candidate {
            size
        } else {
            0
        },
    };
    if charges != plan.charges {
        return Err(Fault::Integrity);
    }
    for acceptance in &plan.acceptances {
        if !plan.records.contains(&acceptance.candidate) {
            return Err(Fault::Integrity);
        }
        acceptance_proof(evidence, request, acceptance)?;
    }
    if plan.kind == CommitKind::AcceptedBatch && plan.acceptances.is_empty()
        || matches!(
            plan.kind,
            CommitKind::Candidate | CommitKind::SecurityEvidence
        ) && !plan.acceptances.is_empty()
    {
        return Err(Fault::Integrity);
    }
    if plan.kind == CommitKind::QualifiedFork
        && (plan.qualified_forks.is_empty()
            || plan.acceptances.is_empty()
                && (plan.local_precondition.is_some()
                    || plan.qualified_forks.len() != 1
                    || plan.records
                        != vec![
                            plan.qualified_forks[0].first.clone(),
                            plan.qualified_forks[0].second.clone(),
                        ]))
    {
        return Err(Fault::Integrity);
    }
    for fork in &plan.qualified_forks {
        let first = record(evidence, request, &fork.first)?;
        let second = record(evidence, request, &fork.second)?;
        let a = fork.first.operation.inner();
        let b = fork.second.operation.inner();
        if a.origin != b.origin
            || a.seq != b.seq
            || fork.first.operation == fork.second.operation
            || !qualified(&first)
            || !qualified(&second)
        {
            return Err(Fault::Integrity);
        }
    }
    Ok(decoded.plan)
}
fn qualified(facts: &Facts) -> bool {
    facts.descriptor_valid
        && facts.payload_valid
        && facts.signatures_valid
        && facts.issuance_unique
        && facts.historical_admitter_authorized
        && facts.historical_policy_allowed
        && facts
            .historical_time
            .as_ref()
            .is_some_and(|time| super::super::auth::fits(time, &facts.permit_window))
}
pub(super) fn record<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    candidate: &Candidate,
) -> Result<Facts, Fault> {
    let query = Query {
        id: 0,
        instance: candidate.instance.clone(),
        descriptor: request.descriptor.clone(),
        operation: candidate.operation.clone(),
        evidence: candidate.evidence.clone(),
        mode: Mode::Historical,
        verified_facts: None,
        validated_under: None,
    };
    evidence.checked(
        &BoundQuery {
            query,
            identity: request.identity.clone(),
        },
        &Mode::Historical,
        false,
    )
}
pub(super) fn acceptance_proof<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    acceptance: &StagedAcceptance,
) -> Result<(), Fault> {
    let candidate = &acceptance.candidate;
    let facts = record(evidence, request, candidate)?;
    if !qualified(&facts)
        || candidate.evidence.admission.as_ref() != Some(&acceptance.sealed_admission)
        || acceptance.receipt
            != (Receipt {
                instance: request.instance.clone(),
                operation: candidate.operation.canonical().digest(),
                admission: acceptance.sealed_admission.digest(),
                stable_id: candidate.operation.canonical().digest(),
                storage: StorageClass::LocalProcessRestart,
            })
    {
        return Err(Fault::Integrity);
    }
    let query = &acceptance.verified_query;
    if query.instance != request.instance
        || query.descriptor != request.descriptor
        || query.operation != candidate.operation
        || query.evidence.certificate != candidate.evidence.certificate
        || query.evidence.permit != candidate.evidence.permit
        || query.evidence.policy != candidate.evidence.policy
        || query.verified_facts.is_some()
        || query.validated_under.is_some()
    {
        return Err(Fault::Integrity);
    }
    match &query.mode {
        Mode::Historical => {
            if query.evidence != candidate.evidence
                || acceptance.seal_query.is_some()
                || acceptance.facts != facts
            {
                return Err(Fault::Integrity);
            }
        }
        Mode::Local {
            current_policy,
            time,
        } => {
            let admission: OriginalAdmissionBody = signed_bytes(
                &acceptance.sealed_admission,
                &crate::signing::public_key(&evidence.config.node_seed),
                &evidence.config.limits,
            )?;
            let Some(seal) = &acceptance.seal_query else {
                return Err(Fault::Integrity);
            };
            if query.evidence.admission.is_some()
                || *current_policy != query.evidence.policy.digest()
                || admission.time != *time
                || admission.policy != *current_policy
                || seal.mode != Mode::AdmissionSeal
                || seal.validated_under.as_ref() != Some(&query.mode)
                || seal.verified_facts.as_deref() != Some(&acceptance.facts)
                || seal.instance != query.instance
                || seal.descriptor != query.descriptor
                || seal.operation != query.operation
                || seal.evidence != query.evidence
                || seal.id <= query.id
                || !super::super::auth::fits(time, &facts.permit_window)
            {
                return Err(Fault::Integrity);
            }
            let mut local = facts.clone();
            local.current_allowed = true;
            local.historical_admitter_authorized = false;
            local.historical_policy_allowed = false;
            local.historical_time = None;
            if acceptance.facts != local
                || !evidence.sessions.values().any(|custody| {
                    if !custody.consumed {
                        return false;
                    }
                    let Ok(challenge) = decode_signed_body::<LocalChallengeBody>(
                        &custody.challenge,
                        &evidence.config.limits,
                    ) else {
                        return false;
                    };
                    challenge.operation == candidate.operation.canonical().digest()
                        && challenge.writer_key == facts.certificate.writer_key
                        && challenge.requester == facts.certificate.requester
                        && super::super::auth::fits(time, &challenge.window)
                })
            {
                return Err(Fault::Integrity);
            }
        }
        Mode::AdmissionSeal => {
            return Err(Fault::Integrity);
        }
    }
    Ok(())
}
/// Reconstruct an original local verification from signed historical inputs and
/// the selected signed possession custody. Today's policy/time is not substituted.
pub(super) fn local_facts<P: PolicyClock>(
    evidence: &mut GenuineEvidence<P>,
    request: &ReplicaOpen,
    query: &Query,
) -> Result<Facts, Fault> {
    let Mode::Local {
        current_policy,
        time,
    } = &query.mode
    else {
        return Err(Fault::Integrity);
    };
    if query.evidence.admission.is_some() || *current_policy != query.evidence.policy.digest() {
        return Err(Fault::Integrity);
    }
    let candidate = Candidate {
        instance: query.instance.clone(),
        operation: query.operation.clone(),
        evidence: query.evidence.clone(),
    };
    let mut facts = record(evidence, request, &candidate)?;
    let policy: PolicyBody = signed_bytes(
        &query.evidence.policy,
        &evidence.config.trusted_root,
        &evidence.config.limits,
    )?;
    let wrapper: PermitWrapper =
        decode_profile(query.evidence.permit.value(), &evidence.config.limits)
            .map_err(codec_fault)?;
    let permit: PermitBody = super::super::namespace::signed_body(
        &wrapper.signed_permit,
        &evidence.config.trusted_root,
        &evidence.config.limits,
    )?;
    let permit_digest =
        super::super::namespace::record_bytes(&wrapper.signed_permit, &evidence.config.limits)?
            .digest();
    let node = crate::signing::public_key(&evidence.config.node_seed);
    let custody = evidence
        .sessions
        .values()
        .find_map(|entry| {
            if !entry.consumed {
                return None;
            }
            let challenge: LocalChallengeBody =
                decode_signed_body(&entry.challenge, &evidence.config.limits).ok()?;
            if challenge.operation == query.operation.canonical().digest()
                && challenge.requester == facts.certificate.requester
                && challenge.writer_key == facts.certificate.writer_key
                && super::super::auth::fits(time, &challenge.window)
            {
                Some(challenge)
            } else {
                None
            }
        })
        .ok_or(Fault::Integrity)?;
    facts.current_allowed = super::super::auth::grant(
        &policy,
        &custody.requester,
        &custody.writer_key,
        &request.descriptor.resource,
    ) && !super::super::auth::revoked(
        &policy,
        &[
            custody.writer_key,
            query.evidence.certificate.digest(),
            permit_digest,
            node,
        ],
    ) && super::super::auth::fits(time, &permit.window)
        && super::super::auth::fits(time, &policy.window)
        && permit.admitting_nodes.contains(&node)
        && policy.admitting_nodes.contains(&node);
    Ok(facts)
}
