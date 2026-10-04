//! Genuine keys and signed records only; no Facts/receipts from an alternate model.
use glade_crdt_recovery_codec::*;
use glade_node::signing::{public_key, sign_in};
use glade_wire::{
    cbor::{self, Cbor},
    generated::{Op, Shape},
};
pub const ROOT: [u8; 32] = [11; 32];
pub const WRITER: [u8; 32] = [22; 32];
pub const NODE: [u8; 32] = [33; 32];
pub fn limits() -> DecodeLimits {
    DecodeLimits {
        bytes: 1_000_000,
        items: 20_000,
        depth: 64,
        name_bytes: 512,
    }
}
pub fn signed<T: ProofBody>(seed: &Digest, body: &T) -> SignedRecord {
    let body = encode_profile(body, &limits()).unwrap();
    SignedRecord {
        purpose: T::SIGNATURE_DOMAIN.into(),
        version: 1,
        signer: public_key(seed),
        signature: Bytes::new(sign_in(seed, T::SIGNATURE_DOMAIN.as_bytes(), body.value()).to_vec()),
        body,
    }
}
pub fn envelope(record: &SignedRecord) -> Bytes {
    encode_profile(record, &limits()).unwrap()
}
pub struct Fixture {
    pub descriptor: Descriptor,
    pub identity: IdentityBinding,
    pub certificate: SignedRecord,
    pub policy: SignedRecord,
    pub query: Query,
}
impl Fixture {
    pub fn new() -> Self {
        let bounds = Bounds {
            max_operation_bytes: 8192,
            max_origins: 8,
            max_refs: 16,
            edit_bytes: 1_000_000,
            recovery_bytes: 100_000,
            evidence_bytes: 100_000,
            pending_bytes: 100_000,
            pending_items: 32,
        };
        let decode = limits();
        let replica = ReplicaLimits {
            storage: Limits {
                instances: 2,
                attempts: 32,
                invocation_history: 128,
                max_batch_bytes: 500_000,
                max_receipts: 8,
                max_windows: 8,
                max_name_bytes: 512,
                critical_bytes: 100_000,
                critical_per_attempt: 1024,
            },
            decode: decode.clone(),
            image_bytes: 1_000_000,
            guard_history: 64,
            inbox_items: 64,
            inbox_bytes: 100_000,
            inventory_items: 128,
            inventory_bytes: 100_000,
        };
        let declaration = Bytes::new(cbor::encode(&Cbor::Map(vec![
            (0, Cbor::Text("crdt.oracle/v1".into())),
            (1, Cbor::Text("text_crdt.profile/v1".into())),
            (2, Cbor::Text("Crdt".into())),
        ])));
        let schema = Bytes::new(cbor::encode(&Cbor::Map(vec![(
            0,
            Cbor::Text("ic3-empty-key/v1".into()),
        )])));
        let parameters = Bytes::new(cbor::encode(&Cbor::Map(vec![])));
        let intent = CreationIntent {
            root_key: public_key(&ROOT),
            nonce: [44; 32],
            incarnation: "fresh-ic3".into(),
            resource: "private:alice:text".into(),
            private_owner: "alice".into(),
            zone: "private".into(),
            share: "ic3-text".into(),
            glade_id: "document".into(),
            key: Bytes::new(vec![]),
            declaration_hash: declaration.digest(),
            declaration_version: "ic3-text/v1".into(),
            shape: DeclarationShape::Crdt,
            schema_identity: "ic3-empty-key".into(),
            schema_hash: schema.digest(),
            schema_version: "1".into(),
            parameters: parameters.clone(),
            engine: "crdt.oracle/v1".into(),
            payload_profile: "text_crdt.profile/v1".into(),
            admission_profile: "glade.crdt.independent-admission/v1".into(),
            authorization_profile: "bounded-offline-permit/v1".into(),
            proof_profile: "ic3-direct-root-ed25519/v1".into(),
            recovery_profile: "ic3-records-floor/v1".into(),
            retention_policy: "full-history-no-gc/v1".into(),
            rejoin_policy: "exact-evidence-full-cut/v1".into(),
            corpus_identity: "taut-shape/text_crdt.profile.v1/concurrent_siblings".into(),
            corpus_version: "1".into(),
            corpus_digest: [
                0x31, 0xef, 0x14, 0x95, 0xab, 0xbe, 0x47, 0x69, 0x45, 0xfd, 0x07, 0xa1, 0xf0, 0x36,
                0x30, 0x81, 0x78, 0xfc, 0xd5, 0x70, 0x56, 0xec, 0x8d, 0x02, 0x9f, 0xed, 0xfc, 0xdd,
                0x55, 0xa8, 0xad, 0x90,
            ],
            mode: AdmissionMode::Independent,
            storage: StorageClass::LocalProcessRestart,
            bounds: bounds.clone(),
            limits: replica,
            initial_chain_base: 0,
            migration_generation: 0,
        };
        let intent_bytes = encode_profile(&intent, &decode).unwrap();
        let creation_root = intent_bytes.digest();
        let identity = IdentityBinding {
            root: intent.root_key,
            intent: intent_bytes.clone(),
            zone: intent.zone.clone(),
            parameters,
            declaration,
            declaration_version: intent.declaration_version.clone(),
            schema,
            schema_identity: intent.schema_identity.clone(),
            schema_version: intent.schema_version.clone(),
            corpus_identity: intent.corpus_identity.clone(),
            corpus_version: intent.corpus_version.clone(),
            corpus_digest: intent.corpus_digest,
            proof_profile: intent.proof_profile.clone(),
            recovery_profile: intent.recovery_profile.clone(),
            retention_policy: intent.retention_policy.clone(),
            rejoin_policy: intent.rejoin_policy.clone(),
            initial_chain_base: 0,
            migration_generation: 0,
        };
        let descriptor_record = signed(
            &ROOT,
            &SignedDescriptorBody {
                intent: intent_bytes,
                creation_root,
            },
        );
        let canonical = envelope(&descriptor_record);
        let instance = Instance {
            descriptor: canonical.digest(),
            incarnation: intent.incarnation,
        };
        let descriptor = Descriptor {
            instance: instance.clone(),
            canonical,
            creation_root,
            resource: intent.resource.clone(),
            share: intent.share,
            glade_id: intent.glade_id,
            key: vec![],
            engine: intent.engine,
            payload_profile: intent.payload_profile,
            admission_profile: intent.admission_profile,
            authorization_profile: intent.authorization_profile,
            mode: AdmissionMode::Independent,
            bounds,
        };
        let certificate = signed(
            &ROOT,
            &CertificateBody {
                instance: instance.clone(),
                identity: identity.clone(),
                resource: descriptor.resource.clone(),
                canonical_origin: "writer-e0".into(),
                epoch: "e0".into(),
                display_name: "writer".into(),
                writer_key: public_key(&WRITER),
                requester: "alice".into(),
                creation_root,
                issuance: 0,
            },
        );
        let policy = signed(
            &ROOT,
            &PolicyBody {
                instance: instance.clone(),
                identity: identity.clone(),
                creation_root,
                generation: 0,
                previous: None,
                window: Window {
                    not_before_ms: 0,
                    not_after_ms: 1000,
                },
                grants: vec![ScopedGrant {
                    requester: "alice".into(),
                    key: public_key(&WRITER),
                    action: ResourceAction::Append,
                    resource: descriptor.resource.clone(),
                }],
                revoked: vec![],
                admitting_nodes: vec![public_key(&NODE)],
                replica_holders: vec![public_key(&NODE)],
            },
        );
        let operation = Operation::new(Op {
            share: descriptor.share.clone(),
            glade_id: descriptor.glade_id.clone(),
            key: vec![],
            origin: "writer-e0".into(),
            seq: 0,
            prev: None,
            lamport: 1,
            refs: vec![],
            shape: Shape::Crdt,
            payload: br#"{"after":null,"atom_id":"a","kind":"insert","text":"A"}"#.to_vec(),
        });
        let query = Query {
            id: 1,
            instance: instance.clone(),
            descriptor: descriptor.clone(),
            operation: operation.clone(),
            evidence: Self::proofs(
                &instance,
                &identity,
                &descriptor,
                &certificate,
                &policy,
                &operation,
                Some((public_key(&NODE), &NODE)),
            ),
            mode: Mode::Historical,
            verified_facts: None,
            validated_under: None,
        };
        Self {
            descriptor,
            identity,
            certificate,
            policy,
            query,
        }
    }
    pub fn proofs(
        instance: &Instance,
        identity: &IdentityBinding,
        descriptor: &Descriptor,
        certificate: &SignedRecord,
        policy: &SignedRecord,
        operation: &Operation,
        admission: Option<(Digest, &Digest)>,
    ) -> EvidenceSet {
        let certificate_bytes = envelope(certificate);
        let policy_bytes = envelope(policy);
        let permit = signed(
            &ROOT,
            &PermitBody {
                instance: instance.clone(),
                identity: identity.clone(),
                resource: descriptor.resource.clone(),
                certificate: certificate_bytes.digest(),
                requester: "alice".into(),
                writer_key: public_key(&WRITER),
                canonical_origin: operation.inner().origin.clone(),
                action: ResourceAction::Append,
                admitting_nodes: vec![public_key(&NODE)],
                policy: policy_bytes.digest(),
                policy_generation: 0,
                window: Window {
                    not_before_ms: 10,
                    not_after_ms: 100,
                },
                first_seq: 0,
                last_seq: 8,
            },
        );
        let permit_bytes = envelope(&permit);
        let proof = signed(
            &WRITER,
            &OperationProofBody {
                instance: instance.clone(),
                identity: identity.clone(),
                resource: descriptor.resource.clone(),
                operation: operation.canonical().clone(),
                certificate: certificate_bytes.digest(),
                signed_permit: permit_bytes.digest(),
                requester: "alice".into(),
            },
        );
        let wrapper = encode_profile(
            &PermitWrapper {
                signed_permit: permit,
                writer_proof: proof,
            },
            &limits(),
        )
        .unwrap();
        let admission = admission.map(|(node, seed)| {
            envelope(&signed(
                seed,
                &OriginalAdmissionBody {
                    instance: instance.clone(),
                    identity: identity.clone(),
                    resource: descriptor.resource.clone(),
                    operation: operation.canonical().digest(),
                    certificate: certificate_bytes.digest(),
                    signed_permit: permit_bytes.digest(),
                    policy: policy_bytes.digest(),
                    policy_generation: 0,
                    time: TimeInterval {
                        earliest_ms: 20,
                        latest_ms: 21,
                    },
                    admitting_node: node,
                    storage: StorageClass::LocalProcessRestart,
                    stable_id: operation.canonical().digest(),
                },
            ))
        });
        EvidenceSet {
            certificate: certificate_bytes,
            permit: wrapper,
            policy: policy_bytes,
            admission,
        }
    }
}
