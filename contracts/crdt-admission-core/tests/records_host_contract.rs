//! Behavioral RED consumers compiled against the public crate, never private modules.
mod support;
use glade_crdt_admission_core::*;
use glade_wire::generated::Head;
use support::*;

#[test]
fn icd002_partitioned_admission_commits_without_a_holder() {
    let d = descriptor("x");
    let c = candidate(&d, "writer-e0", 0, None, vec![], insert("a", None, "A"));
    let result = drive(
        initial(d),
        Event::SubmitLocal(Box::new(c.clone())),
        facts(&c),
    );
    assert!(
        result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_)))),
        "ICD-002 valid isolated edit requires local receipt"
    );
}
#[test]
fn icd005_evidence_reply_binds_mode_bytes_scope_and_candidate() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let started = step(&initial(d.clone()), Event::SubmitLocal(Box::new(c.clone())));
    let query = started
        .effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Verify(q) => Some(q),
            _ => None,
        })
        .expect("ICD-005 evidence query required");
    let mut variants = vec![query.clone(); 6];
    let mut altered = c.operation.inner().clone();
    altered.payload.push(9);
    variants[0].operation = Operation::new(altered);
    variants[1].instance.incarnation = "foreign".into();
    variants[2].evidence.permit = Bytes::new(b"other permit".to_vec());
    variants[3].mode = Mode::Historical;
    variants[4].descriptor.share = "foreign".into();
    variants[5].validated_under = Some(Mode::Historical);
    for wrong in variants {
        let result = step(
            &started.state,
            Event::EvidenceResolved(Box::new(EvidenceReply {
                query: wrong,
                verdict: Verdict::Verified(Box::new(facts(&c))),
            })),
        );
        assert!(
            result
                .effects
                .contains(&Effect::Report(Outcome::CallbackMismatch)),
            "ICD-005 mismatched evidence must be identified"
        );
    }
}
#[test]
fn icd006_revoked_expired_unadmitted_rival_preserves_ad_and_receipts() {
    let d = descriptor("x");
    let a = candidate(
        &d,
        "writer-e0",
        0,
        None,
        vec![],
        insert("atom-a", None, "A"),
    );
    let dependent = candidate(
        &d,
        "dependent",
        0,
        None,
        vec![Head {
            origin: "writer-e0".into(),
            seq: 0,
            hash: None,
        }],
        insert("atom-d", Some("atom-a"), "D"),
    );
    for reason in ["unadmitted", "historically_denied", "expired"] {
        for reverse in [false, true] {
            let mut rival = candidate(
                &d,
                "writer-e0",
                0,
                None,
                vec![],
                insert("atom-x", None, "X"),
            );
            let mut denied = facts(&rival);
            match reason {
                "unadmitted" => {
                    rival.evidence.admission = None;
                }
                "historically_denied" => {
                    denied.historical_policy_allowed = false;
                }
                _ => {
                    denied.permit_window.not_after_ms = 10;
                }
            }
            denied.current_allowed = false;
            let mut state = initial(d.clone());
            if reverse {
                state = drive(
                    state,
                    Event::OfferReplica(Box::new(rival.clone())),
                    denied.clone(),
                )
                .state;
            }
            for c in [a.clone(), dependent.clone()] {
                state = drive(state, Event::OfferReplica(Box::new(c.clone())), facts(&c)).state;
            }
            if !reverse {
                state = drive(state, Event::OfferReplica(Box::new(rival.clone())), denied).state;
            }
            let read = cut(&state, d.instance.clone());
            assert_eq!(
                std::collections::BTreeSet::from_iter(read.eligible),
                std::collections::BTreeSet::from([
                    a.operation.canonical().digest(),
                    dependent.operation.canonical().digest()
                ]),
                "ICD-006 retain AD regardless of rival arrival order/rejection reason"
            );
            assert!(
                read.quarantine.is_empty(),
                "ICD-006 unauthorized signature has no conviction power"
            );
            assert!(
                state.instances[&d.instance]
                    .security_evidence
                    .contains(&rival)
            );
        }
    }
}
#[test]
fn icd007_new_epoch_uses_new_origin_and_old_retries_stay_old() {
    let d = descriptor("x");
    let a = candidate(
        &d,
        "writer-e0",
        0,
        None,
        vec![],
        insert("atom-a", None, "A"),
    );
    let b = candidate(
        &d,
        "writer-e1",
        0,
        None,
        vec![Head {
            origin: "writer-e0".into(),
            seq: 0,
            hash: None,
        }],
        insert("atom-b", Some("atom-a"), "B"),
    );
    let mut state = initial(d.clone());
    seed(&mut state, a.clone());
    let mut authorized = facts(&b);
    authorized.certificate.epoch = "epoch-1".into();
    let result = drive(state, Event::SubmitLocal(Box::new(b.clone())), authorized);
    assert!(
        result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_)))),
        "ICD-007 fresh canonical recovery origin must admit"
    );
    let read = cut(&result.state, d.instance);
    assert_eq!(
        std::collections::BTreeSet::from_iter(read.eligible),
        std::collections::BTreeSet::from([
            a.operation.canonical().digest(),
            b.operation.canonical().digest()
        ]),
        "ICD-007 retain both AB identities"
    );
}
#[test]
fn icd007_reused_canonical_origin_and_numeric_overflow_refuse() {
    let d = descriptor("x");
    let a = candidate(&d, "writer-e0", 0, None, vec![], insert("a", None, "A"));
    let mut state = initial(d.clone());
    seed(&mut state, a.clone());
    let mut reminted = candidate(&d, "writer-e0", 0, None, vec![], insert("x", None, "X"));
    reminted.evidence.certificate = Bytes::new(b"new-epoch-reused-origin".to_vec());
    let mut reused = facts(&reminted);
    reused.certificate.epoch = "epoch-1".into();
    reused.issuance_unique = false;
    let refused = drive(
        state.clone(),
        Event::SubmitLocal(Box::new(reminted)),
        reused,
    );
    assert!(
        refused.effects.contains(&Effect::Report(Outcome::Invalid)),
        "ICD-007 reused canonical origin fails before mutation"
    );
    let c = candidate(
        &d,
        "overflow",
        9_007_199_254_740_991,
        None,
        vec![],
        insert("x", None, "X"),
    );
    let result = step(&state, Event::SubmitLocal(Box::new(c)));
    assert!(
        result.effects.contains(&Effect::Report(Outcome::Invalid)),
        "ICD-007 unsafe numeric bound"
    );
}
#[test]
fn icd008_successor_requests_predecessor_without_advancing_head() {
    let d = descriptor("x");
    let c = candidate(
        &d,
        "a",
        1,
        Some(vec![3; 32]),
        vec![],
        insert("b", Some("a"), "B"),
    );
    let result = drive(
        initial(d.clone()),
        Event::OfferReplica(Box::new(c.clone())),
        facts(&c),
    );
    assert!(result.effects.iter().any(|e| matches!(e, Effect::RequestMissing(slots) if slots.iter().any(|s| s.origin == "a" && s.seq == 0))), "ICD-008 request exact gap");
    assert!(!result.state.instances[&d.instance].heads.contains_key("a"));
}
#[test]
fn icd009_two_qualified_rivals_quarantine_both_and_dependents_in_either_order() {
    let d = descriptor("x");
    let a = candidate(&d, "writer-e0", 0, None, vec![], insert("a", None, "A"));
    let b = candidate(
        &d,
        "writer-e0",
        1,
        Some(a.operation.canonical().digest().to_vec()),
        vec![],
        insert("b", Some("a"), "B"),
    );
    let rival = candidate(
        &d,
        "writer-e0",
        1,
        Some(a.operation.canonical().digest().to_vec()),
        vec![],
        insert("x", Some("a"), "X"),
    );
    for (first, second) in [(b.clone(), rival.clone()), (rival.clone(), b.clone())] {
        let mut state = initial(d.clone());
        seed(&mut state, a.clone());
        seed(&mut state, first.clone());
        let dependent = candidate(
            &d,
            "dependent",
            0,
            None,
            vec![Head {
                origin: "writer-e0".into(),
                seq: 1,
                hash: Some(first.operation.canonical().digest().to_vec()),
            }],
            insert("d", Some(if first == b { "b" } else { "x" }), "D"),
        );
        seed(&mut state, dependent.clone());
        let result = drive(
            state,
            Event::OfferFork(Box::new(ForkEvidence {
                first,
                second: second.clone(),
            })),
            facts(&second),
        );
        let read = cut(&result.state, d.instance.clone());
        assert_eq!(
            read.quarantine.get("writer-e0"),
            Some(&1),
            "ICD-009 two historical admissions convict"
        );
        assert_eq!(read.eligible, vec![a.operation.canonical().digest()]);
        assert!(
            read.historically_qualified
                .contains(&dependent.operation.canonical().digest()),
            "ICD-009 custody survives derived quarantine"
        );
        assert_eq!(
            read.contiguous_common_heads.get("writer-e0"),
            Some(&(0, a.operation.canonical().digest())),
            "ICD-009 common prefix excludes fork slot"
        );
        assert!(
            read.fork_slots
                .iter()
                .any(|s| s.origin == "writer-e0" && s.seq == 1)
        );
    }
}
#[test]
fn icd010_unknown_commit_retains_reservation_and_foreign_callback_cannot_accept() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut state = initial(d.clone());
    let plan = staged_plan(&d, c.clone());
    state.commits.insert(7, plan.clone());
    let unknown = step(
        &state,
        Event::CommitResolved(CommitReply {
            plan_id: 7,
            instance: d.instance.clone(),
            batch_digest: plan.batch.digest(),
            result: CommitResult::OutcomeUnknown,
        }),
    );
    assert_eq!(
        unknown.state.unknown_commits.get(&7),
        Some(&plan),
        "ICD-010 unknown retains exact reserved plan"
    );
    let mut foreign = d.instance.clone();
    foreign.incarnation = "foreign".into();
    let wrong = step(
        &unknown.state,
        Event::CommitResolved(CommitReply {
            plan_id: 7,
            instance: foreign,
            batch_digest: plan.batch.digest(),
            result: CommitResult::Committed {
                revision: 1,
                storage: StorageClass::VolatileTest,
            },
        }),
    );
    assert!(
        wrong
            .effects
            .contains(&Effect::Report(Outcome::CallbackMismatch))
    );
    let blocked = step(&unknown.state, Event::SubmitLocal(Box::new(c)));
    assert!(
        blocked
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Lookup(_))),
        "ICD-010 ambiguous origin must recover, not advance"
    );
}
#[test]
fn icd013_edit_capacity_preserves_recovery_reserves() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut state = initial(d.clone());
    state.instances.get_mut(&d.instance).unwrap().used.edit = d.bounds.edit_bytes;
    let full = step(&state, Event::SubmitLocal(Box::new(c.clone())));
    assert!(
        full.effects.contains(&Effect::Report(Outcome::Capacity)),
        "ICD-013 full edits refuse"
    );
    let recovering = drive(state, Event::OfferReplica(Box::new(c.clone())), facts(&c));
    assert!(
        recovering.effects.iter().any(
            |e| matches!(e, Effect::Commit(p) if p.charges.edit == 0 && p.charges.recovery > 0)
        ),
        "ICD-013 retained peer history uses reserved recovery budget"
    );
}
#[test]
fn icd004_same_display_names_do_not_share_instance_rights_or_callbacks() {
    let x = descriptor("x");
    let y = descriptor("y");
    let mut state = initial(x.clone());
    state.instances.extend(initial(y.clone()).instances);
    let c = candidate(
        &y,
        "same-display-origin",
        0,
        None,
        vec![],
        insert("a", None, "A"),
    );
    let mut wrong = facts(&c);
    wrong.certificate.instance = x.instance.clone();
    let denied = drive(state, Event::SubmitLocal(Box::new(c)), wrong);
    assert!(
        denied.effects.contains(&Effect::Report(Outcome::Denied)),
        "ICD-004 foreign instance evidence denies"
    );
    assert!(denied.state.instances[&x.instance].accepted.is_empty());
    assert!(denied.state.instances[&y.instance].accepted.is_empty());
}

