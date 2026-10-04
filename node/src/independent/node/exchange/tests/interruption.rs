//! IC3-SYNC-003/004 and the IC3-GUARD receive table: a directed disconnect of
//! the actual glade-node binary's peer link inside every transfer phase.
//!
//! The node under test is the actual binary (`env_clear`, its independent
//! profile, real Iroh and disk). Its peer is an in-process actual replica
//! opened as `lifecycle` opens one (profile, door-guarded `IrohCarrier`,
//! `IndependentReplica::open`, carrier bound like `PEER_CARRIER`) that drives
//! this module's own `round` on the link the node dials. A test-only
//! `CarrierLink` closes that real link before an exact frame of the peer's,
//! once the node's frames show it has entered the phase. No production code
//! is cut. Retry then heals through both nodes' actual `serve`.
use super::*;
use crate::independent::test_support::actual_binary::{port, Process};
use crate::iroh_carrier::Lent;
use crate::netconf::Relays;
use crate::peer::NodeIdentity;
use crate::signing::NodeSigner;
use crate::transport::{Door, EndpointKey};
use glade_carrier_api::{CarrierConfig, CarrierError, ChannelBinding, PortFuture, TransportId};
use glade_crdt_recovery_api::ReplicaRecoveryHost;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
// The fixtures the Records tests already load, reused rather than reloaded.
use crate::independent::records::tests::content::disk_profile;
use crate::independent::records::tests::content::ic3_auth_fixture::*;

/// One node's fixed provisioning: node and endpoint seeds, registration and
/// the first of its three store/plan/invocation namespace bytes.
#[derive(Clone, Copy)]
struct Side {
    seed: Digest,
    endpoint: Digest,
    registration: Digest,
    store: u8,
}
const UNDER_TEST: Side = Side {
    seed: NODE,
    endpoint: [44; 32],
    registration: [73; 32],
    store: 70,
};
const PEER: Side = Side {
    seed: [34; 32],
    endpoint: [45; 32],
    registration: [77; 32],
    store: 74,
};

/// Scheduling bounds in both profiles. Every guarded receive, send and dial is
/// bounded by `DEADLINE_MS`, and a started receive that times out is retained
/// as permanent loss by design. Synced Records writes of ten concurrent
/// replicas can stall a round for over a second, so the witnesses use five.
const TICK_MS: u64 = 50;
const DEADLINE_MS: u64 = 5000;

