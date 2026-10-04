//! Paired physical image/floor custody. Structural decoding is not runtime validation.
use super::floors::FloorCut;
use super::metadata::{DiskOperation, FloorEnvelope, PendingMetadata};
use super::slots::{SlotCut, SlotFile};
use super::*;
mod validation;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairCut {
    Intent(FloorCut),
    Image(SlotCut),
    Selection(FloorCut),
    RecoveryBeforeSync,
    RecoveryFileSynced,
    ObservationMarker(FloorCut),
}
pub(crate) struct PairStore {
    layout: DiskLayout,
    slots: [SlotFile; 2],
    bounds: DecodeLimits,
    uncertain: bool,
    pub image: ReplicaImage,
    pub envelope: FloorEnvelope,
}
impl PairStore {
    pub(crate) fn uncertain(&self) -> bool {
        self.uncertain
    }

    pub(crate) fn owned(&self) -> Result<(), Fault> {
        if self.uncertain {
            return Err(Fault::Unavailable);
        }
        self.layout.owned()
    }
    pub(crate) fn floor_only(
        &mut self,
        mut projected: ReplicaFloor,
        mut at: impl FnMut(FloorCut) -> Result<(), Fault>,
    ) -> Result<(), Fault> {
        self.owned()?;
        if self.envelope.pending.is_some() {
            return Err(Fault::Ownership);
        }
        validation::advance(&self.envelope.floor, &projected)?;
        for (id, status) in &self.envelope.floor.guards {
            if projected.guards.get(id) != Some(status) {
                return Err(Fault::Integrity);
            }
        }
        projected.sequence = projected.sequence.checked_add(1).ok_or(Fault::Capacity)?;
        let mut next = self.envelope.clone();
        next.floor = projected;
        validation::envelope(&next, &self.image, &self.bounds)?;
        let bytes = encode(&next, &self.bounds).map_err(validation::codec_fault)?;
        self.uncertain = true;
        floors::replace(
            &self.layout.floor.join("replica.floor"),
            &bytes,
            self.bounds.bytes,
            &mut at,
        )?;
        self.envelope = next;
        self.uncertain = false;
        Ok(())
    }
    pub(crate) fn floor_digest(&self) -> Result<Digest, Fault> {
        self.layout.floor_digest(self.bounds.bytes)
    }
    pub(crate) fn observation_marker(
        &mut self,
        observing: bool,
        mut at: impl FnMut(PairCut) -> Result<(), Fault>,
    ) -> Result<(), Fault> {
        self.owned()?;
        if self.envelope.pending.is_some() {
            return Err(Fault::Unavailable);
        }
        if self.envelope.observing == observing {
            return Ok(());
        }
        let mut next = self.envelope.clone();
        next.floor.sequence = next.floor.sequence.checked_add(1).ok_or(Fault::Capacity)?;
        next.observing = observing;
        validation::envelope(&next, &self.image, &self.bounds)?;
        let bytes = encode(&next, &self.bounds).map_err(validation::codec_fault)?;
        self.uncertain = true;
        floors::replace(
            &self.layout.floor.join("replica.floor"),
            &bytes,
            self.bounds.bytes,
            |cut| at(PairCut::ObservationMarker(cut)),
        )?;
        self.envelope = next;
        self.uncertain = false;
        Ok(())
    }

