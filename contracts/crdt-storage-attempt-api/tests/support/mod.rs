// Development-only deterministic bounded Records port fixture. No physical claims.
use glade_crdt_storage_attempt_api::*;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
#[derive(Default)]
struct Ledger {
    active: Option<Owner>,
    store: Option<StoreIncarnation>,
    namespace: Option<Digest>,
    owner_floor: u64,
    invocation_floor: u64,
    invocation_count: usize,
    next_attempt: u64,
    attempts: BTreeMap<PlanKey, RetainedAttempt>,
    queued: BTreeMap<Instance, Binding>,
    revisions: BTreeMap<Instance, u64>,
    critical_used: usize,
    observations: BTreeMap<Instance, LocalPrecondition>,
    genesis_custody: BTreeMap<Instance, Bytes>,
}
pub struct OwnedFixtureSeed {
    pub recovery: Recovery,
    pub genesis_custody: BTreeMap<Instance, Bytes>,
}
pub struct MemoryHost {
    ledger: Rc<RefCell<Ledger>>,
}
pub struct MemorySession {
    ledger: Rc<RefCell<Ledger>>,
    open: OpenRequest,
    hold_prepare: bool,
    lost_ack: bool,
    hold_publish: bool,
}
impl MemoryHost {
    pub fn restore(recovery: Recovery) -> Result<Self, Fault> {
        // A trusted validated restoration fixture only. This cannot detect rollback
        // without a qualified external ownership/issuance floor; physical gate remains.
        if recovery.next_attempt == 0
            || recovery.invocation_high_water == u64::MAX
            || recovery.attempts.len() > recovery.limits.attempts
            || recovery.revisions.len() > recovery.limits.instances
            || recovery.preparing.len() > recovery.limits.instances
            || recovery.limits.invocation_history == 0
            || recovery.invocation_count > recovery.limits.invocation_history
            || !u64::try_from(recovery.invocation_count)
                .is_ok_and(|n| n <= recovery.invocation_high_water)
            || recovery
                .attempts
                .len()
                .checked_add(recovery.preparing.len())
                .is_none_or(|n| n > recovery.invocation_count)
        {
            return Err(Fault::Integrity);
        }
        let mut attempts = BTreeMap::new();
        let mut committed_revisions: BTreeMap<Instance, BTreeSet<u64>> = BTreeMap::new();
        let mut identities = BTreeSet::new();
        let mut unresolved = BTreeSet::new();
        for a in recovery.attempts {
            if !identities.insert(a.attempt.id.clone())
                || (!matches!(a.phase, AttemptPhase::Terminal(_))
                    && !unresolved.insert(a.attempt.id.instance.clone()))
            {
                return Err(Fault::Integrity);
            }
            validate_binding(&recovery.limits, &a.attempt.binding)?;
            let current = *recovery
                .revisions
                .get(&a.attempt.id.instance)
                .ok_or(Fault::Integrity)?;
            if !matches!(a.phase, AttemptPhase::Terminal(_))
                && a.attempt.binding.expected_revision != current
            {
                return Err(Fault::Integrity);
            }
            let original_cut = a.attempt.binding.prestart.as_ref().map(|p| p.cut.clone());
            if matches!(
                a.phase,
                AttemptPhase::Started { .. }
                    | AttemptPhase::Terminal(TerminalOutcome::Committed { .. })
            ) && a.attempt.binding.prestart.as_ref().is_some_and(|p| {
                p.windows.iter().any(|w| {
                    p.cut.time.earliest_ms < w.not_before_ms
                        || p.cut.time.latest_ms > w.not_after_ms
                })
            }) {
                return Err(Fault::Integrity);
            }
            match &a.phase {
                AttemptPhase::Reserved => {}
                AttemptPhase::Started { cut } => {
                    if cut != &original_cut {
                        return Err(Fault::Integrity);
                    }
                }
                AttemptPhase::Terminal(TerminalOutcome::Committed {
                    revision,
                    storage,
                    receipts,
                    start_cut,
                }) => {
                    if !committed_revisions
                        .entry(a.attempt.id.instance.clone())
                        .or_default()
                        .insert(*revision)
                        || a.attempt.binding.expected_revision.checked_add(1) != Some(*revision)
                        || *revision > current
                        || storage != &a.attempt.binding.storage
                        || receipts != &a.attempt.binding.receipts
                        || start_cut != &original_cut
                    {
                        return Err(Fault::Integrity);
                    }
                }
                AttemptPhase::Terminal(TerminalOutcome::NonCommit { revision, storage }) => {
                    if *revision != a.attempt.binding.expected_revision
                        || *revision > current
                        || storage != &a.attempt.binding.storage
                    {
                        return Err(Fault::Integrity);
                    }
                }
            }
            if a.attempt.id.store != recovery.owner.store
                || a.attempt.id.instance != a.attempt.binding.plan.instance
                || a.attempt.id.number >= recovery.next_attempt
                || attempts.insert(a.attempt.binding.plan.clone(), a).is_some()
            {
                return Err(Fault::Integrity);
            }
        }
        // Full history has no GC/import base: unique positive revisions must cover 1..=head.
        // Compare finite cardinality rather than iterate an attacker-carried revision range.
        for (instance, current) in &recovery.revisions {
            let count = committed_revisions.get(instance).map_or(0, BTreeSet::len);
            if u64::try_from(count).ok() != Some(*current) {
                return Err(Fault::Integrity);
            }
        }
        let critical_used = attempts
            .len()
            .checked_mul(recovery.limits.critical_per_attempt)
            .ok_or(Fault::Capacity)?;
        if critical_used > recovery.limits.critical_bytes {
            return Err(Fault::Capacity);
        }
        let mut queued = BTreeMap::new();
        for b in recovery.preparing {
            validate_binding(&recovery.limits, &b)?;
            if b.expected_revision != *recovery.revisions.get(&b.plan.instance).unwrap_or(&0) {
                return Err(Fault::Integrity);
            }
            if attempts.contains_key(&b.plan)
                || !unresolved.insert(b.plan.instance.clone())
                || queued.insert(b.plan.instance.clone(), b).is_some()
            {
                return Err(Fault::Integrity);
            }
        }
        let mut instances: BTreeSet<_> = recovery.revisions.keys().cloned().collect();
        instances.extend(queued.keys().cloned());
        if instances.len() > recovery.limits.instances
            || instances
                .iter()
                .any(|i| i.incarnation.len() > recovery.limits.max_name_bytes)
        {
            return Err(Fault::Capacity);
        }
        Ok(Self {
            ledger: Rc::new(RefCell::new(Ledger {
                active: None,
                store: Some(recovery.owner.store),
                namespace: Some(recovery.invocation_namespace),
                owner_floor: recovery.owner.generation,
                invocation_floor: recovery.invocation_high_water,
                invocation_count: recovery.invocation_count,
                next_attempt: recovery.next_attempt,
                attempts,
                queued,
                revisions: recovery.revisions,
                critical_used,
                observations: BTreeMap::new(),
                genesis_custody: BTreeMap::new(),
            })),
        })
    }
    /// Explicit owned test continuation, not a live import/reopen protocol.
    pub fn from_owned_fixture(seed: OwnedFixtureSeed) -> Result<MemorySession, Fault> {
        let open = OpenRequest {
            owner: seed.recovery.owner.clone(),
            invocation_namespace: seed.recovery.invocation_namespace,
            limits: seed.recovery.limits.clone(),
        };
        if seed.genesis_custody.len() > open.limits.instances {
            return Err(Fault::Capacity);
        }
        let mut bytes = 0usize;
        for (instance, image) in &seed.genesis_custody {
            if !seed.recovery.revisions.contains_key(instance)
                || instance.incarnation.len() > open.limits.max_name_bytes
            {
                return Err(Fault::Integrity);
            }
            bytes = bytes
                .checked_add(image.value().len())
                .ok_or(Fault::Capacity)?;
        }
        let host = Self::restore(seed.recovery)?;
        {
            let mut l = host.ledger.borrow_mut();
            l.critical_used = l.critical_used.checked_add(bytes).ok_or(Fault::Capacity)?;
            if l.critical_used > open.limits.critical_bytes {
                return Err(Fault::Capacity);
            }
            l.genesis_custody = seed.genesis_custody;
            // The explicit test seed asserts this execution already owns the scope.
            // This never reopens a real store or validates an external ownership floor.
            l.active = Some(open.owner.clone());
        }
        Ok(MemorySession {
            ledger: host.ledger,
            open,
            hold_prepare: false,
            lost_ack: false,
            hold_publish: false,
        })
    }
    pub fn new() -> Self {
        Self {
            ledger: Rc::new(RefCell::new(Ledger {
                next_attempt: 1,
                ..Ledger::default()
            })),
        }
    }
}
impl StorageAttemptHost for MemoryHost {
    type Session = MemorySession;
    fn open(&mut self, request: OpenRequest) -> Result<Self::Session, Fault> {
        let mut l = self.ledger.borrow_mut();
        if l.active.is_some()
            || request.owner.generation <= l.owner_floor
            || l.store.as_ref().is_some_and(|x| x != &request.owner.store)
            || l.namespace
                .is_some_and(|x| x != request.invocation_namespace)
        {
            return Err(Fault::Ownership);
        }
        let limits = &request.limits;
        if limits.instances == 0
            || limits.attempts == 0
            || limits.invocation_history == 0
            || limits.max_batch_bytes == 0
            || limits.max_name_bytes == 0
            || limits.critical_per_attempt == 0
        {
            return Err(Fault::Capacity);
        }
        // Validate the complete retained image under proposed bounds before ownership changes.
        Self::restore(Recovery {
            owner: request.owner.clone(),
            invocation_namespace: request.invocation_namespace,
            limits: limits.clone(),
            preparing: l.queued.values().cloned().collect(),
            revisions: l.revisions.clone(),
            attempts: l.attempts.values().cloned().collect(),
            next_attempt: l.next_attempt,
            invocation_high_water: l.invocation_floor,
            invocation_count: l.invocation_count,
        })
        .map_err(|e| {
            if e == Fault::Integrity {
                Fault::Capacity
            } else {
                e
            }
        })?;
        let mut reserved = l
            .attempts
            .len()
            .checked_mul(limits.critical_per_attempt)
            .ok_or(Fault::Capacity)?;
        for (instance, bytes) in &l.genesis_custody {
            if instance.incarnation.len() > limits.max_name_bytes {
                return Err(Fault::Capacity);
            }
            reserved = reserved
                .checked_add(bytes.value().len())
                .ok_or(Fault::Capacity)?;
        }
        if l.observations.len() > limits.instances
            || l.observations
                .keys()
                .any(|i| i.incarnation.len() > limits.max_name_bytes)
            || reserved < l.critical_used
            || reserved > limits.critical_bytes
            || l.revisions
                .keys()
                .any(|i| i.incarnation.len() > limits.max_name_bytes)
        {
            return Err(Fault::Capacity);
        }
        l.critical_used = reserved;
        l.store = Some(request.owner.store.clone());
        l.namespace = Some(request.invocation_namespace);
        l.owner_floor = request.owner.generation;
        l.active = Some(request.owner.clone());
        Ok(MemorySession {
            ledger: self.ledger.clone(),
            open: request,
            hold_prepare: false,
            lost_ack: false,
            hold_publish: false,
        })
    }
}
impl MemorySession {
    pub fn fixture_seed(&self) -> OwnedFixtureSeed {
        let l = self.ledger.borrow();
        OwnedFixtureSeed {
            recovery: Recovery {
                owner: self.open.owner.clone(),
                invocation_namespace: self.open.invocation_namespace,
                limits: self.open.limits.clone(),
                preparing: l.queued.values().cloned().collect(),
                revisions: l.revisions.clone(),
                attempts: l.attempts.values().cloned().collect(),
                next_attempt: l.next_attempt,
                invocation_high_water: l.invocation_floor,
                invocation_count: l.invocation_count,
            },
            genesis_custody: l.genesis_custody.clone(),
        }
    }
    pub fn observe_policy(
        &mut self,
        instance: Instance,
        cut: LocalPrecondition,
    ) -> Result<(), Fault> {
        let mut l = self.ledger.borrow_mut();
        let z = &self.open.limits;
        if instance.incarnation.len() > z.max_name_bytes
            || (!l.observations.contains_key(&instance) && l.observations.len() >= z.instances)
        {
            return Err(Fault::Capacity);
        }
        l.observations.insert(instance, cut);
        Ok(())
    }
    pub fn hold_preparation(&mut self, value: bool) {
        self.hold_prepare = value;
    }
    pub fn lose_prepare_ack(&mut self, value: bool) {
        self.lost_ack = value;
    }
    pub fn hold_publication(&mut self, value: bool) {
        self.hold_publish = value;
    }
    pub fn set_attempt_floor(&mut self, value: u64) {
        self.ledger.borrow_mut().next_attempt = value;
    }
    fn authorized(&self, owner: &Owner) -> bool {
        owner == &self.open.owner && self.ledger.borrow().active.as_ref() == Some(owner)
    }
    fn invocation(&self, owner: &Owner, id: &InvocationId) -> Result<(), Fault> {
        if !self.authorized(owner) {
            return Err(Fault::Ownership);
        }
        let mut l = self.ledger.borrow_mut();
        if id.namespace != self.open.invocation_namespace || id.number <= l.invocation_floor {
            return Err(Fault::CallbackMismatch);
        }
        if id.number == u64::MAX || l.invocation_count >= self.open.limits.invocation_history {
            return Err(Fault::Capacity);
        }
        l.invocation_floor = id.number;
        l.invocation_count = l.invocation_count.checked_add(1).ok_or(Fault::Capacity)?;
        Ok(())
    }
    fn validate(&self, b: &Binding) -> Result<(), Fault> {
        validate_binding(&self.open.limits, b)
    }
    fn preparation(&mut self, q: PrepareRequest, recover_only: bool) -> PrepareReply {
        let result = self.prepare_inner(&q, recover_only);
        PrepareReply { request: q, result }
    }
    fn prepare_inner(&mut self, q: &PrepareRequest, recover_only: bool) -> PrepareResult {
        if let Err(e) = self.invocation(&q.owner, &q.invocation) {
            return PrepareResult::Refused(e);
        }
        let mut l = self.ledger.borrow_mut();
        let b = &q.binding;
        if let Some(a) = l.attempts.get(&b.plan) {
            return if a.attempt.binding == *b {
                PrepareResult::Prepared(Box::new(a.attempt.clone()))
            } else {
                PrepareResult::Refused(Fault::BindingMismatch)
            };
        }
        if let Some(queued) = l.queued.get(&b.plan.instance) {
            if queued.plan == b.plan && queued != b {
                return PrepareResult::Refused(Fault::BindingMismatch);
            }
            if queued.plan != b.plan {
                return PrepareResult::Refused(Fault::Unavailable);
            }
        }
        if l.attempts.values().any(|a| {
            a.attempt.id.instance == b.plan.instance
                && !matches!(a.phase, AttemptPhase::Terminal(_))
        }) {
            return PrepareResult::Refused(Fault::Unavailable);
        }
        if recover_only {
            return PrepareResult::Pending;
        }
        if let Err(e) = self.validate(b) {
            return PrepareResult::Refused(e);
        }
        if b.expected_revision != *l.revisions.get(&b.plan.instance).unwrap_or(&0) {
            return PrepareResult::Refused(Fault::BindingMismatch);
        }
        if !l.revisions.contains_key(&b.plan.instance)
            && !l.queued.contains_key(&b.plan.instance)
            && l.revisions
                .len()
                .checked_add(l.queued.len())
                .is_none_or(|n| n >= self.open.limits.instances)
        {
            return PrepareResult::Refused(Fault::Capacity);
        }
        if self.hold_prepare && b.plan.instance.incarnation == "x" {
            l.queued.insert(b.plan.instance.clone(), b.clone());
            return PrepareResult::Pending;
        }
        let Some(next) = l.next_attempt.checked_add(1) else {
            return PrepareResult::Refused(Fault::Capacity);
        };
        let Some(critical) = l
            .critical_used
            .checked_add(self.open.limits.critical_per_attempt)
        else {
            return PrepareResult::Refused(Fault::Capacity);
        };
        if l.attempts.len() >= self.open.limits.attempts
            || critical > self.open.limits.critical_bytes
        {
            return PrepareResult::Refused(Fault::Capacity);
        }
        let a = AttemptBinding {
            id: AttemptId {
                store: q.owner.store.clone(),
                instance: b.plan.instance.clone(),
                number: l.next_attempt,
            },
            binding: b.clone(),
        };
        l.next_attempt = next;
        l.critical_used = critical;
        l.queued.remove(&b.plan.instance);
        l.revisions.entry(b.plan.instance.clone()).or_insert(0);
        l.attempts.insert(
            b.plan.clone(),
            RetainedAttempt {
                attempt: a.clone(),
                phase: AttemptPhase::Reserved,
            },
        );
        if self.lost_ack {
            PrepareResult::Pending
        } else {
            PrepareResult::Prepared(Box::new(a))
        }
    }
    fn resolve(
        &mut self,
        q: &ResolveRequest,
        kind: OperationKind,
        current: Option<&LocalPrecondition>,
    ) -> ResolveResult {
        if let Err(e) = self.invocation(&q.owner, &q.invocation) {
            return ResolveResult::Refused(e);
        }
        if q.operation != kind {
            return ResolveResult::Refused(Fault::CallbackMismatch);
        }
        let mut l = self.ledger.borrow_mut();
        let Some(record) = l.attempts.get(&q.attempt.binding.plan).cloned() else {
            return ResolveResult::Pending;
        };
        if record.attempt != q.attempt {
            return ResolveResult::Refused(Fault::BindingMismatch);
        }
        if let AttemptPhase::Terminal(outcome) = record.phase {
            return ResolveResult::Terminal(Box::new(Terminal {
                attempt: record.attempt,
                outcome,
            }));
        }
        if kind == OperationKind::Inspect {
            return ResolveResult::Pending;
        }
        if kind == OperationKind::Begin {
            if let AttemptPhase::Started { cut } = record.phase {
                drop(l);
                return if self.hold_publish {
                    ResolveResult::Started { cut }
                } else {
                    self.publish(&q.attempt, q.owner.clone())
                };
            }
        }
        let b = &q.attempt.binding;
        let observed = l
            .observations
            .get(&b.plan.instance)
            .cloned()
            .unwrap_or_else(cut);
        let invalid_start = kind == OperationKind::Begin
            && (l.revisions.get(&b.plan.instance) != Some(&b.expected_revision)
                || b.prestart.as_ref().is_some_and(|p| {
                    current != Some(&p.cut)
                        || current != Some(&observed)
                        || current.is_none_or(|c| {
                            c.time.earliest_ms > c.time.latest_ms
                                || p.windows.iter().any(|w| {
                                    c.time.earliest_ms < w.not_before_ms
                                        || c.time.latest_ms > w.not_after_ms
                                })
                        })
                }));
        if kind == OperationKind::Fence || invalid_start {
            let outcome = TerminalOutcome::NonCommit {
                revision: b.expected_revision,
                storage: b.storage.clone(),
            };
            l.attempts.get_mut(&b.plan).unwrap().phase = AttemptPhase::Terminal(outcome.clone());
            return ResolveResult::Terminal(Box::new(Terminal {
                attempt: q.attempt.clone(),
                outcome,
            }));
        }
        let cut = b.prestart.as_ref().map(|p| p.cut.clone());
        l.attempts.get_mut(&b.plan).unwrap().phase = AttemptPhase::Started { cut: cut.clone() };
        drop(l);
        if self.hold_publish {
            ResolveResult::Started { cut }
        } else {
            self.publish(&q.attempt, q.owner.clone())
        }
    }
    pub fn publish(&mut self, a: &AttemptBinding, owner: Owner) -> ResolveResult {
        if !self.authorized(&owner) {
            return ResolveResult::Refused(Fault::Ownership);
        }
        let mut l = self.ledger.borrow_mut();
        let Some(record) = l.attempts.get(&a.binding.plan).cloned() else {
            return ResolveResult::Pending;
        };
        if record.attempt != *a {
            return ResolveResult::Refused(Fault::BindingMismatch);
        }
        match record.phase {
            AttemptPhase::Terminal(outcome) => ResolveResult::Terminal(Box::new(Terminal {
                attempt: a.clone(),
                outcome,
            })),
            AttemptPhase::Reserved => ResolveResult::Pending,
            AttemptPhase::Started { cut } => {
                if l.revisions.get(&a.id.instance) != Some(&a.binding.expected_revision) {
                    return ResolveResult::Refused(Fault::Integrity);
                }
                let Some(revision) = a.binding.expected_revision.checked_add(1) else {
                    return ResolveResult::Refused(Fault::Capacity);
                };
                let outcome = TerminalOutcome::Committed {
                    revision,
                    storage: a.binding.storage.clone(),
                    receipts: a.binding.receipts.clone(),
                    start_cut: cut,
                };
                // One bounded in-memory atomic transition couples exact custody binding and outcome.
                l.revisions.insert(a.id.instance.clone(), revision);
                l.attempts.get_mut(&a.binding.plan).unwrap().phase =
                    AttemptPhase::Terminal(outcome.clone());
                ResolveResult::Terminal(Box::new(Terminal {
                    attempt: a.clone(),
                    outcome,
                }))
            }
        }
    }
}
impl StorageAttemptSession for MemorySession {
    fn recover(&mut self) -> RecoveryResult {
        if !self.authorized(&self.open.owner) {
            return RecoveryResult::Unavailable;
        }
        let l = self.ledger.borrow();
        RecoveryResult::Validated(Box::new(Recovery {
            owner: self.open.owner.clone(),
            invocation_namespace: self.open.invocation_namespace,
            limits: self.open.limits.clone(),
            preparing: l.queued.values().cloned().collect(),
            revisions: l.revisions.clone(),
            attempts: l.attempts.values().cloned().collect(),
            next_attempt: l.next_attempt,
            invocation_high_water: l.invocation_floor,
            invocation_count: l.invocation_count,
        }))
    }
    fn prepare(&mut self, q: PrepareRequest) -> PrepareReply {
        self.preparation(q, false)
    }
    fn recover_plan(&mut self, q: PrepareRequest) -> PrepareReply {
        self.preparation(q, true)
    }
    fn begin(&mut self, q: BeginRequest) -> ResolveReply {
        let result = self.resolve(&q.request, OperationKind::Begin, Some(&q.current));
        ResolveReply {
            request: q.request,
            result,
        }
    }
    fn inspect(&mut self, q: ResolveRequest) -> ResolveReply {
        let result = self.resolve(&q, OperationKind::Inspect, None);
        ResolveReply { request: q, result }
    }
    fn request_fence(&mut self, q: ResolveRequest) -> ResolveReply {
        let result = self.resolve(&q, OperationKind::Fence, None);
        ResolveReply { request: q, result }
    }
    fn close(&mut self) -> CloseResult {
        if !self.authorized(&self.open.owner) {
            return CloseResult::Pending;
        }
        let mut l = self.ledger.borrow_mut();
        if !l.queued.is_empty()
            || l.attempts
                .values()
                .any(|a| !matches!(a.phase, AttemptPhase::Terminal(_)))
        {
            return CloseResult::Pending;
        }
        l.active = None;
        CloseResult::Closed
    }
}
pub fn open_request() -> OpenRequest {
    OpenRequest {
        owner: Owner {
            store: StoreIncarnation([1; 32]),
            generation: 1,
        },
        invocation_namespace: [2; 32],
        limits: Limits {
            instances: 4,
            attempts: 16,
            invocation_history: 128,
            max_batch_bytes: 1024,
            max_receipts: 8,
            max_windows: 8,
            max_name_bytes: 64,
            critical_bytes: 65536,
            critical_per_attempt: 4096,
        },
    }
}
pub fn binding(name: &str, kind: CommitKind, number: u64) -> Binding {
    let instance = Instance {
        descriptor: [3; 32],
        incarnation: name.into(),
    };
    Binding {
        plan: PlanKey {
            namespace: [4; 32],
            instance: instance.clone(),
            number,
        },
        batch: Bytes::new(format!("exact:{name}:{number}").into_bytes()),
        expected_revision: 0,
        storage: StorageClass::VolatileTest,
        kind: kind.clone(),
        charges: Charges {
            edit: 0,
            recovery: 3,
            evidence: 2,
            pending: 1,
        },
        receipts: if kind == CommitKind::AcceptedBatch {
            vec![Receipt {
                instance,
                operation: [5; 32],
                admission: [6; 32],
                stable_id: [7; 32],
                storage: StorageClass::VolatileTest,
            }]
        } else {
            vec![]
        },
        prestart: if kind == CommitKind::AcceptedBatch {
            Some(StartPrecondition {
                cut: cut(),
                windows: vec![Window {
                    not_before_ms: 1,
                    not_after_ms: 100,
                }],
            })
        } else {
            None
        },
    }
}
pub fn cut() -> LocalPrecondition {
    LocalPrecondition {
        policy: [8; 32],
        time: TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        },
    }
}
pub fn prepare_request(b: Binding, n: u64) -> PrepareRequest {
    PrepareRequest {
        owner: open_request().owner,
        invocation: InvocationId {
            namespace: [2; 32],
            number: n,
        },
        binding: b,
    }
}
pub fn resolution(a: AttemptBinding, n: u64, operation: OperationKind) -> ResolveRequest {
    ResolveRequest {
        owner: open_request().owner,
        invocation: InvocationId {
            namespace: [2; 32],
            number: n,
        },
        operation,
        attempt: a,
    }
}
pub fn prepared(reply: PrepareReply) -> AttemptBinding {
    match reply.result {
        PrepareResult::Prepared(a) => *a,
        _ => panic!("must retain prepared attempt"),
    }
}

