//! Fixed v1 assignments; explicit fields, no serde/Debug recovery.
use crate::*;
use glade_wire::cbor::Cbor;
record!(Instance; descriptor, incarnation);
record!(TimeInterval; earliest_ms, latest_ms);
record!(Window; not_before_ms, not_after_ms);
impl Canonical for StorageClass {
    const KIND: &'static str = "StorageClass";

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
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::VolatileTest => Cbor::Array(vec![Cbor::Int(0)]),
            Self::LocalProcessRestart => Cbor::Array(vec![Cbor::Int(1)]),
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
            0 if values.len() == 1 => Ok(Self::VolatileTest),
            1 if values.len() == 1 => Ok(Self::LocalProcessRestart),
            _ => Err(Error::Invalid),
        }
    }
}
record!(Receipt; instance, operation, admission, stable_id, storage);
record!(Charges; edit, recovery, evidence, pending);
impl Canonical for CommitKind {
    const KIND: &'static str = "CommitKind";

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
            3 if values.len() == 1 => Ok(()),
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::AcceptedBatch => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Candidate => Cbor::Array(vec![Cbor::Int(1)]),
            Self::SecurityEvidence => Cbor::Array(vec![Cbor::Int(2)]),
            Self::QualifiedFork => Cbor::Array(vec![Cbor::Int(3)]),
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
            0 if values.len() == 1 => Ok(Self::AcceptedBatch),
            1 if values.len() == 1 => Ok(Self::Candidate),
            2 if values.len() == 1 => Ok(Self::SecurityEvidence),
            3 if values.len() == 1 => Ok(Self::QualifiedFork),
            _ => Err(Error::Invalid),
        }
    }
}
record!(LocalPrecondition; policy, time);
record!(PlanKey; namespace, instance, number);
record!(AttemptId; store, instance, number);
record!(InvocationId; namespace, number);
record!(Owner; store, generation);
record!(StartPrecondition; cut, windows);
record!(Binding; plan, batch, expected_revision, storage, kind, charges, receipts, prestart);
record!(AttemptBinding; id, binding);
record!(PrepareRequest; owner, invocation, binding);
impl Canonical for PrepareResult {
    const KIND: &'static str = "PrepareResult";

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
            0 if values.len() == 1 => Ok(()),
            1 if values.len() == 2 => {
                <Box<AttemptBinding>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            2 if values.len() == 2 => {
                <Fault>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Pending => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Prepared(v0) => Cbor::Array(vec![Cbor::Int(1), v0.encode_value()]),
            Self::Refused(v0) => Cbor::Array(vec![Cbor::Int(2), v0.encode_value()]),
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
            0 if values.len() == 1 => Ok(Self::Pending),
            1 if values.len() == 2 => Ok(Self::Prepared(Canonical::decode_value(&values[1])?)),
            2 if values.len() == 2 => Ok(Self::Refused(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
record!(PrepareReply; request, result);
impl Canonical for OperationKind {
    const KIND: &'static str = "OperationKind";

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
            Self::Begin => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Inspect => Cbor::Array(vec![Cbor::Int(1)]),
            Self::Fence => Cbor::Array(vec![Cbor::Int(2)]),
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
            0 if values.len() == 1 => Ok(Self::Begin),
            1 if values.len() == 1 => Ok(Self::Inspect),
            2 if values.len() == 1 => Ok(Self::Fence),
            _ => Err(Error::Invalid),
        }
    }
}
record!(ResolveRequest; owner, invocation, operation, attempt);
record!(BeginRequest; request, current);
impl Canonical for TerminalOutcome {
    const KIND: &'static str = "TerminalOutcome";

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
            0 if values.len() == 5 => {
                <u64>::preflight_value(&values[1], budget, depth + 1)?;
                <StorageClass>::preflight_value(&values[2], budget, depth + 1)?;
                <Vec<Receipt>>::preflight_value(&values[3], budget, depth + 1)?;
                <Option<LocalPrecondition>>::preflight_value(&values[4], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 3 => {
                <u64>::preflight_value(&values[1], budget, depth + 1)?;
                <StorageClass>::preflight_value(&values[2], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Committed {
                revision,
                storage,
                receipts,
                start_cut,
            } => Cbor::Array(vec![
                Cbor::Int(0),
                revision.encode_value(),
                storage.encode_value(),
                receipts.encode_value(),
                start_cut.encode_value(),
            ]),
            Self::NonCommit { revision, storage } => Cbor::Array(vec![
                Cbor::Int(1),
                revision.encode_value(),
                storage.encode_value(),
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
            0 if values.len() == 5 => Ok(Self::Committed {
                revision: Canonical::decode_value(&values[1])?,
                storage: Canonical::decode_value(&values[2])?,
                receipts: Canonical::decode_value(&values[3])?,
                start_cut: Canonical::decode_value(&values[4])?,
            }),
            1 if values.len() == 3 => Ok(Self::NonCommit {
                revision: Canonical::decode_value(&values[1])?,
                storage: Canonical::decode_value(&values[2])?,
            }),
            _ => Err(Error::Invalid),
        }
    }
}
record!(Terminal; attempt, outcome);
impl Canonical for ResolveResult {
    const KIND: &'static str = "ResolveResult";

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
            0 if values.len() == 1 => Ok(()),
            1 if values.len() == 2 => {
                <Option<LocalPrecondition>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            2 if values.len() == 2 => {
                <Box<Terminal>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            3 if values.len() == 2 => {
                <Fault>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Pending => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Started { cut } => Cbor::Array(vec![Cbor::Int(1), cut.encode_value()]),
            Self::Terminal(v0) => Cbor::Array(vec![Cbor::Int(2), v0.encode_value()]),
            Self::Refused(v0) => Cbor::Array(vec![Cbor::Int(3), v0.encode_value()]),
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
            0 if values.len() == 1 => Ok(Self::Pending),
            1 if values.len() == 2 => Ok(Self::Started {
                cut: Canonical::decode_value(&values[1])?,
            }),
            2 if values.len() == 2 => Ok(Self::Terminal(Canonical::decode_value(&values[1])?)),
            3 if values.len() == 2 => Ok(Self::Refused(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
record!(ResolveReply; request, result);
impl Canonical for AttemptPhase {
    const KIND: &'static str = "AttemptPhase";

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
            0 if values.len() == 1 => Ok(()),
            1 if values.len() == 2 => {
                <Option<LocalPrecondition>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            2 if values.len() == 2 => {
                <TerminalOutcome>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Reserved => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Started { cut } => Cbor::Array(vec![Cbor::Int(1), cut.encode_value()]),
            Self::Terminal(v0) => Cbor::Array(vec![Cbor::Int(2), v0.encode_value()]),
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
            0 if values.len() == 1 => Ok(Self::Reserved),
            1 if values.len() == 2 => Ok(Self::Started {
                cut: Canonical::decode_value(&values[1])?,
            }),
            2 if values.len() == 2 => Ok(Self::Terminal(Canonical::decode_value(&values[1])?)),
            _ => Err(Error::Invalid),
        }
    }
}
record!(RetainedAttempt; attempt, phase);
record!(Limits; instances, attempts, invocation_history, max_batch_bytes, max_receipts, max_windows, max_name_bytes, critical_bytes, critical_per_attempt);
record!(OpenRequest; owner, invocation_namespace, limits);
record!(Recovery; owner, invocation_namespace, limits, preparing, revisions, attempts, next_attempt, invocation_high_water, invocation_count);
impl Canonical for RecoveryResult {
    const KIND: &'static str = "RecoveryResult";

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
                <Box<Recovery>>::preflight_value(&values[1], budget, depth + 1)?;
                Ok(())
            }
            1 if values.len() == 1 => Ok(()),
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Validated(v0) => Cbor::Array(vec![Cbor::Int(0), v0.encode_value()]),
            Self::Unavailable => Cbor::Array(vec![Cbor::Int(1)]),
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
            0 if values.len() == 2 => Ok(Self::Validated(Canonical::decode_value(&values[1])?)),
            1 if values.len() == 1 => Ok(Self::Unavailable),
            _ => Err(Error::Invalid),
        }
    }
}
impl Canonical for CloseResult {
    const KIND: &'static str = "CloseResult";

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
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::Closed => Cbor::Array(vec![Cbor::Int(0)]),
            Self::Pending => Cbor::Array(vec![Cbor::Int(1)]),
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
            0 if values.len() == 1 => Ok(Self::Closed),
            1 if values.len() == 1 => Ok(Self::Pending),
            _ => Err(Error::Invalid),
        }
    }
}
impl Canonical for Fault {
    const KIND: &'static str = "Fault";

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
            3 if values.len() == 1 => Ok(()),
            4 if values.len() == 1 => Ok(()),
            5 if values.len() == 1 => Ok(()),
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        match self {
            Self::BindingMismatch => Cbor::Array(vec![Cbor::Int(0)]),
            Self::CallbackMismatch => Cbor::Array(vec![Cbor::Int(1)]),
            Self::Ownership => Cbor::Array(vec![Cbor::Int(2)]),
            Self::Capacity => Cbor::Array(vec![Cbor::Int(3)]),
            Self::Integrity => Cbor::Array(vec![Cbor::Int(4)]),
            Self::Unavailable => Cbor::Array(vec![Cbor::Int(5)]),
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
            0 if values.len() == 1 => Ok(Self::BindingMismatch),
            1 if values.len() == 1 => Ok(Self::CallbackMismatch),
            2 if values.len() == 1 => Ok(Self::Ownership),
            3 if values.len() == 1 => Ok(Self::Capacity),
            4 if values.len() == 1 => Ok(Self::Integrity),
            5 if values.len() == 1 => Ok(Self::Unavailable),
            _ => Err(Error::Invalid),
        }
    }
}