#[test]
fn icd003_legacy_and_strong_descriptors_do_not_activate_independent_admission() {
    for mode in [AdmissionMode::Legacy, AdmissionMode::Strong] {
        let mut d = descriptor("x");
        d.mode = mode;
        let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
        let result = step(&initial(d), Event::SubmitLocal(Box::new(c)));
        assert!(
            result
                .effects
                .contains(&Effect::Report(Outcome::Unsupported)),
            "ICD-003 explicit profiles required"
        );
    }
}
#[test]
fn icd006_offline_window_and_clock_uncertainty_refuse_new_intent() {
    for time in [
        TimeInterval {
            earliest_ms: 101,
            latest_ms: 102,
        },
        TimeInterval {
            earliest_ms: 99,
            latest_ms: 101,
        },
    ] {
        let d = descriptor("x");
        let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
        let mut state = initial(d.clone());
        let policy = state.instances[&d.instance].policy.clone();
        state = step(
            &state,
            Event::ObservePolicy {
                instance: d.instance.clone(),
                policy,
                time,
            },
        )
        .state;
        let result = drive(state, Event::SubmitLocal(Box::new(c.clone())), facts(&c));
        assert!(
            result.effects.contains(&Effect::Report(Outcome::Denied)),
            "ICD-006 entire conservative interval must fit permit"
        );
        assert!(result.state.instances[&d.instance].accepted.is_empty());
    }
}
#[test]
fn icd006_historical_authorization_is_separate_from_current_revocation() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut verified = facts(&c);
    verified.current_allowed = false;
    let local = drive(
        initial(d.clone()),
        Event::SubmitLocal(Box::new(c.clone())),
        verified.clone(),
    );
    assert!(
        local.effects.contains(&Effect::Report(Outcome::Denied)),
        "ICD-006 current revocation blocks new intent"
    );
    let replay = drive(
        initial(d.clone()),
        Event::OfferReplica(Box::new(c.clone())),
        verified,
    );
    assert!(
        cut(&replay.state, d.instance)
            .historically_qualified
            .contains(&c.operation.canonical().digest()),
        "ICD-006 qualified replay uses historical permit and admission cut"
    );
}
#[test]
fn icd007_zero_base_prev_refs_and_exact_safe_numbers() {
    let d = descriptor("x");
    let invalid = [
        candidate(&d, "a", -1, None, vec![], insert("a", None, "A")),
        candidate(
            &d,
            "a",
            0,
            Some(vec![3; 32]),
            vec![],
            insert("a", None, "A"),
        ),
        candidate(&d, "a", 1, None, vec![], insert("a", None, "A")),
        candidate(
            &d,
            "a",
            0,
            None,
            vec![Head {
                origin: "b".into(),
                seq: -1,
                hash: None,
            }],
            insert("a", None, "A"),
        ),
        candidate(
            &d,
            "a",
            0,
            None,
            vec![Head {
                origin: "b".into(),
                seq: 9_007_199_254_740_991,
                hash: None,
            }],
            insert("a", None, "A"),
        ),
        candidate(
            &d,
            "a",
            0,
            None,
            vec![
                Head {
                    origin: "b".into(),
                    seq: 0,
                    hash: None
                };
                9
            ],
            insert("a", None, "A"),
        ),
    ];
    for c in invalid {
        let result = step(&initial(d.clone()), Event::SubmitLocal(Box::new(c)));
        assert!(
            result.effects.contains(&Effect::Report(Outcome::Invalid)),
            "ICD-007 invalid strict zero-base/ref profile"
        );
    }
}
#[test]
fn icd009_qualified_dependent_custody_survives_quarantined_support_but_local_intent_refuses() {
    let d = descriptor("x");
    let a = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let dependent = candidate(
        &d,
        "d",
        0,
        None,
        vec![Head {
            origin: "a".into(),
            seq: 0,
            hash: Some(a.operation.canonical().digest().to_vec()),
        }],
        insert("d", Some("a"), "D"),
    );
    let mut state = initial(d.clone());
    seed(&mut state, a.clone());
    let rival = candidate(&d, "a", 0, None, vec![], insert("x", None, "X"));
    seed(&mut state, rival.clone());
    state
        .instances
        .get_mut(&d.instance)
        .unwrap()
        .qualified_forks
        .push(ForkEvidence {
            first: a,
            second: rival,
        });
    state
        .instances
        .get_mut(&d.instance)
        .unwrap()
        .heads
        .remove("a");
    state
        .instances
        .get_mut(&d.instance)
        .unwrap()
        .quarantine
        .insert("a".into(), 0);
    let replay = drive(
        state.clone(),
        Event::OfferReplica(Box::new(dependent.clone())),
        facts(&dependent),
    );
    let read = cut(&replay.state, d.instance.clone());
    assert!(
        read.historically_qualified
            .contains(&dependent.operation.canonical().digest()),
        "ICD-009 noncircular qualification retains dependent custody"
    );
    assert!(
        !read
            .eligible
            .contains(&dependent.operation.canonical().digest()),
        "ICD-009 derived quarantine excludes projection"
    );
    let local = drive(
        state,
        Event::SubmitLocal(Box::new(dependent.clone())),
        facts(&dependent),
    );
    assert!(
        local
            .effects
            .contains(&Effect::Report(Outcome::IntegrityConflict)),
        "ICD-009 new intent needs eligible closure"
    );
}
#[test]
fn icd010_old_receipt_retry_does_not_reauthorize_or_reappend() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut state = initial(d.clone());
    seed(&mut state, c.clone());
    let expected = state.instances[&d.instance].accepted[0].receipt.clone();
    let result = step(&state, Event::SubmitLocal(Box::new(c)));
    assert!(
        result
            .effects
            .contains(&Effect::Report(Outcome::ExactRetry(expected))),
        "ICD-010 exact old identity returns stable receipt"
    );
    assert!(
        !result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Commit(_) | Effect::Verify(_)))
    );
}
#[test]
fn icd013_recovery_exhaustion_is_explicit_and_cannot_spend_edit_budget() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut state = initial(d.clone());
    state.instances.get_mut(&d.instance).unwrap().used.recovery = d.bounds.recovery_bytes;
    let result = drive(state, Event::OfferReplica(Box::new(c.clone())), facts(&c));
    assert!(
        result.effects.contains(&Effect::Report(Outcome::Capacity)),
        "ICD-013 exhausted recovery pauses rejoin explicitly"
    );
    assert!(result.state.instances[&d.instance].accepted.is_empty());
}

