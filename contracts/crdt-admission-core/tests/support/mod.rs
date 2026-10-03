//! Explicit trusted test inputs and volatile Records consumer. Not crypto/durability.
use glade_crdt_admission_core::*;
use glade_wire::generated::{Head, Op, Shape};
use std::collections::BTreeMap;

pub fn descriptor(name: &str) -> Descriptor {
    let canonical = Bytes::new(name.as_bytes().to_vec());
    Descriptor {
        instance: Instance {
            descriptor: canonical.digest(),
            incarnation: name.into(),
        },
        canonical,
        creation_root: [1; 32],
        resource: name.into(),
        share: name.into(),
        glade_id: "document".into(),
        key: vec![],
        engine: "crdt.oracle/v1".into(),
        payload_profile: "text_crdt.profile/v1".into(),
        admission_profile: "glade.crdt.independent-admission/v1".into(),
        authorization_profile: "bounded-offline-permit/v1".into(),
        mode: AdmissionMode::Independent,
        bounds: Bounds {
            max_operation_bytes: 4096,
            max_origins: 8,
            max_refs: 8,
            edit_bytes: 65536,
            recovery_bytes: 8192,
            evidence_bytes: 8192,
            pending_bytes: 8192,
            pending_items: 16,
        },
    }
}
pub fn initial(d: Descriptor) -> State {
    let instance = d.instance.clone();
    let value = InstanceState {
        descriptor: d,
        revision: 0,
        storage: StorageClass::VolatileTest,
        accepted: vec![],
        candidates: vec![],
        security_evidence: vec![],
        qualified_forks: vec![],
        canonical_origins: BTreeMap::new(),
        heads: BTreeMap::new(),
        pending: vec![],
        quarantine: BTreeMap::new(),
        policy: Bytes::new(b"granted-test-cut".to_vec()),
        recovery_incomplete: false,
        storage_integrity_failed: false,
        time_floor_ms: 10,
        policy_time: TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        },
        used: Charges {
            edit: 0,
            recovery: 0,
            evidence: 0,
            pending: 0,
        },
    };
    State {
        instances: BTreeMap::from([(instance, value)]),
        storage_owner: Some(storage_host::open_request().owner),
        storage_limits: Some(storage_host::open_request().limits),
        plan_namespace: [4; 32],
        invocation_namespace: [2; 32],
        ..State::default()
    }
}
pub fn insert(atom: &str, after: Option<&str>, text: &str) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"kind":"insert","atom_id":atom,"after":after,"text":text}),
    )
    .unwrap()
}
pub fn candidate(
    d: &Descriptor,
    origin: &str,
    seq: i64,
    prev: Option<Vec<u8>>,
    refs: Vec<Head>,
    payload: Vec<u8>,
) -> Candidate {
    Candidate {
        instance: d.instance.clone(),
        operation: Operation::new(Op {
            share: d.share.clone(),
            glade_id: d.glade_id.clone(),
            key: d.key.clone(),
            origin: origin.into(),
            seq,
            prev,
            lamport: seq + 1,
            refs,
            shape: Shape::Crdt,
            payload,
        }),
        evidence: EvidenceSet {
            certificate: Bytes::new(format!("certificate:{origin}").into_bytes()),
            permit: Bytes::new(b"finite-test-permit".to_vec()),
            policy: Bytes::new(b"granted-test-cut".to_vec()),
            admission: Some(Bytes::new(
                format!("test-admission:{origin}:{seq}").into_bytes(),
            )),
        },
    }
}
pub fn facts(c: &Candidate) -> Facts {
    Facts {
        certificate: OriginCertificate {
            instance: c.instance.clone(),
            canonical_origin: c.operation.inner().origin.clone(),
            epoch: if c.operation.inner().origin == "writer-e1" {
                "epoch-1".into()
            } else {
                "epoch-0".into()
            },
            display_name: "repeated-display-name".into(),
            writer_key: [2; 32],
            requester: "alice".into(),
            canonical: c.evidence.certificate.clone(),
        },
        descriptor_valid: true,
        payload_valid: true,
        signatures_valid: true,
        issuance_unique: true,
        permit_window: Window {
            not_before_ms: 1,
            not_after_ms: 100,
        },
        permit_first_seq: 0,
        permit_last_seq: 100,
        current_allowed: true,
        historical_admitter_authorized: true,
        historical_policy_allowed: true,
        historical_time: Some(TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        }),
    }
}
pub fn accepted(c: Candidate) -> Accepted {
    let receipt = Receipt {
        instance: c.instance.clone(),
        operation: c.operation.canonical().digest(),
        admission: c.evidence.admission.as_ref().unwrap().digest(),
        stable_id: c.operation.canonical().digest(),
        storage: StorageClass::VolatileTest,
    };
    Accepted {
        facts: facts(&c),
        candidate: c,
        receipt,
    }
}
pub fn seed(state: &mut State, c: Candidate) {
    let value = state.instances.get_mut(&c.instance).unwrap();
    let a = accepted(c);
    let op = a.candidate.operation.inner();
    value
        .canonical_origins
        .insert(op.origin.clone(), a.facts.certificate.clone());
    value.heads.insert(
        op.origin.clone(),
        (op.seq, a.candidate.operation.canonical().digest()),
    );
    value.accepted.push(a);
}
/// Compiled downstream host executes the public effects, never private kernel helpers.
/// Test evidence is intentionally trusted; commit replies attest VOLATILE test retention.
pub fn drive(state: State, first: Event, supplied: Facts) -> Transition {
    let mut fixture = FixtureReplica::new(state).expect("explicit consistent trusted fixture seed");
    fixture.advance(first, supplied)
}
/// Caller owns the session and retains it across event/effect continuations.
pub fn drive_with_port<S: StorageAttemptSession>(
    mut state: State,
    first: Event,
    supplied: Facts,
    host: &mut S,
) -> Transition {
    let mut known = BTreeMap::new();
    for instance in state.instances.values() {
        for record in &instance.accepted {
            known.insert(
                (
                    record.candidate.instance.clone(),
                    record.candidate.operation.canonical().digest(),
                ),
                record.facts.clone(),
            );
        }
        for record in &instance.candidates {
            known.insert(
                (
                    record.instance.clone(),
                    record.operation.canonical().digest(),
                ),
                facts(record),
            );
        }
    }
    match &first {
        Event::SubmitLocal(c) | Event::OfferReplica(c) => {
            known.insert(
                (c.instance.clone(), c.operation.canonical().digest()),
                supplied,
            );
        }
        Event::OfferFork(pair) => {
            known
                .entry((
                    pair.first.instance.clone(),
                    pair.first.operation.canonical().digest(),
                ))
                .or_insert_with(|| facts(&pair.first));
            known.insert(
                (
                    pair.second.instance.clone(),
                    pair.second.operation.canonical().digest(),
                ),
                supplied,
            );
        }
        _ => {}
    }
    let mut pending = vec![first];
    let mut observed = Vec::new();
    for _ in 0..32 {
        let Some(event) = pending.pop() else {
            return Transition {
                state,
                effects: observed,
            };
        };
        let result = step(&state, event);
        state = result.state;
        for effect in result.effects {
            match &effect {
                Effect::Verify(query) => {
                    pending.push(Event::EvidenceResolved(Box::new(EvidenceReply {
                        query: query.clone(),
                        verdict: known
                            .get(&(query.instance.clone(), query.operation.canonical().digest()))
                            .map(|f| Verdict::Verified(Box::new(f.clone())))
                            .unwrap_or(Verdict::Unavailable),
                    })))
                }
                Effect::Seal(query) => {
                    pending.push(Event::EvidenceResolved(Box::new(EvidenceReply {
                        query: query.clone(),
                        verdict: Verdict::Sealed(Bytes::new(b"test-sealed-admission".to_vec())),
                    })))
                }
                Effect::Prepare { request, .. } => {
                    pending.push(Event::PrepareResolved(host.prepare(request.clone())));
                }
                Effect::RecoverPlan(request) => {
                    pending.push(Event::PrepareResolved(host.recover_plan(request.clone())));
                }
                Effect::Begin(request) => {
                    pending.push(Event::CommitResolved(host.begin(request.clone())));
                }
                Effect::Lookup(request) => {
                    pending.push(Event::LookupResolved(host.inspect(request.clone())));
                }
                Effect::Fence(request) => {
                    pending.push(Event::FenceResolved(host.request_fence(request.clone())));
                }
                _ => {}
            }
            observed.push(effect);
        }
    }
    panic!("ICD-017 bounded host failed to finish");
}
pub fn cut(state: &State, instance: Instance) -> ReadCut {
    let transition = step(state, Event::Read(instance));
    transition
        .effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Report(Outcome::Read(cut)) => Some(cut),
            _ => None,
        })
        .expect("ICD-011 expected explicit local read cut")
}

