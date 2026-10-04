//! Owned receive continuation; only its issuer may start polling or retire it.
use glade_crdt_admission_data::{Bytes, Fault, GuardId, ReceiveGuard};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
#[derive(Default)]
struct Status {
    started: bool,
    drained: bool,
    received: Option<(usize, glade_crdt_admission_data::Digest)>,
    closed: bool,
}
pub struct IngressAuthority {
    key: Arc<()>,
    live: BTreeMap<GuardId, Arc<Mutex<Status>>>,
    capacity: usize,
}
pub struct IngressPermit {
    key: Arc<()>,
    guard: ReceiveGuard,
    status: Arc<Mutex<Status>>,
}
pub struct GuardedReceive<F: Future<Output = Result<Bytes, Fault>>> {
    future: Pin<Box<F>>,
    permit: Option<IngressPermit>,
}

impl IngressAuthority {
    pub fn new(capacity: usize) -> Self {
        Self {
            key: Arc::new(()),
            live: BTreeMap::new(),
            capacity,
        }
    }
    pub fn issue(&mut self, guard: ReceiveGuard) -> Result<IngressPermit, Fault> {
        if self.live.contains_key(&guard.id) {
            return Err(Fault::Ownership);
        }
        if self.live.len() >= self.capacity {
            return Err(Fault::Capacity);
        }
        let status = Arc::new(Mutex::new(Status::default()));
        self.live.insert(guard.id.clone(), status.clone());
        Ok(IngressPermit {
            key: self.key.clone(),
            guard,
            status,
        })
    }
    pub fn owns(&self, permit: &IngressPermit) -> bool {
        Arc::ptr_eq(&self.key, &permit.key)
    }
    pub fn receive<F: Future<Output = Result<Bytes, Fault>>>(
        &self,
        permit: IngressPermit,
        make: impl FnOnce() -> F,
    ) -> Result<GuardedReceive<F>, Fault> {
        if !self.owns(&permit) {
            return Err(Fault::Ownership);
        }
        {
            let mut state = permit.status.lock().map_err(|_| Fault::Integrity)?;
            if state.closed || state.started {
                return Err(Fault::Ownership);
            }
            state.started = true;
        }
        Ok(GuardedReceive {
            future: Box::pin(make()),
            permit: Some(permit),
        })
    }
    pub fn abandon(&self, permit: &IngressPermit) -> Result<(), Fault> {
        self.retire(permit, false)
    }
    pub fn settle(&self, permit: &IngressPermit) -> Result<(), Fault> {
        self.retire(permit, true)
    }
    /// Checks drained settlement eligibility without retiring the owned capability.
    pub fn can_settle(&self, permit: &IngressPermit) -> Result<(), Fault> {
        self.eligible(permit, true)
    }
    /// Normal settlement requires the issuer-owned exact successful bounded receive.
    pub fn can_settle_received(&self, permit: &IngressPermit, bytes: &Bytes) -> Result<(), Fault> {
        self.can_settle(permit)?;
        if bytes.value().len() > permit.guard.max_bytes {
            return Err(Fault::Capacity);
        }
        let state = permit.status.lock().map_err(|_| Fault::Integrity)?;
        let Some((len, digest)) = state.received else {
            return Err(Fault::BindingMismatch);
        };
        if bytes.value().len() != len || bytes.digest() != digest {
            return Err(Fault::BindingMismatch);
        }
        Ok(())
    }
    /// Checks never-started abandonment eligibility without retiring the capability.
    pub fn can_abandon(&self, permit: &IngressPermit) -> Result<(), Fault> {
        self.eligible(permit, false)
    }
    fn eligible(&self, permit: &IngressPermit, settle: bool) -> Result<(), Fault> {
        if !self.owns(permit) {
            return Err(Fault::Ownership);
        }
        let state = permit.status.lock().map_err(|_| Fault::Integrity)?;
        if state.closed
            || if settle {
                !state.started || !state.drained
            } else {
                state.started
            }
        {
            return Err(Fault::Ownership);
        }
        Ok(())
    }
    fn retire(&self, permit: &IngressPermit, settle: bool) -> Result<(), Fault> {
        if !self.owns(permit) {
            return Err(Fault::Ownership);
        }
        let mut state = permit.status.lock().map_err(|_| Fault::Integrity)?;
        if state.closed
            || if settle {
                !state.started || !state.drained
            } else {
                state.started
            }
        {
            return Err(Fault::Ownership);
        }
        state.closed = true;
        Ok(())
    }
    pub fn can_close(&self) -> bool {
        self.live
            .values()
            .all(|state| state.lock().is_ok_and(|s| s.closed))
    }
}
impl IngressPermit {
    /// Issuer-recorded successful bounded result, never a caller-authored assertion.
    pub fn received_digest(&self) -> Option<glade_crdt_admission_data::Digest> {
        self.status.lock().ok()?.received.map(|(_, digest)| digest)
    }
    pub fn guard(&self) -> &ReceiveGuard {
        &self.guard
    }
    pub fn was_started(&self) -> bool {
        self.status.lock().map_or(true, |s| s.started)
    }
}
impl<F: Future<Output = Result<Bytes, Fault>>> Future for GuardedReceive<F> {
    type Output = (IngressPermit, F::Output);
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match this.future.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(value) => {
                let permit = this.permit.take().expect("future polled after completion");
                if let Ok(mut state) = permit.status.lock() {
                    state.drained = true;
                    state.received = value
                        .as_ref()
                        .ok()
                        .filter(|bytes| bytes.value().len() <= permit.guard.max_bytes)
                        .map(|bytes| (bytes.value().len(), bytes.digest()));
                }
                Poll::Ready((permit, value))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_started_unjoined_receive_refuses_both_retirement_eligibility_queries() {
        let guard = ReceiveGuard {
            id: GuardId {
                store: glade_crdt_admission_data::StoreIncarnation([1; 32]),
                instance: glade_crdt_admission_data::Instance {
                    descriptor: [2; 32],
                    incarnation: "i".into(),
                },
                number: 1,
            },
            owner: glade_crdt_admission_data::Owner {
                store: glade_crdt_admission_data::StoreIncarnation([1; 32]),
                generation: 1,
            },
            lease: 1,
            source: [3; 32],
            channel: [4; 32],
            role: "inventory".into(),
            base_generation: 0,
            base_digest: [0; 32],
            max_bytes: 1024,
            max_items: 8,
            deadline_ms: 100,
        };
        let mut owner = IngressAuthority::new(8);
        let permit = owner.issue(guard).unwrap();
        let receive = owner
            .receive(permit, std::future::pending::<Result<Bytes, Fault>>)
            .unwrap();
        let permit = receive.permit.as_ref().unwrap();
        assert_eq!(owner.can_settle(permit), Err(Fault::Ownership));
        assert_eq!(owner.can_abandon(permit), Err(Fault::Ownership));
        assert!(!owner.can_close());
        drop(receive);
        assert!(
            !owner.can_close(),
            "dropping a pending worker is not its join/drain evidence"
        );
    }
}
