use super::support::{
    behind_door, endpoint_of, fresh, node_of, noting_door, signed, Lines, A_KEY, A_SEED, B_KEY,
    B_SEED, C_KEY,
};
use crate::assembly::RelayState;
use crate::frame::MAX_FRAME_BYTES;
use crate::iroh_carrier::{carrier_addr, IrohCarrier};
use crate::mesh::home::pull_from;
use crate::mesh::link::relay_notes;
use crate::mesh::testing::meshed;
use crate::mesh::{hex_id, ingest_and_fanout};
use crate::netconf::PeerEntry;
use crate::peer::NodeIdentity;
use crate::registry::Record;
use crate::server::Server;
use crate::store::Append;
use crate::transport::EndpointKey;
use glade_carrier_api::CarrierPort;
use std::io;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

// ---- the door (plan Step 4.2b), over real iroh ---------------------------

/// A note that begins with `head` and ends with a time, ` ms`.
fn timed(head: String) -> impl Fn(&str) -> bool {
    move |line: &str| line.starts_with(&head) && line.ends_with(" ms")
}

/// Wait, bounded at 5 s, for a line in `lines` that `wanted` takes.
async fn noted(lines: &Lines, wanted: impl Fn(&str) -> bool) {
    for _ in 0..500 {
        if lines.lock().unwrap().iter().any(|line| wanted(line)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("not noted: {:?}", lines.lock().unwrap());
}

/// Plan Step 4.5, over real iroh on loopback: each end of a link notes the
/// path it sends on at HELLO, `link <node> via direct <ip:port>, rtt <n>
/// ms`, the address the other end is bound at, and notes `link <node>
/// closed` once the other end has closed.
#[tokio::test(flavor = "multi_thread")]
async fn each_end_notes_its_link_at_hello_and_its_close() {
    let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));
    let admits_a = [endpoint_of(A_KEY)];
    let b = noting_door("notes-link-b", (B_SEED, B_KEY), &admits_a, &[]).await;
    let (_b, _, b_notes, at_b) = b;
    let admits_b = [endpoint_of(B_KEY)];
    let a = noting_door("notes-link-a", (A_SEED, A_KEY), &admits_b, &[]).await;
    let (a, _, a_notes, at_a) = a;
    a.connect_peer(at_b.clone()).await.expect("a link");
    let link = |id: &str, at: &PeerEntry| format!("link {id} via direct {}, rtt ", at.via[0]);
    noted(&a_notes, timed(link(&b_id, &at_b))).await;
    noted(&b_notes, timed(link(&a_id, &at_a))).await;

    a.shared.mesh.get().unwrap().port.close().await;
    let closed = format!("link {a_id} closed");
    noted(&b_notes, |line| line == closed).await;
}

/// Plan Step 4.5: each end notes its `home` round when its pull from the
/// other ends, `home round with node <id>: <n> record(s) in <ms> ms`,
/// `n` being the other's `home` records it took: B holds two of its own,
/// and A one.
#[tokio::test(flavor = "multi_thread")]
async fn each_end_notes_its_home_round() {
    use crate::sysdata::{NodeRecord, PrincipalRecord};
    let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));
    let presence = |id: &str| {
        let (node_id, operator) = (id.to_string(), "gianni".to_string());
        Record::Node(NodeRecord { node_id, operator })
    };
    let principal = Record::Principal(PrincipalRecord {
        principal: "alice".into(),
    });
    let b_own = [signed(B_SEED, presence(&b_id)), signed(B_SEED, principal)];
    let a_own = [signed(A_SEED, presence(&a_id))];
    let admits_a = [endpoint_of(A_KEY)];
    let b = noting_door("notes-round-b", (B_SEED, B_KEY), &admits_a, &b_own).await;
    let (_b, _, b_notes, at_b) = b;
    let admits_b = [endpoint_of(B_KEY)];
    let a = noting_door("notes-round-a", (A_SEED, A_KEY), &admits_b, &a_own).await;
    let (a, _, a_notes, _) = a;
    a.connect_peer(at_b).await.expect("a link");
    let round = |id: &str, n: usize| format!("home round with node {id}: {n} record(s) in ");
    noted(&a_notes, timed(round(&b_id, 2))).await;
    noted(&b_notes, timed(round(&a_id, 1))).await;
}

