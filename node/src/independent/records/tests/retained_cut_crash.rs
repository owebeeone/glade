//! SIGKILL on the corrected exact-observation -> original Fence callback path.
use super::*;
use crate::independent::disk::{floors::FloorCut, PairCut};
use std::io::{BufRead, Write};
#[derive(Clone)]
struct Advancing {
    calls: usize,
    armed: bool,
    old: TrustedCut,
    later: TrustedCut,
}
impl PolicyClock for Advancing {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        if self.armed {
            self.calls += 1;
        }
        Ok(if self.armed && self.calls >= 7 {
            self.later.clone()
        } else {
            self.old.clone()
        })
    }
}
fn provider(f: &Fixture, advanced: bool) -> GenuineEvidence<Advancing> {
    let mut p: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    p.generation = 1;
    p.previous = Some(envelope(&f.policy).digest());
    p.revoked.push(crate::signing::public_key(&NODE));
    let old = TrustedCut {
        policy: f.policy.clone(),
        time: Some(TimeInterval {
            earliest_ms: 20,
            latest_ms: 21,
        }),
        time_floor_ms: 10,
    };
    let later = TrustedCut {
        policy: signed(&ROOT, &p),
        ..old.clone()
    };
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    GenuineEvidence::new(
        EvidenceConfig {
            descriptor: f.descriptor.clone(),
            identity: f.identity.clone(),
            trusted_root: crate::signing::public_key(&ROOT),
            node_seed: NODE,
            limits: intent.limits.decode,
            certificates: vec![f.certificate.clone()],
            session_capacity: 16,
        },
        Advancing {
            calls: if advanced { 7 } else { 0 },
            armed: advanced,
            old,
            later,
        },
    )
    .unwrap()
}
fn cuts() -> [PairCut; 3] {
    [
        PairCut::Intent(FloorCut::BeforeTemp),
        PairCut::Intent(FloorCut::DirectorySynced),
        PairCut::Selection(FloorCut::DirectorySynced),
    ]
}
#[test]
#[ignore = "owned exact retained-cut Fence crash child"]
fn retained_cut_child() {
    let root = std::path::PathBuf::from(std::env::var_os("IC3_RETAINED_ROOT").unwrap());
    let index = std::env::var("IC3_RETAINED_CUT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let f = ic3_disk_fixture::disk_profile();
    let mut host = DiskHost::new(provider(&f, false), config(&root, true));
    let mut session = host.open_replica(request(&f, [0; 32])).unwrap();
    ic3_disk_fixture::local_authorization(&f, &mut session, 92);
    session.evidence.source.armed = true;
    session.observe_cut = Box::new(move |kind, cut| {
        if kind == DiskOperation::Fence && cut == cuts()[index] {
            println!("retained-cut-ready");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    });
    let mut c = Candidate {
        instance: f.query.instance.clone(),
        operation: f.query.operation.clone(),
        evidence: f.query.evidence.clone(),
    };
    c.evidence.admission = None;
    session.submit_local(c, [90; 32], [91; 32]).unwrap();
    panic!("exact corrected Fence cut reached");
}
#[test]
fn actual_death_before_during_after_original_fence_replays_exact_noncommit_without_reminting() {
    for index in 0..3 {
        let root = root(&format!("retained-fence-cut-{index}"));
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .env("IC3_RETAINED_ROOT", &root)
            .env("IC3_RETAINED_CUT", index.to_string())
            .args([
                "--exact",
                "independent::records::tests::content::retained_cut_crash::retained_cut_child",
                "--ignored",
                "--nocapture",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = crate::test_support::children::spawn(&mut command).unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if line.unwrap() == "retained-cut-ready" {
                    send.send(()).unwrap();
                    return;
                }
            }
        });
        let ready = receive.recv_timeout(std::time::Duration::from_secs(5));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        ready.expect("corrected observation and original Fence process cut");
        let f = ic3_disk_fixture::disk_profile();
        let digest =
            Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
        let mut host = DiskHost::new(provider(&f, true), config(&root, false));
        let mut session = host.open_replica(request(&f, digest)).unwrap();
        let LoadResult::Validated(loaded) = session.load() else {
            panic!("original callback reattached before usable load");
        };
        assert_eq!(loaded.image.recovery.attempts.len(), 1);
        assert_eq!(loaded.image.recovery.attempts[0].attempt.id.number, 1);
        assert!(matches!(
            loaded.image.recovery.attempts[0].phase,
            AttemptPhase::Terminal(TerminalOutcome::NonCommit { .. })
        ));
        assert!(loaded.image.state.storage_invocations.is_empty());
        assert!(loaded.image.state.instances[&f.query.instance]
            .accepted
            .is_empty());
        assert_eq!(
            ReplicaRecoverySession::close(&mut session),
            CloseResult::Closed
        );
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
}
