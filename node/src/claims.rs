//! Live directory minting (GLP-0006 P0.S2 — the audit's F1, fused with F2's
//! create ceremony). Production paths that mint `WorkspaceEntry` + `ServeClaim`
//! as ordinary origin-attributed REGISTRY appends — until this module, only
//! tests minted them (E2E-stage-1 audit, finding F1).
//!
//! The server ADOPTS the boot instance ([`Server::adopt_boot`]): the boot
//! `Registry` stays the single chain authority for this node's own directory
//! writes (records.json stays current; the instance lock lives as long as the
//! server), and every runtime mint is (1) appended to the registry, (2)
//! persisted, (3) landed in the served replica through the same verify path as
//! any carrier, (4) fanned out to local home subscribers, and (5) PUSHED to
//! every live peer link — the traces' B9 "directory ops replicate" step
//! (`mesh::push_home`).
//!
//! Serving a workspace ([`Server::serve_workspace`]) mints the entry (diffed —
//! re-serving appends nothing) + the first claim (epoch = fold max + 1, so a
//! restarted or taking-over node fences out any stale claim), then RENEWS the
//! lease on a cadence while serving. Lease expiry stays an absolute wall-clock
//! stamp judged at each reader's clock — the write path uses the clock, the
//! fold never does (WD §2).

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use glade_wire::cbor;
use glade_wire::generated::Op;

use crate::registry::{Record, RegistryApi, StoreApi, G_CLAIMS, G_PRINCIPALS, HOME};
use crate::server::{Server, Shared};
use crate::store::Store;
use crate::sysdata::{PrincipalRecord, ServeClaim, WorkspaceCreateReq, WorkspaceCreateRes, WorkspaceEntry};
use crate::sysdir::{now_ms, Boot};

/// Default serve-lease TTL — matches the 30s the traces and tests use.
pub const LEASE_TTL_MS: i64 = 30_000;
/// Default renewal cadence: a third of the TTL, so one missed renewal never
/// lapses a healthy holder.
pub const RENEW_EVERY_MS: u64 = 10_000;

fn other<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// The adopted directory-write authority: the boot instance (registry = chain
/// tips + records.json engine + instance lock) plus the shares this node is
/// live-serving (share -> claim epoch, the renewal set).
pub(crate) struct DirState {
    /// Our directory node id — the origin every mint is attributed to.
    pub(crate) node_id: String,
    lease_ms: i64,
    inner: Mutex<DirAuthority>,
}

pub(crate) struct DirAuthority {
    boot: Boot,
    served: BTreeMap<String, i64>,
}

impl DirAuthority {
    /// One attributed registry append; the caller persists + publishes.
    fn append(&mut self, rec: Record, origin: &str) -> io::Result<Op> {
        self.boot
            .registry
            .append_returning(rec, origin)
            .map_err(|e| other(format!("registry append rejected: {e:?}")))
    }

    /// Diff-idempotent append: a byte-identical record already in the fold is
    /// skipped (the `appdecl::register` rule, applied to runtime mints).
    fn append_diffed(&mut self, rec: Record, origin: &str) -> io::Result<Option<Op>> {
        let (glade_id, payload) = (rec.glade_id().to_string(), rec.encode());
        if self.boot.registry.contains(&glade_id, &payload) {
            return Ok(None);
        }
        self.append(rec, origin).map(Some)
    }

    /// Rewrite records.json (tmp+rename) with the registry's current fold.
    fn persist(&mut self) -> io::Result<()> {
        let snap = self.boot.registry.snapshot();
        self.boot.store.save(&snap)
    }
}

impl Server {
    /// Adopt the boot instance as this server's directory-write authority
    /// with the default lease tuning. See [`Server::adopt_boot_tuned`].
    pub async fn adopt_boot(&self, boot: Boot) -> io::Result<usize> {
        self.adopt_boot_tuned(boot, LEASE_TTL_MS, RENEW_EVERY_MS).await
    }

