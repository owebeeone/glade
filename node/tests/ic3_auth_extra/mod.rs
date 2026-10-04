//! Additional genuine signature/scope/issuance controls; no trusted Facts injection.
use super::*;
#[test]
fn all_evidence_layers_reject_validly_signed_changed_bindings_or_wrong_keys() {
    let f = Fixture::new();
    for mutation in 0..7 {
        let mut q = f.query.clone();
        match mutation {
            0 => {
                let mut r: SignedRecord =
                    decode_profile(q.evidence.policy.value(), &limits()).unwrap();
                r.signature = Bytes::new(vec![0; 64]);
                q.evidence.policy = envelope(&r);
            }
            1 => {
                let mut w: PermitWrapper =
                    decode_profile(q.evidence.permit.value(), &limits()).unwrap();
                let mut b: PermitBody = decode_signed_body(&w.signed_permit, &limits()).unwrap();
                b.resource = "private:bob:text".into();
                w.signed_permit = signed(&ROOT, &b);
                q.evidence.permit = encode_profile(&w, &limits()).unwrap();
            }
            2 => {
                let mut w: PermitWrapper =
                    decode_profile(q.evidence.permit.value(), &limits()).unwrap();
                let mut b: OperationProofBody =
                    decode_signed_body(&w.writer_proof, &limits()).unwrap();
                b.requester = "bob".into();
                w.writer_proof = signed(&WRITER, &b);
                q.evidence.permit = encode_profile(&w, &limits()).unwrap();
            }
            3 => {
                let mut w: PermitWrapper =
                    decode_profile(q.evidence.permit.value(), &limits()).unwrap();
                w.writer_proof = signed(
                    &NODE,
                    &decode_signed_body::<OperationProofBody>(&w.writer_proof, &limits()).unwrap(),
                );
                q.evidence.permit = encode_profile(&w, &limits()).unwrap();
            }
            4 => {
                let mut b: OriginalAdmissionBody = decode_signed_body(
                    &decode_profile(q.evidence.admission.as_ref().unwrap().value(), &limits())
                        .unwrap(),
                    &limits(),
                )
                .unwrap();
                b.stable_id = [1; 32];
                q.evidence.admission = Some(envelope(&signed(&NODE, &b)));
            }
            5 => {
                let mut r: SignedRecord =
                    decode_profile(q.evidence.policy.value(), &limits()).unwrap();
                let mut bytes = r.body.value().to_vec();
                bytes.push(0);
                r.body = Bytes::new(bytes);
                r.signature = Bytes::new(
                    glade_node::signing::sign_in(&ROOT, POLICY_DOMAIN.as_bytes(), r.body.value())
                        .to_vec(),
                );
                q.evidence.policy = envelope(&r);
            }
            _ => {
                let mut op = q.operation.inner().clone();
                op.share = "foreign".into();
                q.operation = Operation::new(op);
                q.evidence = Fixture::proofs(
                    &q.instance,
                    &f.identity,
                    &q.descriptor,
                    &f.certificate,
                    &f.policy,
                    &q.operation,
                    Some((glade_node::signing::public_key(&NODE), &NODE)),
                );
            }
        }
        assert!(
            matches!(
                provider(&f)
                    .verify(BoundQuery {
                        query: q,
                        identity: f.identity.clone()
                    })
                    .verdict,
                Verdict::Invalid
            ),
            "genuine mutation{mutation}"
        );
    }
}
#[test]
fn distinct_fresh_origin_qualifies_but_reused_origin_epoch_or_issuance_refuses() {
    let f = Fixture::new();
    let mut p = provider(&f);
    let mut fresh: CertificateBody = decode_signed_body(&f.certificate, &limits()).unwrap();
    fresh.canonical_origin = "writer-e1".into();
    fresh.epoch = "e1".into();
    fresh.issuance = 1;
    let record = signed(&ROOT, &fresh);
    p.register_certificate(record.clone()).unwrap();
    let mut op = f.query.operation.inner().clone();
    op.origin = fresh.canonical_origin.clone();
    op.payload = br#"{"kind":"insert","atom_id":"b","after":"a","text":"B"}"#.to_vec();
    let mut q = f.query.clone();
    q.operation = Operation::new(op);
    q.evidence = Fixture::proofs(
        &q.instance,
        &f.identity,
        &q.descriptor,
        &record,
        &f.policy,
        &q.operation,
        Some((glade_node::signing::public_key(&NODE), &NODE)),
    );
    let Verdict::Verified(facts) = p
        .verify(BoundQuery {
            query: q,
            identity: f.identity.clone(),
        })
        .verdict
    else {
        panic!("genuine fresh E1")
    };
    assert!(facts.issuance_unique && facts.historical_policy_allowed);
    let mut reused = fresh.clone();
    reused.epoch = "different".into();
    assert_eq!(
        p.register_certificate(signed(&ROOT, &reused)),
        Err(Fault::Integrity)
    );
    fresh.canonical_origin = "third".into();
    assert_eq!(
        p.register_certificate(signed(&ROOT, &fresh)),
        Err(Fault::Integrity)
    );
}
#[test]
fn bare_signed_rival_does_not_convict_but_original_signed_admission_qualifies() {
    let f = Fixture::new();
    let mut rival = f.query.clone();
    let mut op = rival.operation.inner().clone();
    op.payload = br#"{"kind":"insert","atom_id":"rival","after":null,"text":"D"}"#.to_vec();
    rival.operation = Operation::new(op);
    rival.evidence = Fixture::proofs(
        &rival.instance,
        &f.identity,
        &rival.descriptor,
        &f.certificate,
        &f.policy,
        &rival.operation,
        None,
    );
    let Verdict::Verified(bare) = provider(&f)
        .verify(BoundQuery {
            query: rival.clone(),
            identity: f.identity.clone(),
        })
        .verdict
    else {
        panic!("authentic bare rival")
    };
    assert!(bare.signatures_valid && bare.payload_valid);
    assert!(!bare.historical_policy_allowed && !bare.historical_admitter_authorized);
    rival.evidence = Fixture::proofs(
        &rival.instance,
        &f.identity,
        &rival.descriptor,
        &f.certificate,
        &f.policy,
        &rival.operation,
        Some((glade_node::signing::public_key(&NODE), &NODE)),
    );
    let Verdict::Verified(qualified) = provider(&f)
        .verify(BoundQuery {
            query: rival,
            identity: f.identity.clone(),
        })
        .verdict
    else {
        panic!("genuine qualified rival")
    };
    assert!(qualified.historical_policy_allowed && qualified.historical_admitter_authorized);
}
#[test]
fn strict_ed25519_refuses_small_order_writer_and_changed_domain() {
    let mut key = [0; 32];
    key[0] = 1;
    let mut signature = [0; 64];
    signature[0] = 1;
    assert_eq!(
        glade_node::signing::verify_in(&key, OPERATION_DOMAIN.as_bytes(), b"body", &signature),
        glade_signer_api::SignatureStatus::Invalid
    );
    let signature = glade_node::signing::sign_in(&WRITER, OPERATION_DOMAIN.as_bytes(), b"body");
    let key = glade_node::signing::public_key(&WRITER);
    assert_eq!(
        glade_node::signing::verify_in(&key, OPERATION_DOMAIN.as_bytes(), b"body", &signature),
        glade_signer_api::SignatureStatus::Valid
    );
    assert_eq!(
        glade_node::signing::verify_in(&key, PERMIT_DOMAIN.as_bytes(), b"body", &signature),
        glade_signer_api::SignatureStatus::Invalid
    );
}