#[test]
fn icd010_commit_callbacks_bind_batch_revision_and_have_three_distinct_outcomes() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let plan = staged_plan(&d, c.clone());
    let mut state = initial(d.clone());
    state.commits.insert(plan.id, plan.clone());
    for (id, digest, revision) in [
        (8, plan.batch.digest(), 1),
        (7, [9; 32], 1),
        (7, plan.batch.digest(), 0),
    ] {
        let result = step(
            &state,
            Event::CommitResolved(CommitReply {
                plan_id: id,
                instance: d.instance.clone(),
                batch_digest: digest,
                result: CommitResult::Committed {
                    revision,
                    storage: StorageClass::VolatileTest,
                },
            }),
        );
        assert!(
            result
                .effects
                .contains(&Effect::Report(Outcome::CallbackMismatch)),
            "ICD-010 foreign/stale commit cannot accept"
        );
        assert_eq!(result.state.commits.get(&7), Some(&plan));
    }
    let failed = step(
        &state,
        Event::CommitResolved(CommitReply {
            plan_id: 7,
            instance: d.instance.clone(),
            batch_digest: plan.batch.digest(),
            result: CommitResult::KnownNotCommitted,
        }),
    );
    assert!(
        !failed.state.commits.contains_key(&7),
        "ICD-010 known failure releases only staged reservation"
    );
    assert!(failed.state.instances[&d.instance].accepted.is_empty());
    let committed = step(
        &state,
        Event::CommitResolved(CommitReply {
            plan_id: 7,
            instance: d.instance.clone(),
            batch_digest: plan.batch.digest(),
            result: CommitResult::Committed {
                revision: 1,
                storage: StorageClass::VolatileTest,
            },
        }),
    );
    assert_eq!(
        committed.state.instances[&d.instance].accepted[0].candidate, c,
        "ICD-010 matching commit installs exact retained candidate/facts/seal"
    );
    assert!(
        committed
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Report(Outcome::AcceptedLocal(_))))
    );
}

