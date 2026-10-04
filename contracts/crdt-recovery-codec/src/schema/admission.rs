//! Fixed v1 assignments; explicit fields, no serde/Debug recovery.
use crate::*;
use glade_wire::cbor::Cbor;
use std::collections::BTreeMap;
impl Canonical for AdmissionMode {
    const KIND: &'static str = "AdmissionMode";

    fn preflight_value(
        value: &Cbor,
        _budget: &mut DecodeBudget<'_>,
        _depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 1 => Ok(()),
            1 if values.len() == 1 => Ok(()),
            2 if values.len() == 1 => Ok(()),
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Independent => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Legacy => Cbor::Array(vec![Cbor::Int(1)]),
            Self::Strong => Cbor::Array(vec![Cbor::Int(2)]),
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
            0 if values.len() == 1 => Ok(Self::Independent),
            1 if values.len() == 1 => Ok(Self::Legacy),
            2 if values.len() == 1 => Ok(Self::Strong),
            _ => Err(Error::Invalid),
        }
    }
}
record!(Bounds; max_operation_bytes, max_origins, max_refs, edit_bytes, recovery_bytes, evidence_bytes, pending_bytes, pending_items);
record!(Descriptor; instance, canonical, creation_root, resource, share, glade_id, key, engine, payload_profile, admission_profile, authorization_profile, mode, bounds);
record!(Slot; instance, origin, seq);
record!(OriginCertificate; instance, canonical_origin, epoch, display_name, writer_key, requester, canonical);
record!(EvidenceSet; certificate, permit, policy, admission);
impl Canonical for Mode {
    const KIND: &'static str = "Mode";

    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 3 => {
                <Digest>::preflight_value(&values[1], budget, depth + 1)?;
                <TimeInterval>::preflight_value(&values[2], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 1 => Ok(()),
            2 if values.len() == 1 => Ok(()),
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Local {
                current_policy,
                time,
            } => Cbor::Array(vec![
                Cbor::Int(0),
                current_policy.encode_value(),
                time.encode_value(),
            ]),
            Self::Historical => Cbor::Array(vec![Cbor::Int(1)]),
            Self::AdmissionSeal => Cbor::Array(vec![Cbor::Int(2)]),
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
            0 if values.len() == 3 => Ok(Self::Local {
                current_policy: Canonical::decode_value(&values[1])?,
                time: Canonical::decode_value(&values[2])?,
            }),
            1 if values.len() == 1 => Ok(Self::Historical),
            2 if values.len() == 1 => Ok(Self::AdmissionSeal),
            _ => Err(Error::Invalid),
        }
    }
}
record!(Query; id, instance, descriptor, operation, evidence, mode, verified_facts, validated_under);
record!(Facts; certificate, descriptor_valid, payload_valid, signatures_valid, issuance_unique, permit_window, permit_first_seq, permit_last_seq, current_allowed, historical_admitter_authorized, historical_policy_allowed, historical_time);
impl Canonical for Verdict {
    const KIND: &'static str = "Verdict";

    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 2 => {
                <Box<Facts>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 1 => Ok(()),
            2 if values.len() == 1 => Ok(()),
            3 if values.len() == 2 => {
                <Bytes>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Verified(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::Invalid => Cbor::Array(vec![Cbor::Int(1)]),
            Self::Unavailable => Cbor::Array(vec![Cbor::Int(2)]),
            Self::Sealed(v0) => Cbor::Array(vec![Cbor::Int(3), v0.encode_value()]),
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
            0 if values.len() == 2 => Ok(Self::Verified(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 1 => Ok(Self::Invalid),
            2 if values.len() == 1 => Ok(Self::Unavailable),
            3 if values.len() == 2 => Ok(Self::Sealed(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
record!(EvidenceReply; query, verdict);
record!(Candidate; instance, operation, evidence);
record!(Accepted; candidate, facts, receipt);
record!(StagedAcceptance; candidate, facts, verified_query, seal_query, sealed_admission, receipt);
record!(PendingVerification; query, candidate, verified, verified_query);
record!(CommitPlan; id, instance, expected_revision, local_precondition, batch, kind, records, acceptances, qualified_forks, charges);
record!(KernelAttempt; attempt, phase);
impl Canonical for StorageInvocation {
    const KIND: &'static str = "StorageInvocation";

    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 2 => {
                <PrepareRequest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 2 => {
                <PrepareRequest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            2 if values.len() == 2 => {
                <ResolveRequest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Prepare(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::RecoverPlan(v0) => Cbor::Array(vec![Cbor::Int(1), v0.encode_value()]),
            Self::Resolve(v0) => Cbor::Array(vec![Cbor::Int(2), v0.encode_value()]),
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
            0 if values.len() == 2 => Ok(Self::Prepare(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 2 => Ok(Self::RecoverPlan(Canonical::decode_value(&values[1])?)),
            2 if values.len() == 2 => Ok(Self::Resolve(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
record!(ForkEvidence; first, second);
record!(ReadCut; instance, contiguous_common_heads, historically_qualified, fork_slots, eligible, pending, quarantine, complete_local);
impl Canonical for Outcome {
    const KIND: &'static str = "Outcome";

    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 2 => {
                <Receipt>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 2 => {
                <Receipt>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            2 if values.len() == 6 => {
                <Instance>::preflight_value(&values[1], budget, depth + 1)?;
                <CommitKind>::preflight_value(&values[2], budget, depth + 1)?;
                <Digest>::preflight_value(&values[3], budget, depth + 1)?;
                <u64>::preflight_value(&values[4], budget, depth + 1)?;
                <StorageClass>::preflight_value(&values[5], budget, depth + 1)?;
                Ok(())
            }
            3 if values.len() == 2 => {
                <Digest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            4 if values.len() == 2 => {
                <Vec<Slot>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            5 if values.len() == 1 => Ok(()),
            6 if values.len() == 1 => Ok(()),
            7 if values.len() == 1 => Ok(()),
            8 if values.len() == 1 => Ok(()),
            9 if values.len() == 1 => Ok(()),
            10 if values.len() == 1 => Ok(()),
            11 if values.len() == 1 => Ok(()),
            12 if values.len() == 1 => Ok(()),
            13 if values.len() == 2 => {
                <ReadCut>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::AcceptedLocal(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::ExactRetry(v0) => Cbor::Array(vec![Cbor::Int(1), v0.encode_value()]),
            Self::BatchRetained {
                instance,
                kind,
                batch_digest,
                revision,
                storage,
            } => Cbor::Array(vec![
                Cbor::Int(2),
                instance.encode_value(),
                kind.encode_value(),
                batch_digest.encode_value(),
                revision.encode_value(),
                storage.encode_value(),
            ]),
            Self::OutcomeUnknown(v0) => Cbor::Array(vec![Cbor::Int(3), v0.encode_value()]),
            Self::MissingHistory(v0) => Cbor::Array(vec![Cbor::Int(4), v0.encode_value()]),
            Self::PolicyPending => Cbor::Array(vec![Cbor::Int(5)]),
            Self::Denied => Cbor::Array(vec![Cbor::Int(6)]),
            Self::Invalid => Cbor::Array(vec![Cbor::Int(7)]),
            Self::Unsupported => Cbor::Array(vec![Cbor::Int(8)]),
            Self::Capacity => Cbor::Array(vec![Cbor::Int(9)]),
            Self::IntegrityConflict => Cbor::Array(vec![Cbor::Int(10)]),
            Self::CallbackMismatch => Cbor::Array(vec![Cbor::Int(11)]),
            Self::Unavailable => Cbor::Array(vec![Cbor::Int(12)]),
            Self::Read(v0) => Cbor::Array(vec![Cbor::Int(13), v0.encode_value()]),
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
            0 if values.len() == 2 => Ok(Self::AcceptedLocal(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 2 => Ok(Self::ExactRetry(Canonical::decode_value(&values[1])?)),
            2 if values.len() == 6 => Ok(Self::BatchRetained {
                instance: Canonical::decode_value(&values[1])?,
                kind: Canonical::decode_value(&values[2])?,
                batch_digest: Canonical::decode_value(&values[3])?,
                revision: Canonical::decode_value(&values[4])?,
                storage: Canonical::decode_value(&values[5])?,
            }),
            3 if values.len() == 2 => {
                Ok(Self::OutcomeUnknown(Canonical::decode_value(&values[1])?))
            }
            4 if values.len() == 2 => {
                Ok(Self::MissingHistory(Canonical::decode_value(&values[1])?))
            }
            5 if values.len() == 1 => Ok(Self::PolicyPending),
            6 if values.len() == 1 => Ok(Self::Denied),
            7 if values.len() == 1 => Ok(Self::Invalid),
            8 if values.len() == 1 => Ok(Self::Unsupported),
            9 if values.len() == 1 => Ok(Self::Capacity),
            10 if values.len() == 1 => Ok(Self::IntegrityConflict),
            11 if values.len() == 1 => Ok(Self::CallbackMismatch),
            12 if values.len() == 1 => Ok(Self::Unavailable),
            13 if values.len() == 2 => Ok(Self::Read(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
impl Canonical for Event {
    const KIND: &'static str = "Event";

    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 2 => {
                <Box<Candidate>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 2 => {
                <Box<Candidate>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            2 if values.len() == 2 => {
                <Box<EvidenceReply>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            3 if values.len() == 2 => {
                <PrepareReply>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            4 if values.len() == 2 => {
                <CommitReply>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            5 if values.len() == 2 => {
                <ResolveReply>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            6 if values.len() == 2 => {
                <LookupReply>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            7 if values.len() == 4 => {
                <Instance>::preflight_value(&values[1], budget, depth + 1)?;
                <Bytes>::preflight_value(&values[2], budget, depth + 1)?;
                <TimeInterval>::preflight_value(&values[3], budget, depth + 1)?;
                Ok(())
            }
            8 if values.len() == 3 => {
                <Instance>::preflight_value(&values[1], budget, depth + 1)?;
                <BTreeMap<String, (i64, Digest)>>::preflight_value(&values[2], budget, depth + 1)?;
                Ok(())
            }
            9 if values.len() == 2 => {
                <Box<ForkEvidence>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            10 if values.len() == 2 => {
                <Instance>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            11 if values.len() == 2 => {
                <Instance>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::SubmitLocal(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::OfferReplica(v0) => Cbor::Array(vec![Cbor::Int(1), v0.encode_value()]),
            Self::EvidenceResolved(v0) => Cbor::Array(vec![Cbor::Int(2), v0.encode_value()]),
            Self::PrepareResolved(v0) => Cbor::Array(vec![Cbor::Int(3), v0.encode_value()]),
            Self::CommitResolved(v0) => Cbor::Array(vec![Cbor::Int(4), v0.encode_value()]),
            Self::FenceResolved(v0) => Cbor::Array(vec![Cbor::Int(5), v0.encode_value()]),
            Self::LookupResolved(v0) => Cbor::Array(vec![Cbor::Int(6), v0.encode_value()]),
            Self::ObservePolicy {
                instance,
                policy,
                time,
            } => Cbor::Array(vec![
                Cbor::Int(7),
                instance.encode_value(),
                policy.encode_value(),
                time.encode_value(),
            ]),
            Self::ObserveFrontier { instance, heads } => Cbor::Array(vec![
                Cbor::Int(8),
                instance.encode_value(),
                heads.encode_value(),
            ]),
            Self::OfferFork(v0) => Cbor::Array(vec![Cbor::Int(9), v0.encode_value()]),
            Self::ResumeRecovery(v0) => Cbor::Array(vec![Cbor::Int(10), v0.encode_value()]),
            Self::Read(v0) => Cbor::Array(vec![Cbor::Int(11), v0.encode_value()]),
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
            0 if values.len() == 2 => Ok(Self::SubmitLocal(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 2 => Ok(Self::OfferReplica(Canonical::decode_value(&values[1])?)),
            2 if values.len() == 2 => {
                Ok(Self::EvidenceResolved(Canonical::decode_value(&values[1])?))
            }
            3 if values.len() == 2 => {
                Ok(Self::PrepareResolved(Canonical::decode_value(&values[1])?))
            }
            4 if values.len() == 2 => {
                Ok(Self::CommitResolved(Canonical::decode_value(&values[1])?))
            }
            5 if values.len() == 2 => Ok(Self::FenceResolved(Canonical::decode_value(&values[1])?)),
            6 if values.len() == 2 => {
                Ok(Self::LookupResolved(Canonical::decode_value(&values[1])?))
            }
            7 if values.len() == 4 => Ok(Self::ObservePolicy {
                instance: Canonical::decode_value(&values[1])?,
                policy: Canonical::decode_value(&values[2])?,
                time: Canonical::decode_value(&values[3])?,
            }),
            8 if values.len() == 3 => Ok(Self::ObserveFrontier {
                instance: Canonical::decode_value(&values[1])?,
                heads: Canonical::decode_value(&values[2])?,
            }),
            9 if values.len() == 2 => Ok(Self::OfferFork(Canonical::decode_value(&values[1])?)),
            10 if values.len() == 2 => {
                Ok(Self::ResumeRecovery(Canonical::decode_value(&values[1])?))
            }
            11 if values.len() == 2 => Ok(Self::Read(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
impl Canonical for Effect {
    const KIND: &'static str = "Effect";

    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        let Some(Cbor::Int(tag)) = values.first() else {
            return Err(Error::Invalid);
        };
        match *tag {
            0 if values.len() == 2 => {
                <Query>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 2 => {
                <Query>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            2 if values.len() == 3 => {
                <CommitPlan>::preflight_value(&values[1], budget, depth + 1)?;
                <PrepareRequest>::preflight_value(&values[2], budget, depth + 1)?;
                Ok(())
            }
            3 if values.len() == 2 => {
                <PrepareRequest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            4 if values.len() == 2 => {
                <BeginRequest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            5 if values.len() == 2 => {
                <ResolveRequest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            6 if values.len() == 2 => {
                <LookupRequest>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            7 if values.len() == 2 => {
                <Vec<Slot>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            8 if values.len() == 2 => {
                <Vec<Candidate>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            9 if values.len() == 2 => {
                <ReadCut>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            10 if values.len() == 2 => {
                <Outcome>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Verify(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::Seal(v0) => Cbor::Array(vec![Cbor::Int(1), v0.encode_value()]),
            Self::Prepare { plan, request } => Cbor::Array(vec![
                Cbor::Int(2),
                plan.encode_value(),
                request.encode_value(),
            ]),
            Self::RecoverPlan(v0) => Cbor::Array(vec![Cbor::Int(3), v0.encode_value()]),
            Self::Begin(v0) => Cbor::Array(vec![Cbor::Int(4), v0.encode_value()]),
            Self::Fence(v0) => Cbor::Array(vec![Cbor::Int(5), v0.encode_value()]),
            Self::Lookup(v0) => Cbor::Array(vec![Cbor::Int(6), v0.encode_value()]),
            Self::RequestMissing(v0) => Cbor::Array(vec![Cbor::Int(7), v0.encode_value()]),
            Self::OfferMissing(v0) => Cbor::Array(vec![Cbor::Int(8), v0.encode_value()]),
            Self::ProjectionChanged(v0) => Cbor::Array(vec![Cbor::Int(9), v0.encode_value()]),
            Self::Report(v0) => Cbor::Array(vec![Cbor::Int(10), v0.encode_value()]),
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
            0 if values.len() == 2 => Ok(Self::Verify(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 2 => Ok(Self::Seal(Canonical::decode_value(&values[1])?)),
            2 if values.len() == 3 => Ok(Self::Prepare {
                plan: Canonical::decode_value(&values[1])?,
                request: Canonical::decode_value(&values[2])?,
            }),
            3 if values.len() == 2 => Ok(Self::RecoverPlan(Canonical::decode_value(&values[1])?)),
            4 if values.len() == 2 => Ok(Self::Begin(Canonical::decode_value(&values[1])?)),
            5 if values.len() == 2 => Ok(Self::Fence(Canonical::decode_value(&values[1])?)),
            6 if values.len() == 2 => Ok(Self::Lookup(Canonical::decode_value(&values[1])?)),
            7 if values.len() == 2 => {
                Ok(Self::RequestMissing(Canonical::decode_value(&values[1])?))
            }
            8 if values.len() == 2 => Ok(Self::OfferMissing(Canonical::decode_value(&values[1])?)),
            9 if values.len() == 2 => Ok(Self::ProjectionChanged(Canonical::decode_value(
                &values[1],
            )?)),
            10 if values.len() == 2 => Ok(Self::Report(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
record!(InstanceState; descriptor, revision, storage, accepted, candidates, security_evidence, qualified_forks, canonical_origins, heads, pending, quarantine, policy, recovery_incomplete, storage_integrity_failed, time_floor_ms, policy_time, used);
record!(State; instances, queries, commits, unknown_commits, lookups, storage_owner, storage_limits, plan_namespace, invocation_namespace, attempts, preparing, storage_invocations, retired_storage_invocations, terminal_results, terminal_conflicts, next_effect_id);
record!(Transition; state, effects);
