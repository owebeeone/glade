//! GREEN representation/scaffold checks make no admission-success claim.
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
fn refusing_scaffold_preserves_state_and_emits_no_side_effects() {
    let d = descriptor("x");
    let c = candidate(&d, "a", 0, None, vec![], insert("a", None, "A"));
    let state = initial(d.clone());
    for event in [
        Event::SubmitLocal(Box::new(c.clone())),
        Event::OfferReplica(Box::new(c)),
        Event::Read(d.instance),
    ] {
        let result = step(&state, event);
        assert_eq!(result.state, state);
        assert_eq!(result.effects, vec![Effect::Report(Outcome::Unavailable)]);
    }
}
