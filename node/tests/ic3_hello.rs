//! Genuine HELLO envelope uses the existing accepted inventory domain/context.
mod ic3_auth_fixture;
use glade_crdt_recovery_codec::*;
use glade_node::independent::{record_bytes, sign_record};
use glade_node::signing::{public_key, verify_in};
use glade_signer_api::SignatureStatus;
use ic3_auth_fixture::*;
#[test]
fn genuine_hello_roundtrips_existing_closed_envelope_and_body_context() {
    assert_eq!(HELLO_DOMAIN, INVENTORY_DOMAIN);
    let f = Fixture::new();
    let certificate: CertificateBody = decode_signed_body(&f.certificate, &limits()).unwrap();
    let policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    assert_eq!(certificate.instance, f.query.instance);
    assert!(policy.replica_holders.contains(&public_key(&NODE)));
    let hello = ExchangeHello {
        instance: f.descriptor.instance,
        identity: f.identity,
        source: public_key(&NODE),
        target: [44; 32],
        dialer_endpoint: [55; 32],
        acceptor_endpoint: [66; 32],
        exporter: [77; 32],
        nonce: [88; 32],
        role: "replica.hold".into(),
    };
    let record = sign_record(&NODE, &hello, &limits()).unwrap();
    let raw = record_bytes(&record, &limits()).unwrap();
    let decoded: SignedRecord = decode_profile(raw.value(), &limits()).unwrap();
    assert_eq!(
        decode_signed_body::<ExchangeHello>(&decoded, &limits()).unwrap(),
        hello
    );
    assert_eq!(
        verify_in(
            &public_key(&NODE),
            HELLO_DOMAIN.as_bytes(),
            decoded.body.value(),
            decoded.signature.value()
        ),
        SignatureStatus::Valid
    );
    assert!(
        decode_signed_body::<InventoryHeaderBody>(&decoded, &limits()).is_err(),
        "shared signature domain cannot replace the closed canonical body context"
    );
    let mut wrong = decoded;
    wrong.purpose = "glade/ic3/v1/inventory".into();
    assert!(decode_signed_body::<ExchangeHello>(&wrong, &limits()).is_err());
    assert!(decode_profile::<SignedRecord>(
        record_bytes(&wrong, &limits()).unwrap().value(),
        &limits()
    )
    .is_err());
}
