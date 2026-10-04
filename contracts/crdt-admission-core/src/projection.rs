//! Finite closure and projection from retained qualified custody. Quarantine is
//! derived only after qualification, and never participates in that predicate.
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn slot(c: &Candidate) -> Slot {
    Slot {
        instance: c.instance.clone(),
        origin: c.operation.inner().origin.clone(),
        seq: c.operation.inner().seq,
    }
}
pub(crate) fn dependencies(c: &Candidate) -> Vec<(Slot, Option<Digest>)> {
    let o = c.operation.inner();
    let mut result = vec![];
    if o.seq > 0 {
        result.push((
            Slot {
                instance: c.instance.clone(),
                origin: o.origin.clone(),
                seq: o.seq - 1,
            },
            o.prev.as_ref().and_then(|p| p.as_slice().try_into().ok()),
        ));
    }
    for h in &o.refs {
        result.push((
            Slot {
                instance: c.instance.clone(),
                origin: h.origin.clone(),
                seq: h.seq,
            },
            h.hash.as_ref().and_then(|p| p.as_slice().try_into().ok()),
        ));
    }
    result
}
pub(crate) fn missing(v: &InstanceState, c: &Candidate) -> Vec<Slot> {
    dependencies(c)
        .into_iter()
        .filter_map(|(s, h)| {
            if v.accepted.iter().any(|a| {
                slot(&a.candidate) == s
                    && h.is_none_or(|h| a.candidate.operation.canonical().digest() == h)
            }) {
                None
            } else {
                Some(s)
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
/// Reserve the deduplicated union, including unpublished owned candidate work.
pub(crate) fn pending_slots(s: &State, instance: &Instance, extra: &[Slot]) -> BTreeSet<Slot> {
    let v = &s.instances[instance];
    let mut slots: BTreeSet<_> = v.pending.iter().chain(extra).cloned().collect();
    for c in &v.candidates {
        slots.extend(missing(v, c));
    }
    for p in s.commits.values().chain(s.unknown_commits.values()) {
        if &p.instance == instance && p.kind == CommitKind::Candidate {
            for c in &p.records {
                slots.extend(missing(v, c));
            }
        }
    }
    slots
}
pub(crate) fn pending_fits(
    s: &State,
    instance: &Instance,
    slots: &BTreeSet<Slot>,
    extra_bytes: usize,
) -> bool {
    let v = &s.instances[instance];
    let Some(z) = &s.storage_limits else {
        return false;
    };
    // Slot owns its fixed representation and both variable strings. Count
    // those retained bytes alongside charged candidates and live queries;
    // hints do not fabricate durable storage charges.
    let bytes = slots
        .iter()
        .fold(v.used.pending.checked_add(extra_bytes), |n, slot| {
            n?.checked_add(std::mem::size_of::<Slot>())?
                .checked_add(slot.instance.incarnation.len())?
                .checked_add(slot.origin.len())
        });
    let bytes = s
        .queries
        .values()
        .filter(|q| &q.candidate.instance == instance)
        .fold(bytes, |n, q| {
            n?.checked_add(crate::admission::candidate_size(&q.candidate)?)
        });
    let bytes = s
        .commits
        .values()
        .chain(s.unknown_commits.values())
        .filter(|p| &p.instance == instance)
        .fold(bytes, |n, p| n?.checked_add(p.charges.pending));
    slots.len() <= v.descriptor.bounds.pending_items
        && slots.iter().all(|slot| {
            !slot.origin.is_empty()
                && slot.origin.len() <= z.max_name_bytes
                && slot.instance.incarnation.len() <= z.max_name_bytes
        })
        && bytes.is_some_and(|n| n <= v.descriptor.bounds.pending_bytes)
}
pub(crate) fn eligible(v: &InstanceState) -> BTreeSet<Digest> {
    let mut set = BTreeSet::new();
    loop {
        let before = set.len();
        for a in &v.accepted {
            let c = &a.candidate;
            let o = c.operation.inner();
            if v.quarantine.get(&o.origin).is_some_and(|f| o.seq >= *f) {
                continue;
            }
            if dependencies(c).iter().all(|(s, h)| {
                v.accepted.iter().any(|a| {
                    slot(&a.candidate) == *s
                        && h.is_none_or(|h| a.candidate.operation.canonical().digest() == h)
                        && set.contains(&a.candidate.operation.canonical().digest())
                })
            }) {
                set.insert(c.operation.canonical().digest());
            }
        }
        if set.len() == before {
            break;
        }
    }
    set
}
pub(crate) fn refresh(v: &mut InstanceState) {
    // Retain existing convictions (trusted restored state), add independently
    // qualified exact rival pairs, and never select either fork's hash as head.
    for pair in &v.qualified_forks {
        let s = slot(&pair.first);
        if s == slot(&pair.second) && pair.first.operation != pair.second.operation {
            v.quarantine
                .entry(s.origin)
                .and_modify(|f| *f = (*f).min(s.seq))
                .or_insert(s.seq);
        }
    }
    let eligible = eligible(v);
    let mut heads = BTreeMap::new();
    for a in &v.accepted {
        let c = &a.candidate;
        let o = c.operation.inner();
        if eligible.contains(&c.operation.canonical().digest()) {
            let entry = heads
                .entry(o.origin.clone())
                .or_insert((o.seq, c.operation.canonical().digest()));
            if o.seq > entry.0 {
                *entry = (o.seq, c.operation.canonical().digest());
            }
        }
    }
    v.heads = heads;
    let mut pending: BTreeSet<_> = v
        .pending
        .iter()
        .filter(|s| !v.accepted.iter().any(|a| slot(&a.candidate) == **s))
        .cloned()
        .collect();
    for c in &v.candidates {
        pending.extend(missing(v, c));
    }
    v.pending = pending.into_iter().collect();
}
pub(crate) fn cut(state: &State, instance: &Instance) -> Option<ReadCut> {
    let mut v = state.instances.get(instance)?.clone();
    refresh(&mut v);
    let eligible = eligible(&v);
    let unresolved = state
        .queries
        .values()
        .any(|q| &q.candidate.instance == instance)
        || state
            .commits
            .values()
            .chain(state.unknown_commits.values())
            .any(|p| &p.instance == instance)
        || state.preparing.keys().any(|p| &p.instance == instance)
        || state
            .lookups
            .values()
            .any(|q| &q.attempt.id.instance == instance);
    Some(ReadCut {
        instance: instance.clone(),
        contiguous_common_heads: v.heads,
        historically_qualified: v
            .accepted
            .iter()
            .map(|a| a.candidate.operation.canonical().digest())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        fork_slots: v
            .qualified_forks
            .iter()
            .map(|p| slot(&p.first))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        eligible: eligible.into_iter().collect(),
        complete_local: !unresolved
            && !v.recovery_incomplete
            && !v.storage_integrity_failed
            && v.pending.is_empty()
            && v.candidates.is_empty(),
        pending: v.pending,
        quarantine: v.quarantine,
    })
}
