//! Reject behavior-changing host mutants through the same public-port probe.
#[allow(dead_code)]
mod support;
use glade_crdt_storage_attempt_api::*;
use support::*;

#[derive(Clone, Copy, Debug)]
enum Mutation {
    AbsenceIsTerminal,
    DuplicatePrepare,
    ReusedLookup,
    RebaseAfterFence,
    ReceiptRemint,
    OutcomeAfterCustody,
}
struct Mutant<S> {
    inner: S,
    mutation: Mutation,
}
impl<S: StorageAttemptSession> StorageAttemptSession for Mutant<S> {
    fn recover(&mut self) -> RecoveryResult {
        let mut result = self.inner.recover();
        if matches!(self.mutation, Mutation::OutcomeAfterCustody) {
            if let RecoveryResult::Validated(r) = &mut result {
                for a in &mut r.attempts {
                    a.phase = AttemptPhase::Started { cut: None };
                }
            }
        }
        result
    }
    fn prepare(&mut self, q: PrepareRequest) -> PrepareReply {
        self.inner.prepare(q)
    }
    fn recover_plan(&mut self, q: PrepareRequest) -> PrepareReply {
        let mut r = self.inner.recover_plan(q);
        if matches!(self.mutation, Mutation::DuplicatePrepare) {
            if let PrepareResult::Prepared(a) = &mut r.result {
                a.id.number = a.id.number.checked_add(1).unwrap();
            }
        }
        r
    }
    fn begin(&mut self, q: BeginRequest) -> ResolveReply {
        let mut r = self.inner.begin(q);
        if let ResolveResult::Terminal(t) = &mut r.result {
            match self.mutation {
                Mutation::ReceiptRemint => {
                    if let TerminalOutcome::Committed { receipts, .. } = &mut t.outcome {
                        for receipt in receipts {
                            receipt.stable_id = [9; 32];
                        }
                    }
                }
                Mutation::RebaseAfterFence => {
                    if matches!(t.outcome, TerminalOutcome::NonCommit { .. }) {
                        t.outcome = TerminalOutcome::Committed {
                            revision: 1,
                            storage: StorageClass::VolatileTest,
                            receipts: vec![],
                            start_cut: None,
                        };
                    }
                }
                _ => {}
            }
        }
        r
    }
    fn inspect(&mut self, q: ResolveRequest) -> ResolveReply {
        let mut r = self.inner.inspect(q);
        match self.mutation {
            Mutation::AbsenceIsTerminal if r.result == ResolveResult::Pending => {
                r.result = ResolveResult::Terminal(Box::new(Terminal {
                    attempt: r.request.attempt.clone(),
                    outcome: TerminalOutcome::NonCommit {
                        revision: 0,
                        storage: StorageClass::VolatileTest,
                    },
                }));
            }
            Mutation::ReusedLookup
                if r.result == ResolveResult::Refused(Fault::CallbackMismatch) =>
            {
                r.result = ResolveResult::Pending;
            }
            _ => {}
        }
        r
    }
    fn request_fence(&mut self, q: ResolveRequest) -> ResolveReply {
        self.inner.request_fence(q)
    }
    fn close(&mut self) -> CloseResult {
        self.inner.close()
    }
}
#[test]
fn shared_public_port_probe_accepts_fixture_and_rejects_six_semantic_mutants() {
    let mut host = MemoryHost::new();
    assert_eq!(
        portable_conformance(&mut host.open(open_request()).unwrap()),
        Ok(())
    );
    for mutation in [
        Mutation::AbsenceIsTerminal,
        Mutation::DuplicatePrepare,
        Mutation::ReusedLookup,
        Mutation::RebaseAfterFence,
        Mutation::ReceiptRemint,
        Mutation::OutcomeAfterCustody,
    ] {
        let mut host = MemoryHost::new();
        let mut s = Mutant {
            inner: host.open(open_request()).unwrap(),
            mutation,
        };
        assert!(portable_conformance(&mut s).is_err(), "{mutation:?}");
    }
}
