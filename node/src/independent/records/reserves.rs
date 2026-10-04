//! Fixed, actually written twin-slot capacity; ordinary data cannot consume its
//! declared critical region. Exact future native/callback frames refine admission.
use super::super::namespace::codec_fault;
use super::*;
pub(super) fn ordinary(image: &ReplicaImage) -> Result<(), Fault> {
    let ceiling = image
        .limits
        .image_bytes
        .checked_sub(image.limits.storage.critical_bytes)
        .ok_or(Fault::Capacity)?;
    if encode(image, &image.limits.decode)
        .map_err(codec_fault)?
        .value()
        .len()
        > ceiling
    {
        return Err(Fault::Capacity);
    }
    Ok(())
}
/// Serialize every structural future native/callback frame from the exact owned
/// plan. This is space calculation, never cryptographic authority or publication.
/// The selected initial profile serializes a single instance; X/Y have distinct
/// sessions/slots. The actual core constructs all callback continuation states.
pub(super) fn critical(image: &ReplicaImage, floor: &ReplicaFloor) -> Result<(), Fault> {
    let current = bytes(image)?;
    let ceiling = image.limits.image_bytes;
    let discharged = super::loss::expanded(image, floor)?;
    if current > ceiling
        || discharged > ceiling
        || discharged.saturating_sub(current) > image.limits.storage.critical_bytes
    {
        return Err(Fault::Capacity);
    }
    let keys: std::collections::BTreeSet<_> = image
        .state
        .preparing
        .keys()
        .chain(
            image
                .state
                .attempts
                .iter()
                .filter(|(_, a)| !matches!(a.phase, AttemptPhase::Terminal(_)))
                .map(|(key, _)| key),
        )
        .cloned()
        .collect();
    if keys.len() > 1 {
        return Err(Fault::Integrity);
    }
    for key in keys {
        let maximum = future(image, floor, &key)?;
        let needed = maximum.saturating_sub(current);
        if maximum > ceiling
            || needed > image.limits.storage.critical_per_attempt
            || needed > image.limits.storage.critical_bytes
        {
            return Err(Fault::Capacity);
        }
    }
    Ok(())
}
fn bytes(image: &ReplicaImage) -> Result<usize, Fault> {
    Ok(encode(image, &image.limits.decode)
        .map_err(codec_fault)?
        .value()
        .len())
}
fn frame(image: &mut ReplicaImage, maximum: &mut usize) -> Result<(), Fault> {
    image.generation = image.generation.checked_add(1).ok_or(Fault::Capacity)?;
    // Physical u64 counters have the fixed eight-byte representation. Digests
    // are fixed-width, so future selections cannot increase this encoded size.
    image.prior_floor.selected.generation = image.generation;
    image.prior_floor.selected.digest = [255; 32];
    image.predecessor = [255; 32];
    *maximum = (*maximum)
        .max(bytes(image)?)
        .max(super::loss::expanded(image, &image.prior_floor)?);
    Ok(())
}
fn future(original: &ReplicaImage, floor: &ReplicaFloor, key: &PlanKey) -> Result<usize, Fault> {
    let mut image = original.clone();
    image.prior_floor = floor.clone();
    image.prior_floor.intent = None;
    let mut maximum = bytes(&image)?.max(super::loss::expanded(&image, floor)?);
    let attempt = if let Some(retained) = image
        .recovery
        .attempts
        .iter()
        .find(|a| &a.attempt.binding.plan == key)
    {
        retained.attempt.clone()
    } else {
        let prepare = image.state.preparing.get(key).ok_or(Fault::Integrity)?;
        let number = image.recovery.next_attempt.max(floor.next_attempt);
        let attempt = AttemptBinding {
            id: AttemptId {
                store: image.recovery.owner.store.clone(),
                instance: key.instance.clone(),
                number,
            },
            binding: prepare.binding.clone(),
        };
        image.recovery.next_attempt = number.checked_add(1).ok_or(Fault::Capacity)?;
        image.recovery.attempts.push(RetainedAttempt {
            attempt: attempt.clone(),
            phase: AttemptPhase::Reserved,
        });
        frame(&mut image, &mut maximum)?;
        attempt
    };
    if let Some(prepare) = image.state.preparing.get(key).cloned() {
        image.state = glade_crdt_admission_core::step(
            &image.state,
            Event::PrepareResolved(PrepareReply {
                request: prepare,
                result: PrepareResult::Prepared(Box::new(attempt.clone())),
            }),
        )
        .state;
        frame(&mut image, &mut maximum)?;
    }
    let at = image
        .recovery
        .attempts
        .iter()
        .position(|a| a.attempt == attempt)
        .ok_or(Fault::Integrity)?;
    if matches!(image.recovery.attempts[at].phase, AttemptPhase::Reserved) {
        image.recovery.attempts[at].phase = AttemptPhase::Started {
            cut: attempt.binding.prestart.as_ref().map(|p| p.cut.clone()),
        };
        frame(&mut image, &mut maximum)?;
    }
    let outcome = match &image.recovery.attempts[at].phase {
        AttemptPhase::Terminal(outcome) => outcome.clone(),
        AttemptPhase::Started { cut } => {
            let revision = attempt
                .binding
                .expected_revision
                .checked_add(1)
                .ok_or(Fault::Capacity)?;
            let outcome = TerminalOutcome::Committed {
                revision,
                storage: attempt.binding.storage.clone(),
                receipts: attempt.binding.receipts.clone(),
                start_cut: cut.clone(),
            };
            image
                .recovery
                .revisions
                .insert(key.instance.clone(), revision);
            image.recovery.attempts[at].phase = AttemptPhase::Terminal(outcome.clone());
            frame(&mut image, &mut maximum)?;
            outcome
        }
        AttemptPhase::Reserved => return Err(Fault::Integrity),
    };
    let mut resolve = image
        .state
        .storage_invocations
        .values()
        .find_map(|invocation| match invocation {
            StorageInvocation::Resolve(request) if request.attempt == attempt => {
                Some(request.clone())
            }
            _ => None,
        });
    if resolve.is_none() && !image.state.terminal_results.contains_key(&attempt.id) {
        image.state = glade_crdt_admission_core::step(
            &image.state,
            Event::ResumeRecovery(key.instance.clone()),
        )
        .state;
        frame(&mut image, &mut maximum)?;
        resolve =
            image
                .state
                .storage_invocations
                .values()
                .find_map(|invocation| match invocation {
                    StorageInvocation::Resolve(request) if request.attempt == attempt => {
                        Some(request.clone())
                    }
                    _ => None,
                });
    }
    if let Some(request) = resolve {
        let reply = ResolveReply {
            request: request.clone(),
            result: ResolveResult::Terminal(Box::new(Terminal { attempt, outcome })),
        };
        let event = match request.operation {
            OperationKind::Begin => Event::CommitResolved(reply),
            OperationKind::Inspect => Event::LookupResolved(reply),
            OperationKind::Fence => Event::FenceResolved(reply),
        };
        image.state = glade_crdt_admission_core::step(&image.state, event).state;
        frame(&mut image, &mut maximum)?;
    }
    Ok(maximum)
}
