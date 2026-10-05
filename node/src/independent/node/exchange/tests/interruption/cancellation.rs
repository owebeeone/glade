//! IC3-GUARD-003: cancellation before any guard exists begins none. A deadline
//! or a stop that closes a connection first cannot be followed by a durable
//! guard for it (and so by permanent loss for a receive that never began): not
//! at the node's local listener, which waits on the exchange gate, and not in
//! `serve`, whose dial can complete after the stop.
//!
//! The replicas are in-process actual replicas (real Iroh carrier, real disk);
//! the listener is `accept_independent_clients` itself on a real socket.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Provisioned scheduling for the listener witnesses: the deadline is long
/// enough for the possession exchange's own durable writes, short enough to wait.
const LOCAL_DEADLINE_MS: u64 = 1500;
/// Time for the node's local task, past its response read, to be waiting on the
/// exchange gate or the session lock held by the witness.
const SETTLE_MS: u64 = 300;

struct Roots(Vec<PathBuf>);
impl Drop for Roots {
    fn drop(&mut self) {
        for root in &self.0 {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
fn roots(name: &str, sides: &[&str]) -> Roots {
    let base = std::env::temp_dir().canonicalize().unwrap();
    Roots(
        sides
            .iter()
            .map(|side| {
                let root = base.join(format!(
                    "glade-ic3-cancel-{name}-{side}-{}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&root);
                std::fs::create_dir_all(&root).unwrap();
                root
            })
            .collect(),
    )
}

/// A writer that has sent its signed request and answered the node's challenge.
/// The node reads its bundle only once it holds the exchange gate. `before_response`
/// runs once the challenge is in hand, before the response that releases the node.
async fn possession<T>(
    port: u16,
    h: &History,
    before_response: impl FnOnce() -> T,
) -> (tokio::net::TcpStream, T) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let request = LocalChallengeBody {
        instance: h.f.descriptor.instance.clone(),
        identity: h.f.identity.clone(),
        resource: h.f.descriptor.resource.clone(),
        node: public_key(&UNDER_TEST.seed),
        requester: "alice".into(),
        writer_key: public_key(&WRITER),
        operation: h.node_ops[0].operation.canonical().digest(),
        role: "local-append-request".into(),
        channel: [81; 32],
        session_nonce: [82; 32],
        challenge_nonce: [83; 32],
        window: Window {
            not_before_ms: 0,
            not_after_ms: 1000,
        },
    };
    let raw = envelope(&signed(&WRITER, &request));
    stream.write_u32_le(raw.value().len() as u32).await.unwrap();
    stream.write_all(raw.value()).await.unwrap();
    let length = stream.read_u32_le().await.unwrap() as usize;
    let mut raw = vec![0; length];
    stream.read_exact(&mut raw).await.unwrap();
    let challenge: SignedRecord = decode_profile(&raw, &limits()).unwrap();
    let body: LocalChallengeBody = decode_signed_body(&challenge, &limits()).unwrap();
    let response = LocalResponseBody {
        instance: body.instance.clone(),
        node: body.node,
        requester: body.requester,
        writer_key: body.writer_key,
        signed_challenge: envelope(&challenge).digest(),
        operation: body.operation,
        channel: body.channel,
        session_nonce: body.session_nonce,
        challenge_nonce: body.challenge_nonce,
    };
    let held = before_response();
    let raw = envelope(&signed(&WRITER, &response));
    stream.write_u32_le(raw.value().len() as u32).await.unwrap();
    stream.write_all(raw.value()).await.unwrap();
    (stream, held)
}

/// How the local append's connection is cancelled.
#[derive(Clone, Copy)]
enum Cancel {
    /// The server's deadline for the connection passes.
    Deadline,
    /// The node is stopped (SIGTERM), with the deadline still far off.
    Stop,
}
/// What the local task is blocked on when it is cancelled.
#[derive(Clone, Copy)]
enum Blocked {
    /// The exchange gate, held by an exchange round (here by the witness).
    OnGate,
    /// The session lock, taken after the gate: the cancellation arrives with the
    /// gate already won and before `begin_ingress`, where only a check under the
    /// session lock can refuse it.
    OnSession,
}

/// A local writer, past its possession exchange, is cancelled before any guard
/// exists. The cancelled append must begin no guard, so retain no loss, and the
/// prior complete cut must stay complete.
async fn local_append_cancelled(name: &str, cancel: Cancel, blocked: Blocked) {
    let roots = roots(name, &["node"]);
    let root = &roots.0[0];
    let h = history();
    prepare(&h, UNDER_TEST, root, &h.node_ops);
    let before = Durable::stopped(&h, UNDER_TEST, root);
    assert!(before.complete && !before.loss);
    let mut provision = Provision::read(&profile(&h, UNDER_TEST, root, PEER, port())).unwrap();
    if matches!(cancel, Cancel::Deadline) {
        provision.deadline_ms = LOCAL_DEADLINE_MS;
    }
    crate::test_support::children::settled();
    let replica = Arc::new(
        IndependentReplica::open(
            Arc::new(NodeAssembly::builder().build()),
            Arc::new(provision),
            IrohCarrier::new(None),
        )
        .unwrap(),
    );
    let lines = Lines::default();
    let listener = Arc::new(
        tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap(),
    );
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let console: Arc<dyn Console> = Arc::new(lines.clone());
    let serving = tokio::spawn({
        let replica = replica.clone();
        async move {
            crate::server::accept_independent_clients(replica, &listener, console, async move {
                let _ = stopped.await;
            })
            .await
        }
    });
    let refused = || {
        lines
            .all()
            .iter()
            .any(|line| line.starts_with("independent local refused"))
    };

    let gate = match blocked {
        Blocked::OnGate => Some(replica.exchange_gate.lock().await),
        Blocked::OnSession => None,
    };
    let (mut stream, session) = possession(port, &h, || match blocked {
        Blocked::OnGate => None,
        Blocked::OnSession => Some(replica.session.lock().unwrap()),
    })
    .await;
    // The node reads the response, then waits on what is held: no step between is
    // durable or observable, so give it time to be there before anything cancels
    // it (a stop that came first would end it at the read instead). A plain thread
    // sleep: see the session case below.
    std::thread::sleep(Duration::from_millis(SETTLE_MS));
    let mut stop = Some(stop);
    if matches!(cancel, Cancel::Stop) {
        stop.take().unwrap().send(()).unwrap();
    }
    match blocked {
        // The cancellation ends the wait while the gate is still held, not only
        // once it is freed.
        Blocked::OnGate => {
            eventually(
                "the cancelled local append ends while the gate is held",
                refused,
                || format!("{:?}", lines.all()),
            )
            .await;
        }
        // The task is parked in its synchronous session lock: nothing can end it
        // until the lock is freed, by which time the cancellation has happened.
        // The wait is a plain thread sleep: the blocked worker may be the one the
        // runtime would drive its timers from, so a runtime timer could not fire.
        Blocked::OnSession => {
            let passed = match cancel {
                Cancel::Deadline => LOCAL_DEADLINE_MS + 300,
                Cancel::Stop => 500,
            };
            std::thread::sleep(Duration::from_millis(passed));
        }
    }
    drop(session);
    drop(gate);
    eventually("the local append ends", refused, || {
        format!("{:?}", lines.all())
    })
    .await;
    // Closed from the node's side: by its deadline, or by the stop.
    let mut byte = [0_u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut byte)).await;
    assert!(
        matches!(closed, Ok(Ok(0) | Err(_))),
        "{name}: the node closes the cancelled connection: {closed:?}"
    );
    if let Some(stop) = stop {
        stop.send(()).unwrap();
    }
    serving.await.unwrap().unwrap();
    replica.close().unwrap();
    drop(replica);

    let after = Durable::stopped(&h, UNDER_TEST, root);
    assert!(
        !lines
            .all()
            .iter()
            .any(|line| line.starts_with("independent local receive")),
        "{name}: a guard was begun for a cancelled append: {:?}",
        lines.all()
    );
    assert_eq!(after.floor.guards, before.floor.guards, "{name}: guards");
    assert_eq!(
        after.image.permanent_loss, before.image.permanent_loss,
        "{name}: no loss was retained"
    );
    assert_eq!(after.history, before.history, "{name}: history");
    assert!(
        after.complete && !after.loss,
        "{name}: the prior complete cut stays complete: {}",
        after.summary()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn local_append_whose_deadline_passes_waiting_on_the_gate_begins_no_guard() {
    local_append_cancelled("deadline-gate", Cancel::Deadline, Blocked::OnGate).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_append_waiting_on_the_gate_at_stop_begins_no_guard() {
    local_append_cancelled("stop-gate", Cancel::Stop, Blocked::OnGate).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_append_whose_deadline_passes_after_the_gate_is_won_begins_no_guard() {
    local_append_cancelled("deadline-session", Cancel::Deadline, Blocked::OnSession).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_append_stopped_after_the_gate_is_won_begins_no_guard() {
    local_append_cancelled("stop-session", Cancel::Stop, Blocked::OnSession).await;
}

/// `serve` is stopped while its dial is in flight: the peer's carrier does not
/// accept until after the stop, so the dial completes only then. That link
/// must not run a round, so the node begins no guard after its stop.
#[tokio::test(flavor = "multi_thread")]
async fn serve_stopped_while_a_dial_completes_begins_no_guard() {
    let roots = roots("serve", &["node", "peer"]);
    let (node_root, peer_root) = (&roots.0[0], &roots.0[1]);
    let h = history();
    prepare(&h, UNDER_TEST, node_root, &h.node_ops);
    prepare(&h, PEER, peer_root, &h.peer_ops);
    let before = Durable::stopped(&h, UNDER_TEST, node_root);
    assert!(before.complete && before.accepted().len() == 1);
    let ports = [port(), port()];
    let node = open_peer(
        &profile(&h, UNDER_TEST, node_root, PEER, ports[1]),
        ports[0],
    )
    .await;
    let peer = open_peer(
        &profile(&h, PEER, peer_root, UNDER_TEST, ports[0]),
        ports[1],
    )
    .await;
    let node_lines = Lines::default();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(serve(
        node.clone(),
        Arc::new(node_lines.clone()),
        async move {
            let _ = stopped.await;
        },
    ));
    // The first tick dials the peer, whose carrier is not accepting yet.
    tokio::time::sleep(Duration::from_millis(6 * TICK_MS)).await;
    assert!(!serving.is_finished());
    stop.send(()).unwrap();
    // Only now does the peer accept: the node's dial completes after its stop.
    let peer_lines = Lines::default();
    let peers = tokio::spawn({
        let peer = peer.clone();
        let peer_lines = peer_lines.clone();
        async move {
            let link: Arc<dyn CarrierLink> = Arc::from(accept(&peer).await);
            let result = round(&peer, link.clone(), false, &peer_lines).await;
            link.close().await;
            result
        }
    });
    let served = tokio::time::timeout(Duration::from_secs(20), serving).await;
    assert!(
        matches!(served, Ok(Ok(Ok(())))),
        "the stopped serve ends once its dial completes: {served:?}; node {:?}",
        node_lines.all()
    );
    let _ = peers.await;
    node.carrier.close().await;
    node.close().unwrap();
    peer.carrier.close().await;
    peer.close().unwrap();
    drop((node, peer));

    let after = Durable::stopped(&h, UNDER_TEST, node_root);
    assert_eq!(
        after.floor.guards,
        before.floor.guards,
        "a dial that completes after the stop begins no guard: node {:?}; peer {:?}",
        node_lines.all(),
        peer_lines.all()
    );
    assert_eq!(after.image.permanent_loss, before.image.permanent_loss);
    assert_eq!(after.history, before.history);
    assert!(after.complete && !after.loss, "{}", after.summary());
}