/// Reusable public-port subset; physical adapters need their additional fault suite.
pub fn portable_conformance<S: StorageAttemptSession>(s: &mut S) -> Result<(), &'static str> {
    let b = binding("x", CommitKind::AcceptedBatch, 1);
    let a = match s.prepare(prepare_request(b.clone(), 1)).result {
        PrepareResult::Prepared(a) => *a,
        _ => return Err("prepare must retain binding"),
    };
    if s.recover_plan(prepare_request(b.clone(), 2)).result
        != PrepareResult::Prepared(Box::new(a.clone()))
    {
        return Err("prepare deduplication");
    }
    let lookup = resolution(a.clone(), 3, OperationKind::Inspect);
    if s.inspect(lookup.clone()).result != ResolveResult::Pending {
        return Err("temporary absence is Pending");
    }
    if s.inspect(lookup).result != ResolveResult::Refused(Fault::CallbackMismatch) {
        return Err("lookup invocation replay");
    }
    let terminal = s
        .begin(BeginRequest {
            request: resolution(a.clone(), 4, OperationKind::Begin),
            current: cut(),
        })
        .result;
    let ResolveResult::Terminal(t) = &terminal else {
        return Err("bounded no-fault publication");
    };
    if !matches!(&t.outcome,TerminalOutcome::Committed {revision:1,storage:StorageClass::VolatileTest,receipts,start_cut:Some(c)} if receipts==&b.receipts && c==&cut())
    {
        return Err("original whole-batch receipts and start cut");
    }
    if s.request_fence(resolution(a.clone(), 5, OperationKind::Fence))
        .result
        != terminal
    {
        return Err("commit wins fence");
    }
    let RecoveryResult::Validated(r) = s.recover() else {
        return Err("complete retained recovery");
    };
    if r.attempts.len() != 1
        || !matches!(
            &r.attempts[0].phase,
            AttemptPhase::Terminal(TerminalOutcome::Committed { .. })
        )
        || r.revisions.get(&a.id.instance) != Some(&1)
    {
        return Err("outcome and visible custody must be coupled");
    }
    let mut negative = binding("y", CommitKind::Candidate, 1);
    negative.charges.edit = 0;
    let y = match s.prepare(prepare_request(negative, 6)).result {
        PrepareResult::Prepared(a) => *a,
        _ => return Err("independent instance"),
    };
    let fenced = s
        .request_fence(resolution(y.clone(), 7, OperationKind::Fence))
        .result;
    if !matches!(&fenced,ResolveResult::Terminal(t) if matches!(t.outcome,TerminalOutcome::NonCommit {revision:0,..}))
    {
        return Err("terminal fence");
    }
    if s.begin(BeginRequest {
        request: resolution(y, 8, OperationKind::Begin),
        current: cut(),
    })
    .result
        != fenced
    {
        return Err("old worker cannot rebase past fence");
    }
    Ok(())
}

