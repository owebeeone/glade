// Actual host private qualification with the original signed fixture inputs.
use super::*;
use crate::independent::{EvidenceConfig, SessionRequest, TrustedCut};
#[path = "../../../tests/ic3_auth_fixture/mod.rs"]
pub(in crate::independent) mod ic3_auth_fixture;
use ic3_auth_fixture::*;
#[derive(Clone)]
struct Source(TrustedCut);
impl PolicyClock for Source {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        Ok(self.0.clone())
    }
}
fn evidence(f: &Fixture) -> GenuineEvidence<Source> {
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
        Source(TrustedCut {
            policy: f.policy.clone(),
            time: Some(TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            }),
            time_floor_ms: 10,
        }),
    )
    .unwrap()
}
fn request(f: &Fixture, expected_floor: Digest) -> ReplicaOpen {
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    ReplicaOpen {
        instance: f.descriptor.instance.clone(),
        descriptor: f.descriptor.clone(),
        identity: f.identity.clone(),
        owner: Owner {
            store: StoreIncarnation([70; 32]),
            generation: 1,
        },
        plan_namespace: [71; 32],
        invocation_namespace: [72; 32],
        limits: intent.limits,
        expected_floor,
    }
}
fn root(name: &str) -> std::path::PathBuf {
    let root = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("glade-ic3-b2-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    root
}
fn config(root: &std::path::Path, provision: bool) -> DiskRootConfig {
    DiskRootConfig {
        data: root.join("data"),
        trusted_floor: root.join("trusted"),
        registration: [73; 32],
        provision,
    }
}

#[path = "tests/controls.rs"]
mod controls;
#[path = "../../../tests/ic3_disk_fixture/mod.rs"]
mod ic3_disk_fixture;
/// The larger disposable storage profile for `independent` tests outside
/// this module (its fixture functions are visible only here).
pub(in crate::independent) fn disk_profile() -> Fixture {
    ic3_disk_fixture::disk_profile()
}

#[path = "tests/restart.rs"]
mod restart;

#[path = "tests/native_cuts.rs"]
mod native_cuts;

#[path = "tests/kinds.rs"]
mod kinds;

#[path = "tests/observation_restart.rs"]
mod observation_restart;

#[path = "tests/ingress.rs"]
mod ingress;

#[path = "tests/fences.rs"]
mod fences;

#[path = "tests/ingress_cuts.rs"]
mod ingress_cuts;

#[path = "tests/ingress_cancellation.rs"]
mod ingress_cancellation;
#[path = "tests/ingress_capacity.rs"]
mod ingress_capacity;

#[path = "tests/marker_cuts.rs"]
mod marker_cuts;

#[path = "tests/oversized_loss.rs"]
mod oversized_loss;

#[path = "tests/interpreted_budget.rs"]
mod interpreted_budget;

#[path = "tests/retained_cut_crash.rs"]
mod retained_cut_crash;

#[test]
fn node_locked_reopen_refuses_explicit_wrong_plan_namespace() {
    let f = Fixture::new();
    let root = root("c-node-namespace");
    let mut fresh = DiskHost::new(evidence(&f), config(&root, true));
    let mut session = fresh.open_node(request(&f, [0; 32])).unwrap();
    assert!(matches!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    ));
    crate::test_support::children::settled();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut wrong = request(&f, [0; 32]);
    wrong.plan_namespace = [99; 32];
    assert!(
        matches!(host.open_node(wrong), Err(Fault::BindingMismatch)),
        "explicit plan namespace cannot silently adapt to retained authority"
    );
    crate::test_support::children::settled();
    let mut host = DiskHost::new(evidence(&f), config(&root, false));
    let mut restored = host.open_node(request(&f, [0; 32])).unwrap();
    assert!(matches!(restored.load(), LoadResult::Validated(_)));
    assert!(matches!(
        ReplicaRecoverySession::close(&mut restored),
        CloseResult::Closed
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[path = "tests/node_history.rs"]
mod node_history;
