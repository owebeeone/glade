//! Coupled selection controls; a by-value consumed capability is never reconstructed.
use super::*;
pub(super) enum PendingIngress {
    Settle {
        permit: IngressPermit,
        observation: Box<Observation>,
        checkpoint: Box<Checkpoint>,
    },
    Abandon {
        permit: IngressPermit,
    },
    Loss {
        permit: IngressPermit,
        observed: Bytes,
    },
}

fn same_kernel(old: &ReplicaImage, next: &ReplicaImage) -> Result<(), Fault> {
    if next.state != old.state
        || next.recovery != old.recovery
        || next.origins != old.origins
        || next.sessions != old.sessions
        || next.permanent_loss != old.permanent_loss
    {
        return Err(Fault::BindingMismatch);
    }
    Ok(())
}
fn binding(pair: &PairStore, request: &Checkpoint) -> Result<(), Fault> {
    pair.owned()?;
    if request.expected_floor != pair.floor_digest()?
        || request.expected_generation != pair.image.generation
        || request.expected_digest != pair.envelope.floor.selected.digest
    {
        return Err(Fault::BindingMismatch);
    }
    same_kernel(&pair.image, &request.image)
}
pub(super) fn select<P: PolicyClock>(
    session: &mut DiskSession<P>,
    request: Checkpoint,
    projected: ReplicaFloor,
    kind: DiskOperation,
) -> PersistResult {
    let Some(pair) = session.pair.as_mut() else {
        return PersistResult::Refused(Fault::Ownership);
    };
    if request.image.permanent_loss == pair.image.permanent_loss {
        if let Err(fault) = super::reserves::ordinary(&request.image) {
            return PersistResult::Refused(fault);
        }
    }
    if let Err(fault) = super::reserves::critical(&request.image, &projected) {
        return PersistResult::Refused(fault);
    }
    match pair.transact(request.image, projected, kind, |cut| {
        (session.observe_cut)(kind, cut)
    }) {
        Ok(()) => PersistResult::Committed {
            generation: pair.image.generation,
            digest: pair.envelope.floor.selected.digest,
        },
        Err(fault) => {
            if pair.uncertain() {
                PersistResult::Unknown
            } else {
                PersistResult::Refused(fault)
            }
        }
    }
}
pub(super) fn plain<P: PolicyClock>(
    session: &mut DiskSession<P>,
    request: Checkpoint,
) -> PersistResult {
    let result = (|| {
        let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
        binding(pair, &request)?;
        if request.image.observations != pair.image.observations
            || request.image.obligations != pair.image.obligations
            || request.image.watermark != pair.image.watermark
        {
            return Err(Fault::BindingMismatch);
        }
        Ok(pair.envelope.floor.clone())
    })();
    match result {
        Ok(projected) => select(session, request, projected, DiskOperation::Checkpoint),
        Err(fault) => PersistResult::Refused(fault),
    }
}
pub(super) fn settle<P: PolicyClock>(
    session: &mut DiskSession<P>,
    permit: IngressPermit,
    observation: Observation,
    request: Checkpoint,
) -> PersistResult {
    let result = (|| {
        if !session.authority.owns(&permit) {
            return Err(Fault::Ownership);
        }
        let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
        binding(pair, &request)?;
        if pair.envelope.floor.guards.get(&permit.guard().id)
            != Some(&GuardStatus::Active(permit.guard().clone()))
        {
            return Err(Fault::Ownership);
        }
        observations::checked(&session.evidence, permit.guard(), &observation)?;
        let mut expected = pair.image.observations.clone();
        expected.push(observation.clone());
        let mut obligations = pair.image.obligations.clone();
        for reference in &observation.expected {
            obligations.push(Obligation {
                observation: observation.id,
                expected: reference.clone(),
                classification: Classification::Missing,
            });
        }
        if request.image.observations != expected
            || request.image.obligations != obligations
            || request.image.watermark != observation.id
        {
            return Err(Fault::BindingMismatch);
        }
        let mut projected = pair.envelope.floor.clone();
        projected.guards.insert(
            permit.guard().id.clone(),
            GuardStatus::Settled {
                guard: permit.guard().clone(),
                observation: observation.bytes.digest(),
                image_generation: request.image.generation,
            },
        );
        observations::retained(
            &session.evidence,
            &session.request,
            &request.image,
            &projected,
        )?;
        session.authority.can_settle(&permit)?;
        Ok(projected)
    })();
    let retained_checkpoint = request.clone();
    let outcome = match result {
        Ok(projected) => select(session, request, projected, DiskOperation::SettleIngress),
        Err(fault) => PersistResult::Refused(fault),
    };
    if matches!(outcome, PersistResult::Committed { .. }) {
        if session.authority.settle(&permit).is_err() {
            return PersistResult::Unknown;
        }
    } else if session.authority.owns(&permit) {
        session.pending_ingress.insert(
            permit.guard().id.clone(),
            PendingIngress::Settle {
                permit,
                observation: Box::new(observation),
                checkpoint: Box::new(retained_checkpoint),
            },
        );
    }
    outcome
}
pub(super) fn abandon<P: PolicyClock>(
    session: &mut DiskSession<P>,
    permit: IngressPermit,
) -> PersistResult {
    let result = (|| {
        if !session.authority.owns(&permit) || permit.was_started() {
            return Err(Fault::Ownership);
        }
        let pair = session.pair.as_ref().ok_or(Fault::Ownership)?;
        pair.owned()?;
        if pair.envelope.floor.guards.get(&permit.guard().id)
            != Some(&GuardStatus::Active(permit.guard().clone()))
        {
            return Err(Fault::Ownership);
        }
        let request = session.checkpoint_template()?;
        let mut projected = pair.envelope.floor.clone();
        projected.guards.insert(
            permit.guard().id.clone(),
            GuardStatus::Abandoned {
                guard: permit.guard().clone(),
                floor_sequence: projected.sequence.checked_add(2).ok_or(Fault::Capacity)?,
            },
        );
        session.authority.can_abandon(&permit)?;
        Ok((request, projected))
    })();
    let outcome = match result {
        Ok((request, projected)) => {
            select(session, request, projected, DiskOperation::AbandonIngress)
        }
        Err(fault) => PersistResult::Refused(fault),
    };
    if matches!(outcome, PersistResult::Committed { .. }) {
        if session.authority.abandon(&permit).is_err() {
            return PersistResult::Unknown;
        }
    } else if session.authority.owns(&permit) {
        session.pending_ingress.insert(
            permit.guard().id.clone(),
            PendingIngress::Abandon { permit },
        );
    }
    outcome
}

