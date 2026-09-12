//! Resource cleanup obligations, not a scheduler, actor system or sdax port.
use std::{future::Future, time::Duration};
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Open,
    Closing,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Drain,
    Cancel,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shutdown {
    pub mode: Mode,
    pub budget: Duration,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub resource: String,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    pub remaining: Vec<String>,
    pub failures: Vec<Failure>,
}

/// A resource owner, not an async executor. Shutdown MUST stop admitting new work
/// when first polled. Drain permits existing work within the monotonic budget;
/// Cancel requests cooperative cancellation. Neither means rollback of external
/// effects. Budget expiry MUST return an honest remaining-resource report.
///
/// Reports MUST retain cleanup failures (one failure MUST NOT hide another).
/// Closed requires no remaining owned work/resources; otherwise phase is Closing.
/// Repeated shutdown MUST retry unfinished cleanup without repeating completed
/// irreversible cleanup, and Closed MUST remain terminal. Errors may remain in
/// the report after resources were successfully released; they are not success
/// claims. Resource lists MUST be bounded/configured at ownership acquisition.
///
/// Futures MUST be lazy. Dropping a polled shutdown leaves ownership and its
/// progress ledger recoverable for another shutdown attempt; drop is not cleanup.
/// Implementations MUST NOT detach untracked tasks to manufacture completion.
/// Dependency ordering belongs to composition; independent cleanup MAY run in
/// parallel. This trait chooses neither channels, threads, macros nor a runtime.
/// Timely return requires cooperative/pollable host operations; adapters must not
/// block an executor thread in an uncancellable operation.
///
/// ```compile_fail
/// use glade_lifecycle_api::ManagedResource;
/// struct Missing;
/// impl ManagedResource for Missing {}
/// ```
pub trait ManagedResource: Send {
    fn phase(&self) -> Phase;
    fn shutdown(&mut self, request: Shutdown) -> impl Future<Output = ShutdownReport> + Send;
}
