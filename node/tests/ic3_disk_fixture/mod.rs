//! Explicit larger disposable storage profile; the original B1 Fixture::new stays exact.
use super::*;
pub(super) fn disk_profile() -> Fixture {
    disk_profile_with(|_| {})
}
pub(super) fn disk_profile_with(change: impl FnOnce(&mut ReplicaLimits)) -> Fixture {
    disk_profile_for("fresh-ic3", change)
}
pub(super) fn disk_profile_named(name: &str) -> Fixture {
    disk_profile_for(name, |_| {})
}
fn disk_profile_for(name: &str, change: impl FnOnce(&mut ReplicaLimits)) -> Fixture {
    let mut f = Fixture::new();
    let mut intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    intent.incarnation = name.into();
    intent.limits.storage.attempts = 4;
    intent.limits.storage.invocation_history = 32;
    intent.limits.storage.critical_per_attempt = 131072;
    intent.limits.storage.critical_bytes = 524288;
    intent.limits.image_bytes = 4_000_000;
    intent.limits.decode.bytes = 4_000_000;
    intent.limits.decode.items = 80000;
    change(&mut intent.limits);
    let bytes = encode_profile(&intent, &limits()).unwrap();
    f.identity.intent = bytes.clone();
    f.descriptor.creation_root = bytes.digest();
    f.descriptor.instance.incarnation = intent.incarnation;
    f.descriptor.canonical = envelope(&signed(
        &ROOT,
        &SignedDescriptorBody {
            intent: bytes.clone(),
            creation_root: bytes.digest(),
        },
    ));
    f.descriptor.instance.descriptor = f.descriptor.canonical.digest();
    let mut certificate: CertificateBody = decode_signed_body(&f.certificate, &limits()).unwrap();
    certificate.instance = f.descriptor.instance.clone();
    certificate.identity = f.identity.clone();
    certificate.creation_root = f.descriptor.creation_root;
    f.certificate = signed(&ROOT, &certificate);
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.instance = f.descriptor.instance.clone();
    policy.identity = f.identity.clone();
    policy.creation_root = f.descriptor.creation_root;
    f.policy = signed(&ROOT, &policy);
    f.query.instance = f.descriptor.instance.clone();
    f.query.descriptor = f.descriptor.clone();
    f.query.evidence = Fixture::proofs(
        &f.query.instance,
        &f.identity,
        &f.descriptor,
        &f.certificate,
        &f.policy,
        &f.query.operation,
        Some((glade_node::signing::public_key(&NODE), &NODE)),
    );
    f
}
pub(super) fn local_authorization<P: PolicyClock>(
    f: &Fixture,
    session: &mut DiskSession<P>,
    nonce: u8,
) {
    let challenge = session
        .issue_challenge(SessionRequest {
            requester: "alice".into(),
            writer_key: glade_node::signing::public_key(&WRITER),
            operation: f.query.operation.canonical().digest(),
            channel: [90; 32],
            session_nonce: [91; 32],
            challenge_nonce: [nonce; 32],
            window: Window {
                not_before_ms: 10,
                not_after_ms: 100,
            },
        })
        .unwrap();
    let body: LocalChallengeBody = decode_signed_body(&challenge, &limits()).unwrap();
    session
        .authenticate(
            signed(
                &WRITER,
                &LocalResponseBody {
                    instance: body.instance,
                    node: body.node,
                    requester: body.requester,
                    writer_key: body.writer_key,
                    signed_challenge: envelope(&challenge).digest(),
                    operation: body.operation,
                    channel: body.channel,
                    session_nonce: body.session_nonce,
                    challenge_nonce: body.challenge_nonce,
                },
            ),
            [90; 32],
            [91; 32],
        )
        .unwrap();
}