    pub(crate) fn create(
        layout: DiskLayout,
        image: ReplicaImage,
        mut floor: ReplicaFloor,
        bounds: DecodeLimits,
    ) -> Result<Self, Fault> {
        layout.owned()?;
        if image.generation != 0
            || image.predecessor != [0; 32]
            || floor.intent.is_some()
            || floor.selected.generation != 0
            || floor.selected.slot != 0
            || floor.selected.digest != [0; 32]
            || image.prior_floor != floor
        {
            return Err(Fault::Integrity);
        }
        let bytes = restorable(&image, &bounds)?;
        let mut slots = [
            SlotFile::reserve(&layout.data.join("image.0"), image.limits.image_bytes)?,
            SlotFile::reserve(&layout.data.join("image.1"), image.limits.image_bytes)?,
        ];
        slots[0].write_image(&bytes)?;
        floor.selected.digest = bytes.digest();
        let envelope = FloorEnvelope {
            floor,
            pending: None,
            completion: None,
            unmaterialized: vec![],
            observing: false,
        };
        validation::envelope(&envelope, &image, &bounds)?;
        let encoded = encode(&envelope, &bounds).map_err(validation::codec_fault)?;
        floors::replace(
            &layout.floor.join("replica.floor"),
            &encoded,
            bounds.bytes,
            |_| Ok(()),
        )?;
        Ok(Self {
            layout,
            slots,
            bounds,
            uncertain: false,
            image,
            envelope,
        })
    }
    pub(crate) fn open_checked(
        layout: DiskLayout,
        bounds: DecodeLimits,
        mut at: impl FnMut(PairCut) -> Result<(), Fault>,
        mut checked: impl FnMut(&ReplicaImage, &FloorEnvelope) -> Result<(), Fault>,
    ) -> Result<Self, Fault> {
        layout.owned()?;
        let bytes = floors::read(&layout.floor.join("replica.floor"), bounds.bytes)?;
        let mut envelope: FloorEnvelope =
            decode(bytes.value(), &bounds).map_err(validation::codec_fault)?;
        validation::pending(&envelope)?;
        if envelope.observing
            && !envelope
                .pending
                .as_ref()
                .is_some_and(|pending| pending.kind == DiskOperation::Observation)
        {
            return Err(Fault::Unavailable);
        }
        // The fixed capacity comes from the originally selected typed image. Neither a
        // torn next image nor a caller may change that reservation on reopen.
        let selected = envelope.floor.selected.clone();
        if selected.slot > 1 {
            return Err(Fault::Integrity);
        }
        let path = layout.data.join(format!("image.{}", selected.slot));
        let capacity = validation::capacity(&path, &bounds)?;
        let mut slots = [
            SlotFile::open(&layout.data.join("image.0"), capacity)?,
            SlotFile::open(&layout.data.join("image.1"), capacity)?,
        ];
        let mut image = Self::read(&mut slots, &selected, &bounds)?;
        validation::envelope(&envelope, &image, &bounds)?;
        if let Some(pending) = envelope.pending.clone() {
            match Self::read(&mut slots, &pending.next, &bounds) {
                Ok(next) => {
                    if next.predecessor != selected.digest
                        || next.prior_floor.selected != selected
                        || next.limits != image.limits
                        || next.instance != image.instance
                        || next.identity != image.identity
                    {
                        return Err(Fault::Integrity);
                    }
                    let mut candidate = envelope.clone();
                    candidate.floor = candidate.completion.take().ok_or(Fault::Integrity)?;
                    candidate.pending = None;
                    if pending.kind == DiskOperation::Observation {
                        candidate.observing = false;
                    }
                    checked(&next, &candidate)?;
                    at(PairCut::RecoveryBeforeSync)?;
                    slots[usize::from(pending.next.slot)].resync()?;
                    at(PairCut::RecoveryFileSynced)?;
                    layout.owned()?;
                    image = next;
                    envelope.floor = envelope.completion.take().ok_or(Fault::Integrity)?;
                    envelope.observing = candidate.observing;
                }
                Err(Fault::Integrity) => {
                    if envelope.observing {
                        return Err(Fault::Unavailable);
                    }
                    if envelope.unmaterialized.len() >= image.limits.guard_history {
                        return Err(Fault::Capacity);
                    }
                    envelope.unmaterialized.push(pending);
                    envelope.floor.sequence = envelope
                        .floor
                        .sequence
                        .checked_add(1)
                        .ok_or(Fault::Capacity)?;
                }
                Err(other) => {
                    return Err(other);
                }
            }
            envelope.floor.intent = None;
            envelope.pending = None;
            envelope.completion = None;
            validation::envelope(&envelope, &image, &bounds)?;
            checked(&image, &envelope)?;
            let encoded = encode(&envelope, &bounds).map_err(validation::codec_fault)?;
            floors::replace(
                &layout.floor.join("replica.floor"),
                &encoded,
                bounds.bytes,
                |_| Ok(()),
            )?;
        }
        checked(&image, &envelope)?;
        Ok(Self {
            layout,
            slots,
            bounds,
            uncertain: false,
            image,
            envelope,
        })
    }
    fn read(
        slots: &mut [SlotFile; 2],
        selection: &Selection,
        bounds: &DecodeLimits,
    ) -> Result<ReplicaImage, Fault> {
        let slot = slots
            .get_mut(usize::from(selection.slot))
            .ok_or(Fault::Integrity)?;
        let bytes = slot.read_image(selection.digest)?;
        let image: ReplicaImage = decode(bytes.value(), bounds).map_err(validation::codec_fault)?;
        if image.generation != selection.generation {
            return Err(Fault::Integrity);
        }
        Ok(image)
    }
    pub(crate) fn transact(
        &mut self,
        image: ReplicaImage,
        projected: ReplicaFloor,
        kind: DiskOperation,
        mut at: impl FnMut(PairCut) -> Result<(), Fault>,
    ) -> Result<(), Fault> {
        self.owned()?;
        let old = &self.envelope.floor;
        if self.envelope.pending.is_some() || old.intent.is_some() {
            return Err(Fault::Ownership);
        }
        if image.generation
            != old
                .selected
                .generation
                .checked_add(1)
                .ok_or(Fault::Capacity)?
            || image.predecessor != old.selected.digest
            || image.prior_floor != *old
            || image.instance != self.image.instance
            || image.identity != self.image.identity
            || image.limits != self.image.limits
        {
            return Err(Fault::Integrity);
        }
        validation::advance(old, &projected)?;
        let bytes = restorable(&image, &self.bounds)?;
        if bytes.value().len() > image.limits.image_bytes {
            return Err(Fault::Capacity);
        }
        let next = Selection {
            slot: 1 - old.selected.slot,
            generation: image.generation,
            digest: bytes.digest(),
        };
        let mut retained_guards = projected.guards.clone();
        // Before selection, clearing an old active guard would lie on fallback.
        for (id, status) in &old.guards {
            if matches!(status, GuardStatus::Active(_)) {
                retained_guards.insert(id.clone(), status.clone());
            }
        }
        let intent = FloorIntent {
            issuance: projected.issuance.clone(),
            prior: old.selected.clone(),
            next: next.clone(),
            next_attempt: projected.next_attempt,
            invocation_high_water: projected.invocation_high_water,
            guard_high_water: projected.guard_high_water,
            guards: retained_guards.clone(),
        };
        let pending = PendingMetadata {
            kind,
            intent: encode(&intent, &self.bounds)
                .map_err(validation::codec_fault)?
                .digest(),
            prior: old.selected.clone(),
            next: next.clone(),
        };
        let mut staged = self.envelope.clone();
        staged.floor = projected.clone();
        staged.floor.guards = retained_guards;
        staged.floor.intent = Some(intent);
        staged.floor.sequence = old.sequence.checked_add(1).ok_or(Fault::Capacity)?;
        staged.pending = Some(pending);
        let mut completion = projected.clone();
        completion.selected = next.clone();
        completion.sequence = staged
            .floor
            .sequence
            .checked_add(1)
            .ok_or(Fault::Capacity)?;
        completion.intent = None;
        staged.completion = Some(completion);
        validation::envelope(&staged, &self.image, &self.bounds)?;
        let mut selected = staged.clone();
        selected.floor = selected.completion.take().ok_or(Fault::Integrity)?;
        selected.floor.intent = None;
        selected.pending = None;
        if kind == DiskOperation::Observation {
            selected.observing = false;
        }
        validation::envelope(&selected, &image, &self.bounds)?;
        let selected_bytes = encode(&selected, &self.bounds).map_err(validation::codec_fault)?;
        let encoded = encode(&staged, &self.bounds).map_err(validation::codec_fault)?;
        // After this durable intent, failures are unknown. The in-memory session is
        // poisoned to pending before I/O, so no old floor CAS can proceed on error.
        self.uncertain = true;
        self.envelope = staged;
        floors::replace(
            &self.layout.floor.join("replica.floor"),
            &encoded,
            self.bounds.bytes,
            |cut| at(PairCut::Intent(cut)),
        )?;
        self.slots[usize::from(next.slot)]
            .write_image_with(&bytes, |cut| at(PairCut::Image(cut)))?;
        floors::replace(
            &self.layout.floor.join("replica.floor"),
            &selected_bytes,
            self.bounds.bytes,
            |cut| at(PairCut::Selection(cut)),
        )?;
        self.image = image;
        self.envelope = selected;
        self.uncertain = false;
        Ok(())
    }
}
/// Encode an image only if its reopen decodes it under the same limits. `encode`
/// bounds the outer syntax; `decode` also interprets every embedded Operation.
fn restorable(image: &ReplicaImage, bounds: &DecodeLimits) -> Result<Bytes, Fault> {
    let bytes = encode(image, bounds).map_err(validation::codec_fault)?;
    decode::<ReplicaImage>(bytes.value(), bounds).map_err(validation::codec_fault)?;
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[path = "../tests.rs"]
    mod content;
}

#[cfg(test)]
mod structural_support {
    use super::*;
    impl PairStore {
        pub(crate) fn open(layout: DiskLayout, bounds: DecodeLimits) -> Result<Self, Fault> {
            Self::open_with(layout, bounds, |_| Ok(()))
        }
        pub(crate) fn open_with(
            layout: DiskLayout,
            bounds: DecodeLimits,
            mut at: impl FnMut(PairCut) -> Result<(), Fault>,
        ) -> Result<Self, Fault> {
            Self::open_checked(layout, bounds, &mut at, |_, _| Ok(()))
        }
    }
}