/// The released text fixture, both nodes holders and admitters, and a second
/// writer origin so each node starts with its own complete accepted history:
/// the node under test holds A (writer-e0), the peer X0 and X1 (writer-e1).
struct History {
    f: Fixture,
    certificates: Vec<SignedRecord>,
    node_ops: Vec<Candidate>,
    peer_ops: Vec<Candidate>,
}
fn history() -> History {
    let mut f = disk_profile();
    let nodes = vec![public_key(&UNDER_TEST.seed), public_key(&PEER.seed)];
    let mut policy: PolicyBody = decode_signed_body(&f.policy, &limits()).unwrap();
    policy.admitting_nodes = nodes.clone();
    policy.replica_holders = nodes;
    f.policy = signed(&ROOT, &policy);
    let mut second: CertificateBody = decode_signed_body(&f.certificate, &limits()).unwrap();
    second.canonical_origin = "writer-e1".into();
    second.epoch = "e1".into();
    second.issuance = 1;
    let second = signed(&ROOT, &second);
    let insert = |atom: &str, after: &str, text: &str| {
        format!(r#"{{"after":{after},"atom_id":"{atom}","kind":"insert","text":"{text}"}}"#)
    };
    let a = candidate(
        &f,
        &f.certificate,
        ("writer-e0", 0),
        None,
        insert("a", "null", "A"),
    );
    let x0 = candidate(
        &f,
        &second,
        ("writer-e1", 0),
        None,
        insert("x", "null", "X"),
    );
    let x1 = candidate(
        &f,
        &second,
        ("writer-e1", 1),
        Some(&x0),
        insert("y", "\"x\"", "Y"),
    );
    History {
        certificates: vec![f.certificate.clone(), second],
        f,
        node_ops: vec![a],
        peer_ops: vec![x0, x1],
    }
}
/// A genuine writer-signed text insert whose permit names both admitters.
fn candidate(
    f: &Fixture,
    certificate: &SignedRecord,
    (origin, seq): (&str, i64),
    prev: Option<&Candidate>,
    payload: String,
) -> Candidate {
    let mut op = f.query.operation.inner().clone();
    op.origin = origin.into();
    op.seq = seq;
    op.lamport = seq + 1;
    op.prev = prev.map(|prev| prev.operation.canonical().digest().to_vec());
    op.payload = payload.into_bytes();
    let operation = Operation::new(op);
    let mut evidence = Fixture::proofs(
        &f.query.instance,
        &f.identity,
        &f.descriptor,
        certificate,
        &f.policy,
        &operation,
        None,
    );
    let wrapper: PermitWrapper = decode_profile(evidence.permit.value(), &limits()).unwrap();
    let mut permit: PermitBody = decode_signed_body(&wrapper.signed_permit, &limits()).unwrap();
    permit.admitting_nodes = vec![public_key(&UNDER_TEST.seed), public_key(&PEER.seed)];
    let signed_permit = signed(&ROOT, &permit);
    let mut proof: OperationProofBody =
        decode_signed_body(&wrapper.writer_proof, &limits()).unwrap();
    proof.signed_permit = envelope(&signed_permit).digest();
    evidence.permit = encode_profile(
        &PermitWrapper {
            signed_permit,
            writer_proof: signed(&WRITER, &proof),
        },
        &limits(),
    )
    .unwrap();
    Candidate {
        instance: f.query.instance.clone(),
        operation,
        evidence,
    }
}
fn request(f: &Fixture, side: Side) -> ReplicaOpen {
    let intent: CreationIntent = decode_profile(f.identity.intent.value(), &limits()).unwrap();
    ReplicaOpen {
        instance: f.descriptor.instance.clone(),
        descriptor: f.descriptor.clone(),
        identity: f.identity.clone(),
        owner: Owner {
            store: StoreIncarnation([side.store; 32]),
            generation: 1,
        },
        plan_namespace: [side.store + 1; 32],
        invocation_namespace: [side.store + 2; 32],
        limits: intent.limits,
        expected_floor: [0; 32],
    }
}
/// The same evidence/clock/root composition `IndependentReplica::open` uses.
fn host(h: &History, side: Side, root: &Path, fresh: bool) -> DiskHost<CurrentSource> {
    let request = request(&h.f, side);
    let config = EvidenceConfig {
        descriptor: h.f.descriptor.clone(),
        identity: h.f.identity.clone(),
        trusted_root: h.f.identity.root,
        node_seed: side.seed,
        limits: request.limits.decode.clone(),
        certificates: h.certificates.clone(),
        session_capacity: request.limits.guard_history,
    };
    let source = CurrentSource {
        cut: TrustedCut {
            policy: h.f.policy.clone(),
            time: Some(TimeInterval {
                earliest_ms: 20,
                latest_ms: 21,
            }),
            time_floor_ms: 10,
        },
    };
    DiskHost::new(
        GenuineEvidence::new(config, source).unwrap(),
        DiskRootConfig {
            data: root.join("data"),
            trusted_floor: root.join("trusted"),
            registration: side.registration,
            provision: fresh,
        },
    )
}
/// Fresh writer possession, then genuine native local admission.
fn admit(session: &mut DiskSession<CurrentSource>, candidate: &Candidate, nonce: u8) {
    let challenge = session
        .issue_challenge(SessionRequest {
            requester: "alice".into(),
            writer_key: public_key(&WRITER),
            operation: candidate.operation.canonical().digest(),
            channel: [90; 32],
            session_nonce: [91; 32],
            challenge_nonce: [nonce; 32],
            window: Window {
                not_before_ms: 10,
                not_after_ms: 100,
            },
        })
        .unwrap();
    let body: LocalChallengeBody = decode_signed_body(&challenge, &limits()).unwrap();
    session
        .authenticate(
            signed(
                &WRITER,
                &LocalResponseBody {
                    instance: body.instance,
                    node: body.node,
                    requester: body.requester,
                    writer_key: body.writer_key,
                    signed_challenge: envelope(&challenge).digest(),
                    operation: body.operation,
                    channel: body.channel,
                    session_nonce: body.session_nonce,
                    challenge_nonce: body.challenge_nonce,
                },
            ),
            [90; 32],
            [91; 32],
        )
        .unwrap();
    session
        .submit_local(candidate.clone(), [90; 32], [91; 32])
        .unwrap();
}
fn prepare(h: &History, side: Side, root: &Path, ops: &[Candidate]) {
    let mut session = host(h, side, root, true)
        .open_replica(request(&h.f, side))
        .unwrap();
    for (index, op) in ops.iter().enumerate() {
        admit(&mut session, op, 92 + index as u8);
    }
    session.classify_node_inputs().unwrap();
    assert_eq!(
        ReplicaRecoverySession::close(&mut session),
        CloseResult::Closed
    );
}
/// The twelve-field private profile the binary reads, naming the other side.
fn profile(h: &History, own: Side, root: &Path, other: Side, other_port: u16) -> PathBuf {
    let key: String = public_key(&other.endpoint)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let fields = vec![
        Bytes::new(b"glade/ic3/node-qualification/v1".to_vec()),
        encode(&request(&h.f, own), &limits()).unwrap(),
        encode(&h.f.policy, &limits()).unwrap(),
        encode(&h.certificates, &limits()).unwrap(),
        Bytes::new(own.seed.to_vec()),
        Bytes::new(own.endpoint.to_vec()),
        encode(
            &vec![
                root.join("data").display().to_string(),
                root.join("trusted").display().to_string(),
            ],
            &limits(),
        )
        .unwrap(),
        Bytes::new(own.registration.to_vec()),
        encode(&false, &limits()).unwrap(),
        encode(
            &(
                Some(TimeInterval {
                    earliest_ms: 20,
                    latest_ms: 21,
                }),
                10_u64,
            ),
            &limits(),
        )
        .unwrap(),
        encode(
            &vec![(
                public_key(&other.seed),
                (
                    public_key(&other.endpoint),
                    format!("{key}@127.0.0.1:{other_port}"),
                ),
            )],
            &limits(),
        )
        .unwrap(),
        encode(&(TICK_MS, DEADLINE_MS), &limits()).unwrap(),
    ];
    let path = root.join("profile");
    std::fs::write(&path, encode(&fields, &limits()).unwrap().value()).unwrap();
    path
}

/// What one replica holds durably, read through its own validated session.
struct Durable {
    image: ReplicaImage,
    floor: ReplicaFloor,
    history: Vec<TransferBundle>,
    complete: bool,
    loss: bool,
}
impl Durable {
    fn of(session: &mut DiskSession<CurrentSource>) -> Result<Self, Fault> {
        let LoadResult::Validated(loaded) = session.load() else {
            return Err(Fault::Unavailable);
        };
        let history = session.node_history()?;
        let request = session.reconstruction_request()?;
        let ReconstructResult::Cut(cut) = session.reconstruct_cut(request) else {
            return Err(Fault::Unavailable);
        };
        Ok(Self {
            image: loaded.image.clone(),
            floor: loaded.authoritative_floor.clone(),
            history,
            complete: cut.complete_local,
            loss: cut.permanent_loss,
        })
    }
    /// The stopped binary's state, opened at its selected floor as it left it.
    fn stopped(h: &History, side: Side, root: &Path) -> Self {
        crate::test_support::children::settled();
        let floor = Bytes::new(std::fs::read(root.join("trusted/replica.floor")).unwrap()).digest();
        let mut request = request(&h.f, side);
        request.expected_floor = floor;
        let mut session = host(h, side, root, false).open_replica(request).unwrap();
        let durable = Self::of(&mut session).unwrap();
        assert_eq!(
            ReplicaRecoverySession::close(&mut session),
            CloseResult::Closed
        );
        durable
    }
    /// Guards in floor order, as (status, role).
    fn guards(&self) -> Vec<(&'static str, String)> {
        self.floor
            .guards
            .values()
            .map(|status| match status {
                GuardStatus::Active(guard) => ("active", guard.role.clone()),
                GuardStatus::Settled { guard, .. } => ("settled", guard.role.clone()),
                GuardStatus::Abandoned { guard, .. } => ("abandoned", guard.role.clone()),
                GuardStatus::Loss { guard, .. } => ("loss", guard.role.clone()),
            })
            .collect()
    }
    fn losses(&self) -> Vec<GuardStatus> {
        self.floor
            .guards
            .values()
            .filter(|status| matches!(status, GuardStatus::Loss { .. }))
            .cloned()
            .collect()
    }
    fn accepted(&self) -> BTreeSet<Digest> {
        self.image.state.instances[&self.image.instance]
            .accepted
            .iter()
            .map(|accepted| accepted.candidate.operation.canonical().digest())
            .collect()
    }
    /// Compact failure evidence: guards, loss, unresolved work, completeness.
    fn summary(&self) -> String {
        let missing = self
            .image
            .obligations
            .iter()
            .filter(|obligation| obligation.classification == Classification::Missing)
            .count();
        format!(
            "guards={:?} permanent_loss={} missing={missing} accepted={} history={} complete={}",
            self.guards(),
            self.image.permanent_loss.len(),
            self.accepted().len(),
            self.history.len(),
            self.complete
        )
    }
}

/// The peer, opened exactly as `lifecycle` opens a configured node: its
/// profile read once, an `IrohCarrier` lent its endpoint key behind a door of
/// its configured peers, the assembly holding that one carrier, the replica
/// recovered before exchange, then the carrier bound as `PEER_CARRIER` binds.
async fn open_peer(profile: &Path, at: u16) -> Arc<IndependentReplica> {
    crate::test_support::children::settled();
    let provision = Arc::new(Provision::read(profile).unwrap());
    let keys = provision.peers.iter().map(|(_, (endpoint, _))| *endpoint);
    let door = Arc::new(Door::new(keys, |_| {}));
    let carrier = IrohCarrier::new(Some(Lent {
        key: EndpointKey::from_seed(provision.endpoint_seed),
        door: Some(door),
        relays: Relays::Off,
        first_word: Duration::from_millis(provision.deadline_ms),
    }));
    let module = Arc::new(
        NodeAssembly::builder()
            .with_component_parameters::<NodeSigner>(Some(NodeIdentity::from_key(
                provision.node_seed,
            )))
            .with_component_parameters::<IrohCarrier>(Some(carrier.clone()))
            .build(),
    );
    let replica = IndependentReplica::open(module, provision.clone(), carrier).unwrap();
    let limits = &provision.request.limits;
    let config = CarrierConfig {
        local: CarrierAddr(format!("127.0.0.1:{at}")),
        max_frame_bytes: std::num::NonZeroUsize::new(limits.decode.bytes.min(limits.inbox_bytes))
            .unwrap(),
    };
    replica.carrier.bind(config).await.unwrap();
    Arc::new(replica)
}

#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);
impl Console for Lines {
    fn out(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_owned());
    }
    fn err(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_owned());
    }
}
impl Lines {
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}
fn idle(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix("independent idle "))
        .map(str::to_owned)
}
fn count(lines: &[String], prefix: &str) -> usize {
    lines.iter().filter(|line| line.starts_with(prefix)).count()
}
async fn eventually(what: &str, mut ready: impl FnMut() -> bool, details: impl Fn() -> String) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: {}",
            details()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The next link the node dials to the peer, accepted on the peer's carrier.
