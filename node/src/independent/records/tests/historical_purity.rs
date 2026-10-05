//! Historical verification is pure (GladeEvidenceProviderClassification): it reads
//! only the query's own signed bytes, the static configuration and the certificate
//! registry. It makes no `PolicyClock::current` call and changes no provider field,
//! so a crash inside it hides no observation and it needs no durable barrier.
//! Local verification is the contrast: it consults the current cut and moves custody.
use super::*;
use glade_crdt_admission_data::{Mode, Verdict};
use glade_crdt_evidence_api::{BoundQuery, EvidencePort};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The trusted source, counting every consultation of it.
struct Counting {
    cut: TrustedCut,
    calls: Arc<AtomicUsize>,
}
impl PolicyClock for Counting {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.cut.clone())
    }
}
fn counting(f: &Fixture, calls: Arc<AtomicUsize>) -> GenuineEvidence<Counting> {
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    let session_capacity = 16.min(intent.limits.guard_history);
    GenuineEvidence::new(
        EvidenceConfig {
            descriptor: f.descriptor.clone(),
            identity: f.identity.clone(),
            trusted_root: glade_node::signing::public_key(&ROOT),
            node_seed: NODE,
            limits: intent.limits.decode,
            certificates: vec![f.certificate.clone()],
            session_capacity,
        },
        Counting {
            cut: TrustedCut {
                policy: f.policy.clone(),
                time: Some(TimeInterval {
                    earliest_ms: 20,
                    latest_ms: 21,
                }),
                time_floor_ms: 10,
            },
            calls,
        },
    )
    .unwrap()
}

/// Every provider field a verification could change.
#[derive(Debug, PartialEq)]
struct Provider {
    observed_policy: Option<(u64, Digest)>,
    observed_time_floor: u64,
    authorized: BTreeMap<Digest, LocalChallengeBody>,
    sealable: Vec<Digest>,
    sessions: Vec<SessionCustody>,
}
fn provider(session: &DiskSession<Counting>) -> Provider {
    let evidence = &session.evidence;
    Provider {
        observed_policy: evidence.observed_policy,
        observed_time_floor: evidence.observed_time_floor,
        authorized: evidence.authorized.clone(),
        sealable: evidence.sealable.keys().copied().collect(),
        sessions: evidence.session_custody(),
    }
}

#[test]
fn historical_verify_consults_no_current_cut_and_changes_no_provider_field() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("historical-purity");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut host = DiskHost::new(counting(&f, calls.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    // A writer holds possession for the very operation the historical query names:
    // a verification that took custody would show.
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let before = provider(&session);
    assert!(before.observed_policy.is_some() && !before.authorized.is_empty());
    let consulted = calls.load(Ordering::SeqCst);
    assert!(consulted > 0, "the setup itself consulted the source");
    assert_eq!(f.query.mode, Mode::Historical);
    let reply = session.evidence.verify(BoundQuery {
        query: f.query.clone(),
        identity: f.identity.clone(),
    });
    assert!(
        matches!(reply.verdict, Verdict::Verified(_)),
        "the historical query verifies: {:?}",
        reply.verdict
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        consulted,
        "a historical verify makes no PolicyClock::current call"
    );
    assert_eq!(
        provider(&session),
        before,
        "a historical verify changes no provider field"
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_verify_consults_the_current_cut_and_moves_custody() {
    let f = ic3_disk_fixture::disk_profile();
    let root = root("local-contrast");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut host = DiskHost::new(counting(&f, calls.clone()), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    let before = provider(&session);
    let consulted = calls.load(Ordering::SeqCst);
    let mut candidate = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    candidate.evidence.admission = None;
    session.submit_local(candidate, [90; 32], [91; 32]).unwrap();
    assert!(
        calls.load(Ordering::SeqCst) > consulted,
        "a local verify consults the current cut"
    );
    assert!(
        !before.authorized.is_empty() && provider(&session).authorized.is_empty(),
        "a local verify consumes the writer's possession"
    );
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
