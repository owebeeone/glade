//! Pure independent admission kernel. All evidence, time, policy and storage
//! ownership are explicit; effects request work without performing I/O.
mod admission;
mod callbacks;
mod encoder;
mod lifecycle;
mod projection;
mod staging;
mod types;
pub use types::*;

/// Consume one explicit event and return owned state plus concrete effects.
pub fn step(state: &State, event: Event) -> Transition {
    let mut state = state.clone();
    let mut effects = vec![];
    match event {
        Event::SubmitLocal(c) => admission::offer(&mut state, *c, true, &mut effects),
        Event::OfferReplica(c) => admission::offer(&mut state, *c, false, &mut effects),
        Event::EvidenceResolved(r) => admission::evidence(&mut state, *r, &mut effects),
        Event::PrepareResolved(r) => lifecycle::prepare_reply(&mut state, r, &mut effects),
        Event::CommitResolved(r) => {
            callbacks::resolve(&mut state, r, OperationKind::Begin, &mut effects)
        }
        Event::FenceResolved(r) => {
            callbacks::resolve(&mut state, r, OperationKind::Fence, &mut effects)
        }
        Event::LookupResolved(r) => {
            callbacks::resolve(&mut state, r, OperationKind::Inspect, &mut effects)
        }
        Event::ObservePolicy {
            instance,
            policy,
            time,
        } => lifecycle::observe(&mut state, instance, policy, time, &mut effects),
        Event::OfferFork(p) => admission::fork(&mut state, *p, &mut effects),
        Event::ResumeRecovery(instance) => {
            lifecycle::resume(&mut state, &instance, &mut effects);
            if effects.is_empty() {
                if let Some(v) = state.instances.get(&instance) {
                    if let Some(a) = v.accepted.last() {
                        effects.push(Effect::Report(Outcome::AcceptedLocal(a.receipt.clone())));
                    }
                }
            }
        }
        Event::Read(instance) => {
            if let Some(c) = projection::cut(&state, &instance) {
                effects.push(Effect::Report(Outcome::Read(c)));
            } else {
                effects.push(Effect::Report(Outcome::Invalid));
            }
        }
        Event::ObserveFrontier { instance, heads } => {
            frontier(&mut state, instance, heads, &mut effects)
        }
    }
    Transition { state, effects }
}
fn frontier(
    state: &mut State,
    instance: Instance,
    heads: std::collections::BTreeMap<String, (i64, Digest)>,
    effects: &mut Vec<Effect>,
) {
    let Some(v) = state.instances.get(&instance) else {
        effects.push(Effect::Report(Outcome::Invalid));
        return;
    };
    if heads.len() > v.descriptor.bounds.max_origins
        || heads.keys().any(|origin| origin.is_empty())
        || heads
            .values()
            .any(|(n, _)| *n < 0 || *n >= 9_007_199_254_740_991)
    {
        effects.push(Effect::Report(Outcome::Invalid));
        return;
    }
    if state.storage_limits.as_ref().is_none_or(|z| {
        instance.incarnation.len() > z.max_name_bytes
            || heads.keys().any(|origin| origin.len() > z.max_name_bytes)
    }) {
        state
            .instances
            .get_mut(&instance)
            .expect("selected instance")
            .recovery_incomplete = true;
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    }
    let mut missing = vec![];
    let mut offer = vec![];
    for a in &v.accepted {
        let o = a.candidate.operation.inner();
        if heads.get(&o.origin).is_none_or(|(n, _)| o.seq > *n) {
            offer.push(a.candidate.clone());
        }
    }
    for (origin, (n, h)) in heads {
        if !v.accepted.iter().any(|a| {
            a.candidate.operation.inner().origin == origin
                && a.candidate.operation.inner().seq == n
                && a.candidate.operation.canonical().digest() == h
        }) {
            missing.push(Slot {
                instance: instance.clone(),
                origin: origin.clone(),
                seq: n,
            });
        }
    }
    let union = projection::pending_slots(state, &instance, &missing);
    if !projection::pending_fits(state, &instance, &union, 0) {
        state
            .instances
            .get_mut(&instance)
            .expect("selected instance")
            .recovery_incomplete = true;
        effects.push(Effect::Report(Outcome::Capacity));
        return;
    }
    if !missing.is_empty() {
        let v = state
            .instances
            .get_mut(&instance)
            .expect("selected instance");
        // Slot retains the requested coordinate, but the accepted typed grammar
        // has no custody for an unknown frontier's expected digest. Mark that
        // unretained evidence cut sticky incomplete rather than later treating
        // a different arriving hash as completion. No reconstruction/clear
        // event exists; this conservative limitation requires component review.
        v.recovery_incomplete = true;
        for slot in &missing {
            if !v.pending.contains(slot) {
                v.pending.push(slot.clone());
            }
        }
        effects.push(Effect::RequestMissing(missing));
    }
    if !offer.is_empty() {
        effects.push(Effect::OfferMissing(offer));
    }
}
