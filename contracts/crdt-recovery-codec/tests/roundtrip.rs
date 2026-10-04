use glade_crdt_recovery_codec::*;
fn limits() -> DecodeLimits {
    DecodeLimits {
        bytes: 8192,
        items: 512,
        depth: 32,
        name_bytes: 128,
    }
}
fn identity() -> IdentityBinding {
    IdentityBinding {
        root: [1; 32],
        intent: Bytes::new(vec![2]),
        zone: "dev".into(),
        parameters: Bytes::new(vec![3]),
        declaration: Bytes::new(vec![4]),
        declaration_version: "1".into(),
        schema: Bytes::new(vec![5]),
        schema_identity: "text".into(),
        schema_version: "1".into(),
        corpus_identity: "glade".into(),
        corpus_version: "1".into(),
        corpus_digest: [6; 32],
        proof_profile: "ic3-proof-v1".into(),
        recovery_profile: "ic3-recovery-v1".into(),
        retention_policy: "retain".into(),
        rejoin_policy: "never-reset".into(),
        initial_chain_base: -1,
        migration_generation: u64::MAX,
    }
}
#[test]
fn identity_roundtrip_preserves_full_namespaces_and_unsigned_floor() {
    let value = identity();
    let encoded = encode(&value, &limits()).expect("versioned identity encoding");
    assert_eq!(
        decode::<IdentityBinding>(encoded.value(), &limits()),
        Ok(value)
    );
}
#[test]
fn inventory_empty_is_exact_observation_not_a_completeness_claim() {
    let value = Inventory {
        instance: Instance {
            descriptor: [7; 32],
            incarnation: "i".into(),
        },
        source: [8; 32],
        number: 9,
        entries: vec![],
        total_bytes: 0,
    };
    let bytes = encode(&value, &limits()).unwrap();
    assert_eq!(decode::<Inventory>(bytes.value(), &limits()), Ok(value));
}
#[test]
fn type_domain_cannot_substitute_plan_identity_for_receive_guard() {
    let plan = PlanKey {
        namespace: [1; 32],
        instance: Instance {
            descriptor: [2; 32],
            incarnation: "i".into(),
        },
        number: 1,
    };
    let bytes = encode(&plan, &limits()).unwrap();
    assert_eq!(
        decode::<GuardId>(bytes.value(), &limits()),
        Err(Error::Invalid)
    );
}

#[test]
fn proof_body_requires_exact_purpose_version_signature_shape_and_type() {
    let body = LocalResponseBody {
        instance: Instance {
            descriptor: [1; 32],
            incarnation: "i".into(),
        },
        node: [2; 32],
        requester: "alice".into(),
        writer_key: [3; 32],
        signed_challenge: [4; 32],
        operation: [5; 32],
        channel: [6; 32],
        session_nonce: [7; 32],
        challenge_nonce: [8; 32],
    };
    let mut record = SignedRecord {
        purpose: RESPONSE_DOMAIN.into(),
        version: 1,
        signer: [3; 32],
        body: encode_profile(&body, &limits()).unwrap(),
        signature: Bytes::new(vec![9; 64]),
    };
    assert_eq!(
        decode_signed_body::<LocalResponseBody>(&record, &limits()),
        Ok(body)
    );
    record.purpose = ADMISSION_DOMAIN.into();
    assert_eq!(
        decode_signed_body::<LocalResponseBody>(&record, &limits()),
        Err(Error::Invalid)
    );
    record.purpose = RESPONSE_DOMAIN.into();
    record.version = 2;
    assert_eq!(
        decode_signed_body::<LocalResponseBody>(&record, &limits()),
        Err(Error::Invalid)
    );
    record.version = 1;
    record.signature = Bytes::new(vec![0; 63]);
    assert_eq!(
        decode_signed_body::<LocalResponseBody>(&record, &limits()),
        Err(Error::Invalid)
    );
}
#[test]
fn accepted_signature_domains_are_full_and_zero_terminated() {
    assert_eq!(CERTIFICATE_DOMAIN, "glade/ic3/v1/origin-certificate\0");
    assert_eq!(PERMIT_DOMAIN, "glade/ic3/v1/offline-permit\0");
    assert_eq!(OPERATION_DOMAIN, "glade/ic3/v1/application-op\0");
    assert_eq!(POLICY_DOMAIN, "glade/ic3/v1/policy-cut\0");
    assert_eq!(CHALLENGE_DOMAIN, "glade/ic3/v1/local-session\0");
    assert_eq!(RESPONSE_DOMAIN, CHALLENGE_DOMAIN);
    assert_eq!(ADMISSION_DOMAIN, "glade/ic3/v1/admission\0");
}

#[test]
fn remote_profile_is_exact_canonical_integer_key_map_without_recovery_magic() {
    let body = SignedDescriptorBody {
        intent: Bytes::new(vec![1, 2]),
        creation_root: [3; 32],
    };
    let bytes = encode_profile(&body, &limits()).expect("remote profile map");
    assert!(matches!(
        glade_wire::cbor::try_decode(bytes.value()).unwrap(),
        glade_wire::cbor::Cbor::Map(_)
    ));
    assert_eq!(
        decode_profile::<SignedDescriptorBody>(bytes.value(), &limits()),
        Ok(body)
    );
}