pub(super) fn retry<P: PolicyClock>(
    session: &mut DiskSession<P>,
    guard: &GuardId,
) -> PersistResult {
    let Some(pending) = session.pending_ingress.remove(guard) else {
        return PersistResult::Refused(Fault::Ownership);
    };
    match pending {
        PendingIngress::Settle {
            permit,
            observation,
            checkpoint,
        } => settle(session, permit, *observation, *checkpoint),
        PendingIngress::Abandon { permit } => abandon(session, permit),
        PendingIngress::Loss { permit, observed } => super::loss::retain(session, permit, observed),
    }
}

pub(super) fn observation<P: PolicyClock>(
    session: &mut DiskSession<P>,
    observation: Observation,
    checkpoint: Checkpoint,
) -> PersistResult {
    if let Some(pending) = session.pending_ingress.remove(&observation.guard) {
        match pending {
            PendingIngress::Settle {
                permit,
                observation: retained,
                checkpoint: original,
            } => {
                if *retained != observation {
                    session.pending_ingress.insert(
                        retained.guard.clone(),
                        PendingIngress::Settle {
                            permit,
                            observation: retained,
                            checkpoint: original,
                        },
                    );
                    return PersistResult::Refused(Fault::BindingMismatch);
                }
                return settle(session, permit, *retained, checkpoint);
            }
            other => {
                session
                    .pending_ingress
                    .insert(observation.guard.clone(), other);
                return PersistResult::Refused(Fault::Ownership);
            }
        }
    }
    let Some(pair) = session.pair.as_ref() else {
        return PersistResult::Refused(Fault::Ownership);
    };
    if !pair.image.observations.contains(&observation) {
        return PersistResult::Refused(Fault::Ownership);
    }
    plain(session, checkpoint)
}