    /// Adopt the boot instance: seed its registry snapshot into the served
    /// replica (the home share stays an ORDINARY share, GDL-038), keep the
    /// registry as the chain authority for this node's own directory writes,
    /// and spawn the lease-renewal loop. `lease_ms`/`renew_ms` tune the claim
    /// TTL and renewal cadence (tests shorten them to observe renewal live).
    /// Returns how many ops the seed newly appended. Call once, before serving.
    pub async fn adopt_boot_tuned(&self, boot: Boot, lease_ms: i64, renew_ms: u64) -> io::Result<usize> {
        let seeded = self.seed_registry(&boot.registry.snapshot()).await;
        let state = DirState {
            node_id: boot.node_id.clone(),
            lease_ms,
            inner: Mutex::new(DirAuthority { boot, served: BTreeMap::new() }),
        };
        self.shared
            .dir
            .set(state)
            .map_err(|_| other("directory authority already adopted"))?;
        let shared = self.shared.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(renew_ms)).await;
                renew_leases(&shared).await;
            }
        });
        Ok(seeded)
    }

    /// Serve `share` from this node (F1): mint the `WorkspaceEntry` (diffed)
    /// and the first `ServeClaim` (epoch = fold max + 1), join the renewal
    /// set. In-process idempotent: a share already being served is a no-op.
    pub async fn serve_workspace(&self, share: &str, name: &str) -> io::Result<()> {
        serve_workspace_on(&self.shared, share, name).await.map(|_| ())
    }
}

/// The mint itself, callable from the create ceremony (`exchange.rs`) as well
/// as [`Server::serve_workspace`]. Returns whether anything NEW was minted —
/// false = we already held the live serve (the re-create idempotence case).
pub(crate) async fn serve_workspace_on(shared: &Arc<Shared>, share: &str, name: &str) -> io::Result<bool> {
    let Some(state) = shared.dir.get() else {
        return Err(other("no directory authority (adopt_boot first)"));
    };
    let node = state.node_id.clone();
    let mut ops = Vec::new();
    {
        let mut dir = state.inner.lock().await;
        if dir.served.contains_key(share) {
            return Ok(false); // already serving: records diff to nothing
        }
        let entry = WorkspaceEntry {
            workspace: share.into(),
            name: name.into(),
            eligible_hosts: vec![node.clone()],
        };
        if let Some(op) = dir.append_diffed(Record::Workspace(entry), &node)? {
            ops.push(op);
        }
        // Epoch fencing reads the SERVED replica (it may hold peer claims the
        // boot registry never saw); +1 bumps over any stale claim, ours or not.
        let epoch = 1 + {
            let st = shared.store.lock().await;
            max_claim_epoch(&st, share)
        };
        let claim = ServeClaim {
            node: node.clone(),
            share: share.into(),
            lease_expiry_ms: now_ms() + state.lease_ms,
            epoch,
        };
        ops.push(dir.append(Record::Serve(claim), &node)?);
        dir.served.insert(share.into(), epoch);
        dir.persist()?;
    }
    publish(shared, ops).await;
    Ok(true)
}

/// The glade-side create ceremony at the TARGET node (s-create D3→K1→H1,
/// audit F2): mint the WorkspaceEntry + first ServeClaim under our own origin
/// and join the renewal set — exactly [`serve_workspace_on`]. gwz-core
/// MATERIALIZATION (repos on disk) is deliberately EXTERNAL: grazel hooks it
/// around this ceremony (the app-owned-storage seam); the ceremony creates the
/// glade-side records only. Idempotent by diff: re-creating a workspace this
/// node already serves appends nothing and answers `created: false`.
pub(crate) async fn create_workspace(shared: &Arc<Shared>, req: &WorkspaceCreateReq) -> io::Result<WorkspaceCreateRes> {
    let Some(state) = shared.dir.get() else {
        return Err(other("no directory authority at the create target"));
    };
    let name = if req.name.is_empty() { req.workspace.clone() } else { req.name.clone() };
    let created = serve_workspace_on(shared, &req.workspace, &name).await?;
    Ok(WorkspaceCreateRes {
        workspace: req.workspace.clone(),
        node: state.node_id.clone(),
        created,
    })
}

