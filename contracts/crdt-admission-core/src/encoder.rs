//! Version 1 internal batch encoding. Schema order is fixed; blobs are length
//! framed and interned in first-use order. References preserve every repeated
//! value without debug formatting or a dependency on a remote storage format.
use crate::*;

struct Encoder {
    out: Vec<u8>,
    blobs: Vec<Vec<u8>>,
}
impl Encoder {
    fn n(&mut self, mut n: u64) {
        while n >= 128 {
            self.out.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        self.out.push(n as u8);
    }
    fn b(&mut self, b: bool) {
        self.n(u64::from(b));
    }
    fn blob(&mut self, bytes: &[u8]) {
        if let Some(i) = self.blobs.iter().position(|old| old == bytes) {
            self.n(i as u64 + 1);
        } else {
            self.n(0);
            self.n(bytes.len() as u64);
            self.out.extend_from_slice(bytes);
            self.blobs.push(bytes.to_vec());
        }
    }
    fn text(&mut self, s: &str) {
        self.blob(s.as_bytes());
    }
    fn bytes(&mut self, b: &Bytes) {
        self.blob(b.value());
    }
    fn instance(&mut self, i: &Instance) {
        self.blob(&i.descriptor);
        self.text(&i.incarnation);
    }
    fn time(&mut self, t: &TimeInterval) {
        self.n(t.earliest_ms);
        self.n(t.latest_ms);
    }
    fn window(&mut self, w: &Window) {
        self.n(w.not_before_ms);
        self.n(w.not_after_ms);
    }
    fn local(&mut self, p: &LocalPrecondition) {
        self.blob(&p.policy);
        self.time(&p.time);
    }
    fn storage(&mut self, s: &StorageClass) {
        self.n(match s {
            StorageClass::VolatileTest => 0,
            StorageClass::LocalProcessRestart => 1,
        });
    }
    fn mode(&mut self, m: &Mode) {
        match m {
            Mode::Local {
                current_policy,
                time,
            } => {
                self.n(0);
                self.blob(current_policy);
                self.time(time);
            }
            Mode::Historical => {
                self.n(1);
            }
            Mode::AdmissionSeal => {
                self.n(2);
            }
        }
    }
    fn descriptor(&mut self, d: &Descriptor) {
        self.instance(&d.instance);
        self.bytes(&d.canonical);
        self.blob(&d.creation_root);
        for s in [
            &d.resource,
            &d.share,
            &d.glade_id,
            &d.engine,
            &d.payload_profile,
            &d.admission_profile,
            &d.authorization_profile,
        ] {
            self.text(s);
        }
        self.blob(&d.key);
        self.n(match d.mode {
            AdmissionMode::Independent => 0,
            AdmissionMode::Legacy => 1,
            AdmissionMode::Strong => 2,
        });
        let b = &d.bounds;
        for n in [
            b.max_operation_bytes,
            b.max_origins,
            b.max_refs,
            b.edit_bytes,
            b.recovery_bytes,
            b.evidence_bytes,
            b.pending_bytes,
            b.pending_items,
        ] {
            self.n(n as u64);
        }
    }
    fn evidence(&mut self, e: &EvidenceSet) {
        self.bytes(&e.certificate);
        self.bytes(&e.permit);
        self.bytes(&e.policy);
        self.b(e.admission.is_some());
        if let Some(a) = &e.admission {
            self.bytes(a);
        }
    }
    fn candidate(&mut self, c: &Candidate) {
        self.instance(&c.instance);
        self.bytes(c.operation.canonical());
        self.evidence(&c.evidence);
    }
    fn facts(&mut self, f: &Facts) {
        let c = &f.certificate;
        self.instance(&c.instance);
        self.text(&c.canonical_origin);
        self.text(&c.epoch);
        self.text(&c.display_name);
        self.blob(&c.writer_key);
        self.text(&c.requester);
        self.bytes(&c.canonical);
        for b in [
            f.descriptor_valid,
            f.payload_valid,
            f.signatures_valid,
            f.issuance_unique,
        ] {
            self.b(b);
        }
        self.window(&f.permit_window);
        self.n(f.permit_first_seq as u64);
        self.n(f.permit_last_seq as u64);
        for b in [
            f.current_allowed,
            f.historical_admitter_authorized,
            f.historical_policy_allowed,
        ] {
            self.b(b);
        }
        self.b(f.historical_time.is_some());
        if let Some(t) = &f.historical_time {
            self.time(t);
        }
    }
    fn query(&mut self, q: &Query) {
        self.n(q.id);
        self.instance(&q.instance);
        self.descriptor(&q.descriptor);
        self.bytes(q.operation.canonical());
        self.evidence(&q.evidence);
        self.mode(&q.mode);
        self.b(q.verified_facts.is_some());
        if let Some(f) = &q.verified_facts {
            self.facts(f);
        }
        self.b(q.validated_under.is_some());
        if let Some(m) = &q.validated_under {
            self.mode(m);
        }
    }
    fn receipt(&mut self, r: &Receipt) {
        self.instance(&r.instance);
        self.blob(&r.operation);
        self.blob(&r.admission);
        self.blob(&r.stable_id);
        self.storage(&r.storage);
    }
}
pub(crate) fn encode(state: &State, descriptor: &Descriptor, plan: &CommitPlan) -> Bytes {
    let mut e = Encoder {
        out: b"glade.crdt.batch\0\x01".to_vec(),
        blobs: vec![],
    };
    e.blob(&state.plan_namespace);
    e.blob(&state.invocation_namespace);
    e.n(state.next_effect_id);
    e.b(state.instances.contains_key(&plan.instance));
    if let Some(v) = state.instances.get(&plan.instance) {
        e.storage(&v.storage);
    }
    e.b(state.storage_owner.is_some());
    if let Some(o) = &state.storage_owner {
        e.blob(&o.store.0);
        e.n(o.generation);
    }
    e.descriptor(descriptor);
    e.n(plan.id);
    e.instance(&plan.instance);
    e.n(plan.expected_revision);
    e.n(match plan.kind {
        CommitKind::AcceptedBatch => 0,
        CommitKind::Candidate => 1,
        CommitKind::SecurityEvidence => 2,
        CommitKind::QualifiedFork => 3,
    });
    e.b(plan.local_precondition.is_some());
    if let Some(p) = &plan.local_precondition {
        e.local(p);
    }
    for n in [
        plan.charges.edit,
        plan.charges.recovery,
        plan.charges.evidence,
        plan.charges.pending,
    ] {
        e.n(n as u64);
    }
    e.n(plan.records.len() as u64);
    for c in &plan.records {
        e.candidate(c);
    }
    e.n(plan.acceptances.len() as u64);
    for a in &plan.acceptances {
        e.candidate(&a.candidate);
        e.facts(&a.facts);
        e.query(&a.verified_query);
        e.b(a.seal_query.is_some());
        if let Some(q) = &a.seal_query {
            e.query(q);
        }
        e.bytes(&a.sealed_admission);
        e.receipt(&a.receipt);
    }
    e.n(plan.qualified_forks.len() as u64);
    for f in &plan.qualified_forks {
        e.candidate(&f.first);
        e.candidate(&f.second);
    }
    Bytes::new(e.out)
}