fn validate_binding(z: &Limits, b: &Binding) -> Result<(), Fault> {
    if b.batch.value().len() > z.max_batch_bytes
        || b.receipts.len() > z.max_receipts
        || b.plan.instance.incarnation.len() > z.max_name_bytes
        || b.prestart
            .as_ref()
            .is_some_and(|p| p.windows.len() > z.max_windows)
    {
        return Err(Fault::Capacity);
    }
    if b.storage != StorageClass::VolatileTest
        || b.plan.number == u64::MAX
        || b.expected_revision == u64::MAX
        || b.receipts
            .iter()
            .any(|r| r.instance != b.plan.instance || r.storage != b.storage)
        || matches!(b.kind, CommitKind::Candidate | CommitKind::SecurityEvidence)
            && !b.receipts.is_empty()
        || b.kind == CommitKind::AcceptedBatch && b.receipts.is_empty()
    {
        return Err(Fault::BindingMismatch);
    }
    if let Some(p) = &b.prestart {
        if p.windows.is_empty()
            || p.cut.time.earliest_ms > p.cut.time.latest_ms
            || p.windows.iter().any(|w| w.not_before_ms >= w.not_after_ms)
        {
            return Err(Fault::BindingMismatch);
        }
    }
    let mut footprint = 256usize
        .checked_add(b.batch.value().len())
        .ok_or(Fault::Capacity)?;
    footprint = footprint
        .checked_add(b.plan.instance.incarnation.len())
        .ok_or(Fault::Capacity)?;
    for receipt in &b.receipts {
        footprint = footprint
            .checked_add(160)
            .and_then(|n| n.checked_add(receipt.instance.incarnation.len()))
            .ok_or(Fault::Capacity)?;
    }
    if let Some(prestart) = &b.prestart {
        footprint = footprint
            .checked_add(
                prestart
                    .windows
                    .len()
                    .checked_mul(16)
                    .ok_or(Fault::Capacity)?,
            )
            .ok_or(Fault::Capacity)?;
    }
    if footprint > z.critical_per_attempt {
        return Err(Fault::Capacity);
    }
    // Checked charges even though this storage host neither computes eligibility nor consumes edit quota.
    b.charges
        .edit
        .checked_add(b.charges.recovery)
        .and_then(|n| n.checked_add(b.charges.evidence))
        .and_then(|n| n.checked_add(b.charges.pending))
        .ok_or(Fault::Capacity)?;
    Ok(())
}