/// Plan Step 4.5: the `relay` lines, from home relay states as the
/// adapter reads them off iroh, with no relay reached: `relay <url>` once
/// one is connected, again after a drop and at a change of home relay,
/// and `relay <url> not connected: <error>` once for each error, where
/// iroh reports a failure again at every retry.
#[test]
fn the_relay_lines_follow_the_home_relays_states() {
    let ap = "https://aps1-1.relay.n0.iroh.link./";
    let eu = "https://euc1-1.relay.n0.iroh.link./";
    let state = |url: &str, connected: bool, error: Option<&str>| {
        let (url, error) = (url.to_string(), error.map(str::to_string));
        vec![RelayState {
            url,
            connected,
            error,
        }]
    };
    let failed = |error: &str| vec![format!("relay {ap} not connected: {error}")];
    let (reset, late) = ("connection reset", "timed out");
    let steps = [
        (vec![], vec![]),
        (state(ap, false, None), vec![]),
        (state(ap, true, None), vec![format!("relay {ap}")]),
        (state(ap, true, None), vec![]),
        (state(ap, false, Some(reset)), failed(reset)),
        (state(ap, false, Some(reset)), vec![]),
        (state(ap, false, Some(late)), failed(late)),
        (state(ap, true, None), vec![format!("relay {ap}")]),
        (state(eu, false, None), vec![]),
        (state(eu, true, None), vec![format!("relay {eu}")]),
    ];
    let mut before = Vec::new();
    for (now, lines) in steps {
        assert_eq!(relay_notes(&before, &now), lines, "{now:?}");
        before = now;
    }
}

async fn links(server: &Server) -> usize {
    server.shared.mesh.get().unwrap().links.lock().await.len()
}

/// Done-when (plan Step 4.2b): an endpoint key the door does not know is
/// refused at accept, before HELLO. The refusing node reports one line
/// naming the key and the reason; the dialer learns no reason.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_endpoint_key_is_refused_at_accept_and_reported() {
    let (b, b_lines, at_b) = behind_door("door-unknown-b", (B_SEED, B_KEY), &[], &[]).await;
    let (a, _, _) = behind_door(
        "door-unknown-a",
        (A_SEED, A_KEY),
        &[endpoint_of(B_KEY)],
        &[],
    )
    .await;
    let refused = a
        .connect_peer(at_b.clone())
        .await
        .expect_err("an unknown key linked");
    assert_ne!(refused.kind(), io::ErrorKind::PermissionDenied, "{refused}");
    assert!(
        !refused.to_string().contains("unknown"),
        "a reason crossed: {refused}"
    );
    let line = format!(
        "peer refused: endpoint {}: unknown endpoint key",
        crate::transport::tag(&endpoint_of(A_KEY))
    );
    assert_eq!(*b_lines.lock().unwrap(), [line]);
    assert_eq!(links(&b).await, 0);
}