/// Principals minimal (GLP-0006 P0.S7): a session Hello naming an UNKNOWN
/// principal auto-appends a minimal `PrincipalRecord` to `dir.principals` —
/// identity as DATA, nothing enforced (lifecycle is P2/glade-users; the two
/// layers stay unsmeared). No-op when the replica already knows the principal,
/// and on a store-only node (no directory authority to attribute the append
/// to — such sessions keep origin-as-identity, byte-for-byte).
pub(crate) async fn note_principal(shared: &Arc<Shared>, principal: &str) {
    let Some(state) = shared.dir.get() else { return };
    {
        let st = shared.store.lock().await;
        if knows_principal(&st, principal) {
            return; // already directory data — ours or a peer's witness
        }
    }
    let node = state.node_id.clone();
    let mut ops = Vec::new();
    {
        let mut dir = state.inner.lock().await;
        // append_diffed re-checks under the lock: two racing Hellos for the
        // same principal serialize here and the second diffs away.
        if let Ok(Some(op)) = dir.append_diffed(Record::Principal(PrincipalRecord { principal: principal.into() }), &node) {
            let _ = dir.persist();
            ops.push(op);
        }
    }
    publish(shared, ops).await;
}

/// Does the replica hold a PrincipalRecord for `principal` (any origin)?
fn knows_principal(store: &Store, principal: &str) -> bool {
    for (origin, _) in store.heads(HOME, G_PRINCIPALS, &[]) {
        for op in store.scan(HOME, G_PRINCIPALS, &[], &origin, i64::MIN) {
            if PrincipalRecord::from_cbor(&cbor::decode(&op.payload)).principal == principal {
                return true;
            }
        }
    }
    false
}

/// Renew every served share's lease: same epoch, fresh absolute expiry — an
/// ordinary ServeClaim append (a renewal is data, never a heartbeat protocol).
async fn renew_leases(shared: &Arc<Shared>) {
    let Some(state) = shared.dir.get() else { return };
    let node = state.node_id.clone();
    let mut ops = Vec::new();
    {
        let mut dir = state.inner.lock().await;
        if dir.served.is_empty() {
            return;
        }
        let served: Vec<(String, i64)> = dir.served.iter().map(|(s, e)| (s.clone(), *e)).collect();
        for (share, epoch) in served {
            let claim = ServeClaim {
                node: node.clone(),
                share,
                lease_expiry_ms: now_ms() + state.lease_ms,
                epoch,
            };
            match dir.append(Record::Serve(claim), &node) {
                Ok(op) => ops.push(op),
                Err(_) => break, // a rejected chain append: stop, next tick retries
            }
        }
        let _ = dir.persist();
    }
    publish(shared, ops).await;
}

/// Land freshly-minted directory ops: into the served replica (same verify
/// path as any carrier), out to local home subscribers, and pushed to every
/// live peer link (trace B9 — directory ops replicate).
pub(crate) async fn publish(shared: &Arc<Shared>, ops: Vec<Op>) {
    if ops.is_empty() {
        return;
    }
    let from = shared.next.fetch_add(1, Ordering::SeqCst);
    for op in &ops {
        crate::mesh::ingest_and_fanout(shared, from, op.clone()).await;
    }
    crate::mesh::push_home(shared, ops).await;
}