async fn accept(peer: &IndependentReplica) -> Box<dyn CarrierLink> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(Some(link)) = peer.carrier.accept().await {
                break link;
            }
        }
    })
    .await
    .expect("the actual node dials its configured peer")
}
/// Converged once the peer has accepted every operation, classified every
/// retained obligation, holds no active guard, and the latest full inventory
/// it retained from the node names exactly the peer's own history: both then
/// hold the same records. Independent of idle reports, which a replica
/// holding loss never prints.
fn converged(
    session: &mut DiskSession<CurrentSource>,
    all: &BTreeSet<Digest>,
) -> Result<bool, Fault> {
    let LoadResult::Validated(loaded) = session.load() else {
        return Ok(false);
    };
    let accepted: BTreeSet<Digest> = loaded.image.state.instances[&loaded.image.instance]
        .accepted
        .iter()
        .map(|accepted| accepted.candidate.operation.canonical().digest())
        .collect();
    let limits = &loaded.image.limits.decode;
    let mut refs = vec![];
    for bundle in session.node_history()? {
        let raw = encode_profile(&bundle, limits).map_err(|_| Fault::Capacity)?;
        refs.push(crate::independent::records::node_reference(&bundle, &raw));
    }
    let node = public_key(&UNDER_TEST.seed);
    let latest = loaded.image.observations.iter().rev().find(|observation| {
        observation.source == node
            && matches!(
                loaded.authoritative_floor.guards.get(&observation.guard),
                Some(GuardStatus::Settled { guard, .. }) if guard.role == "inventory"
            )
    });
    let settled = loaded
        .image
        .obligations
        .iter()
        .all(|obligation| obligation.classification != Classification::Missing)
        && !loaded
            .authoritative_floor
            .guards
            .values()
            .any(|status| matches!(status, GuardStatus::Active(_)));
    Ok(accepted == *all
        && settled
        && latest.is_some_and(|observation| observation.expected == refs))
}
/// A stopped replica reopened exactly as the binary reopens one on its next
/// start (`IndependentReplica::open`: node-locked open, validated load and
/// replay/classification of retained inputs), then read and closed.
fn recovered(profile: &Path) -> Durable {
    crate::test_support::children::settled();
    let provision = Arc::new(Provision::read(profile).unwrap());
    let module = Arc::new(NodeAssembly::builder().build());
    let replica = IndependentReplica::open(module, provision, IrohCarrier::new(None)).unwrap();
    let durable = with(&replica, Durable::of).unwrap();
    replica.close().unwrap();
    durable
}