pub fn staged_plan(d: &Descriptor, c: Candidate) -> CommitPlan {
    let verified = facts(&c);
    let verify = Query {
        id: 5,
        instance: d.instance.clone(),
        descriptor: d.clone(),
        operation: c.operation.clone(),
        evidence: c.evidence.clone(),
        mode: Mode::Local {
            current_policy: c.evidence.policy.digest(),
            time: TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            },
        },
        verified_facts: None,
        validated_under: None,
    };
    let seal = Query {
        id: 6,
        mode: Mode::AdmissionSeal,
        verified_facts: Some(Box::new(verified.clone())),
        validated_under: Some(verify.mode.clone()),
        ..verify.clone()
    };
    let stored = accepted(c.clone());
    CommitPlan {
        id: 7,
        instance: d.instance.clone(),
        expected_revision: 0,
        local_precondition: Some(LocalPrecondition {
            policy: c.evidence.policy.digest(),
            time: TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            },
        }),
        batch: Bytes::new(b"exact-test-batch".to_vec()),
        kind: CommitKind::AcceptedBatch,
        records: vec![c.clone()],
        qualified_forks: vec![],
        acceptances: vec![StagedAcceptance {
            candidate: c,
            facts: verified,
            verified_query: verify,
            seal_query: Some(seal),
            sealed_admission: stored
                .candidate
                .evidence
                .admission
                .as_ref()
                .unwrap()
                .clone(),
            receipt: stored.receipt,
        }],
        charges: Charges {
            edit: 50,
            recovery: 30,
            evidence: 20,
            pending: 0,
        },
    }
}

