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
        .collect()
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
