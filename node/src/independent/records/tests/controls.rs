use super::*;
#[test]
fn original_verified_continuation_rejects_mutated_facts_before_load() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("verified-mutation");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    let pair = session.pair.as_ref().unwrap();
    let transition =
        glade_crdt_admission_core::step(&pair.image.state, Event::SubmitLocal(Box::new(candidate)));
    let Effect::Verify(query) = transition
        .effects
        .into_iter()
        .find(|effect| matches!(effect, Effect::Verify(_)))
        .unwrap()
    else {
        panic!("genuine issued verify");
    };
    let reply = glade_crdt_evidence_api::EvidencePort::verify(
        &mut session.evidence,
        glade_crdt_evidence_api::BoundQuery {
            query,
            identity: f.identity.clone(),
        },
    );
    let mut next = glade_crdt_admission_core::step(
        &transition.state,
        Event::EvidenceResolved(Box::new(reply)),
    )
    .state;
    next.queries
        .values_mut()
        .next()
        .unwrap()
        .verified
        .as_mut()
        .unwrap()
        .signatures_valid = false;
    let image = session.checkpoint_template().unwrap().image;
    super::super::runtime::save(
        &mut session,
        next,
        image.recovery,
        super::super::DiskOperation::Checkpoint,
    )
    .unwrap();
    assert!(
        matches!(session.load(), LoadResult::Unavailable),
        "mutated stored trusted Facts must be refused before exposing State"
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn full_selected_state_refuses_omitted_terminal_request_and_mutated_projection() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("state-mutations");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    let original = session.pair.as_ref().unwrap().image.clone();
    for mutation in 0..9 {
        session.pair.as_mut().unwrap().image = original.clone();
        let image = &mut session.pair.as_mut().unwrap().image;
        match mutation {
            0 => {
                image.state.terminal_results.clear();
            }
            1 => {
                image.state.retired_storage_invocations.clear();
                image.recovery.invocation_count = 0;
            }
            2 => {
                image
                    .state
                    .instances
                    .get_mut(&f.query.instance)
                    .unwrap()
                    .used
                    .edit = 0;
            }
            3 => {
                image
                    .state
                    .instances
                    .get_mut(&f.query.instance)
                    .unwrap()
                    .heads
                    .clear();
            }
            4 => {
                image
                    .state
                    .instances
                    .get_mut(&f.query.instance)
                    .unwrap()
                    .accepted[0]
                    .facts
                    .signatures_valid = false;
            }
            5 => {
                image
                    .state
                    .instances
                    .get_mut(&f.query.instance)
                    .unwrap()
                    .accepted[0]
                    .receipt
                    .admission = [251; 32];
            }
            6 => {
                image.recovery.attempts[0].attempt.binding.batch = Bytes::new(vec![255]);
            }
            7 => {
                image.recovery.revisions.insert(f.query.instance.clone(), 2);
            }
            8 => {
                image
                    .state
                    .retired_storage_invocations
                    .retain(|_, invocation| matches!(invocation, StorageInvocation::Resolve(_)));
                image.recovery.invocation_count = image.state.retired_storage_invocations.len();
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(session.load(), LoadResult::Unavailable),
            "full selected original custody mutation {mutation}"
        );
    }
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn genuine_ordinary_policy_observation_cannot_consume_preallocated_critical_region() {
    let f = ic3_disk_fixture::disk_profile_with(|limits| {
        limits.image_bytes = 100_000;
        limits.storage.critical_per_attempt = 20_000;
        limits.storage.critical_bytes = 80_000;
    });
    let root = root("ordinary-critical-reserve");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    let original_floor = session.pair.as_ref().unwrap().floor_digest().unwrap();
    physical_allocation::assert_owned_slots(&root, 100_000);
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.generation = 1;
    policy.previous = Some(envelope(&f.policy).digest());
    policy.revoked = (0u64..900)
        .map(|number| {
            let mut digest = [249; 32];
            digest[..8].copy_from_slice(&number.to_be_bytes());
            digest
        })
        .collect();
    session.evidence.source.0.policy = signed(&ROOT, &policy);
    let result = session.issue_challenge(crate::independent::SessionRequest {
        requester: "alice".into(),
        writer_key: crate::signing::public_key(&WRITER),
        operation: f.query.operation.canonical().digest(),
        channel: [90; 32],
        session_nonce: [91; 32],
        challenge_nonce: [92; 32],
        window: Window {
            not_before_ms: 10,
            not_after_ms: 100,
        },
    });
    assert!(matches!(result, Err(Fault::Capacity)), "signed ordinary policy bytes must leave the physically allocated critical region available");
    assert_eq!(
        session.pair.as_ref().unwrap().floor_digest().unwrap(),
        original_floor,
        "ordinary refusal precedes floor intent and preserves usable image"
    );
    assert!(!session.pair.as_ref().unwrap().uncertain());
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn durable_current_expiry_floor_preserves_historical_original_receipt_and_projection() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("expired-current");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    let receipts = session
        .submit_local(candidate.clone(), [90; 32], [91; 32])
        .unwrap();
    let original = receipts
        .into_iter()
        .find_map(|outcome| match outcome {
            Outcome::AcceptedLocal(receipt) => Some(receipt),
            _ => None,
        })
        .unwrap();
    session.evidence.source.0.time = Some(TimeInterval {
        earliest_ms: 1001,
        latest_ms: 1002,
    });
    super::super::authentication::persist(&mut session).unwrap();
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("current expiry cannot invalidate historical selected receipts");
    };
    assert_eq!(loaded.authoritative_floor.issuance.time_floor_ms, 1001);
    assert_eq!(
        loaded.image.state.instances[&f.query.instance].accepted[0].receipt,
        original
    );
    assert_eq!(
        session.submit_local(candidate, [90; 32], [91; 32]),
        Err(Fault::Ownership)
    );
    drop(session);
    let digest = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut session = host.open_replica(request(&f, digest)).unwrap();
    let LoadResult::Validated(reopened) = session.load() else {
        panic!("retained expiry floor validates independently of original admission time");
    };
    assert_eq!(reopened.authoritative_floor.issuance.time_floor_ms, 1001);
    assert_eq!(
        reopened.image.state.instances[&f.query.instance].accepted[0].receipt,
        original
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

pub(super) struct ShiftingClock {
    pub(super) cut: TrustedCut,
    pub(super) calls: usize,
}
impl PolicyClock for ShiftingClock {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        self.calls += 1;
        if self.calls >= 7 {
            self.cut.time = Some(TimeInterval {
                earliest_ms: 80,
                latest_ms: 81,
            });
        }
        Ok(self.cut.clone())
    }
}
#[test]
fn unavailable_verification_retains_provider_observed_floor_before_reply() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("verification-observed-floor");
    let mut existing = evidence(&f);
    existing.config.certificates = vec![f.certificate.clone()];
    let provider = GenuineEvidence::new(
        existing.config,
        ShiftingClock {
            cut: existing.source.0,
            calls: 0,
        },
    )
    .unwrap();
    let mut host = DiskHost::new(provider, config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    let outcomes = session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    assert!(!outcomes
        .iter()
        .any(|outcome| matches!(outcome, Outcome::AcceptedLocal(_) | Outcome::ExactRetry(_))));
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("unchanged original query plus retained current observation");
    };
    assert_eq!(
        loaded.authoritative_floor.issuance.time_floor_ms, 80,
        "unavailable provider verification must not forget its freshly observed monotone floor"
    );
    assert_eq!(
        loaded.image.state.instances[&f.query.instance].policy_time,
        TimeInterval {
            earliest_ms: 80,
            latest_ms: 81
        }
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn structurally_valid_pending_next_with_bad_authenticated_state_refuses_before_floor_selection() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("pending-next-bad-state");
    let mut host = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    let mut next = session.checkpoint_template().unwrap().image;
    next.state
        .instances
        .get_mut(&f.query.instance)
        .unwrap()
        .accepted[0]
        .facts
        .signatures_valid = false;
    let floor = session.pair.as_ref().unwrap().envelope.floor.clone();
    let result =
        session
            .pair
            .as_mut()
            .unwrap()
            .transact(next, floor, DiskOperation::Checkpoint, |cut| {
                if cut
                    == crate::independent::disk::PairCut::Selection(
                        crate::independent::disk::floors::FloorCut::BeforeTemp,
                    )
                {
                    Err(Fault::Unavailable)
                } else {
                    Ok(())
                }
            });
    assert_eq!(result, Err(Fault::Unavailable));
    drop(session);
    let before = std::fs::read(root.join("trusted/replica.floor")).unwrap();
    let digest = Bytes::new(before.clone()).digest();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    assert!(
        host.open_replica(request(&f, digest)).is_err(),
        "full authenticated next must refuse"
    );
    assert!(
        std::fs::read(root.join("trusted/replica.floor")).unwrap() == before,
        "invalid-but-structural next must never be selected before full validation"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
mod physical_allocation {
    use std::os::unix::fs::MetadataExt;
    pub(super) fn assert_owned_slots(root: &std::path::Path, capacity: usize) {
        for slot in ["image.0", "image.1"] {
            let metadata = std::fs::metadata(root.join("data").join(slot)).unwrap();
            let header = b"glade.ic3.image\0\x01".len() + 8 + 32;
            assert_eq!(metadata.len(), u64::try_from(capacity + header).unwrap());
            assert!(
                metadata.blocks() * 512 >= u64::try_from(capacity).unwrap(),
                "owned slot backing blocks are allocated, not only a sparse logical length"
            );
        }
    }
}
#[cfg(not(unix))]
mod physical_allocation {
    pub(super) fn assert_owned_slots(_: &std::path::Path, _: usize) {
        panic!("physical profile is Unix-qualified only");
    }
}