#[allow(dead_code)]
pub mod storage_host {
    include!("../../../crdt-storage-attempt-api/tests/support/mod.rs");
}

pub fn binding_for(plan: &CommitPlan) -> Binding {
    Binding {
        plan: PlanKey {
            namespace: [4; 32],
            instance: plan.instance.clone(),
            number: plan.id,
        },
        batch: plan.batch.clone(),
        expected_revision: plan.expected_revision,
        storage: StorageClass::VolatileTest,
        kind: plan.kind.clone(),
        charges: plan.charges.clone(),
        receipts: plan.acceptances.iter().map(|a| a.receipt.clone()).collect(),
        prestart: plan
            .local_precondition
            .clone()
            .map(|cut| StartPrecondition {
                cut,
                windows: plan
                    .acceptances
                    .iter()
                    .map(|a| a.facts.permit_window.clone())
                    .collect(),
            }),
    }
}
pub fn attempt(plan: &CommitPlan) -> AttemptBinding {
    AttemptBinding {
        id: AttemptId {
            store: StoreIncarnation([1; 32]),
            instance: plan.instance.clone(),
            number: 1,
        },
        binding: binding_for(plan),
    }
}
pub fn resolve_request(plan: &CommitPlan, number: u64, operation: OperationKind) -> ResolveRequest {
    ResolveRequest {
        owner: storage_host::open_request().owner,
        invocation: InvocationId {
            namespace: [2; 32],
            number,
        },
        operation,
        attempt: attempt(plan),
    }
}
#[allow(dead_code)]
pub fn committed_result(plan: &CommitPlan) -> ResolveResult {
    ResolveResult::Terminal(Box::new(Terminal {
        attempt: attempt(plan),
        outcome: TerminalOutcome::Committed {
            revision: plan.expected_revision.checked_add(1).unwrap(),
            storage: StorageClass::VolatileTest,
            receipts: plan.acceptances.iter().map(|a| a.receipt.clone()).collect(),
            start_cut: plan.local_precondition.clone(),
        },
    }))
}
pub fn noncommit_result(plan: &CommitPlan) -> ResolveResult {
    ResolveResult::Terminal(Box::new(Terminal {
        attempt: attempt(plan),
        outcome: TerminalOutcome::NonCommit {
            revision: plan.expected_revision,
            storage: StorageClass::VolatileTest,
        },
    }))
}
pub fn commit_reply(plan: &CommitPlan, number: u64, result: ResolveResult) -> ResolveReply {
    ResolveReply {
        request: resolve_request(plan, number, OperationKind::Begin),
        result,
    }
}
pub fn register_attempt(state: &mut State, plan: &CommitPlan) {
    let a = attempt(plan);
    state.attempts.insert(
        a.binding.plan.clone(),
        KernelAttempt {
            attempt: a,
            phase: AttemptPhase::Reserved,
        },
    );
    let prepare = PrepareRequest {
        owner: state.storage_owner.clone().unwrap(),
        invocation: InvocationId {
            namespace: state.invocation_namespace,
            number: 7,
        },
        binding: binding_for(plan),
    };
    let id = prepare.invocation.clone();
    let previous = state
        .retired_storage_invocations
        .insert(id, StorageInvocation::Prepare(prepare.clone()));
    assert!(previous.is_none_or(|old| old == StorageInvocation::Prepare(prepare)));
    let q = resolve_request(plan, 8, OperationKind::Begin);
    let old = state
        .storage_invocations
        .insert(q.invocation.clone(), StorageInvocation::Resolve(q.clone()));
    assert!(old.is_none_or(|old| old == StorageInvocation::Resolve(q)));
    assert!(
        state
            .storage_invocations
            .len()
            .checked_add(state.retired_storage_invocations.len())
            .unwrap()
            <= state.storage_limits.as_ref().unwrap().invocation_history
    );
    state.next_effect_id = state.next_effect_id.max(9);
}

