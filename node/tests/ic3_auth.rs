//! IC3-B1 genuine provider qualification; signed records, not harness Facts.
mod ic3_auth_extra;
mod ic3_auth_fixture;
use glade_crdt_evidence_api::*;
use glade_crdt_recovery_codec::*;
use glade_node::independent::*;
#[derive(Clone)]
struct Source(TrustedCut);
impl PolicyClock for Source {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        Ok(self.0.clone())
    }
}
fn provider(f: &Fixture) -> GenuineEvidence<Source> {
    GenuineEvidence::new(
        EvidenceConfig {
            descriptor: f.descriptor.clone(),
            identity: f.identity.clone(),
            trusted_root: glade_node::signing::public_key(&ROOT),
            node_seed: NODE,
            limits: limits(),
            certificates: vec![f.certificate.clone()],
            session_capacity: 16,
        },
        Source(TrustedCut {
            policy: f.policy.clone(),
            time: Some(TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            }),
            time_floor_ms: 10,
        }),
    )
    .unwrap()
}
use ic3_auth_fixture::*;
#[test]
fn genuine_historical_admission_verifies_exact_scoped_signed_records() {
    let f = Fixture::new();
    let mut provider = provider(&f);
    let reply = provider.verify(BoundQuery {
        query: f.query.clone(),
        identity: f.identity,
    });
    assert_eq!(reply.query, f.query);
    let Verdict::Verified(facts) = reply.verdict else {
        panic!("genuine historical admission must verify")
    };
    assert!(
        facts.signatures_valid
            && facts.descriptor_valid
            && facts.payload_valid
            && facts.issuance_unique
    );
    assert!(facts.historical_admitter_authorized && facts.historical_policy_allowed);
    assert_eq!(
        facts.historical_time,
        Some(TimeInterval {
            earliest_ms: 20,
            latest_ms: 21
        })
    );
}
#[test]
fn provider_observation_is_genuine_current_signed_policy_and_owned_clock() {
    let f = Fixture::new();
    let mut provider = provider(&f);
    let observation = provider
        .observe(&f.descriptor.instance, &f.identity)
        .expect("configured genuine policy/time provider");
    assert_eq!(observation.policy, envelope(&f.policy));
    assert_eq!(observation.node, glade_node::signing::public_key(&NODE));
    assert_eq!(
        observation.time,
        TimeInterval {
            earliest_ms: 20,
            latest_ms: 21
        }
    );
    assert!(observation.available && observation.allowed);
}
#[test]
fn bad_signature_wrong_domain_or_altered_namespace_are_invalid() {
    let f = Fixture::new();
    for mutation in 0..5 {
        let mut q = f.query.clone();
        let mut identity = f.identity.clone();
        if mutation < 2 {
            let mut c: SignedRecord =
                decode_profile(q.evidence.certificate.value(), &limits()).unwrap();
            if mutation == 0 {
                let mut signature = c.signature.value().to_vec();
                signature[0] ^= 1;
                c.signature = Bytes::new(signature);
            } else {
                c.purpose = POLICY_DOMAIN.into();
            }
            q.evidence.certificate = envelope(&c);
        } else if mutation == 2 {
            identity.declaration_version = "other".into();
        } else if mutation == 3 {
            identity.schema_identity = "other".into();
        } else {
            identity.schema_version = "other".into();
        }
        let reply = provider(&f).verify(BoundQuery { query: q, identity });
        assert_eq!(reply.verdict, Verdict::Invalid, "mutation{mutation}");
    }
}
#[test]
fn signed_invalid_payload_is_authenticated_security_data_not_qualified_admission() {
    let f = Fixture::new();
    let mut q = f.query.clone();
    let mut op = q.operation.inner().clone();
    op.payload = b"not-json".to_vec();
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
    let reply = provider(&f).verify(BoundQuery {
        query: q,
        identity: f.identity,
    });
    let Verdict::Verified(facts) = reply.verdict else {
        panic!("authentic signed invalid payload retains its verifier classification")
    };
    assert!(facts.signatures_valid);
    assert!(!facts.payload_valid);
}
fn local(f: &Fixture, provider: &mut GenuineEvidence<Source>, nonce: u8) -> Query {
    let mut query = f.query.clone();
    query.evidence.admission = None;
    query.mode = Mode::Local {
        current_policy: envelope(&f.policy).digest(),
        time: TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        },
    };
    let challenge = provider
        .issue_challenge(SessionRequest {
            requester: "alice".into(),
            writer_key: glade_node::signing::public_key(&WRITER),
            operation: query.operation.canonical().digest(),
            channel: [9; 32],
            session_nonce: [8; 32],
            challenge_nonce: [nonce; 32],
            window: Window {
                not_before_ms: 10,
                not_after_ms: 100,
            },
        })
        .unwrap();
    let body: LocalChallengeBody = decode_signed_body(&challenge, &limits()).unwrap();
    let response = signed(
        &WRITER,
        &LocalResponseBody {
            instance: query.instance.clone(),
            node: body.node,
            requester: body.requester,
            writer_key: body.writer_key,
            signed_challenge: envelope(&challenge).digest(),
            operation: body.operation,
            channel: body.channel,
            session_nonce: body.session_nonce,
            challenge_nonce: body.challenge_nonce,
        },
    );
    provider.authenticate(response, [9; 32], [8; 32]).unwrap();
    query
}
#[test]
fn local_possession_seal_and_foreign_or_replayed_session_controls() {
    let f = Fixture::new();
    let mut p = provider(&f);
    let mut missing = f.query.clone();
    missing.evidence.admission = None;
    missing.mode = Mode::Local {
        current_policy: envelope(&f.policy).digest(),
        time: TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        },
    };
    assert_eq!(
        p.verify(BoundQuery {
            query: missing.clone(),
            identity: f.identity.clone()
        })
        .verdict,
        Verdict::Invalid
    );
    let query = local(&f, &mut p, 1);
    let reply = p.verify(BoundQuery {
        query: query.clone(),
        identity: f.identity.clone(),
    });
    let Verdict::Verified(facts) = reply.verdict else {
        panic!("genuine possessed local writer")
    };
    assert!(facts.current_allowed);
    let mut seal = query.clone();
    seal.id = 2;
    seal.mode = Mode::AdmissionSeal;
    seal.validated_under = Some(query.mode.clone());
    seal.verified_facts = Some(facts);
    let sealed = p.seal(BoundQuery {
        query: seal.clone(),
        identity: f.identity.clone(),
    });
    assert_eq!(sealed.query, seal);
    let Verdict::Sealed(bytes) = sealed.verdict else {
        panic!("genuine conditional seal")
    };
    let record: SignedRecord = decode_profile(bytes.value(), &limits()).unwrap();
    let body: OriginalAdmissionBody = decode_signed_body(&record, &limits()).unwrap();
    assert_eq!(body.operation, query.operation.canonical().digest());
    assert_eq!(body.stable_id, body.operation);
    assert_eq!(
        body.time,
        TimeInterval {
            earliest_ms: 20,
            latest_ms: 21
        }
    );
    assert_eq!(body.policy, envelope(&f.policy).digest());
    let custody = p.session_custody();
    let response = custody[0].response.clone().unwrap();
    assert!(p.authenticate(response.clone(), [9; 32], [8; 32]).is_err());
    let mut other = provider(&f);
    assert!(other.authenticate(response, [9; 32], [8; 32]).is_err());
    let mut fake = seal;
    fake.verified_facts.as_mut().unwrap().payload_valid = false;
    assert_eq!(
        p.seal(BoundQuery {
            query: fake,
            identity: f.identity.clone()
        })
        .verdict,
        Verdict::Invalid
    );
}
#[test]
fn uncertain_expired_or_revoked_current_cut_does_not_change_historical_validity() {
    let f = Fixture::new();
    let mut p = provider(&f);
    let query = local(&f, &mut p, 2);
    for time in [
        TimeInterval {
            earliest_ms: 100,
            latest_ms: 101,
        },
        TimeInterval {
            earliest_ms: 101,
            latest_ms: 102,
        },
    ] {
        let mut p = provider(&f);
        let mut query = local(&f, &mut p, 2);
        p.source_mut().0.time = Some(time.clone());
        query.mode = Mode::Local {
            current_policy: envelope(&f.policy).digest(),
            time,
        };
        let Verdict::Verified(facts) = p
            .verify(BoundQuery {
                query: query.clone(),
                identity: f.identity.clone(),
            })
            .verdict
        else {
            panic!("known denied current cut")
        };
        assert!(!facts.current_allowed);
    }
    p.source_mut().0.time = None;
    assert_eq!(
        p.verify(BoundQuery {
            query: query.clone(),
            identity: f.identity.clone()
        })
        .verdict,
        Verdict::Unavailable
    );
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.generation = 1;
    policy.previous = Some(envelope(&f.policy).digest());
    policy
        .revoked
        .push(glade_node::signing::public_key(&WRITER));
    policy.admitting_nodes.clear();
    p.source_mut().0.policy = signed(&ROOT, &policy);
    p.source_mut().0.time = Some(TimeInterval {
        earliest_ms: 200,
        latest_ms: 201,
    });
    assert!(
        !p.observe(&f.descriptor.instance, &f.identity)
            .unwrap()
            .allowed
    );
    let Verdict::Verified(facts) = p
        .verify(BoundQuery {
            query: f.query.clone(),
            identity: f.identity.clone(),
        })
        .verdict
    else {
        panic!("historical validity survives current changes")
    };
    assert!(facts.historical_policy_allowed && facts.historical_admitter_authorized);
}
#[test]
fn trusted_source_cannot_regress_observed_clock_or_policy_floor() {
    let f = Fixture::new();
    let mut p = provider(&f);
    p.observe(&f.descriptor.instance, &f.identity).unwrap();
    p.source_mut().0.time = Some(TimeInterval {
        earliest_ms: 19,
        latest_ms: 20,
    });
    assert_eq!(
        p.observe(&f.descriptor.instance, &f.identity),
        Err(Fault::Unavailable)
    );
    let mut p = provider(&f);
    p.observe(&f.descriptor.instance, &f.identity).unwrap();
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.generation = 1;
    policy.previous = Some(envelope(&f.policy).digest());
    p.source_mut().0.policy = signed(&ROOT, &policy);
    p.observe(&f.descriptor.instance, &f.identity).unwrap();
    p.source_mut().0.policy = f.policy.clone();
    assert_eq!(
        p.observe(&f.descriptor.instance, &f.identity),
        Err(Fault::Unavailable)
    );
}
#[test]
fn every_local_verification_requires_new_possession_not_operation_cache_reuse() {
    let f = Fixture::new();
    let mut p = provider(&f);
    let query = local(&f, &mut p, 3);
    let bound = BoundQuery {
        query,
        identity: f.identity.clone(),
    };
    assert!(matches!(
        p.verify(bound.clone()).verdict,
        Verdict::Verified(_)
    ));
    assert_eq!(
        p.verify(bound).verdict,
        Verdict::Invalid,
        "a separate local attempt cannot reuse the consumed possession gate"
    );
}
#[test]
fn root_signature_does_not_authorize_an_unsupported_merge_corpus() {
    let f = Fixture::new();
    let mut identity = f.identity.clone();
    let mut intent: CreationIntent = decode_profile(identity.intent.value(), &limits()).unwrap();
    intent.corpus_digest = [99; 32];
    identity.corpus_digest = intent.corpus_digest;
    identity.intent = encode_profile(&intent, &limits()).unwrap();
    let mut descriptor = f.descriptor.clone();
    descriptor.creation_root = identity.intent.digest();
    descriptor.canonical = envelope(&signed(
        &ROOT,
        &SignedDescriptorBody {
            intent: identity.intent.clone(),
            creation_root: descriptor.creation_root,
        },
    ));
    descriptor.instance.descriptor = descriptor.canonical.digest();
    let candidate = GenuineEvidence::new(
        EvidenceConfig {
            descriptor,
            identity,
            trusted_root: glade_node::signing::public_key(&ROOT),
            node_seed: NODE,
            limits: limits(),
            certificates: vec![],
            session_capacity: 16,
        },
        Source(TrustedCut {
            policy: f.policy,
            time: Some(TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            }),
            time_floor_ms: 10,
        }),
    );
    assert!(
        matches!(candidate, Err(Fault::Integrity)),
        "unsupported signed corpus must refuse"
    );
}