#[test]
fn icd010_lookup_recovery_binds_request_and_restores_original_receipt() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let plan = staged_plan(&d, c.clone());
    let request = LookupRequest {
        plan_id: 7,
        expected_revision: 0,
        instance: d.instance.clone(),
        operation: c.operation.canonical().digest(),
        batch_digest: plan.batch.digest(),
    };
    let mut state = initial(d.clone());
    state.unknown_commits.insert(7, plan.clone());
    state.lookups.insert(7, request.clone());
    let receipt = plan.acceptances[0].receipt.clone();
    let foreign = step(
        &state,
        Event::LookupResolved(LookupReply {
            plan_id: 7,
            expected_revision: 0,
            instance: d.instance.clone(),
            operation: [9; 32],
            batch_digest: request.batch_digest,
            result: LookupResult::Committed {
                receipt: receipt.clone(),
                revision: 1,
            },
        }),
    );
    assert!(
        foreign
            .effects
            .contains(&Effect::Report(Outcome::CallbackMismatch)),
        "ICD-010 lookup cannot bind another operation"
    );
    assert_eq!(foreign.state.unknown_commits.get(&7), Some(&plan));
    let found = step(
        &state,
        Event::LookupResolved(LookupReply {
            plan_id: 7,
            expected_revision: 0,
            instance: d.instance.clone(),
            operation: request.operation,
            batch_digest: request.batch_digest,
            result: LookupResult::Committed {
                receipt: receipt.clone(),
                revision: 1,
            },
        }),
    );
    assert!(
        found
            .effects
            .contains(&Effect::Report(Outcome::ExactRetry(receipt))),
        "ICD-010 uncertain committed batch recovers original receipt"
    );
    assert_eq!(found.state.instances[&d.instance].accepted[0].candidate, c);
    assert!(
        !found
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Seal(_) | Effect::Commit(_)))
    );
}

