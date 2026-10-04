//! Permanent untracked loss is conservative, bounded and coupled to its floor guard.
use super::*;
type LossBody = (String, (ReceiveGuard, (u64, Bytes)));
fn marker(
    guard: &ReceiveGuard,
    generation: u64,
    observed: Digest,
    limits: &DecodeLimits,
) -> Result<Bytes, Fault> {
    encode(
        &(
            "glade/ic3/ingress-loss/v1".to_owned(),
            (guard.clone(), (generation, Bytes::new(observed.to_vec()))),
        ),
        limits,
    )
    .map_err(super::super::namespace::codec_fault)
}
pub(super) fn expanded(image: &ReplicaImage, floor: &ReplicaFloor) -> Result<usize, Fault> {
    let mut worst = image.clone();
    worst.prior_floor = floor.clone();
    for status in floor.guards.values() {
        if let GuardStatus::Active(guard) = status {
            worst.generation = image.generation.checked_add(1).ok_or(Fault::Capacity)?;
            worst.permanent_loss.push(marker(
                guard,
                worst.generation,
                [255; 32],
                &image.limits.decode,
            )?);
        }
    }
    Ok(encode(&worst, &image.limits.decode)
        .map_err(super::super::namespace::codec_fault)?
        .value()
        .len())
}
pub(super) fn reserve(
    image: &ReplicaImage,
    projected: &ReplicaFloor,
    _: &ReceiveGuard,
) -> Result<(), Fault> {
    reserves::critical(image, projected)
}
pub(super) fn retained(image: &ReplicaImage, floor: &ReplicaFloor) -> Result<(), Fault> {
    if image.permanent_loss.len() > image.limits.guard_history {
        return Err(Fault::Capacity);
    }
    let mut seen = std::collections::BTreeSet::new();
    for bytes in &image.permanent_loss {
        let (purpose, (guard, (generation, observed))): LossBody =
            decode(bytes.value(), &image.limits.decode)
                .map_err(super::super::namespace::codec_fault)?;
        if purpose != "glade/ic3/ingress-loss/v1"
            || observed.value().len() != 32
            || generation > image.generation
            || !seen.insert(guard.id.clone())
            || floor.guards.get(&guard.id)
                != Some(&GuardStatus::Loss {
                    guard,
                    loss: bytes.digest(),
                    image_generation: generation,
                })
        {
            return Err(Fault::Integrity);
        }
    }
    if floor
        .guards
        .values()
        .filter(|status| matches!(status, GuardStatus::Loss { .. }))
        .count()
        != seen.len()
    {
        return Err(Fault::Integrity);
    }
    Ok(())
}
pub(super) fn retain<P: PolicyClock>(
    session: &mut DiskSession<P>,
    permit: IngressPermit,
    observed: Bytes,
) -> PersistResult {
    let result = (|| {
        session.authority.can_settle(&permit)?;
        let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        if pair.envelope.floor.guards.get(&permit.guard().id)
            != Some(&GuardStatus::Active(permit.guard().clone()))
        {
            return Err(Fault::Ownership);
        }
        if observed.value().len() > permit.guard().max_bytes {
            return Err(Fault::Capacity);
        }
        let mut checkpoint = session.checkpoint_template()?;
        let bytes = marker(
            permit.guard(),
            checkpoint.image.generation,
            observed.digest(),
            &checkpoint.image.limits.decode,
        )?;
        checkpoint.image.permanent_loss.push(bytes.clone());
        let mut projected = pair.envelope.floor.clone();
        projected.guards.insert(
            permit.guard().id.clone(),
            GuardStatus::Loss {
                guard: permit.guard().clone(),
                loss: bytes.digest(),
                image_generation: checkpoint.image.generation,
            },
        );
        retained(&checkpoint.image, &projected)?;
        Ok((checkpoint, projected))
    })();
    let outcome = match result {
        Ok((checkpoint, projected)) => {
            persist::select(session, checkpoint, projected, DiskOperation::SettleIngress)
        }
        Err(fault) => PersistResult::Refused(fault),
    };
    if matches!(outcome, PersistResult::Committed { .. }) {
        if session.authority.settle(&permit).is_err() {
            return PersistResult::Unknown;
        }
    } else if session.authority.owns(&permit) {
        session.pending_ingress.insert(
            permit.guard().id.clone(),
            persist::PendingIngress::Loss { permit, observed },
        );
    }
    outcome
}
