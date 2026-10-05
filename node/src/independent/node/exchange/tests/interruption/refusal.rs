//! IC3-GUARD-002 / IC3-SYNC-004 at the actual node: a peer frame that decodes
//! and passes the receive's own checks, yet is off the header's signed manifest
//! when its settlement is checked, is retained as loss. It is never left as an
//! Active guard, so the next round proceeds and the stop is clean.
//!
//! The node under test is the actual binary. Its peer is an in-process actual
//! replica whose real link a test-only `CarrierLink` rewrites: one entry of the
//! peer's full-inventory page no longer matches the manifest the signed header
//! names, while the page's own manifest field is left alone, so only the
//! settlement can refuse it.
use super::*;

/// The peer's frames in order: hello 0, confirmation 1, ready 2, full-inventory
/// header 3, full-inventory page 4.
const PAGE: usize = 4;

struct RewritePage {
    inner: Box<dyn CarrierLink>,
    sent: AtomicUsize,
}
impl CarrierLink for RewritePage {
    fn send<'a>(&'a self, frame: &'a [u8]) -> PortFuture<'a, Result<(), CarrierError>> {
        Box::pin(async move {
            if self.sent.fetch_add(1, Ordering::SeqCst) != PAGE {
                return self.inner.send(frame).await;
            }
            let mut page: InventoryPage =
                decode_profile(frame, &limits()).expect("the peer's page frame");
            page.entries[0].bundle[0] ^= 0xFF;
            let rewritten = encode_profile(&page, &limits()).unwrap();
            self.inner.send(rewritten.value()).await
        })
    }
    fn recv(&self) -> PortFuture<'_, Result<Option<Vec<u8>>, CarrierError>> {
        self.inner.recv()
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

struct Roots(Vec<PathBuf>);
impl Drop for Roots {
    fn drop(&mut self) {
        for root in &self.0 {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn page_entry_off_the_signed_manifest_retains_loss_then_the_next_round_proceeds() {
    let base = std::env::temp_dir().canonicalize().unwrap();
    let roots = Roots(
        ["node", "peer"]
            .iter()
            .map(|side| {
                let root = base.join(format!("glade-ic3-refusal-{side}-{}", std::process::id()));
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
    assert!(before.complete && before.accepted().len() == 1);
    let ports = [port(), port()];
    let node_profile = profile(&h, UNDER_TEST, node_root, PEER, ports[1]);
    let peer_profile = profile(&h, PEER, peer_root, UNDER_TEST, ports[0]);
    let peer = open_peer(&peer_profile, ports[1]).await;
    let peer_lines = Lines::default();
    let mut node = Process::start(&node_profile, ports[0]);

    // The first round's page is rewritten off its signed manifest.
    let link: Arc<dyn CarrierLink> = Arc::new(RewritePage {
        inner: accept(&peer).await,
        sent: AtomicUsize::new(0),
    });
    let first = round(&peer, link.clone(), false, &peer_lines).await;
    link.close().await;
    assert!(
        first.is_err(),
        "the node refuses the rewritten page: {first:?}"
    );
    eventually(
        "the node reports its refused inventory",
        || {
            node.lines()
                .iter()
                .any(|line| line.starts_with("independent inventory: Integrity"))
        },
        || format!("node {:?}", node.lines()),
    )
    .await;

    // The node redials and the next round is honest: it must proceed.
    let link: Arc<dyn CarrierLink> = Arc::from(accept(&peer).await);
    let second = round(&peer, link.clone(), false, &peer_lines).await;
    link.close().await;
    assert!(
        second.is_ok(),
        "the round after a refused input proceeds: {second:?}; node {:?}; peer {:?}",
        node.lines(),
        peer_lines.all()
    );
    // The node redials to confirm what it received; that link stays silent so the
    // stop closes it inside HELLO, before any guard.
    let redial = accept(&peer).await;
    node.stop().await;
    redial.close().await;

    let after = Durable::stopped(&h, UNDER_TEST, node_root);
    let losses = after.losses();
    assert_eq!(
        losses.len(),
        1,
        "exactly one Loss guard: {}",
        after.summary()
    );
    let GuardStatus::Loss { guard, .. } = &losses[0] else {
        unreachable!("losses are Loss guards");
    };
    assert_eq!(guard.role, "inventory");
    assert_eq!(guard.source, public_key(&PEER.seed));
    assert!(
        !after.guards().iter().any(|(status, _)| *status == "active"),
        "no Active guard: {}",
        after.summary()
    );
    assert_eq!(after.image.permanent_loss.len(), 1);
    assert!(after.loss && !after.complete, "{}", after.summary());
    let all: BTreeSet<Digest> = h
        .node_ops
        .iter()
        .chain(&h.peer_ops)
        .map(|op| op.operation.canonical().digest())
        .collect();
    assert_eq!(
        after.accepted(),
        all,
        "the round that proceeded delivered the peer's history: {}",
        after.summary()
    );
    peer.carrier.close().await;
    peer.close().unwrap();
}