pub fn terminal_result(
    plan: &CommitPlan,
    digest: Digest,
    revision: u64,
    storage: StorageClass,
    receipts: Vec<Receipt>,
) -> ResolveResult {
    let mut a = attempt(plan);
    if digest != plan.batch.digest() {
        a.binding.batch = Bytes::new(b"changed-result-batch".to_vec());
    }
    ResolveResult::Terminal(Box::new(Terminal {
        attempt: a,
        outcome: TerminalOutcome::Committed {
            revision,
            storage,
            receipts,
            start_cut: plan.local_precondition.clone(),
        },
    }))
}

/// Explicit trusted, bounded development assembly. No physical import/encoding claim.
pub struct FixtureReplica {
    pub state: State,
    pub session: storage_host::MemorySession,
}
impl FixtureReplica {
    pub fn new(state: State) -> Result<Self, Fault> {
        let seed = owned_fixture_seed(&state)?;
        let mut session = storage_host::MemoryHost::from_owned_fixture(seed)?;
        for (instance, value) in &state.instances {
            session.observe_policy(
                instance.clone(),
                LocalPrecondition {
                    policy: value.policy.digest(),
                    time: value.policy_time.clone(),
                },
            )?;
        }
        Ok(Self { state, session })
    }
    pub fn observe_policy(
        &mut self,
        instance: Instance,
        cut: LocalPrecondition,
    ) -> Result<(), Fault> {
        // Provider input is independent of a later Begin caller expectation.
        self.session.observe_policy(instance, cut)
    }
    pub fn advance(&mut self, event: Event, supplied: Facts) -> Transition {
        if let Event::ObservePolicy {
            instance,
            policy,
            time,
        } = &event
        {
            self.observe_policy(
                instance.clone(),
                LocalPrecondition {
                    policy: policy.digest(),
                    time: time.clone(),
                },
            )
            .unwrap();
        }
        let result = drive_with_port(self.state.clone(), event, supplied, &mut self.session);
        self.state = result.state.clone();
        result
    }
}
fn request_identity(request: &StorageInvocation) -> (&Owner, &InvocationId) {
    match request {
        StorageInvocation::Prepare(q) | StorageInvocation::RecoverPlan(q) => {
            (&q.owner, &q.invocation)
        }
        StorageInvocation::Resolve(q) => (&q.owner, &q.invocation),
    }
}
fn owned_fixture_seed(state: &State) -> Result<storage_host::OwnedFixtureSeed, Fault> {
    let owner = state.storage_owner.clone().ok_or(Fault::Ownership)?;
    let limits = state.storage_limits.clone().ok_or(Fault::Capacity)?;
    if state.instances.len() > limits.instances || state.attempts.len() > limits.attempts {
        return Err(Fault::Capacity);
    }
    let count = state
        .storage_invocations
        .len()
        .checked_add(state.retired_storage_invocations.len())
        .ok_or(Fault::Capacity)?;
    if count > limits.invocation_history {
        return Err(Fault::Capacity);
    }
    let mut high = state.next_effect_id.saturating_sub(1);
    for (id, request) in state
        .storage_invocations
        .iter()
        .chain(state.retired_storage_invocations.iter())
    {
        let (issued_owner, issued_id) = request_identity(request);
        if issued_id != id
            || issued_owner != &owner
            || id.namespace != state.invocation_namespace
            || id.number == u64::MAX
            || id.number >= state.next_effect_id
        {
            return Err(Fault::Integrity);
        }
        if state.storage_invocations.contains_key(id)
            && state.retired_storage_invocations.contains_key(id)
        {
            return Err(Fault::Integrity);
        }
        high = high.max(id.number);
        match request {
            StorageInvocation::Resolve(q) => {
                if state
                    .attempts
                    .get(&q.attempt.binding.plan)
                    .is_none_or(|a| a.attempt != q.attempt)
                {
                    return Err(Fault::Integrity);
                }
            }
            StorageInvocation::Prepare(q) | StorageInvocation::RecoverPlan(q) => {
                if state
                    .attempts
                    .get(&q.binding.plan)
                    .is_none_or(|a| a.attempt.binding != q.binding)
                    && state
                        .preparing
                        .get(&q.binding.plan)
                        .is_none_or(|p| p.binding != q.binding)
                {
                    return Err(Fault::Integrity);
                }
            }
        }
    }
    for (number, q) in &state.lookups {
        if *number != q.invocation.number
            || q.operation != OperationKind::Inspect
            || state.storage_invocations.get(&q.invocation)
                != Some(&StorageInvocation::Resolve(q.clone()))
        {
            return Err(Fault::Integrity);
        }
    }
    let mut attempts = Vec::new();
    let mut next_attempt = 1;
    for (plan, a) in &state.attempts {
        if plan != &a.attempt.binding.plan
            || a.attempt.id.store != owner.store
            || a.attempt.id.instance != plan.instance
        {
            return Err(Fault::Integrity);
        }
        next_attempt = next_attempt.max(a.attempt.id.number.checked_add(1).ok_or(Fault::Capacity)?);
        if let Some(t) = state.terminal_results.get(&a.attempt.id) {
            if t.attempt != a.attempt || a.phase != AttemptPhase::Terminal(t.outcome.clone()) {
                return Err(Fault::Integrity);
            }
        } else if matches!(a.phase, AttemptPhase::Terminal(_)) {
            return Err(Fault::Integrity);
        }
        attempts.push(RetainedAttempt {
            attempt: a.attempt.clone(),
            phase: a.phase.clone(),
        });
    }
    for (id, t) in &state.terminal_results {
        if &t.attempt.id != id
            || state
                .attempts
                .get(&t.attempt.binding.plan)
                .is_none_or(|a| a.attempt != t.attempt)
        {
            return Err(Fault::Integrity);
        }
    }
    for p in state.commits.values().chain(state.unknown_commits.values()) {
        let b = binding_for(p);
        if state
            .attempts
            .get(&b.plan)
            .is_none_or(|a| a.attempt.binding != b)
            && state.preparing.get(&b.plan).is_none_or(|q| q.binding != b)
        {
            return Err(Fault::Integrity);
        }
    }
    let mut revisions = BTreeMap::new();
    let mut genesis_custody = BTreeMap::new();
    for (instance, value) in &state.instances {
        let latest = state
            .terminal_results
            .values()
            .filter(|t| &t.attempt.id.instance == instance)
            .filter_map(|t| {
                if let TerminalOutcome::Committed { revision, .. } = &t.outcome {
                    Some(*revision)
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);
        if latest != value.revision {
            return Err(Fault::Integrity);
        }
        let mut charged = Charges {
            edit: 0,
            recovery: 0,
            evidence: 0,
            pending: 0,
        };
        for t in state
            .terminal_results
            .values()
            .filter(|t| &t.attempt.id.instance == instance)
        {
            if let TerminalOutcome::Committed { receipts, .. } = &t.outcome {
                if receipts
                    .iter()
                    .any(|receipt| !value.accepted.iter().any(|a| &a.receipt == receipt))
                {
                    return Err(Fault::Integrity);
                }
                let c = &t.attempt.binding.charges;
                charged.edit = charged.edit.checked_add(c.edit).ok_or(Fault::Capacity)?;
                charged.recovery = charged
                    .recovery
                    .checked_add(c.recovery)
                    .ok_or(Fault::Capacity)?;
                charged.evidence = charged
                    .evidence
                    .checked_add(c.evidence)
                    .ok_or(Fault::Capacity)?;
                charged.pending = charged
                    .pending
                    .checked_add(c.pending)
                    .ok_or(Fault::Capacity)?;
            }
        }
        if value.used.edit < charged.edit
            || value.used.recovery < charged.recovery
            || value.used.evidence < charged.evidence
            || value.used.pending < charged.pending
        {
            return Err(Fault::Integrity);
        }
        revisions.insert(instance.clone(), value.revision);
        // An explicit immutable development snapshot of all semantic custody,
        // receipts, derived state and reservations at the seed cut. Not a wire encoder.
        genesis_custody.insert(
            instance.clone(),
            Bytes::new(format!("{value:?}").into_bytes()),
        );
    }
    Ok(storage_host::OwnedFixtureSeed {
        recovery: Recovery {
            owner,
            invocation_namespace: state.invocation_namespace,
            limits,
            preparing: state
                .preparing
                .values()
                .map(|q| q.binding.clone())
                .collect(),
            revisions,
            attempts,
            next_attempt,
            invocation_high_water: high,
            invocation_count: count,
        },
        genesis_custody,
    })
}
pub fn register_lookup(state: &mut State, request: ResolveRequest) -> Result<(), Fault> {
    if request.owner != state.storage_owner.clone().ok_or(Fault::Ownership)?
        || request.invocation.namespace != state.invocation_namespace
        || request.operation != OperationKind::Inspect
        || state
            .attempts
            .get(&request.attempt.binding.plan)
            .is_none_or(|a| a.attempt != request.attempt)
        || state.storage_invocations.contains_key(&request.invocation)
        || state
            .retired_storage_invocations
            .contains_key(&request.invocation)
        || request.invocation.number < state.next_effect_id
    {
        return Err(Fault::CallbackMismatch);
    }
    let limits = state.storage_limits.as_ref().ok_or(Fault::Capacity)?;
    if state
        .storage_invocations
        .len()
        .checked_add(state.retired_storage_invocations.len())
        .is_none_or(|n| n >= limits.invocation_history)
    {
        return Err(Fault::Capacity);
    }
    let next = request
        .invocation
        .number
        .checked_add(1)
        .ok_or(Fault::Capacity)?;
    state.next_effect_id = next;
    state
        .lookups
        .insert(request.invocation.number, request.clone());
    state.storage_invocations.insert(
        request.invocation.clone(),
        StorageInvocation::Resolve(request),
    );
    Ok(())
}
