//! Fixed fresh empty-key text namespace; trusted provision is stronger than names.
use crate::signing::{public_key, sign_in, verify_in};
/// Released concurrent_siblings source_text digest, pinned by the existing Glial control.
pub const TEXT_CORPUS_DIGEST: Digest = [
    0x31, 0xef, 0x14, 0x95, 0xab, 0xbe, 0x47, 0x69, 0x45, 0xfd, 0x07, 0xa1, 0xf0, 0x36, 0x30, 0x81,
    0x78, 0xfc, 0xd5, 0x70, 0x56, 0xec, 0x8d, 0x02, 0x9f, 0xed, 0xfc, 0xdd, 0x55, 0xa8, 0xad, 0x90,
];
use glade_crdt_recovery_codec::*;
use glade_signer_api::SignatureStatus;
use glade_wire::cbor::{self, Cbor};
pub fn sign_record<T: ProofBody>(
    seed: &Digest,
    value: &T,
    limits: &DecodeLimits,
) -> Result<SignedRecord, Fault> {
    let body = encode_profile(value, limits).map_err(codec_fault)?;
    Ok(SignedRecord {
        purpose: T::SIGNATURE_DOMAIN.into(),
        version: 1,
        signer: public_key(seed),
        signature: Bytes::new(sign_in(seed, T::SIGNATURE_DOMAIN.as_bytes(), body.value()).to_vec()),
        body,
    })
}
pub fn record_bytes(record: &SignedRecord, limits: &DecodeLimits) -> Result<Bytes, Fault> {
    encode_profile(record, limits).map_err(codec_fault)
}
pub fn signed_body<T: ProofBody>(
    record: &SignedRecord,
    expected: &Digest,
    limits: &DecodeLimits,
) -> Result<T, Fault> {
    if record.signer != *expected
        || verify_in(
            expected,
            T::SIGNATURE_DOMAIN.as_bytes(),
            record.body.value(),
            record.signature.value(),
        ) != SignatureStatus::Valid
    {
        return Err(Fault::Integrity);
    }
    decode_signed_body(record, limits).map_err(codec_fault)
}
pub fn signed_bytes<T: ProofBody>(
    bytes: &Bytes,
    key: &Digest,
    limits: &DecodeLimits,
) -> Result<T, Fault> {
    let envelope = decode_profile(bytes.value(), limits).map_err(codec_fault)?;
    signed_body(&envelope, key, limits)
}
pub fn codec_fault(error: Error) -> Fault {
    match error {
        Error::Capacity => Fault::Capacity,
        Error::Unavailable => Fault::Unavailable,
        Error::Invalid => Fault::Integrity,
    }
}
pub fn validate_namespace(
    descriptor: &Descriptor,
    identity: &IdentityBinding,
    trusted_root: &Digest,
    limits: &DecodeLimits,
) -> Result<CreationIntent, Fault> {
    if descriptor.canonical.digest() != descriptor.instance.descriptor
        || identity.root != *trusted_root
    {
        return Err(Fault::Integrity);
    }
    let body: SignedDescriptorBody = signed_bytes(&descriptor.canonical, trusted_root, limits)?;
    if body.intent != identity.intent
        || body.creation_root != body.intent.digest()
        || descriptor.creation_root != body.creation_root
    {
        return Err(Fault::Integrity);
    }
    let intent: CreationIntent =
        decode_profile(body.intent.value(), limits).map_err(codec_fault)?;
    let expected_declaration = cbor::encode(&Cbor::Map(vec![
        (0, Cbor::Text("crdt.oracle/v1".into())),
        (1, Cbor::Text("text_crdt.profile/v1".into())),
        (2, Cbor::Text("Crdt".into())),
    ]));
    let expected_schema =
        cbor::encode(&Cbor::Map(vec![(0, Cbor::Text("ic3-empty-key/v1".into()))]));
    let matches = intent.root_key == *trusted_root
        && intent.nonce != [0; 32]
        && intent.incarnation == descriptor.instance.incarnation
        && intent.resource == descriptor.resource
        && !intent.private_owner.is_empty()
        && !intent.resource.is_empty()
        && intent.zone == identity.zone
        && intent.share == descriptor.share
        && intent.glade_id == descriptor.glade_id
        && intent.key.value() == descriptor.key
        && descriptor.key.is_empty()
        && identity.declaration.value() == expected_declaration
        && intent.declaration_hash == identity.declaration.digest()
        && intent.declaration_version == identity.declaration_version
        && identity.declaration_version == "ic3-text/v1"
        && intent.shape == DeclarationShape::Crdt
        && identity.schema.value() == expected_schema
        && intent.schema_hash == identity.schema.digest()
        && intent.schema_identity == identity.schema_identity
        && identity.schema_identity == "ic3-empty-key"
        && intent.schema_version == identity.schema_version
        && identity.schema_version == "1"
        && intent.parameters == identity.parameters
        && identity.parameters.value() == cbor::encode(&Cbor::Map(vec![]))
        && intent.engine == descriptor.engine
        && intent.engine == "crdt.oracle/v1"
        && intent.payload_profile == descriptor.payload_profile
        && intent.payload_profile == "text_crdt.profile/v1"
        && intent.admission_profile == descriptor.admission_profile
        && intent.admission_profile == "glade.crdt.independent-admission/v1"
        && intent.authorization_profile == descriptor.authorization_profile
        && intent.authorization_profile == "bounded-offline-permit/v1"
        && intent.proof_profile == identity.proof_profile
        && intent.proof_profile == "ic3-direct-root-ed25519/v1"
        && intent.recovery_profile == identity.recovery_profile
        && intent.recovery_profile == "ic3-records-floor/v1"
        && intent.retention_policy == identity.retention_policy
        && intent.retention_policy == "full-history-no-gc/v1"
        && intent.rejoin_policy == identity.rejoin_policy
        && intent.rejoin_policy == "exact-evidence-full-cut/v1"
        && intent.corpus_identity == identity.corpus_identity
        && intent.corpus_identity == "taut-shape/text_crdt.profile.v1/concurrent_siblings"
        && intent.corpus_version == identity.corpus_version
        && intent.corpus_version == "1"
        && intent.corpus_digest == identity.corpus_digest
        && intent.corpus_digest == TEXT_CORPUS_DIGEST
        && intent.mode == descriptor.mode
        && intent.mode == AdmissionMode::Independent
        && intent.storage == StorageClass::LocalProcessRestart
        && intent.bounds == descriptor.bounds
        && intent.limits.decode == *limits
        && intent.initial_chain_base == identity.initial_chain_base
        && intent.initial_chain_base == 0
        && intent.migration_generation == identity.migration_generation
        && intent.migration_generation == 0;
    if !matches {
        return Err(Fault::Integrity);
    }
    Ok(intent)
}
