//! Fixed v1 assignments; explicit fields, no serde/Debug recovery.
use crate::*;
use glade_wire::cbor::Cbor;
record!(IdentityBinding; root, intent, zone, parameters, declaration, declaration_version, schema, schema_identity, schema_version, corpus_identity, corpus_version, corpus_digest, proof_profile, recovery_profile, retention_policy, rejoin_policy, initial_chain_base, migration_generation);
record!(DecodeLimits; bytes, items, depth, name_bytes);
record!(ReplicaLimits; storage, decode, image_bytes, guard_history, inbox_items, inbox_bytes, inventory_items, inventory_bytes);
record!(ReplicaOpen; instance, descriptor, identity, owner, plan_namespace, invocation_namespace, limits, expected_floor);
record!(GuardId; store, instance, number);
record!(ReceiveGuard; id, owner, lease, source, channel, role, base_generation, base_digest, max_bytes, max_items, deadline_ms);
impl Canonical for GuardStatus {
    const KIND: &'static str = "GuardStatus";
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Active(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::Settled {
                guard,
                observation,
                image_generation,
            } => Cbor::Array(vec![
                Cbor::Int(1),
                guard.encode_value(),
                observation.encode_value(),
                image_generation.encode_value(),
            ]),
            Self::Abandoned {
                guard,
                floor_sequence,
            } => Cbor::Array(vec![
                Cbor::Int(2),
                guard.encode_value(),
                floor_sequence.encode_value(),
            ]),
            Self::Loss {
                guard,
                loss,
                image_generation,
            } => Cbor::Array(vec![
                Cbor::Int(3),
                guard.encode_value(),
                loss.encode_value(),
                image_generation.encode_value(),
            ]),
        }
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 2 => Ok(Self::Active(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 4 => Ok(Self::Settled {
                guard: Canonical::decode_value(&values[1])?,
                observation: Canonical::decode_value(&values[2])?,
                image_generation: Canonical::decode_value(&values[3])?,
            }),
            2 if values.len() == 3 => Ok(Self::Abandoned {
                guard: Canonical::decode_value(&values[1])?,
                floor_sequence: Canonical::decode_value(&values[2])?,
            }),
            3 if values.len() == 4 => Ok(Self::Loss {
                guard: Canonical::decode_value(&values[1])?,
                loss: Canonical::decode_value(&values[2])?,
                image_generation: Canonical::decode_value(&values[3])?,
            }),
            _ => Err(Error::Invalid),
        }
    }
}
record!(Selection; slot, generation, digest);
record!(FloorIntent; issuance, prior, next, next_attempt, invocation_high_water, guard_high_water, guards);
record!(@checked ReplicaFloor; issuance, version, owner, root_registration, path_binding, lease, sequence, selected, next_attempt, invocation_high_water, guard_high_water, guards, intent; |value: &ReplicaFloor| value.version == 1);
impl Canonical for RecordKind {
    const KIND: &'static str = "RecordKind";
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Accepted => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Candidate => Cbor::Array(vec![Cbor::Int(1)]),
            Self::Security => Cbor::Array(vec![Cbor::Int(2)]),
            Self::Fork => Cbor::Array(vec![Cbor::Int(3)]),
        }
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 1 => Ok(Self::Accepted),
            1 if values.len() == 1 => Ok(Self::Candidate),
            2 if values.len() == 1 => Ok(Self::Security),
            3 if values.len() == 1 => Ok(Self::Fork),
            _ => Err(Error::Invalid),
        }
    }
}
record!(RecordRef; kind, origin, seq, operation, bundle, receipt);
record!(Inventory; instance, source, number, entries, total_bytes);
record!(InventoryPage; instance, inventory, index, count, entries);
record!(TransferBundle; instance, candidate, kind, receipt, fork);
record!(@checked SignedRecord; purpose, version, signer, body, signature; |value: &SignedRecord| value.version == 1 && value.signature.value().len() == 64 && [DESCRIPTOR_DOMAIN,CERTIFICATE_DOMAIN,PERMIT_DOMAIN,OPERATION_DOMAIN,POLICY_DOMAIN,CHALLENGE_DOMAIN,ADMISSION_DOMAIN,INVENTORY_DOMAIN].contains(&value.purpose.as_str()));
record!(ExchangeHello; instance, identity, source, target, dialer_endpoint, acceptor_endpoint, exporter, nonce, role);
record!(Observation; id, instance, source, channel, guard, bytes, expected);
impl Canonical for Classification {
    const KIND: &'static str = "Classification";
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Retained(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::Missing => Cbor::Array(vec![Cbor::Int(1)]),
            Self::Rejected(v0) => Cbor::Array(vec![Cbor::Int(2), v0.encode_value()]),
            Self::Loss(v0) => Cbor::Array(vec![Cbor::Int(3), v0.encode_value()]),
        }
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 2 => Ok(Self::Retained(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 1 => Ok(Self::Missing),
            2 if values.len() == 2 => Ok(Self::Rejected(Canonical::decode_value(&values[1])?)),
            3 if values.len() == 2 => Ok(Self::Loss(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
record!(Obligation; observation, expected, classification);
record!(OriginCustody; certificate, next_seq, prev, lamport, frontier, intents, receipts);
record!(@checked ReplicaImage; version, instance, identity, generation, predecessor, state, recovery, prior_floor, origins, observations, obligations, sessions, watermark, permanent_loss, limits; |value: &ReplicaImage| value.version == 1);
record!(Checkpoint; expected_floor, expected_generation, expected_digest, image);
record!(ReconstructionRequest; instance, generation, watermark, manifest);
record!(ReplicaCut; kernel, generation, watermark, manifest, obligations, guards, permanent_loss, complete_local);
record!(ReplicaRecovery; image, authoritative_floor);
record!(CertificateBody; instance, identity, resource, canonical_origin, epoch, display_name, writer_key, requester, creation_root, issuance);
impl Canonical for ResourceAction {
    const KIND: &'static str = "ResourceAction";
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Append => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Read => Cbor::Array(vec![Cbor::Int(1)]),
            Self::ReplicaHold => Cbor::Array(vec![Cbor::Int(2)]),
        }
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 1 => Ok(Self::Append),
            1 if values.len() == 1 => Ok(Self::Read),
            2 if values.len() == 1 => Ok(Self::ReplicaHold),
            _ => Err(Error::Invalid),
        }
    }
}
record!(ScopedGrant; requester, key, action, resource);
record!(PolicyBody; instance, identity, creation_root, generation, previous, window, grants, revoked, admitting_nodes, replica_holders);
record!(PermitBody; instance, identity, resource, certificate, requester, writer_key, canonical_origin, action, admitting_nodes, policy, policy_generation, window, first_seq, last_seq);
record!(OperationProofBody; instance, identity, resource, operation, certificate, signed_permit, requester);
record!(PermitWrapper; signed_permit, writer_proof);
record!(LocalChallengeBody; instance, identity, resource, node, requester, writer_key, operation, role, channel, session_nonce, challenge_nonce, window);
record!(LocalResponseBody; instance, node, requester, writer_key, signed_challenge, operation, channel, session_nonce, challenge_nonce);
record!(OriginalAdmissionBody; instance, identity, resource, operation, certificate, signed_permit, policy, policy_generation, time, admitting_node, storage, stable_id);
record!(OriginFloor; certificate, next_seq, lamport, prev, reserved_intents, frontier);
record!(IssuanceFloor; time_floor_ms, policy_generation, policy_digest, plan_namespace, invocation_namespace, next_effect_id, origins);
record!(SessionCustody; challenge, response, consumed);
impl Canonical for DeclarationShape {
    const KIND: &'static str = "DeclarationShape";
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Crdt => Cbor::Array(vec![Cbor::Int(0)]),
        }
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 1 => Ok(Self::Crdt),
            _ => Err(Error::Invalid),
        }
    }
}
record!(CreationIntent; root_key, nonce, incarnation, resource, private_owner, zone, share, glade_id, key, declaration_hash, declaration_version, shape, schema_identity, schema_hash, schema_version, parameters, engine, payload_profile, admission_profile, authorization_profile, proof_profile, recovery_profile, retention_policy, rejoin_policy, corpus_identity, corpus_version, corpus_digest, mode, storage, bounds, limits, initial_chain_base, migration_generation);
record!(SignedDescriptorBody; intent, creation_root);
record!(InventoryHeaderBody; instance, identity, source, target, channel, nonce, number, pages, entries, total_bytes, manifest);