/// Done-when: a key bound by a record the door holds links; once the
/// door's fold has the node's revocation of it, the live link is closed
/// and the next connection is refused at accept.
#[tokio::test(flavor = "multi_thread")]
async fn a_bound_key_links_and_is_refused_once_its_revocation_lands() {
    use crate::transport::{sign_binding, sign_revocation};
    let bound = signed(
        A_SEED,
        Record::Transport(sign_binding(&A_SEED, &endpoint_of(A_KEY), 1)),
    );
    let (b, b_lines, at_b) = behind_door("door-bound-b", (B_SEED, B_KEY), &[], &[bound]).await;
    let (a, _, _) = behind_door("door-bound-a", (A_SEED, A_KEY), &[endpoint_of(B_KEY)], &[]).await;
    let linked = a.connect_peer(at_b.clone()).await;
    linked.expect("a bound key links");
    assert_eq!(links(&b).await, 1);

    let revoked = signed(
        A_SEED,
        Record::TransportRevoke(sign_revocation(&A_SEED, &endpoint_of(A_KEY))),
    );
    let from = b.shared.next.fetch_add(1, Ordering::SeqCst);
    let landed = ingest_and_fanout(&b.shared, from, revoked).await;
    assert!(matches!(landed, Ok(Append::Appended)), "{landed:?}");
    for _ in 0..500 {
        if links(&b).await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(links(&b).await, 0, "the live link was closed");
    a.connect_peer(at_b.clone())
        .await
        .expect_err("a revoked key linked");
    let key = crate::transport::tag(&endpoint_of(A_KEY));
    let node = hex_id(&node_of(A_SEED));
    let line = format!("peer refused: endpoint {key}: revoked by node {node}");
    assert_eq!(*b_lines.lock().unwrap(), [line]);
}

/// Done-when's HELLO half: a key the door knows, bound to another node,
/// admits the connection at accept, and the HELLO of a node not bound to
/// it is refused and reported, unanswered.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_bound_to_another_node_cannot_complete_hello() {
    let other = [25; 32];
    let binding = crate::transport::sign_binding(&other, &endpoint_of(A_KEY), 1);
    let elsewhere = signed(other, Record::Transport(binding));
    let (b, b_lines, at_b) =
        behind_door("door-elsewhere-b", (B_SEED, B_KEY), &[], &[elsewhere]).await;
    let (a, _, _) = behind_door(
        "door-elsewhere-a",
        (A_SEED, A_KEY),
        &[endpoint_of(B_KEY)],
        &[],
    )
    .await;
    a.connect_peer(at_b.clone())
        .await
        .expect_err("a node linked through another's key");
    let (m, n) = (hex_id(&node_of(other)), hex_id(&node_of(A_SEED)));
    let why = format!("HELLO refused: bound to node {m}, not {n}");
    let line = format!(
        "peer refused: endpoint {}: {why}",
        crate::transport::tag(&endpoint_of(A_KEY))
    );
    assert_eq!(*b_lines.lock().unwrap(), [line]);
    assert_eq!(links(&b).await, 0);
}

// ---- the mesh on the carrier port (plan Step 4.5b, part 3) -------------

/// Plan Step 4.5b (question 4): HELLO runs in the accepted link's own
/// task, not the accept loop. C, a key A's door knows, links to A at the
/// carrier, its first word sent, and says no HELLO; B then dials A and
/// links within a second, while C's HELLO still waits, unreported.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialer_that_never_says_hello_does_not_hold_the_accept_loop() {
    use crate::iroh_carrier::{Lent, FIRST_WORD};
    use glade_carrier_api::{CarrierAddr, CarrierConfig};
    let knows = [endpoint_of(B_KEY), endpoint_of(C_KEY)];
    let (a, a_lines, at_a) = behind_door("held-a", (A_SEED, A_KEY), &knows, &[]).await;
    let (b, _, _) = behind_door("held-b", (B_SEED, B_KEY), &[endpoint_of(A_KEY)], &[]).await;
    let key = EndpointKey::from_seed(C_KEY);
    let (door, relays, first_word) = (None, crate::netconf::Relays::Off, FIRST_WORD);
    let c = IrohCarrier::new(Some(Lent {
        key,
        door,
        relays,
        first_word,
    }));
    let local = CarrierAddr("127.0.0.1:0".into());
    let max_frame_bytes = std::num::NonZeroUsize::new(MAX_FRAME_BYTES).unwrap();
    c.bind(CarrierConfig {
        local,
        max_frame_bytes,
    })
    .await
    .unwrap();
    let to_a = carrier_addr(&at_a).unwrap();
    let silent = c.dial(&to_a).await.expect("C links at the carrier");

    let began = Instant::now();
    let linked = tokio::time::timeout(Duration::from_secs(1), b.connect_peer(at_a)).await;
    let Ok(linked) = linked else {
        panic!("B still dialing after {:?}", began.elapsed());
    };
    assert_eq!(linked.expect("B links"), hex_id(&node_of(A_SEED)));
    assert_eq!(links(&a).await, 1, "B alone is linked");
    let reported = a_lines.lock().unwrap().clone();
    assert_eq!(reported, Vec::<String>::new(), "C's HELLO still waits");
    drop(silent);
    c.close().await;
}

