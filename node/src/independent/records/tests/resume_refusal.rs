//! A retained peer bundle whose offer is refused never refuses the next start:
//! resume leaves its exact obligation unresolved and opens.
use super::*;
async fn retain(
    session: &mut DiskSession<Source>,
    source: Digest,
    channel: Digest,
    role: &str,
    raw: Bytes,
    expected: Vec<RecordRef>,
) {
    let (cut, _) = session.node_observe().unwrap();
    let guard = session
        .node_guard(&cut, source, channel, role, 1000)
        .unwrap();
    let permit = session.begin_ingress(guard).unwrap();
    let bytes = raw.clone();
    let (permit, result) = session
        .receive_ingress(permit, || async move { Ok(bytes) })
        .unwrap()
        .await;
    assert_eq!(result, Ok(raw.clone()));
    session.finish_node_input(permit, raw, expected).unwrap();
}
#[tokio::test]
async fn refused_retained_peer_bundle_leaves_its_obligation_missing_and_the_node_starts() {
    let mut f = ic3_disk_fixture::disk_profile();
    let holder_seed = [34; 32];
    let holder = crate::signing::public_key(&holder_seed);
    let nodes = vec![crate::signing::public_key(&NODE), holder];
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.admitting_nodes = nodes.clone();
    policy.replica_holders = nodes.clone();
    f.policy = signed(&ROOT, &policy);
    let wrapper: PermitWrapper =
        decode_profile(f.query.evidence.permit.value(), &limits()).unwrap();
    let mut permit: PermitBody = decode_signed_body(&wrapper.signed_permit, &limits()).unwrap();
    permit.admitting_nodes = nodes;
    permit.policy = envelope(&f.policy).digest();
    let signed_permit = signed(&ROOT, &permit);
    let mut proof: OperationProofBody =
        decode_signed_body(&wrapper.writer_proof, &limits()).unwrap();
    proof.signed_permit = envelope(&signed_permit).digest();
    f.query.evidence.policy = envelope(&f.policy);
    f.query.evidence.admission = None;
    f.query.evidence.permit = encode_profile(
        &PermitWrapper {
            signed_permit,
            writer_proof: signed(&WRITER, &proof),
        },
        &limits(),
    )
    .unwrap();
    let roots = [root("c-resume-victim"), root("c-resume-holder")];
    let mut victim_host = DiskHost::new(evidence(&f), config(&roots[0], true));
    let mut holder_evidence = evidence(&f);
    holder_evidence.config.node_seed = holder_seed;
    let mut holder_config = config(&roots[1], true);
    holder_config.registration = [77; 32];
    let mut holder_host = DiskHost::new(holder_evidence, holder_config);
    let mut victim = victim_host.open_replica(request(&f, [0; 32])).unwrap();
    let mut holder_request = request(&f, [0; 32]);
    holder_request.owner.store = StoreIncarnation([74; 32]);
    holder_request.plan_namespace = [75; 32];
    holder_request.invocation_namespace = [76; 32];
    let mut holding = holder_host.open_replica(holder_request).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut holding, 92);
    holding
        .submit_local(
            Candidate {
                instance: f.query.instance.clone(),
                operation: f.query.operation.clone(),
                evidence: f.query.evidence.clone(),
            },
            [90; 32],
            [91; 32],
        )
        .unwrap();
    // A genuine accepted record whose receipt names another storage class, as
    // a holder of a later build could serve it: advertised and served alike.
    let mut served = holding.node_history().unwrap().remove(0);
    served.receipt.as_mut().unwrap().storage = StorageClass::VolatileTest;
    let inventory = holding
        .node_inventory(std::slice::from_ref(&served))
        .unwrap();
    let channel = [95; 32];
    let observed = holding.node_observe().unwrap();
    let header = holding
        .node_inventory_header(
            &observed,
            &inventory,
            crate::signing::public_key(&NODE),
            channel,
        )
        .unwrap();
    let raw = encode(&(header, inventory.clone()), &limits()).unwrap();
    retain(
        &mut victim,
        holder,
        channel,
        "inventory",
        raw,
        inventory.entries.clone(),
    )
    .await;
    let raw = encode_profile(&served, &limits()).unwrap();
    let reference = super::super::node_ingress::bundle_ref(&served, &raw);
    retain(
        &mut victim,
        holder,
        channel,
        "bundle",
        raw,
        vec![reference.clone()],
    )
    .await;
    assert!(victim
        .node_offer(served, &reference, &inventory, holder, channel)
        .is_err());
    assert_eq!(
        ReplicaRecoverySession::close(&mut victim),
        CloseResult::Closed
    );
    drop(victim);
    let digest =
        Bytes::new(std::fs::read(roots[0].join("trusted/replica.floor")).unwrap()).digest();
    crate::test_support::children::settled();
    let mut host = DiskHost::new(evidence(&f), config(&roots[0], false));
    let mut restored = host.open_replica(request(&f, digest)).unwrap();
    let resumed = restored.resume_node_inputs();
    let LoadResult::Validated(loaded) = restored.load() else {
        panic!("validated restart")
    };
    let classified: Vec<_> = loaded
        .image
        .obligations
        .iter()
        .map(|obligation| {
            (
                obligation.expected == reference,
                obligation.classification.clone(),
            )
        })
        .collect();
    assert!(
        resumed.is_ok()
            && classified.contains(&(true, Classification::Missing))
            && loaded.image.state.instances[&f.query.instance]
                .accepted
                .is_empty(),
        "a refused retained bundle cannot refuse the start: resume {resumed:?}, \
         obligations {classified:?}"
    );
    assert_eq!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    );
    drop(restored);
    drop(holding);
    for root in roots {
        std::fs::remove_dir_all(root).unwrap();
    }
}