#[test]
fn released_payload_duplicate_properties_remain_supported() {
    let f = Fixture::new();
    {
        let payload = br#"{"atom_id":42,"atom_id":"a","kind":"delete"}"#.as_slice();
        let mut q = f.query.clone();
        let mut op = q.operation.inner().clone();
        op.payload = payload.to_vec();
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
        let Verdict::Verified(facts) = provider(&f)
            .verify(BoundQuery {
                query: q,
                identity: f.identity.clone(),
            })
            .verdict
        else {
            panic!("authenticated profile edit")
        };
        assert!(
            facts.payload_valid,
            "released JSON.parse duplicate-property behavior must be retained"
        );
    }
}
#[test]
fn genuine_provider_matches_shared_released_text_syntax_rows() {
    let f = Fixture::new();
    for row in include_str!("fixtures/ic3_text_payload_rows.tsv").lines() {
        let mut columns = row.split('\t');
        let expected = columns.next().unwrap() == "1";
        let hex = columns.next().unwrap();
        let name = columns.next().unwrap();
        let payload: Vec<_> = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect();
        let mut q = f.query.clone();
        let mut op = q.operation.inner().clone();
        op.payload = payload;
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
        let Verdict::Verified(facts) = provider(&f)
            .verify(BoundQuery {
                query: q,
                identity: f.identity.clone(),
            })
            .verdict
        else {
            panic!("signed proof row {name}")
        };
        assert_eq!(facts.payload_valid, expected, "{name}");
    }
}
#[test]
fn genuine_overlimit_operation_is_refused_before_payload_interpretation() {
    let f = Fixture::new();
    let mut q = f.query.clone();
    let mut op = q.operation.inner().clone();
    let text = "x".repeat(9000);
    op.payload =
        format!("{{\"kind\":\"insert\",\"atom_id\":\"a\",\"after\":null,\"text\":\"{text}\"}}")
            .into_bytes();
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
    assert!(matches!(
        provider(&f)
            .verify(BoundQuery {
                query: q,
                identity: f.identity
            })
            .verdict,
        Verdict::Unavailable
    ));
}