/// Plan Step 4.5b (section 5), in place of 4.1b's protocol-2 test: the
/// node's endpoint offers `glade/carrier/1` alone, so an endpoint that
/// offers only `glade/node/3`, as every node before 4.5b does, fails at
/// the handshake either way, before any HELLO: its dial to the node, and
/// the node's dial to it.
#[tokio::test(flavor = "multi_thread")]
async fn a_glade_node_3_endpoint_fails_at_connect_either_way() {
    use crate::netconf::Via;
    use iroh::endpoint::{presets, PortmapperConfig};
    use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
    let bound = Duration::from_secs(10);
    let v3: &[u8] = b"glade/node/3";
    let old = Endpoint::builder(presets::Minimal)
        .alpns(vec![v3.to_vec()])
        .portmapper_config(PortmapperConfig::Disabled)
        .clear_ip_transports()
        .bind_addr((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .bind()
        .await
        .unwrap();
    let old_accepts = old.clone();
    tokio::spawn(async move {
        while let Some(incoming) = old_accepts.accept().await {
            if let Ok(connecting) = incoming.accept() {
                let _ = connecting.await;
            }
        }
    });
    let a = Server::open(fresh("node-3-a")).unwrap();
    let at_a = meshed(&a, NodeIdentity::from_key(A_SEED)).await;
    let Some(Via::Ip(socket)) = at_a.via.first().cloned() else {
        panic!("A bound no socket: {at_a:?}");
    };
    let id = EndpointId::from_bytes(&at_a.key).unwrap();
    let to_a = EndpointAddr::from_parts(id, [TransportAddr::Ip(socket)]);
    let dialed = tokio::time::timeout(bound, old.connect(to_a, v3)).await;
    let refused = dialed.expect("bounded").is_err();
    assert!(refused, "a glade/node/3 dialer connected");

    let sockets = old.bound_sockets();
    let socket = sockets.into_iter().find(|socket| socket.is_ipv4()).unwrap();
    let key = *old.id().as_bytes();
    let at_old = PeerEntry {
        key,
        via: vec![Via::Ip(socket)],
    };
    let dialed = tokio::time::timeout(bound, a.connect_peer(at_old)).await;
    let refused = dialed.expect("bounded").is_err();
    assert!(refused, "a glade/node/3 endpoint took the node's dial");
    old.close().await;
}

/// Plan Step 4.5b (section 8, the table's race): A links to B twice, and
/// the newer link takes the older's place in each end's table. The older
/// then ends: each end notes its close, and each still holds the newer,
/// which serves a pull either way.
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_link_outlives_the_close_of_an_older_one_to_the_same_node() {
    let (a_id, b_id) = (hex_id(&node_of(A_SEED)), hex_id(&node_of(B_SEED)));
    let b = noting_door("twice-b", (B_SEED, B_KEY), &[endpoint_of(A_KEY)], &[]).await;
    let (b, _, b_notes, at_b) = b;
    let a = noting_door("twice-a", (A_SEED, A_KEY), &[endpoint_of(B_KEY)], &[]).await;
    let (a, _, a_notes, _) = a;
    a.connect_peer(at_b.clone()).await.expect("the older link");
    let (a_mesh, b_mesh) = (a.shared.mesh.get().unwrap(), b.shared.mesh.get().unwrap());
    let older = a_mesh.linked(&b_id).await.unwrap();
    a.connect_peer(at_b).await.expect("the newer link");
    older.end();
    let (a_closed, b_closed) = (format!("link {b_id} closed"), format!("link {a_id} closed"));
    noted(&a_notes, |line| line == a_closed).await;
    noted(&b_notes, |line| line == b_closed).await;
    let held = (links(&a).await, links(&b).await);
    assert_eq!(held, (1, 1), "each end holds the newer link");
    let pulled = pull_from(&a.shared, a_mesh, &b_id, node_of(B_SEED)).await;
    pulled.expect("the newer link serves A's pull");
    let pulled = pull_from(&b.shared, b_mesh, &a_id, node_of(A_SEED)).await;
    pulled.expect("and B's");
}