/// The peer's real link, closed before the peer's `send`-th frame (0-based)
/// once the node's first `after` frames have arrived: the node has then sent
/// everything that precedes its own receive in this phase, so the close lands
/// inside that receive. Frames read ahead for the check stay readable.
struct CutLink {
    inner: Box<dyn CarrierLink>,
    send: usize,
    after: usize,
    sent: AtomicUsize,
    received: AtomicUsize,
    ahead: tokio::sync::Mutex<VecDeque<Vec<u8>>>,
    reached: AtomicBool,
}
impl CarrierLink for CutLink {
    fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
        Box::pin(async move {
            let index = self.sent.fetch_add(1, Ordering::SeqCst);
            if index < self.send {
                return self.inner.send(frame).await;
            }
            if index == self.send {
                while self.received.load(Ordering::SeqCst) < self.after {
                    let mut ahead = self.ahead.lock().await;
                    if self.received.load(Ordering::SeqCst) >= self.after {
                        break;
                    }
                    match self.inner.recv().await {
                        Ok(Some(frame)) => {
                            self.received.fetch_add(1, Ordering::SeqCst);
                            ahead.push_back(frame);
                        }
                        _ => {
                            break;
                        }
                    }
                }
                self.reached.store(
                    self.received.load(Ordering::SeqCst) >= self.after,
                    Ordering::SeqCst,
                );
                self.inner.close().await;
            }
            Err(CarrierError::Closed)
        })
    }
    fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
        Box::pin(async move {
            let mut ahead = self.ahead.lock().await;
            if let Some(frame) = ahead.pop_front() {
                return Ok(Some(frame));
            }
            let frame = self.inner.recv().await;
            if let Ok(Some(_)) = &frame {
                self.received.fetch_add(1, Ordering::SeqCst);
            }
            frame
        })
    }
    fn close(&self) -> PortFuture<'_, ()> {
        self.inner.close()
    }
    fn remote_id(&self) -> Option<TransportId> {
        self.inner.remote_id()
    }
    fn channel_binding(&self, label: &[u8]) -> Option<ChannelBinding> {
        self.inner.channel_binding(label)
    }
}