#[test]
fn icd006_stale_verification_after_policy_change_cannot_seal_or_commit() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let plan = staged_plan(&d, c.clone());
    let query = plan.acceptances[0].verified_query.clone();
    let mut state = initial(d.clone());
    state.queries.insert(
        query.id,
        PendingVerification {
            query: query.clone(),
            candidate: c.clone(),
            verified: None,
            verified_query: None,
        },
    );
    let changed = step(
        &state,
        Event::ObservePolicy {
            instance: d.instance.clone(),
            policy: Bytes::new(b"revoked-test-cut".to_vec()),
            time: TimeInterval {
                earliest_ms: 22,
                latest_ms: 23,
            },
        },
    );
    let stale = step(
        &changed.state,
        Event::EvidenceResolved(Box::new(EvidenceReply {
            query,
            verdict: Verdict::Verified(Box::new(facts(&c))),
        })),
    );
    assert!(
        stale
            .effects
            .contains(&Effect::Report(Outcome::PolicyPending)),
        "ICD-006 matched old-cut validation must require fresh policy before sealing"
    );
    assert!(
        !stale
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Seal(_) | Effect::Commit(_)))
    );
}
#[test]
fn icd006_delayed_prior_cut_committed_callback_retains_historical_admission() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let plan = staged_plan(&d, c);
    let mut state = initial(d.clone());
    state.commits.insert(7, plan.clone());
    // Trusted host began under the plan's valid cut, physically committed, then
    // observed revocation before delivering the delayed barrier reply.
    let changed = step(
        &state,
        Event::ObservePolicy {
            instance: d.instance.clone(),
            policy: Bytes::new(b"revoked-test-cut".to_vec()),
            time: TimeInterval {
                earliest_ms: 22,
                latest_ms: 23,
            },
        },
    );
    let delayed = step(
        &changed.state,
        Event::CommitResolved(CommitReply {
            plan_id: 7,
            instance: d.instance.clone(),
            batch_digest: plan.batch.digest(),
            result: CommitResult::Committed {
                revision: 1,
                storage: StorageClass::VolatileTest,
            },
        }),
    );
    assert!(
        delayed
            .effects
            .contains(&Effect::Report(Outcome::AcceptedLocal(
                plan.acceptances[0].receipt.clone()
            ))),
        "ICD-006 delayed committed prior-cut record is historical evidence, not retroactively cancelled"
    );
    let mut unknown = changed.state;
    unknown.commits.remove(&7);
    unknown.unknown_commits.insert(7, plan.clone());
    unknown.lookups.insert(
        7,
        LookupRequest {
            plan_id: 7,
            expected_revision: 0,
            instance: d.instance.clone(),
            operation: plan.acceptances[0].receipt.operation,
            batch_digest: plan.batch.digest(),
        },
    );
    let recovered = step(
        &unknown,
        Event::LookupResolved(LookupReply {
            plan_id: 7,
            expected_revision: 0,
            instance: d.instance.clone(),
            operation: plan.acceptances[0].receipt.operation,
            batch_digest: plan.batch.digest(),
            result: LookupResult::Committed {
                receipt: plan.acceptances[0].receipt.clone(),
                revision: 1,
            },
        }),
    );
    assert!(
        recovered
            .effects
            .contains(&Effect::Report(Outcome::ExactRetry(
                plan.acceptances[0].receipt.clone()
            ))),
        "ICD-006 prior-cut unknown recovery returns original receipt after current revocation"
    );
}
