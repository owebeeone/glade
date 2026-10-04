//! Private physical operation discrimination; public FloorIntent is unchanged.
use super::*;
use glade_wire::cbor::Cbor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum DiskOperation {
    Checkpoint,
    PrepareAccepted,
    PrepareCandidate,
    PrepareSecurity,
    PrepareFork,
    Start,
    PublishAccepted,
    PublishCandidate,
    PublishSecurity,
    PublishFork,
    Fence,
    BeginIngress,
    SettleIngress,
    AbandonIngress,
    Observation,
    Lease,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingMetadata {
    pub kind: DiskOperation,
    pub intent: Digest,
    pub prior: Selection,
    pub next: Selection,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FloorEnvelope {
    pub floor: ReplicaFloor,
    pub pending: Option<PendingMetadata>,
    pub completion: Option<ReplicaFloor>,
    pub unmaterialized: Vec<PendingMetadata>,
}
impl Canonical for PendingMetadata {
    const KIND: &'static str = "Ic3DiskPending/v1";
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let fields = fields(value, 4)?;
        operation(&fields[0].1)?;
        Digest::preflight_value(&fields[1].1, budget, depth + 1)?;
        Selection::preflight_value(&fields[2].1, budget, depth + 1)?;
        Selection::preflight_value(&fields[3].1, budget, depth + 1)
    }
    fn encode_value(&self) -> Cbor {
        Cbor::Map(vec![
            (0, Cbor::Int(self.kind as i64)),
            (1, self.intent.encode_value()),
            (2, self.prior.encode_value()),
            (3, self.next.encode_value()),
        ])
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let fields = fields(value, 4)?;
        Ok(Self {
            kind: operation(&fields[0].1)?,
            intent: Canonical::decode_value(&fields[1].1)?,
            prior: Canonical::decode_value(&fields[2].1)?,
            next: Canonical::decode_value(&fields[3].1)?,
        })
    }
}
impl Canonical for FloorEnvelope {
    const KIND: &'static str = "Ic3DiskFloor/v1";
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let fields = fields(value, 4)?;
        ReplicaFloor::preflight_value(&fields[0].1, budget, depth + 1)?;
        <Option<PendingMetadata>>::preflight_value(&fields[1].1, budget, depth + 1)?;
        <Option<ReplicaFloor>>::preflight_value(&fields[2].1, budget, depth + 1)?;
        <Vec<PendingMetadata>>::preflight_value(&fields[3].1, budget, depth + 1)
    }
    fn encode_value(&self) -> Cbor {
        Cbor::Map(vec![
            (0, self.floor.encode_value()),
            (1, self.pending.encode_value()),
            (2, self.completion.encode_value()),
            (3, self.unmaterialized.encode_value()),
        ])
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let fields = fields(value, 4)?;
        Ok(Self {
            floor: Canonical::decode_value(&fields[0].1)?,
            pending: Canonical::decode_value(&fields[1].1)?,
            completion: Canonical::decode_value(&fields[2].1)?,
            unmaterialized: Canonical::decode_value(&fields[3].1)?,
        })
    }
}
fn fields(value: &Cbor, count: usize) -> Result<&[(i64, Cbor)], Error> {
    let Cbor::Map(values) = value else {
        return Err(Error::Invalid);
    };
    if values.len() != count
        || values
            .iter()
            .enumerate()
            .any(|(index, (key, _))| *key != index as i64)
    {
        return Err(Error::Invalid);
    }
    Ok(values)
}
fn operation(value: &Cbor) -> Result<DiskOperation, Error> {
    match value {
        Cbor::Int(0) => Ok(DiskOperation::Checkpoint),
        Cbor::Int(1) => Ok(DiskOperation::PrepareAccepted),
        Cbor::Int(2) => Ok(DiskOperation::PrepareCandidate),
        Cbor::Int(3) => Ok(DiskOperation::PrepareSecurity),
        Cbor::Int(4) => Ok(DiskOperation::PrepareFork),
        Cbor::Int(5) => Ok(DiskOperation::Start),
        Cbor::Int(6) => Ok(DiskOperation::PublishAccepted),
        Cbor::Int(7) => Ok(DiskOperation::PublishCandidate),
        Cbor::Int(8) => Ok(DiskOperation::PublishSecurity),
        Cbor::Int(9) => Ok(DiskOperation::PublishFork),
        Cbor::Int(10) => Ok(DiskOperation::Fence),
        Cbor::Int(11) => Ok(DiskOperation::BeginIngress),
        Cbor::Int(12) => Ok(DiskOperation::SettleIngress),
        Cbor::Int(13) => Ok(DiskOperation::AbandonIngress),
        Cbor::Int(14) => Ok(DiskOperation::Observation),
        Cbor::Int(15) => Ok(DiskOperation::Lease),
        _ => Err(Error::Invalid),
    }
}
