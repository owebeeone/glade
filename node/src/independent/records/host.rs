//! Both frozen host interfaces attach to the same authenticated instance custody.
use super::super::namespace::codec_fault;
use super::*;
pub(super) fn narrow<P: PolicyClock>(
    host: &mut DiskHost<P>,
    request: OpenRequest,
) -> Result<DiskSession<P>, Fault> {
    let evidence = host.evidence.as_ref().ok_or(Fault::Ownership)?;
    if request.limits != evidence.intent.limits.storage {
        return Err(Fault::BindingMismatch);
    }
    let (plan_namespace, expected_floor, layout) = if host.roots.provision {
        let namespace = encode(
            &(
                "glade/ic3/records-plan/v1".to_string(),
                (
                    evidence.config.descriptor.instance.clone(),
                    (
                        host.roots.registration,
                        (request.owner.clone(), request.invocation_namespace),
                    ),
                ),
            ),
            &evidence.config.limits,
        )
        .map_err(codec_fault)?
        .digest();
        (namespace, [0; 32], None)
    } else {
        let layout = DiskLayout::open(
            &host.roots.data,
            &host.roots.trusted_floor,
            host.roots.registration,
        )?;
        let floor = layout.read_floor(&evidence.config.limits)?;
        if floor.floor.owner != request.owner
            || floor.floor.issuance.invocation_namespace != request.invocation_namespace
            || floor.floor.issuance.plan_namespace == [0; 32]
        {
            return Err(Fault::BindingMismatch);
        }
        let digest = layout.floor_digest(evidence.config.limits.bytes)?;
        (floor.floor.issuance.plan_namespace, digest, Some(layout))
    };
    let full = ReplicaOpen {
        instance: evidence.config.descriptor.instance.clone(),
        descriptor: evidence.config.descriptor.clone(),
        identity: evidence.config.identity.clone(),
        owner: request.owner,
        plan_namespace,
        invocation_namespace: request.invocation_namespace,
        limits: evidence.intent.limits.clone(),
        expected_floor,
    };
    host.open_owned(full, layout)
}
