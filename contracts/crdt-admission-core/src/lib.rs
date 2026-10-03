//! Internal IC-1 contract and intentionally refusing scaffold. Not a live adapter.
//! No successful admission/reconciliation implementation is included.
mod types;
pub use types::*;

/// Consume explicit inputs only. The future kernel owns no I/O or merge engine.
/// This scaffold preserves state and reports unavailable for every event.
pub fn step(state: &State, _event: Event) -> Transition {
    Transition {
        state: state.clone(),
        effects: vec![Effect::Report(Outcome::Unavailable)],
    }
}
