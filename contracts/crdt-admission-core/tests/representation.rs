//! Immutable representation and deterministic Pure boundary checks.
// Shared fixture has helpers consumed by the separate behavioral suite.
#[allow(dead_code)]
mod support;
use glade_crdt_admission_core::*;
use support::*;

#[test]
fn operation_bytes_and_digest_are_derived_and_owned() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let mut original = c.operation.inner().clone();
    let frozen = c.operation.clone();
    original.origin = "other".into();
    let changed = Operation::new(original);
    assert_ne!(changed.canonical().digest(), frozen.canonical().digest());
    assert_eq!(
        frozen.canonical().value(),
        glade_wire::cbor::encode(&frozen.inner().to_cbor())
    );
    assert_eq!(
        frozen.canonical().digest(),
        Bytes::new(frozen.canonical().value().to_vec()).digest()
    );
}
#[test]
fn pure_step_is_deterministic_preserves_caller_input_and_read_only_state() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let state = initial(d.clone());
    let original = state.clone();
    let event = Event::SubmitLocal(Box::new(c));
    let first = step(&state, event.clone());
    assert_eq!(first, step(&state, event));
    assert_eq!(state, original);
    assert!(first.effects.iter().any(|e| matches!(e, Effect::Verify(_))));
    let read = step(&first.state, Event::Read(d.instance));
    assert_eq!(read.state, first.state);
    assert!(
        matches!(read.effects.as_slice(), [Effect::Report(Outcome::Read(c))] if !c.complete_local)
    );
}