/// One directed cut. Peer frames: hello 0, confirmation 1, ready 2, full
/// inventory header 3 and page 4, missing-request header 5 and page 6, then
/// its two bundles 7 and 8. Node frames in arrival order: hello, confirmation,
/// ready, full inventory header and page, request header and page, its one
/// bundle. `guards` are the node's new floor guards the cut must leave;
/// `delivered` is how many of the peer's two operations reached the node.
struct Cut {
    name: &'static str,
    send: usize,
    after: usize,
    guards: &'static [(&'static str, &'static str)],
    delivered: usize,
    line: Option<&'static str>,
}

async fn witness(cut: &Cut) {
    struct Roots(Vec<PathBuf>);
    impl Drop for Roots {
        fn drop(&mut self) {
            for root in &self.0 {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }
    let base = std::env::temp_dir().canonicalize().unwrap();
    let roots = Roots(
        ["node", "peer"]
            .iter()
            .map(|side| {
                let root = base.join(format!(
                    "glade-ic3-c3a-{}-{side}-{}",
                    cut.name,
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&root);
                std::fs::create_dir_all(&root).unwrap();
                root
            })
            .collect(),
    );
    let (node_root, peer_root) = (&roots.0[0], &roots.0[1]);
    let h = history();
    prepare(&h, UNDER_TEST, node_root, &h.node_ops);
    prepare(&h, PEER, peer_root, &h.peer_ops);
    let before = Durable::stopped(&h, UNDER_TEST, node_root);
    assert!(before.complete, "node starts from its own complete history");
    assert_eq!(before.accepted().len(), 1);
    let ports = [port(), port()];
    let node_profile = profile(&h, UNDER_TEST, node_root, PEER, ports[1]);
    let peer_profile = profile(&h, PEER, peer_root, UNDER_TEST, ports[0]);
    let peer = open_peer(&peer_profile, ports[1]).await;
    let peer_lines = Lines::default();
    let peer_ops: BTreeSet<Digest> = h
        .peer_ops
        .iter()
        .map(|op| op.operation.canonical().digest())
        .collect();

    // Directed cut: the binary dials its configured peer automatically.
    let mut node = Process::start(&node_profile, ports[0]);
    let link = accept(&peer).await;
    let refused = count(&node.lines(), "independent exchange refused: Unavailable");
    let link = Arc::new(CutLink {
        inner: link,
        send: cut.send,
        after: cut.after,
        sent: AtomicUsize::new(0),
        received: AtomicUsize::new(0),
        ahead: tokio::sync::Mutex::new(VecDeque::new()),
        reached: AtomicBool::new(false),
    });
    let shared: Arc<dyn CarrierLink> = link.clone();
    let result = round(&peer, shared.clone(), false, &peer_lines).await;
    shared.close().await;
    assert_eq!(result, Err(Fault::Unavailable), "{}: peer round", cut.name);
    assert!(
        link.reached.load(Ordering::SeqCst),
        "{}: node never entered the phase: {:?}",
        cut.name,
        node.lines()
    );
    eventually(
        "node reports the interrupted round",
        || {
            let lines = node.lines();
            count(&lines, "independent exchange refused: Unavailable") > refused
                && cut
                    .line
                    .is_none_or(|line| lines.iter().any(|seen| seen == line))
        },
        || format!("{}: {:?}", cut.name, node.lines()),
    )
    .await;
    // The node redials automatically. Hold that link open and silent so the
    // clean stop closes it inside HELLO (history-free) instead of waiting out
    // a pending dial.
    let redial = accept(&peer).await;
    node.stop().await;
    redial.close().await;

    // Exact durable outcome of the cut.
    let after = Durable::stopped(&h, UNDER_TEST, node_root);
    let new: Vec<_> = after.guards().split_off(before.floor.guards.len());
    let expected: Vec<_> = cut
        .guards
        .iter()
        .map(|(status, role)| (*status, (*role).to_owned()))
        .collect();
    assert_eq!(new, expected, "{}: node floor guards", cut.name);
    let peer_key = public_key(&PEER.seed);
    for status in after.floor.guards.values().skip(before.floor.guards.len()) {
        let (GuardStatus::Settled { guard, .. } | GuardStatus::Loss { guard, .. }) = status else {
            panic!("{}: unexpected guard {status:?}", cut.name);
        };
        assert_eq!(guard.source, peer_key, "{}: guard source", cut.name);
    }
    let observations = &after.image.observations[before.image.observations.len()..];
    let held: usize = after
        .image
        .observations
        .iter()
        .map(|observation| observation.expected.len())
        .sum();
    assert_eq!(
        after.image.obligations.len(),
        held,
        "{}: every observed obligation is kept",
        cut.name
    );
    if cut.guards.is_empty() {
        // IC3-GUARD table, before begin_ingress: no permit, no consumption.
        assert_eq!(after.floor.guards, before.floor.guards, "{}", cut.name);
        assert_eq!(after.image.observations, before.image.observations);
        assert_eq!(after.image.obligations, before.image.obligations);
        assert_eq!(after.image.permanent_loss, before.image.permanent_loss);
        assert_eq!(after.history, before.history, "{}: history", cut.name);
        assert!(
            after.complete && !after.loss,
            "{}: prior complete cut reopens complete",
            cut.name
        );
    } else {
        // Receive invoked, then disconnect: the drained guard is retained as
        // qualified permanent loss, never as an invented (empty) full cut.
        assert_eq!(after.losses().len(), 1, "{}", cut.name);
        assert_eq!(after.image.permanent_loss.len(), 1, "{}", cut.name);
        assert!(
            after.loss && !after.complete,
            "{}: no complete cut after an interrupted receive",
            cut.name
        );
        let settled = cut.guards.iter().filter(|(status, _)| *status == "settled");
        assert_eq!(observations.len(), settled.count(), "{}", cut.name);
        let inventory = observations.first();
        if let Some(inventory) = inventory {
            let missing = after
                .image
                .obligations
                .iter()
                .filter(|obligation| {
                    obligation.observation == inventory.id
                        && obligation.classification == Classification::Missing
                })
                .count();
            assert_eq!(inventory.expected.len(), 2, "{}: full manifest", cut.name);
            assert_eq!(
                missing,
                2 - cut.delivered,
                "{}: undelivered obligations remain Missing",
                cut.name
            );
        }
        let reached: BTreeSet<_> = after
            .history
            .iter()
            .map(|bundle| bundle.candidate.operation.canonical().digest())
            .filter(|operation| peer_ops.contains(operation))
            .collect();
        assert_eq!(reached.len(), cut.delivered, "{}: history", cut.name);
        if cut.delivered == 0 {
            assert_eq!(after.history, before.history, "{}: history", cut.name);
        }
    }
    println!(
        "c3a cut={} guards={:?} observations={} permanent_loss={} complete={}",
        cut.name,
        new,
        observations.len(),
        after.image.permanent_loss.len(),
        after.complete
    );

    // Retry heals automatically through both nodes' actual serve.
    let all: BTreeSet<Digest> = h
        .node_ops
        .iter()
        .chain(&h.peer_ops)
        .map(|op| op.operation.canonical().digest())
        .collect();
    let mut node = Process::start(&node_profile, ports[0]);
    node.wait("listening ").await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(serve(
        peer.clone(),
        Arc::new(peer_lines.clone()),
        async move {
            let _ = stopped.await;
        },
    ));
    eventually(
        "both histories converge after automatic retry",
        || with(&peer, |session| converged(session, &all)).unwrap_or(false),
        || {
            let state = with(&peer, Durable::of).map(|peer| peer.summary());
            format!(
                "{}: node {:?} peer {:?} peer state {state:?}",
                cut.name,
                node.lines(),
                peer_lines.all()
            )
        },
    )
    .await;
    // Quiesce before the clean stops, so no stop lands inside a last no-op
    // round: a lossless replica's serve reports a settled idle cut only with
    // no round in flight. A node holding loss never reports one, so after the
    // peer's report its last round's tail gets a bounded drain.
    let lossy = !cut.guards.is_empty();
    let quiet = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < quiet
        && (idle(&peer_lines.all()).is_none() || (!lossy && idle(&node.lines()).is_none()))
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if lossy {
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
    let node_idle = idle(&node.lines());
    node.stop().await;
    stop.send(()).unwrap();
    serving.await.unwrap().unwrap();
    peer.carrier.close().await;
    peer.close().unwrap();
    drop(peer);
    let peer_state = recovered(&peer_profile);
    let healed = recovered(&node_profile);
    assert!(
        healed.history == peer_state.history,
        "{}: equal history in both directions; healed {} peer {}",
        cut.name,
        healed.summary(),
        peer_state.summary()
    );
    assert_eq!(healed.accepted(), all, "{}: node accepted", cut.name);
    assert_eq!(peer_state.accepted(), all, "{}: peer accepted", cut.name);
    for state in [&healed, &peer_state] {
        assert!(
            state
                .image
                .obligations
                .iter()
                .all(|obligation| obligation.classification != Classification::Missing),
            "{}: retry classifies every kept obligation: {}",
            cut.name,
            state.summary()
        );
        assert!(
            !state.guards().iter().any(|(status, _)| *status == "active"),
            "{}: no orphan guard: {}",
            cut.name,
            state.summary()
        );
        // With equal history and nothing unresolved, retained loss is the
        // only thing that can block completeness, and it always does.
        assert_eq!(
            state.complete,
            state.losses().is_empty(),
            "{}: {}",
            cut.name,
            state.summary()
        );
    }
    // Once lost, later successful full inventories do not clear the loss:
    // the cut's exact loss survives restart and retry, and the retry itself
    // adds none on either side.
    assert_eq!(
        healed.losses(),
        after.losses(),
        "{}: exactly the cut's loss; healed {}",
        cut.name,
        healed.summary()
    );
    assert_eq!(
        healed.image.permanent_loss, after.image.permanent_loss,
        "{}: permanent loss markers",
        cut.name
    );
    assert!(
        peer_state.losses().is_empty(),
        "{}: the peer loses nothing; peer {}",
        cut.name,
        peer_state.summary()
    );
    if lossy {
        assert!(healed.loss && !healed.complete, "{}: sticky", cut.name);
        assert_eq!(
            node_idle, None,
            "{}: a lossy node reports no idle cut",
            cut.name
        );
    } else {
        assert!(
            node_idle
                .as_ref()
                .is_some_and(|line| line.ends_with(" complete=true")),
            "{}: lossless retry reaches a settled complete cut",
            cut.name
        );
    }
    println!(
        "c3a heal={} history={} accepted={} node_complete={} node_losses={} peer_losses={} node_idle={node_idle:?}",
        cut.name,
        healed.history.len(),
        healed.accepted().len(),
        healed.complete,
        healed.losses().len(),
        peer_state.losses().len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn directed_disconnect_within_hello_and_confirmation_consumes_nothing_then_heals() {
    for cut in [
        Cut {
            name: "hello-before-peer-hello",
            send: 0,
            after: 1,
            guards: &[],
            delivered: 0,
            line: None,
        },
        Cut {
            name: "hello-before-peer-confirmation",
            send: 1,
            after: 2,
            guards: &[],
            delivered: 0,
            line: None,
        },
    ] {
        witness(&cut).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn directed_disconnect_before_ready_busy_consumes_nothing_then_heals() {
    witness(&Cut {
        name: "readiness-before-peer-ready",
        send: 2,
        after: 3,
        guards: &[],
        delivered: 0,
        line: None,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn directed_disconnect_in_full_inventory_header_and_page_retains_loss_then_heals() {
    for cut in [
        Cut {
            name: "inventory-before-header",
            send: 3,
            after: 5,
            guards: &[("loss", "inventory")],
            delivered: 0,
            line: Some("independent inventory: Unavailable"),
        },
        Cut {
            name: "inventory-after-header-before-page",
            send: 4,
            after: 5,
            guards: &[("loss", "inventory")],
            delivered: 0,
            line: Some("independent inventory: Unavailable"),
        },
    ] {
        witness(&cut).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn directed_disconnect_in_missing_request_header_and_page_retains_loss_then_heals() {
    for cut in [
        Cut {
            name: "request-before-header",
            send: 5,
            after: 7,
            guards: &[("settled", "inventory"), ("loss", "page")],
            delivered: 0,
            line: Some("independent request: Unavailable"),
        },
        Cut {
            name: "request-after-header-before-page",
            send: 6,
            after: 7,
            guards: &[("settled", "inventory"), ("loss", "page")],
            delivered: 0,
            line: Some("independent request: Unavailable"),
        },
    ] {
        witness(&cut).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn directed_disconnect_in_peer_bundle_receive_retains_loss_and_obligations_then_heals() {
    for cut in [
        Cut {
            name: "bundle-before-first",
            send: 7,
            after: 8,
            guards: &[
                ("settled", "inventory"),
                ("settled", "page"),
                ("loss", "bundle"),
            ],
            delivered: 0,
            line: None,
        },
        Cut {
            name: "bundle-after-first-of-two",
            send: 8,
            after: 8,
            guards: &[
                ("settled", "inventory"),
                ("settled", "page"),
                ("settled", "bundle"),
                ("loss", "bundle"),
            ],
            delivered: 1,
            line: None,
        },
    ] {
        witness(&cut).await;
    }
}

/// `serve` keeps an inbound attempt across its loop's iterations. The peer's
/// actual `serve` (real Iroh, real disk, its configured door) is reached by a
/// raw dialer holding the node's endpoint key, whose QUIC handshake completes
/// at once but whose stream's first word, the carrier's preamble, is withheld
/// for `HELD_TICKS` of the peer's ticks: the attempt sits inside `serve`'s
/// accept while the loop iterates on every tick. The word is then sent and
/// this module's own `round` runs on the link as the dialer, exactly as the
/// node's own dial would run it.
mod held_accept {
    use super::*;
    use iroh::endpoint::{presets, Connection, PortmapperConfig, RecvStream, SendStream};
    use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr};
    use std::net::{Ipv4Addr, SocketAddr};

    /// The carrier's first word (`iroh_carrier`'s private `PREAMBLE`).
    const PREAMBLE: &[u8; 4] = b"gcl1";
    /// Ticks of the peer's loop the attempt stays in flight: the timer alone
    /// iterates `serve`'s `select!` this many times meanwhile.
    const HELD_TICKS: u64 = 20;

    /// A link on the raw dialer's connection, framed as the carrier frames
    /// (a `u32` little-endian length, then the bytes); a read takes one whole
    /// frame and nothing after it.
    struct Held {
        conn: Connection,
        send: tokio::sync::Mutex<SendStream>,
        recv: tokio::sync::Mutex<RecvStream>,
    }
    fn failed(error: impl std::fmt::Display) -> CarrierError {
        CarrierError::Transport(error.to_string())
    }
    impl CarrierLink for Held {
        fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
            Box::pin(async move {
                let len = u32::try_from(frame.len()).map_err(|_| CarrierError::FrameTooLarge)?;
                let bytes = [&len.to_le_bytes()[..], frame].concat();
                self.send
                    .lock()
                    .await
                    .write_all(&bytes)
                    .await
                    .map_err(failed)
            })
        }
        fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
            Box::pin(async move {
                let mut recv = self.recv.lock().await;
                let mut head = [0; 4];
                recv.read_exact(&mut head).await.map_err(failed)?;
                let mut frame = vec![0; u32::from_le_bytes(head) as usize];
                recv.read_exact(&mut frame).await.map_err(failed)?;
                Ok(Some(frame))
            })
        }
        fn close(&self) -> PortFuture<'_, ()> {
            Box::pin(async move {
                self.conn.close(0u32.into(), b"");
            })
        }
        fn remote_id(&self) -> Option<TransportId> {
            Some(TransportId(self.conn.remote_id().as_bytes().to_vec()))
        }
        fn channel_binding(&self, label: &[u8]) -> Option<ChannelBinding> {
            let mut bytes = [0; 32];
            self.conn
                .export_keying_material(&mut bytes, label, b"")
                .ok()?;
            Some(ChannelBinding(bytes))
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inbound_attempt_in_flight_across_serve_iterations_completes_its_round() {
        let base = std::env::temp_dir().canonicalize().unwrap();
        let roots: Vec<PathBuf> = ["dialer", "peer"]
            .iter()
            .map(|side| {
                let root = base.join(format!("glade-ic3-held-{side}-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&root);
                std::fs::create_dir_all(&root).unwrap();
                root
            })
            .collect();
        let h = history();
        prepare(&h, UNDER_TEST, &roots[0], &h.node_ops);
        prepare(&h, PEER, &roots[1], &h.peer_ops);
        let ports = [port(), port()];
        let dialer_profile = profile(&h, UNDER_TEST, &roots[0], PEER, ports[1]);
        let peer_profile = profile(&h, PEER, &roots[1], UNDER_TEST, ports[0]);
        let peer = open_peer(&peer_profile, ports[1]).await;
        let peer_lines = Lines::default();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(serve(
            peer.clone(),
            Arc::new(peer_lines.clone()),
            async move {
                let _ = stopped.await;
            },
        ));
        // The dialer: the node's own replica, its carrier unbound; only its
        // session hellos, receives and records.
        crate::test_support::children::settled();
        let provision = Arc::new(Provision::read(&dialer_profile).unwrap());
        let module = Arc::new(NodeAssembly::builder().build());
        let dialer = IndependentReplica::open(module, provision, IrohCarrier::new(None)).unwrap();
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::from_bytes(
                &EndpointKey::from_seed(UNDER_TEST.endpoint).seed(),
            ))
            .portmapper_config(PortmapperConfig::Disabled)
            .clear_ip_transports()
            .bind_addr((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .unwrap();
        let at = EndpointAddr::from_parts(
            EndpointId::from_bytes(&EndpointKey::from_seed(PEER.endpoint).endpoint_id).unwrap(),
            [TransportAddr::Ip(SocketAddr::from((
                Ipv4Addr::LOCALHOST,
                ports[1],
            )))],
        );
        let conn = endpoint
            .connect(at.clone(), crate::iroh_carrier::CARRIER_ALPN)
            .await
            .unwrap();
        // The attempt is inside the peer's accept, awaiting its first word.
        tokio::time::sleep(Duration::from_millis(HELD_TICKS * TICK_MS)).await;
        let opened = async {
            let (mut send, recv) = conn.open_bi().await.map_err(failed)?;
            send.write_all(PREAMBLE).await.map_err(failed)?;
            Ok::<_, CarrierError>(Held {
                conn: conn.clone(),
                send: tokio::sync::Mutex::new(send),
                recv: tokio::sync::Mutex::new(recv),
            })
        }
        .await;
        let dialer_lines = Lines::default();
        let result = match opened {
            Ok(held) => {
                let link: Arc<dyn CarrierLink> = Arc::new(held);
                let result = round(&dialer, link.clone(), true, &dialer_lines).await;
                link.close().await;
                result.map_err(|fault| format!("{fault:?}"))
            }
            Err(error) => Err(format!("{error:?}")),
        };
        assert!(
            result.is_ok(),
            "an inbound attempt in flight across {HELD_TICKS} of the peer's ticks completes \
             its round: {result:?}; dialer {:?}; peer {:?}",
            dialer_lines.all(),
            peer_lines.all()
        );
        eventually(
            "the peer's inbound round completes",
            || {
                peer_lines
                    .all()
                    .iter()
                    .any(|line| line.starts_with("independent exchanged "))
            },
            || format!("peer {:?}", peer_lines.all()),
        )
        .await;
        // Stop with a second attempt held in the accept: it is dropped once,
        // at the stop, and its connection closes; nothing keeps it after.
        let pending = endpoint
            .connect(at, crate::iroh_carrier::CARRIER_ALPN)
            .await
            .unwrap();
        stop.send(()).unwrap();
        serving.await.unwrap().unwrap();
        let closed =
            tokio::time::timeout(Duration::from_millis(DEADLINE_MS), pending.closed()).await;
        assert!(
            closed.is_ok(),
            "the attempt pending at the stop is dropped with serve's loop"
        );
        peer.carrier.close().await;
        peer.close().unwrap();
        dialer.close().unwrap();
        endpoint.close().await;
        for root in roots {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
