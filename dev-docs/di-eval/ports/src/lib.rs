//! Dependency-free witness contracts and deterministic fixtures, not Glade API replacements.

use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub trait Clock: Any + Send + Sync {
    fn now(&self) -> u64;
}

pub trait Reader: Any + Send + Sync {
    fn clock(&self) -> Arc<dyn Clock>;
}

#[derive(Clone)]
pub struct FakeClock(Arc<AtomicU64>);

impl FakeClock {
    pub fn new(value: u64) -> Self {
        Self(Arc::new(AtomicU64::new(value)))
    }

    pub fn set(&self, value: u64) {
        self.0.store(value, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct ClockHolder(pub Arc<dyn Clock>);
