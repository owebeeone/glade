//! Full owned image controls. All evidence remains untrusted structural test data.
use glade_crdt_recovery_codec::*;
use std::collections::BTreeMap;
pub(crate) fn limits() -> DecodeLimits {
    DecodeLimits {
        bytes: 262144,
        items: 8192,
        depth: 64,
        name_bytes: 256,
    }
}
fn identity() -> IdentityBinding {
    IdentityBinding {
        root: [1; 32],
        intent: Bytes::new(vec![2]),
        zone: "z".into(),
        parameters: Bytes::new(vec![3]),
        declaration: Bytes::new(vec![4]),
        declaration_version: "1".into(),
        schema: Bytes::new(vec![5]),
        schema_identity: "text".into(),
        schema_version: "1".into(),
        corpus_identity: "glade".into(),
        corpus_version: "1".into(),
        corpus_digest: [6; 32],
        proof_profile: "ic3-direct-root-ed25519/v1".into(),
        recovery_profile: "ic3-records/v1".into(),
        retention_policy: "all".into(),
        rejoin_policy: "no-reset".into(),
        initial_chain_base: -1,
        migration_generation: u64::MAX,
    }
}
pub(crate) fn image() -> ReplicaImage {
    let instance = Instance {
        descriptor: [7; 32],
        incarnation: "i".into(),
    };
    let owner = Owner {
        store: StoreIncarnation([8; 32]),
        generation: u64::MAX,
    };
    let storage = Limits {
        instances: 1,
        attempts: 8,
        invocation_history: 16,
        max_batch_bytes: 65536,
        max_receipts: 8,
        max_windows: 8,
        max_name_bytes: 256,
        critical_bytes: 131072,
        critical_per_attempt: 8192,
    };
    let replica_limits = ReplicaLimits {
        storage: storage.clone(),
        decode: limits(),
        image_bytes: 262144,
        guard_history: 16,
        inbox_items: 16,
        inbox_bytes: 8192,
        inventory_items: 128,
        inventory_bytes: 65536,
    };
    let issuance = IssuanceFloor {
        time_floor_ms: u64::MAX,
        policy_generation: u64::MAX,
        policy_digest: [29; 32],
        plan_namespace: [9; 32],
        invocation_namespace: [10; 32],
        next_effect_id: u64::MAX,
        origins: BTreeMap::from([(
            "o".into(),
            OriginFloor {
                certificate: [11; 32],
                next_seq: i64::MAX,
                lamport: i64::MAX,
                prev: Some([12; 32]),
                reserved_intents: vec![Bytes::new(vec![13])],
                frontier: vec![],
            },
        )]),
    };
    let guard = ReceiveGuard {
        id: GuardId {
            store: owner.store.clone(),
            instance: instance.clone(),
            number: u64::MAX,
        },
        owner: owner.clone(),
        lease: 1,
        source: [14; 32],
        channel: [15; 32],
        role: "inventory".into(),
        base_generation: 0,
        base_digest: [16; 32],
        max_bytes: 8192,
        max_items: 16,
        deadline_ms: u64::MAX,
    };
    let floor = ReplicaFloor {
        issuance,
        version: 1,
        owner: owner.clone(),
        root_registration: [17; 32],
        path_binding: [18; 32],
        lease: 1,
        sequence: u64::MAX,
        selected: Selection {
            slot: 0,
            generation: 0,
            digest: [16; 32],
        },
        next_attempt: u64::MAX,
        invocation_high_water: u64::MAX,
        guard_high_water: u64::MAX,
        guards: BTreeMap::from([(guard.id.clone(), GuardStatus::Active(guard.clone()))]),
        intent: None,
    };
    let mut state = State {
        storage_owner: Some(owner.clone()),
        storage_limits: Some(storage.clone()),
        plan_namespace: [9; 32],
        invocation_namespace: [10; 32],
        next_effect_id: u64::MAX,
        ..State::default()
    };
    let descriptor = Descriptor {
        instance: instance.clone(),
        canonical: Bytes::new(vec![19]),
        creation_root: [1; 32],
        resource: "r".into(),
        share: "s".into(),
        glade_id: "g".into(),
        key: vec![20],
        engine: "crdt.oracle/v1".into(),
        payload_profile: "text_crdt.profile/v1".into(),
        admission_profile: "glade.crdt.independent-admission/v1".into(),
        authorization_profile: "bounded-offline-permit/v1".into(),
        mode: AdmissionMode::Independent,
        bounds: Bounds {
            max_operation_bytes: 4096,
            max_origins: 8,
            max_refs: 8,
            edit_bytes: 8192,
            recovery_bytes: 8192,
            evidence_bytes: 8192,
            pending_bytes: 8192,
            pending_items: 16,
        },
    };
    let operation = Operation::new(glade_wire::generated::Op {
        share: "s".into(),
        glade_id: "g".into(),
        key: vec![20],
        origin: "o".into(),
        seq: 0,
        prev: None,
        lamport: 1,
        refs: vec![],
        shape: glade_wire::generated::Shape::Crdt,
        payload: vec![21],
    });
    let evidence = EvidenceSet {
        certificate: Bytes::new(vec![22]),
        permit: Bytes::new(vec![23]),
        policy: Bytes::new(vec![24]),
        admission: Some(Bytes::new(vec![25])),
    };
    let candidate = Candidate {
        instance: instance.clone(),
        operation,
        evidence: evidence.clone(),
    };
    let certificate = OriginCertificate {
        instance: instance.clone(),
        canonical_origin: "o".into(),
        epoch: "e".into(),
        display_name: "shared".into(),
        writer_key: [26; 32],
        requester: "alice".into(),
        canonical: evidence.certificate.clone(),
    };
    let facts = Facts {
        certificate: certificate.clone(),
        descriptor_valid: true,
        payload_valid: true,
        signatures_valid: true,
        issuance_unique: true,
        permit_window: Window {
            not_before_ms: 0,
            not_after_ms: u64::MAX,
        },
        permit_first_seq: i64::MIN,
        permit_last_seq: i64::MAX,
        current_allowed: false,
        historical_admitter_authorized: true,
        historical_policy_allowed: true,
        historical_time: Some(TimeInterval {
            earliest_ms: u64::MAX,
            latest_ms: u64::MAX,
        }),
    };
    let query = Query {
        id: 31,
        instance: instance.clone(),
        descriptor: descriptor.clone(),
        operation: candidate.operation.clone(),
        evidence: evidence.clone(),
        mode: Mode::Historical,
        verified_facts: Some(Box::new(facts.clone())),
        validated_under: Some(Mode::Local {
            current_policy: evidence.policy.digest(),
            time: TimeInterval {
                earliest_ms: 1,
                latest_ms: 2,
            },
        }),
    };
    let receipt = Receipt {
        instance: instance.clone(),
        operation: candidate.operation.canonical().digest(),
        admission: [27; 32],
        stable_id: [28; 32],
        storage: StorageClass::VolatileTest,
    };
    let cut = LocalPrecondition {
        policy: [29; 32],
        time: TimeInterval {
            earliest_ms: 1,
            latest_ms: 2,
        },
    };
    let plan = CommitPlan {
        id: 32,
        instance: instance.clone(),
        expected_revision: 3,
        local_precondition: Some(cut.clone()),
        batch: Bytes::new(vec![30]),
        kind: CommitKind::AcceptedBatch,
        records: vec![candidate.clone()],
        acceptances: vec![StagedAcceptance {
            candidate: candidate.clone(),
            facts: facts.clone(),
            verified_query: query.clone(),
            seal_query: Some(query.clone()),
            sealed_admission: evidence.admission.clone().unwrap(),
            receipt: receipt.clone(),
        }],
        qualified_forks: vec![ForkEvidence {
            first: candidate.clone(),
            second: candidate.clone(),
        }],
        charges: Charges {
            edit: 1,
            recovery: 2,
            evidence: 3,
            pending: 4,
        },
    };
    let binding = Binding {
        plan: PlanKey {
            namespace: [9; 32],
            instance: instance.clone(),
            number: 32,
        },
        batch: plan.batch.clone(),
        expected_revision: 3,
        storage: StorageClass::VolatileTest,
        kind: plan.kind.clone(),
        charges: plan.charges.clone(),
        receipts: vec![receipt.clone()],
        prestart: Some(StartPrecondition {
            cut: cut.clone(),
            windows: vec![facts.permit_window.clone()],
        }),
    };
    let attempt = AttemptBinding {
        id: AttemptId {
            store: owner.store.clone(),
            instance: instance.clone(),
            number: 1,
        },
        binding: binding.clone(),
    };
    let prepare = PrepareRequest {
        owner: owner.clone(),
        invocation: InvocationId {
            namespace: [10; 32],
            number: 33,
        },
        binding: binding.clone(),
    };
    let resolve = ResolveRequest {
        owner: owner.clone(),
        invocation: InvocationId {
            namespace: [10; 32],
            number: 34,
        },
        operation: OperationKind::Begin,
        attempt: attempt.clone(),
    };
    let terminal = Terminal {
        attempt: attempt.clone(),
        outcome: TerminalOutcome::Committed {
            revision: 4,
            storage: StorageClass::VolatileTest,
            receipts: vec![receipt.clone()],
            start_cut: Some(cut),
        },
    };
    state.queries.insert(
        31,
        PendingVerification {
            query: query.clone(),
            candidate: candidate.clone(),
            verified: Some(facts.clone()),
            verified_query: Some(query),
        },
    );
    state.commits.insert(32, plan.clone());
    state.unknown_commits.insert(35, plan.clone());
    state.lookups.insert(34, resolve.clone());
    state.attempts.insert(
        binding.plan.clone(),
        KernelAttempt {
            attempt: attempt.clone(),
            phase: AttemptPhase::Terminal(terminal.outcome.clone()),
        },
    );
    state
        .preparing
        .insert(binding.plan.clone(), prepare.clone());
    state.storage_invocations.insert(
        resolve.invocation.clone(),
        StorageInvocation::Resolve(resolve.clone()),
    );
    state.retired_storage_invocations.insert(
        prepare.invocation.clone(),
        StorageInvocation::Prepare(prepare),
    );
    state
        .terminal_results
        .insert(attempt.id.clone(), terminal.clone());
    state
        .terminal_conflicts
        .insert(attempt.id.clone(), terminal);
    state.instances.insert(
        instance.clone(),
        InstanceState {
            descriptor,
            revision: 4,
            storage: StorageClass::VolatileTest,
            accepted: vec![Accepted {
                candidate: candidate.clone(),
                facts,
                receipt: receipt.clone(),
            }],
            candidates: vec![candidate.clone()],
            security_evidence: vec![candidate.clone()],
            qualified_forks: plan.qualified_forks,
            canonical_origins: BTreeMap::from([("o".into(), certificate.clone())]),
            heads: BTreeMap::from([("o".into(), (0, [31; 32]))]),
            pending: vec![Slot {
                instance: instance.clone(),
                origin: "other".into(),
                seq: 1,
            }],
            quarantine: BTreeMap::from([("o".into(), 0)]),
            policy: evidence.policy,
            recovery_incomplete: true,
            storage_integrity_failed: true,
            time_floor_ms: u64::MAX,
            policy_time: TimeInterval {
                earliest_ms: u64::MAX,
                latest_ms: u64::MAX,
            },
            used: plan.charges,
        },
    );
    let record = RecordRef {
        kind: RecordKind::Fork,
        origin: "o".into(),
        seq: 0,
        operation: [32; 32],
        bundle: [33; 32],
        receipt: Some(receipt.clone()),
    };
    let observation = Observation {
        id: 1,
        instance: instance.clone(),
        source: [14; 32],
        channel: [15; 32],
        guard: guard.id,
        bytes: Bytes::new(vec![34]),
        expected: vec![record.clone()],
    };
    ReplicaImage {
        version: 1,
        instance: instance.clone(),
        identity: identity(),
        generation: 1,
        predecessor: [16; 32],
        state,
        recovery: Recovery {
            owner,
            invocation_namespace: [10; 32],
            limits: storage,
            preparing: vec![binding],
            revisions: BTreeMap::from([(instance, 4)]),
            attempts: vec![RetainedAttempt {
                attempt,
                phase: AttemptPhase::Started { cut: None },
            }],
            next_attempt: u64::MAX,
            invocation_high_water: u64::MAX,
            invocation_count: 2,
        },
        prior_floor: floor,
        origins: vec![OriginCustody {
            certificate,
            next_seq: i64::MAX,
            prev: Some([35; 32]),
            lamport: i64::MAX,
            frontier: vec![record.clone()],
            intents: vec![Bytes::new(vec![36])],
            receipts: vec![receipt],
        }],
        observations: vec![observation],
        obligations: vec![Obligation {
            observation: 1,
            expected: record,
            classification: Classification::Missing,
        }],
        sessions: vec![SessionCustody {
            challenge: SignedRecord {
                purpose: CHALLENGE_DOMAIN.into(),
                version: 1,
                signer: [37; 32],
                body: Bytes::new(vec![38]),
                signature: Bytes::new(vec![39; 64]),
            },
            response: None,
            consumed: true,
        }],
        watermark: 1,
        permanent_loss: vec![Bytes::new(vec![40])],
        limits: replica_limits,
    }
}
