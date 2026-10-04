use glade_crdt_admission_data::*;
use glade_wire::generated::Op;
#[test]
fn canonical_operation_owns_bytes_and_recomputes_digest() {
    let op = Operation::new(Op {
        origin: "origin".into(),
        ..Op::default()
    });
    assert_eq!(
        op.canonical(),
        &Bytes::new(glade_wire::cbor::encode(&op.inner().to_cbor()))
    );
    let mut inner = op.inner().clone();
    inner.origin = "other".into();
    assert_ne!(op.canonical(), Operation::new(inner).canonical());
}
