//! Immutable semantic plans and complete encoded quota reservation.
use crate::admission::candidate_size;
use crate::*;
use crate::{callbacks, encoder, lifecycle, projection};

pub(crate) fn acceptance(
    s: &mut State,
    mut c: Candidate,
    f: Facts,
    verify: Query,
    seal: Option<Query>,
    admission: Bytes,
    effects: &mut Vec<Effect>,
) {
    let v = &s.instances[&c.instance];
    let receipt = Receipt {
        instance: c.instance.clone(),
        operation: c.operation.canonical().digest(),
        admission: admission.digest(),
        stable_id: c.operation.canonical().digest(),
        storage: v.storage.clone(),
    };
    let forks: Vec<_> = v
        .accepted
        .iter()
        .filter(|a| {
            projection::slot(&a.candidate) == projection::slot(&c)
                && a.candidate.operation != c.operation
        })
        .map(|a| ForkEvidence {
            first: a.candidate.clone(),
            second: c.clone(),
        })
        .collect();
    let kind = if forks.is_empty() {
        CommitKind::AcceptedBatch
    } else {
        CommitKind::QualifiedFork
    };
    let local = if let Mode::Local {
        current_policy,
        time,
    } = &verify.mode
    {
        Some(LocalPrecondition {
            policy: *current_policy,
            time: time.clone(),
        })
    } else {
        None
    };
    c.evidence.admission = Some(admission.clone());
    let a = StagedAcceptance {
        candidate: c.clone(),
        facts: f,
        verified_query: verify,
        seal_query: seal,
        sealed_admission: admission,
        receipt,
    };
    stage(s, kind, vec![c], vec![a], forks, local, effects);
}
pub(crate) fn stage(
    s: &mut State,
    kind: CommitKind,
    records: Vec<Candidate>,
    acceptances: Vec<StagedAcceptance>,
    forks: Vec<ForkEvidence>,
    local: Option<LocalPrecondition>,
    effects: &mut Vec<Effect>,
) {
    let instance = records[0].instance.clone();
    if s.commits
        .values()
        .chain(s.unknown_commits.values())
        .any(|p| p.instance == instance)
    {
        lifecycle::resume(s, &instance, effects);
        return;
    }
    let v = &s.instances[&instance];
    let size = records
        .iter()
        .try_fold(0usize, |n, c| n.checked_add(candidate_size(c)?));
    let Some(size) = size else {
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    };
    let charges = Charges {
        edit: if local.is_some() { size } else { 0 },
        recovery: if local.is_none() { size } else { 0 },
        evidence: if kind == CommitKind::SecurityEvidence || kind == CommitKind::QualifiedFork {
            size
        } else {
            0
        },
        pending: if kind == CommitKind::Candidate {
            size
        } else {
            0
        },
    };
    let bounds = &v.descriptor.bounds;
    let enough = callbacks::add(&v.used, &charges).is_some_and(|n| {
        n.edit <= bounds.edit_bytes
            && n.recovery <= bounds.recovery_bytes
            && n.evidence <= bounds.evidence_bytes
            && n.pending <= bounds.pending_bytes
    }) && (kind != CommitKind::Candidate || v.candidates.len() < bounds.pending_items)
        && v.revision < u64::MAX;
    if !enough {
        if local.is_none() {
            s.instances
                .get_mut(&instance)
                .expect("selected instance")
                .recovery_incomplete = true;
        }
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    }
    let descriptor = v.descriptor.clone();
    let revision = v.revision;
    let Some(id) = lifecycle::allocate(s) else {
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    };
    let mut p = CommitPlan {
        id,
        instance: instance.clone(),
        expected_revision: revision,
        local_precondition: local,
        batch: Bytes::new(vec![]),
        kind,
        records,
        acceptances,
        qualified_forks: forks,
        charges,
    };
    // The batch/digest is this function's derived output, never recursively encoded.
    // Count complete encoded continuations, including seals/facts, rather than
    // only the inner operation. Varint charge widths converge monotonically.
    loop {
        p.batch = encoder::encode(s, &descriptor, &p);
        let bytes = p.batch.value().len();
        let exact = Charges {
            edit: if p.local_precondition.is_some() {
                bytes
            } else {
                0
            },
            recovery: if p.local_precondition.is_none() {
                bytes
            } else {
                0
            },
            evidence: if p.kind == CommitKind::SecurityEvidence
                || p.kind == CommitKind::QualifiedFork
            {
                bytes
            } else {
                0
            },
            pending: if p.kind == CommitKind::Candidate {
                bytes
            } else {
                0
            },
        };
        if p.charges == exact {
            break;
        }
        p.charges = exact;
    }
    let v = &s.instances[&instance];
    let b = &v.descriptor.bounds;
    if callbacks::add(&v.used, &p.charges).is_none_or(|n| {
        n.edit > b.edit_bytes
            || n.recovery > b.recovery_bytes
            || n.evidence > b.evidence_bytes
            || n.pending > b.pending_bytes
    }) {
        if p.local_precondition.is_none() {
            s.instances
                .get_mut(&instance)
                .expect("selected instance")
                .recovery_incomplete = true;
        }
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    }
    let historical = p.local_precondition.is_none();
    if !lifecycle::stage(s, p, effects) {
        if historical {
            s.instances
                .get_mut(&instance)
                .expect("selected instance")
                .recovery_incomplete = true;
        }
        effects.push(Effect::Report(Outcome::Capacity));
    }
}
