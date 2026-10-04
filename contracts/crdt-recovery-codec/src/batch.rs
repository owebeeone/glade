//! Decoder for the unchanged IC2 canonical batch v1; never a debug image.
use crate::*;
use glade_wire::generated::Op;
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedBatch {
    pub plan_namespace: Digest,
    pub invocation_namespace: Digest,
    pub next_effect_id: u64,
    pub storage: Option<StorageClass>,
    pub owner: Option<Owner>,
    pub descriptor: Descriptor,
    pub plan: CommitPlan,
}
pub fn decode_batch(bytes: &[u8], limits: &DecodeLimits) -> Result<DecodedBatch, Error> {
    if bytes.len() > limits.bytes {
        return Err(Error::Capacity);
    }
    if limits.depth < 12 {
        return Err(Error::Capacity);
    }
    let body = bytes
        .strip_prefix(b"glade.crdt.batch\0\x01")
        .ok_or(Error::Invalid)?;
    let mut c = Cursor {
        bytes: body,
        index: 0,
        items: 0,
        allocation: 0,
        blobs: vec![],
        limits,
    };
    let plan_namespace = c.digest()?;
    let invocation_namespace = c.digest()?;
    let next_effect_id = c.n()?;
    let storage = c.optional(SelfRead::storage)?;
    let owner = c.optional(|c| {
        Ok(Owner {
            store: StoreIncarnation(c.digest()?),
            generation: c.n()?,
        })
    })?;
    let descriptor = c.descriptor()?;
    let id = c.n()?;
    let instance = c.instance()?;
    let expected_revision = c.n()?;
    let kind = match c.n()? {
        0 => CommitKind::AcceptedBatch,
        1 => CommitKind::Candidate,
        2 => CommitKind::SecurityEvidence,
        3 => CommitKind::QualifiedFork,
        _ => return Err(Error::Invalid),
    };
    let local_precondition = c.optional(SelfRead::local)?;
    let charges = Charges {
        edit: c.size()?,
        recovery: c.size()?,
        evidence: c.size()?,
        pending: c.size()?,
    };
    let records = c.many(SelfRead::candidate)?;
    let acceptances = c.many(|c| {
        Ok(StagedAcceptance {
            candidate: c.candidate()?,
            facts: c.facts()?,
            verified_query: c.query()?,
            seal_query: c.optional(SelfRead::query)?,
            sealed_admission: c.owned()?,
            receipt: c.receipt()?,
        })
    })?;
    let qualified_forks = c.many(|c| {
        Ok(ForkEvidence {
            first: c.candidate()?,
            second: c.candidate()?,
        })
    })?;
    if c.index != body.len() {
        return Err(Error::Invalid);
    }
    c.charge(bytes.len())?;
    let plan = CommitPlan {
        id,
        instance,
        expected_revision,
        local_precondition,
        batch: Bytes::new(bytes.to_vec()),
        kind,
        records,
        acceptances,
        qualified_forks,
        charges,
    };
    Ok(DecodedBatch {
        plan_namespace,
        invocation_namespace,
        next_effect_id,
        storage,
        owner,
        descriptor,
        plan,
    })
}
// Method pointer alias keeps closures' lifetimes local to this decoder.
use Cursor as SelfRead;
struct Cursor<'a> {
    bytes: &'a [u8],
    index: usize,
    items: usize,
    allocation: usize,
    blobs: Vec<&'a [u8]>,
    limits: &'a DecodeLimits,
}
impl<'a> Cursor<'a> {
    fn charge(&mut self, n: usize) -> Result<(), Error> {
        self.allocation = self.allocation.checked_add(n).ok_or(Error::Capacity)?;
        if self.allocation > self.limits.bytes {
            return Err(Error::Capacity);
        }
        Ok(())
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.index.checked_add(n).ok_or(Error::Capacity)?;
        let out = self.bytes.get(self.index..end).ok_or(Error::Invalid)?;
        self.index = end;
        Ok(out)
    }
    fn n(&mut self) -> Result<u64, Error> {
        self.items = self.items.checked_add(1).ok_or(Error::Capacity)?;
        if self.items > self.limits.items {
            return Err(Error::Capacity);
        }
        let mut n = 0_u64;
        for index in 0..10 {
            let b = self.take(1)?[0];
            if index == 9 && b > 1 {
                return Err(Error::Invalid);
            }
            n |= u64::from(b & 127) << (index * 7);
            if b & 128 == 0 {
                if index > 0 && b == 0 {
                    return Err(Error::Invalid);
                }
                return Ok(n);
            }
        }
        Err(Error::Invalid)
    }
    fn size(&mut self) -> Result<usize, Error> {
        usize::try_from(self.n()?).map_err(|_| Error::Capacity)
    }
    fn signed(&mut self) -> Result<i64, Error> {
        Ok(i64::from_ne_bytes(self.n()?.to_ne_bytes()))
    }
    fn boolean(&mut self) -> Result<bool, Error> {
        match self.n()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Invalid),
        }
    }
    fn blob(&mut self) -> Result<&'a [u8], Error> {
        let index = self.size()?;
        if index != 0 {
            return self.blobs.get(index - 1).copied().ok_or(Error::Invalid);
        }
        let size = self.size()?;
        let value = self.take(size)?;
        if self.blobs.contains(&value) {
            return Err(Error::Invalid);
        }
        if self.blobs.len() >= self.limits.items {
            return Err(Error::Capacity);
        }
        self.charge(std::mem::size_of::<&[u8]>())?;
        self.blobs.push(value);
        Ok(value)
    }
    fn digest(&mut self) -> Result<Digest, Error> {
        self.blob()?.try_into().map_err(|_| Error::Invalid)
    }
    fn text(&mut self) -> Result<String, Error> {
        let max = self.limits.name_bytes;
        let b = self.blob()?;
        if b.len() > max {
            return Err(Error::Capacity);
        }
        self.charge(b.len())?;
        std::str::from_utf8(b)
            .map(str::to_owned)
            .map_err(|_| Error::Invalid)
    }
    fn owned(&mut self) -> Result<Bytes, Error> {
        let b = self.blob()?;
        self.charge(b.len())?;
        Ok(Bytes::new(b.to_vec()))
    }
    fn optional<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        if self.boolean()? {
            Ok(Some(read(self)?))
        } else {
            Ok(None)
        }
    }
    fn many<T>(
        &mut self,
        mut read: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let n = self.size()?;
        if n > self.limits.items.saturating_sub(self.items)
            || n > self.bytes.len().saturating_sub(self.index)
        {
            return Err(Error::Capacity);
        }
        let mut out = Vec::new();
        for _ in 0..n {
            self.charge(std::mem::size_of::<T>())?;
            out.push(read(self)?);
        }
        Ok(out)
    }
    fn instance(&mut self) -> Result<Instance, Error> {
        Ok(Instance {
            descriptor: self.digest()?,
            incarnation: self.text()?,
        })
    }
    fn time(&mut self) -> Result<TimeInterval, Error> {
        Ok(TimeInterval {
            earliest_ms: self.n()?,
            latest_ms: self.n()?,
        })
    }
    fn window(&mut self) -> Result<Window, Error> {
        Ok(Window {
            not_before_ms: self.n()?,
            not_after_ms: self.n()?,
        })
    }
    fn local(&mut self) -> Result<LocalPrecondition, Error> {
        Ok(LocalPrecondition {
            policy: self.digest()?,
            time: self.time()?,
        })
    }
    fn storage(&mut self) -> Result<StorageClass, Error> {
        match self.n()? {
            0 => Ok(StorageClass::VolatileTest),
            1 => Ok(StorageClass::LocalProcessRestart),
            _ => Err(Error::Invalid),
        }
    }
    fn mode(&mut self) -> Result<Mode, Error> {
        match self.n()? {
            0 => Ok(Mode::Local {
                current_policy: self.digest()?,
                time: self.time()?,
            }),
            1 => Ok(Mode::Historical),
            2 => Ok(Mode::AdmissionSeal),
            _ => Err(Error::Invalid),
        }
    }
    fn operation(&mut self) -> Result<Operation, Error> {
        let bytes = self.blob()?;
        self.charge(
            bytes
                .len()
                .checked_mul(16)
                .and_then(|n| n.checked_add(std::mem::size_of::<Op>()))
                .ok_or(Error::Capacity)?,
        )?;
        crate::bounds::check(bytes, self.limits)?;
        let op = Operation::new(Op::decode(bytes).map_err(|_| Error::Invalid)?);
        if op.canonical().value() != bytes {
            return Err(Error::Invalid);
        }
        Ok(op)
    }
    fn descriptor(&mut self) -> Result<Descriptor, Error> {
        let instance = self.instance()?;
        let canonical = self.owned()?;
        let creation_root = self.digest()?;
        let resource = self.text()?;
        let share = self.text()?;
        let glade_id = self.text()?;
        let engine = self.text()?;
        let payload_profile = self.text()?;
        let admission_profile = self.text()?;
        let authorization_profile = self.text()?;
        let raw_key = self.blob()?;
        self.charge(raw_key.len())?;
        let key = raw_key.to_vec();
        let mode = match self.n()? {
            0 => AdmissionMode::Independent,
            1 => AdmissionMode::Legacy,
            2 => AdmissionMode::Strong,
            _ => return Err(Error::Invalid),
        };
        let bounds = Bounds {
            max_operation_bytes: self.size()?,
            max_origins: self.size()?,
            max_refs: self.size()?,
            edit_bytes: self.size()?,
            recovery_bytes: self.size()?,
            evidence_bytes: self.size()?,
            pending_bytes: self.size()?,
            pending_items: self.size()?,
        };
        Ok(Descriptor {
            instance,
            canonical,
            creation_root,
            resource,
            share,
            glade_id,
            key,
            engine,
            payload_profile,
            admission_profile,
            authorization_profile,
            mode,
            bounds,
        })
    }
    fn evidence(&mut self) -> Result<EvidenceSet, Error> {
        Ok(EvidenceSet {
            certificate: self.owned()?,
            permit: self.owned()?,
            policy: self.owned()?,
            admission: self.optional(SelfRead::owned)?,
        })
    }
    fn candidate(&mut self) -> Result<Candidate, Error> {
        Ok(Candidate {
            instance: self.instance()?,
            operation: self.operation()?,
            evidence: self.evidence()?,
        })
    }
    fn facts(&mut self) -> Result<Facts, Error> {
        let certificate = OriginCertificate {
            instance: self.instance()?,
            canonical_origin: self.text()?,
            epoch: self.text()?,
            display_name: self.text()?,
            writer_key: self.digest()?,
            requester: self.text()?,
            canonical: self.owned()?,
        };
        Ok(Facts {
            certificate,
            descriptor_valid: self.boolean()?,
            payload_valid: self.boolean()?,
            signatures_valid: self.boolean()?,
            issuance_unique: self.boolean()?,
            permit_window: self.window()?,
            permit_first_seq: self.signed()?,
            permit_last_seq: self.signed()?,
            current_allowed: self.boolean()?,
            historical_admitter_authorized: self.boolean()?,
            historical_policy_allowed: self.boolean()?,
            historical_time: self.optional(SelfRead::time)?,
        })
    }
    fn query(&mut self) -> Result<Query, Error> {
        Ok(Query {
            id: self.n()?,
            instance: self.instance()?,
            descriptor: self.descriptor()?,
            operation: self.operation()?,
            evidence: self.evidence()?,
            mode: self.mode()?,
            verified_facts: self.optional(|c| Ok(Box::new(c.facts()?)))?,
            validated_under: self.optional(SelfRead::mode)?,
        })
    }
    fn receipt(&mut self) -> Result<Receipt, Error> {
        Ok(Receipt {
            instance: self.instance()?,
            operation: self.digest()?,
            admission: self.digest()?,
            stable_id: self.digest()?,
            storage: self.storage()?,
        })
    }
}