/// Highest claim epoch the replica has seen for `share` — live or lapsed;
/// fencing bumps over both.
fn max_claim_epoch(store: &Store, share: &str) -> i64 {
    let mut max = 0;
    for (origin, _) in store.heads(HOME, G_CLAIMS, &[]) {
        for op in store.scan(HOME, G_CLAIMS, &[], &origin, i64::MIN) {
            let c = ServeClaim::from_cbor(&cbor::decode(&op.payload));
            if c.share == share && c.epoch > max {
                max = c.epoch;
            }
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iroh_carrier::PeerEndpoint;
    use crate::mesh::who_serves;
    use crate::sysdir::boot_at;
    use std::path::PathBuf;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-claims-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Poll until `pred` (over the node's store) holds, or panic after ~5s.
    async fn wait_store<F: Fn(&Store) -> bool>(shared: &Arc<Shared>, pred: F, what: &str) {
        for _ in 0..500 {
            if pred(&*shared.store.lock().await) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn max_lease(store: &Store, share: &str, node: &str) -> i64 {
        let mut max = i64::MIN;
        for (origin, _) in store.heads(HOME, G_CLAIMS, &[]) {
            for op in store.scan(HOME, G_CLAIMS, &[], &origin, i64::MIN) {
                let c = ServeClaim::from_cbor(&cbor::decode(&op.payload));
                if c.share == share && c.node == node && c.lease_expiry_ms > max {
                    max = c.lease_expiry_ms;
                }
            }
        }
        max
    }

    /// F1 live, two booted nodes over real iroh: B starts serving a workspace
    /// AFTER the link is up — the minted WorkspaceEntry + ServeClaim reach A's
    /// replica by PUSH (not connect-time anti-entropy), A's local fold routes
    /// the share to B, and the lease RENEWS while serving (A's observed expiry
    /// advances, so the claim outlives its original horizon).
    #[tokio::test(flavor = "multi_thread")]
    async fn self_claim_mints_renews_and_routes_live_two_node() {
        let boot_a = boot_at(fresh("f1-a-sys"), "gianni").unwrap();
        let boot_b = boot_at(fresh("f1-b-sys"), "gianni").unwrap();
        let b_id = boot_b.node_id.clone();

        let a = Server::open(fresh("f1-a-store")).unwrap();
        let b = Server::open(fresh("f1-b-store")).unwrap();
        let id_a = boot_a.identity().unwrap();
        let id_b = boot_b.identity().unwrap();
        a.adopt_boot(boot_a).await.unwrap();
        // B renews fast so the test OBSERVES renewal (lease 1.5s, renew 300ms).
        b.adopt_boot_tuned(boot_b, 1_500, 300).await.unwrap();

        let ep_a = PeerEndpoint::bind_with(id_a).await.unwrap();
        let ep_b = PeerEndpoint::bind_with(id_b).await.unwrap();
        a.enable_mesh(ep_a).await.unwrap();
        let addr_b = b.enable_mesh(ep_b).await.unwrap();
        a.connect_peer(&addr_b).await.unwrap();

        // Serve AFTER connect: propagation can only be the push path (B9).
        b.serve_workspace("ws-live", "live").await.unwrap();

        // (a) the self-claim appears at A and A's LOCAL fold routes to B.
        let (a_shared, b_shared) = (a.shared.clone(), b.shared.clone());
        {
            let bid = b_id.clone();
            wait_store(&a_shared, move |st| who_serves(st, "ws-live", now_ms()) == Some(bid.clone()), "A to route ws-live to B").await;
        }
        {
            let st = a_shared.store.lock().await;
            let hosts: Vec<String> = {
                // the entry replicated too (eligible host = the loader).
                let mut hosts = Vec::new();
                for (origin, _) in st.heads(HOME, crate::registry::G_WORKSPACES, &[]) {
                    for op in st.scan(HOME, crate::registry::G_WORKSPACES, &[], &origin, i64::MIN) {
                        let e = WorkspaceEntry::from_cbor(&cbor::decode(&op.payload));
                        if e.workspace == "ws-live" {
                            hosts = e.eligible_hosts.clone();
                        }
                    }
                }
                hosts
            };
            assert_eq!(hosts, vec![b_id.clone()]);
        }

        // (b) renewal: the observed lease horizon ADVANCES on both replicas.
        let first = {
            let st = a_shared.store.lock().await;
            max_lease(&st, "ws-live", &b_id)
        };
        {
            let bid = b_id.clone();
            wait_store(&a_shared, move |st| max_lease(st, "ws-live", &bid) > first, "a renewed lease to reach A").await;
        }
        {
            let bid = b_id.clone();
            let st = b_shared.store.lock().await;
            assert!(max_lease(&st, "ws-live", &bid) >= first, "the holder renews its own replica");
        }

        // (c) judged at a reader clock PAST the original horizon, the renewed
        // claim still routes — serving outlives any single lease stamp.
        {
            let st = a_shared.store.lock().await;
            assert_eq!(who_serves(&st, "ws-live", first), Some(b_id.clone()));
        }

        // in-process idempotence: re-serving mints nothing new.
        assert!(!serve_workspace_on(&b.shared, "ws-live", "live").await.unwrap());
    }

    /// Principals minimal (P0.S7), end to end on a booted node: a session
    /// Hello naming a principal BINDS and auto-appends a minimal record that
    /// is served through the ORDINARY subscribe path on dir.principals
    /// (origin-attributed to the node — the R3 precedent); a second Hello for
    /// the same principal appends nothing; a session WITHOUT a Hello keeps
    /// origin-as-identity and mints nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn hello_principal_binds_and_lands_in_dir_principals() {
        use crate::frame::Frame;
        use crate::registry::G_PRINCIPALS;
        use crate::ws;
        use glade_wire::generated::{Hello, Subscribe};

        let boot = boot_at(fresh("p-sys"), "gianni").unwrap();
        let node_id = boot.node_id.clone();
        let server = Server::open(fresh("p-store")).unwrap();
        server.adopt_boot(boot).await.unwrap();
        let shared = server.shared.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(server.run(listener));

        let hello = |principal: Option<&str>| {
            Frame::Hello(Hello {
                session: "s".into(),
                protocol: 1,
                principal: principal.map(str::to_string),
                capability: None,
                heads: vec![],
            })
            .to_bytes()
        };
        let sub = Frame::Subscribe(Subscribe { share: HOME.into(), glade_id: G_PRINCIPALS.into(), key: None, from: None }).to_bytes();
        async fn next(r: &mut ws::WsReader, what: &str) -> Frame {
            let msg = tokio::time::timeout(Duration::from_secs(5), r.read())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                .unwrap();
            match msg {
                ws::Msg::Binary(b) => Frame::from_bytes(&b).unwrap(),
                _ => panic!("unexpected close waiting for {what}"),
            }
        }
        fn principal_count(st: &Store, principal: &str) -> usize {
            let mut n = 0;
            for (origin, _) in st.heads(HOME, G_PRINCIPALS, &[]) {
                for op in st.scan(HOME, G_PRINCIPALS, &[], &origin, i64::MIN) {
                    if PrincipalRecord::from_cbor(&cbor::decode(&op.payload)).principal == principal {
                        n += 1;
                    }
                }
            }
            n
        }

        // (a) bind: Hello with a principal is Welcomed and the record lands.
        let (mut r1, w1) = ws::connect("127.0.0.1", port).await.unwrap();
        w1.send_binary(&hello(Some("alice"))).await.unwrap();
        assert!(matches!(next(&mut r1, "welcome").await, Frame::Welcome(_)));
        assert_eq!(shared.principals.lock().await.values().filter(|p| p.as_str() == "alice").count(), 1, "the session is BOUND");

        // (b) served via the ORDINARY subscribe path, origin-attributed.
        let (mut r2, w2) = ws::connect("127.0.0.1", port).await.unwrap();
        w2.send_binary(&sub).await.unwrap();
        assert!(matches!(next(&mut r2, "dir.principals ack").await, Frame::Heads(_)));
        match next(&mut r2, "the alice record").await {
            Frame::Ops(ops) => {
                assert_eq!(ops.ops.len(), 1);
                assert_eq!(ops.ops[0].origin, node_id, "attributed to the witnessing node's chain");
                assert_eq!(PrincipalRecord::from_cbor(&cbor::decode(&ops.ops[0].payload)).principal, "alice");
            }
            other => panic!("expected the principal record, got {other:?}"),
        }

        // (c) a second Hello for the SAME principal appends nothing new...
        let (mut r3, w3) = ws::connect("127.0.0.1", port).await.unwrap();
        w3.send_binary(&hello(Some("alice"))).await.unwrap();
        assert!(matches!(next(&mut r3, "second welcome").await, Frame::Welcome(_)));
        // ...but a NEW principal lands (and reaches the live subscriber).
        let (mut r4, w4) = ws::connect("127.0.0.1", port).await.unwrap();
        w4.send_binary(&hello(Some("bob"))).await.unwrap();
        assert!(matches!(next(&mut r4, "bob welcome").await, Frame::Welcome(_)));
        match next(&mut r2, "the bob record, live").await {
            Frame::Ops(ops) => {
                assert_eq!(PrincipalRecord::from_cbor(&cbor::decode(&ops.ops[0].payload)).principal, "bob");
            }
            other => panic!("expected the live principal record, got {other:?}"),
        }
        {
            let st = shared.store.lock().await;
            assert_eq!(principal_count(&st, "alice"), 1, "no duplicate for a known principal");
            assert_eq!(principal_count(&st, "bob"), 1);
        }

        // (d) no Hello (and a Hello with NO principal) = origin-as-identity,
        // nothing minted — the back-compat contract.
        let (mut r5, w5) = ws::connect("127.0.0.1", port).await.unwrap();
        w5.send_binary(&hello(None)).await.unwrap();
        assert!(matches!(next(&mut r5, "plain welcome").await, Frame::Welcome(_)));
        {
            let st = shared.store.lock().await;
            let total: usize = st.heads(HOME, G_PRINCIPALS, &[]).iter().map(|(o, s)| { let _ = o; (*s + 1) as usize }).sum();
            assert_eq!(total, 2, "exactly alice + bob — plain sessions mint nothing");
        }
    }
}
