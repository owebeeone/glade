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

use crate::registry::{Record, RegistryApi, StoreApi, G_CLAIMS, HOME};
use crate::server::{Server, Shared};
use crate::store::Store;
use crate::sysdata::{ServeClaim, WorkspaceEntry};
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
}
