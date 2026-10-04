//! A writer selects only bytes its reopen decodes under the same signed limits.
//! `encode` bounds the outer syntax only; `decode` also interprets every embedded
//! Operation into the same item budget.
use super::*;
fn with_items(items: usize) -> Fixture {
    ic3_disk_fixture::disk_profile_with(|limits| {
        limits.decode.items = items;
    })
}
fn least(fits: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (1_usize, 80000_usize);
    assert!(fits(high), "the measuring profile decodes its own images");
    while low < high {
        let middle = low + (high - low) / 2;
        if fits(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}
fn decode_items(image: &ReplicaImage) -> usize {
    let raw = encode(image, &image.limits.decode).unwrap();
    least(|items| {
        let mut limits = image.limits.decode.clone();
        limits.items = items;
        decode::<ReplicaImage>(raw.value(), &limits).is_ok()
    })
}
fn encode_items(image: &ReplicaImage) -> usize {
    least(|items| {
        let mut limits = image.limits.decode.clone();
        limits.items = items;
        encode(image, &limits).is_ok()
    })
}
fn submit_local(f: &Fixture, session: &mut DiskSession<Source>) -> Result<Vec<Effect>, Fault> {
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    super::super::runtime::event(session, Event::SubmitLocal(Box::new(candidate)))
}
fn selected(session: &mut DiskSession<Source>) -> ReplicaImage {
    let LoadResult::Validated(loaded) = session.load() else {
        panic!("validated live cut")
    };
    loaded.image
}
fn floor_digest(root: &std::path::Path) -> Digest {
    Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest()
}
/// The first image holding an Operation, and the item limit just below its
/// interpreted count, measured on a generous profile. Item counts are
/// structural: the signed items number is one CBOR integer whatever its value.
fn band(name: &str) -> (ReplicaImage, usize) {
    let wide = with_items(80000);
    let measured = root(name);
    let mut host = DiskHost::new(evidence(&wide), config(&measured, true));
    let mut session = host.open_replica(request(&wide, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&wide, &mut session, 92);
    let prior = decode_items(&selected(&mut session));
    submit_local(&wide, &mut session).unwrap();
    let grown = selected(&mut session);
    let (outer, interpreted) = (encode_items(&grown), decode_items(&grown));
    drop(session);
    std::fs::remove_dir_all(&measured).unwrap();
    let items = interpreted - 1;
    assert!(
        prior <= items && outer <= items,
        "band: prior {prior}, outer {outer}, interpreted {interpreted}"
    );
    (grown, items)
}
#[test]
fn image_whose_interpreted_operations_cross_decode_items_is_refused_before_intent() {
    let (_, items) = band("interpreted-budget-measure");
    let f = with_items(items);
    let dir = root("interpreted-budget");
    let mut host = DiskHost::new(evidence(&f), config(&dir, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let before = selected(&mut session);
    let floor = floor_digest(&dir);
    let submitted = submit_local(&f, &mut session).map(|_| ());
    let unchanged = floor_digest(&dir) == floor;
    let live = session.pair.as_ref().is_some_and(|pair| !pair.uncertain());
    let closed = matches!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
    drop(session);
    crate::test_support::children::settled();
    let mut host = DiskHost::new(evidence(&f), config(&dir, false));
    let reopened = host
        .open_replica(request(&f, floor_digest(&dir)))
        .map(|mut session| selected(&mut session));
    assert!(
        submitted == Err(Fault::Capacity)
            && unchanged
            && live
            && closed
            && reopened
                .as_ref()
                .is_ok_and(|image| image.state == before.state
                    && image.generation == before.generation + 1),
        "undecodable image must be refused before intent and the prior image reopen: \
         submit {submitted:?}, floor unchanged {unchanged}, live {live}, closed {closed}, \
         reopen {:?} (prior generation {})",
        reopened.map(|image| image.generation),
        before.generation
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn genesis_image_whose_interpreted_operations_cross_decode_items_is_never_created() {
    let (mut image, items) = band("interpreted-genesis-measure");
    let mut floor = image.prior_floor.clone();
    floor.selected = Selection {
        slot: 0,
        generation: 0,
        digest: [0; 32],
    };
    floor.intent = None;
    floor.issuance.next_effect_id = image.state.next_effect_id;
    image.generation = 0;
    image.predecessor = [0; 32];
    image.prior_floor = floor.clone();
    let mut bounds = image.limits.decode.clone();
    bounds.items = items;
    let dir = root("interpreted-genesis");
    let (data, trusted) = (dir.join("data"), dir.join("trusted"));
    let layout =
        crate::independent::disk::DiskLayout::provision(&data, &trusted, [73; 32]).unwrap();
    let created = PairStore::create(layout, image, floor, bounds.clone()).map(drop);
    let reopened = crate::independent::disk::DiskLayout::open(&data, &trusted, [73; 32])
        .and_then(|layout| PairStore::open(layout, bounds))
        .map(drop);
    assert!(
        created == Err(Fault::Capacity) && !trusted.join("replica.floor").exists(),
        "undecodable genesis must not be selected: create {created:?}, reopen {reopened:?}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
