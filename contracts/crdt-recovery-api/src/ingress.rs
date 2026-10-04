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
                }
                Poll::Ready((permit, value))
            }
        }
    }
}
