//! Live directory minting (GLP-0006 P0.S2 — the audit's F1, fused with F2's
//! create ceremony). Production paths that mint `WorkspaceEntry` + `ServeClaim`
//! as ordinary origin-attributed REGISTRY appends — until this module, only
//! tests minted them (E2E-stage-1 audit, finding F1).
//!
//! The server ADOPTS the boot instance ([`Server::adopt_boot`]): the boot
//! `Registry` stays the single chain authority for this node's own directory
//! writes (records.json stays current; the instance lock lives as long as the
//! server), and every runtime mint is (1) appended to a staged copy of the
//! registry, (2) persisted, the copy becoming the fold only once the save
//! succeeded (slice profile SP-L1: nothing unsaved is folded or sent), (3)
//! landed in the served replica through the same verify path as any carrier,
//! (4) fanned out to local home subscribers, and (5) PUSHED to every live peer
//! link — the traces' B9 "directory ops replicate" step (`mesh::push_home`).
//! (3) and (4) run before the directory lock is let go, so the served replica
//! takes each of this node's chains in the order the registry minted it (plan
//! Step 4.4's question 5); (5) runs after it.
//!
//! Serving a workspace ([`Server::serve_workspace`]) mints the entry (diffed —
//! re-serving appends nothing) + the first claim (epoch = fold max + 1, so a
//! restarted or taking-over node fences out any stale claim), then RENEWS the
//! lease on a cadence while serving. Lease expiry stays an absolute wall-clock
//! stamp judged at each reader's clock — the write path uses the clock, the
//! fold never does (WD §2).
//!
//! The `home` share is served like any other (GDL-038; the lane owner's ruling
//! of 2026-09-25): it joins the renewal set at adoption, at the epoch of the
//! node's own claim on it, which a first boot minted (`sysdir.rs`), and is
//! renewed at once. So a node's `home` claim is live while the node runs, and
//! lapses a lease after it stops. Its epoch never moves: every node serves
//! `home` at once, so there is no stale holder to fence out. The design is
//! `glade/dev-docs/GladeNodeAssembly.md`, "The `home` claim renewed like any
//! served share".

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};

use glade_wire::generated::Op;

use crate::checkpoint;
use crate::envelope;
use crate::registry::{Record, Registry, RegistryApi, G_CLAIMS, G_PRINCIPALS, HOME};
use crate::server::{refresh_policy, Server, Shared};
use crate::store::Store;
use crate::sysdata::{PrincipalRecord, ServeClaim, WorkspaceCreateReq, WorkspaceCreateRes, WorkspaceEntry};
use crate::sysdir::{now_ms, Boot};
use crate::tasks::Site;

/// Default serve-lease TTL: five minutes (question 32 (a), the owner's ruling
/// of 2026-09-27), where it was 30 s. Each renewal is a signed record, kept
/// until a checkpoint folds it (plan Step 4.5c).
pub const LEASE_TTL_MS: i64 = 300_000;
/// Default renewal cadence: a third of the TTL, so one missed renewal never
/// lapses a healthy holder. 100 s; it was 10 s.
pub const RENEW_EVERY_MS: u64 = 100_000;
/// Default checkpoint threshold (plan Step 4.5c; the owner's ruling of
/// 2026-09-27): a tick folds the node's claims chain once 1,000 of its claims
/// are superseded, about 14 hours of renewals of two shares.
pub const CHECKPOINT_AFTER: usize = 1_000;

/// The node's leases, which are its settings (question 32 (a)): how long each
/// claim it mints lives, how often it renews the claims of what it serves,
/// and how many of its claims may be superseded before a renewal folds its
/// claims chain into a checkpoint (plan Step 4.5c). A composition root takes
/// them from its entry point and passes them down; the default is
/// [`LEASE_TTL_MS`] renewed every [`RENEW_EVERY_MS`], and a checkpoint once
/// [`CHECKPOINT_AFTER`] claims are superseded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leases {
    pub lease_ms: i64,
    pub renew_ms: u64,
    pub checkpoint_after: usize,
}

impl Default for Leases {
    fn default() -> Leases {
        Leases {
            lease_ms: LEASE_TTL_MS,
            renew_ms: RENEW_EVERY_MS,
            checkpoint_after: CHECKPOINT_AFTER,
        }
    }
}

/// The shortest lease `--lease-ms` sets (plan Step 4.6, question 2, the
/// owner's ruling of 2026-09-30): three seconds.
pub const MIN_LEASE_MS: u32 = 3_000;
/// The longest lease `--lease-ms` sets: an hour.
pub const MAX_LEASE_MS: u32 = 3_600_000;

impl Leases {
    /// The leases `--lease-ms <value>` sets (plan Step 4.6): each claim lives
    /// `value` ms and is renewed every `value / 3` ms, the default's rule, and
    /// the checkpoint threshold stays the default's, a setting with no flag.
    /// Any value but a whole number from [`MIN_LEASE_MS`] to [`MAX_LEASE_MS`]
    /// is refused, and the refusal names the range.
    pub fn from_flag(value: &str) -> io::Result<Leases> {
        let range = MIN_LEASE_MS..=MAX_LEASE_MS;
        let Some(ms) = value.parse::<u32>().ok().filter(|ms| range.contains(ms)) else {
            let why = format!(
                "--lease-ms {value:?}: expected a whole number of milliseconds \
                 from {MIN_LEASE_MS} to {MAX_LEASE_MS}"
            );
            return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
        };
        Ok(Leases {
            lease_ms: i64::from(ms),
            renew_ms: u64::from(ms / 3),
            ..Leases::default()
        })
    }
}

/// The line a booted root prints after `node` when `--lease-ms` set the
/// leases (plan Step 4.6): `leases <n> ms, renewed every <n/3> ms`.
impl fmt::Display for Leases {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (lease, renew) = (self.lease_ms, self.renew_ms);
        write!(f, "leases {lease} ms, renewed every {renew} ms")
    }
}

fn other<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// The adopted directory-write authority: the boot instance (registry = chain
/// tips + records.json engine + instance lock) plus the shares this node is
/// live-serving (share -> claim epoch, the renewal set): `home` from adoption
/// on, and each share `serve_workspace_on` enters.
pub(crate) struct DirState {
    /// Our directory node id — the origin every mint is attributed to.
    pub(crate) node_id: String,
    leases: Leases,
    /// Where the line each checkpoint prints goes: the reporter the
    /// composition root handed adoption (plan Step 4.5c).
    report: Box<dyn Fn(&str) + Send + Sync>,
    /// The directory lock. Lock order: this one, then the served store's, the
    /// router's or the session table's; never the reverse.
    inner: Mutex<DirAuthority>,
}

pub(crate) struct DirAuthority {
    boot: Boot,
    served: BTreeMap<String, i64>,
}

impl DirAuthority {
    /// Durable acceptance (slice profile SP-L1, plan Step 4.4): `change`
    /// appends to a staged copy of the registry, the copy is saved to
    /// records.json, and only then is it the fold. A refused append or a
    /// failed save leaves the fold and records.json as they were, so the
    /// caller has nothing to publish, and a retry starts from what was
    /// accepted.
    fn accept<T>(&mut self, change: impl FnOnce(&mut Registry) -> io::Result<T>) -> io::Result<T> {
        let boot = &mut self.boot;
        boot.registry.accept(&mut boot.store, change)
    }
}

/// One attributed append to the staged `registry`, as `DirAuthority::accept`
/// hands it.
fn append(registry: &mut Registry, rec: Record, origin: &str) -> io::Result<Op> {
    registry
        .append_returning(rec, origin)
        .map_err(|e| other(format!("registry append rejected: {e:?}")))
}

/// Diff-idempotent append: a byte-identical record already in the fold is
/// skipped (the `appdecl::register` rule, applied to runtime mints).
fn append_diffed(registry: &mut Registry, rec: Record, origin: &str) -> io::Result<Option<Op>> {
    let (glade_id, payload) = (rec.glade_id().to_string(), rec.encode());
    if registry.contains(&glade_id, &payload) {
        return Ok(None);
    }
    append(registry, rec, origin).map(Some)
}

impl Server {
    /// Adopt the boot instance as this server's directory-write authority
    /// with the default [`Leases`], where a test's lease does not matter, and
    /// its checkpoint lines going nowhere. See [`Server::adopt_boot_tuned`],
    /// which the composition roots call.
    pub async fn adopt_boot(&self, boot: Boot) -> io::Result<usize> {
        let (leases, nowhere) = (Leases::default(), |_: &str| {});
        self.adopt_boot_tuned(boot, leases, nowhere).await
    }

    /// Adopt the boot instance: seed its registry snapshot into the served
    /// replica (the home share stays an ORDINARY share, GDL-038), keep the
    /// registry as the chain authority for this node's own directory writes,
    /// start the renewal set with `home` at its epoch (`home_epoch`), renew
    /// it at once, and spawn the lease-renewal loop. `leases` are the claim
    /// TTL, the renewal cadence and the checkpoint threshold, the node's
    /// [`Leases`]: each composition root passes the ones its entry point gave
    /// it, and tests shorten them to observe renewal live. `report` takes the
    /// line each checkpoint prints (plan Step 4.5c): stdout on the
    /// hand-written root, the console on the assembled one. Returns how many
    /// ops the seed newly appended. Call once, before serving.
    ///
    /// The renewal at once makes a `home` claim that lapsed while the node was
    /// stopped live again before any peer or client can connect, and folds a
    /// long claims chain there. If its save fails, it is neither folded nor
    /// published, as at any tick, and the next tick retries it.
    ///
    /// The replica holds none of this node's `home` records from before plan
    /// Step 4.1b, unsigned: its `open` set those aside (as it did plan Step
    /// 4.1a's, under the node's old id), as boot set records.json's aside, so
    /// the seed lands the registry's signed records on empty chains.
    ///
    /// The registry's grant fold becomes the one the serve paths check (plan
    /// Step 4.3): until adoption they had none, and refused.
    pub async fn adopt_boot_tuned(
        &self,
        boot: Boot,
        leases: Leases,
        report: impl Fn(&str) + Send + Sync + 'static,
    ) -> io::Result<usize> {
        let seeded = self.seed_registry(&boot.registry.snapshot()).await;
        let policy = boot.registry.policy();
        let home = home_epoch(&*self.shared.store.lock().await, &boot.node_id);
        let served = BTreeMap::from([(HOME.to_string(), home)]);
        let state = DirState {
            node_id: boot.node_id.clone(),
            leases,
            report: Box::new(report),
            inner: Mutex::new(DirAuthority { boot, served }),
        };
        self.shared
            .dir
            .set(state)
            .map_err(|_| other("directory authority already adopted"))?;
        refresh_policy(&self.shared, policy).await;
        renew_leases(&self.shared).await;
        let shared = self.shared.clone();
        self.shared.tasks.spawn(Site::Renewal, async move {
            loop {
                tokio::time::sleep(Duration::from_millis(leases.renew_ms)).await;
                renew_leases(&shared).await;
            }
        });
        Ok(seeded)
    }

    /// Serve `share` from this node (F1): mint the `WorkspaceEntry` (diffed)
    /// and the first `ServeClaim` (epoch = fold max + 1), join the renewal
    /// set. In-process idempotent: a share already being served is a no-op,
    /// and so is `home`, in the set from adoption on.
    pub async fn serve_workspace(&self, share: &str, name: &str) -> io::Result<()> {
        serve_workspace_on(&self.shared, share, name).await.map(|_| ())
    }

    /// Which node serves `share` now, by the adopted registry's fold at this
    /// node's clock: what both roots print for `home` after adoption, in
    /// `registry ready (home served: ...)`. `None` before adoption.
    pub async fn serves(&self, share: &str) -> Option<String> {
        let state = self.shared.dir.get()?;
        let dir = state.inner.lock().await;
        dir.boot.registry.who_serves(share, now_ms())
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
    let mut dir = state.inner.lock().await;
    if dir.served.contains_key(share) {
        return Ok(false); // already serving: records diff to nothing
    }
    let entry = WorkspaceEntry {
        workspace: share.into(),
        name: name.into(),
        eligible_hosts: vec![node.clone()],
    };
    // Epoch fencing reads the SERVED replica (it may hold peer claims the
    // boot registry never saw); +1 bumps over any stale claim, ours or not.
    let epoch = 1 + {
        let st = shared.store.lock().await;
        max_claim_epoch(&st, share)
    };
    let claim = ServeClaim {
        node: node.clone(),
        share: share.into(),
        lease_expiry_ms: now_ms() + state.leases.lease_ms,
        epoch,
    };
    // Entry and claim are accepted together, after the last await before the
    // save: a cancelled call has folded nothing, and a failed save leaves the
    // share unserved, so a retry mints both again.
    let ops = dir.accept(|registry| {
        let mut ops = Vec::new();
        if let Some(op) = append_diffed(registry, Record::Workspace(entry), &node)? {
            ops.push(op);
        }
        ops.push(append(registry, Record::Serve(claim), &node)?);
        Ok(ops)
    })?;
    dir.served.insert(share.into(), epoch);
    publish(shared, dir, ops).await;
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
    let mut dir = state.inner.lock().await;
    // append_diffed re-checks under the lock: two racing Hellos for the
    // same principal serialize here and the second diffs away. A record
    // that fails to save is not published; the next Hello retries it.
    let record = Record::Principal(PrincipalRecord {
        principal: principal.into(),
    });
    let ops = match dir.accept(|registry| append_diffed(registry, record, &node)) {
        Ok(Some(op)) => vec![op],
        _ => Vec::new(),
    };
    publish(shared, dir, ops).await;
}

/// Does the replica hold a PrincipalRecord for `principal` (any origin)?
fn knows_principal(store: &Store, principal: &str) -> bool {
    for (origin, _) in store.heads(HOME, G_PRINCIPALS, &[]) {
        for op in store.scan(HOME, G_PRINCIPALS, &[], &origin, i64::MIN) {
            let record = envelope::folded(&op, PrincipalRecord::from_cbor);
            if record.is_some_and(|record| record.principal == principal) {
                return true;
            }
        }
    }
    false
}

/// Renew every served share's lease: same epoch, fresh absolute expiry — an
/// ordinary ServeClaim append (a renewal is data, never a heartbeat protocol).
/// Once enough of the node's claims are superseded, the tick also folds its
/// claims chain into a checkpoint (plan Step 4.5c, `checkpoint::tick`), and
/// says so through the root's reporter.
async fn renew_leases(shared: &Arc<Shared>) {
    let Some(state) = shared.dir.get() else { return };
    let (node, leases) = (state.node_id.clone(), state.leases);
    let mut dir = state.inner.lock().await;
    if dir.served.is_empty() {
        return;
    }
    let lease_expiry_ms = now_ms() + leases.lease_ms;
    let renewals = dir.served.iter().map(|(share, epoch)| ServeClaim {
        node: node.clone(),
        share: share.clone(),
        lease_expiry_ms,
        epoch: *epoch,
    });
    let renewals = renewals.collect();
    // One acceptance for the tick, its checkpoint included: if an append is
    // refused or the save fails, none is folded or published, and the next
    // tick retries.
    let tick = |registry: &mut Registry| {
        let ticked = checkpoint::tick(registry, &node, renewals, leases.checkpoint_after);
        ticked.map_err(|e| other(format!("registry append rejected: {e:?}")))
    };
    let (ops, folded) = dir.accept(tick).unwrap_or_default();
    publish(shared, dir, ops).await;
    if let Some(folded) = folded {
        (state.report)(&folded.to_string());
    }
}

/// Land freshly minted directory ops, then send them to peers. `dir` is the
/// directory lock they were minted under. Before it is let go, the ops go into
/// the served replica (the same verify path as any carrier) and out to local
/// home subscribers, so the served store takes each of this node's chains in
/// the order the registry minted it (plan Step 4.4's question 5). Local
/// fan-out only queues each session's frames on its unbounded channel, for
/// its writer task to send. The push to every live peer link (trace B9 —
/// directory ops replicate) comes after the release: no peer holds the lock.
pub(crate) async fn publish(shared: &Arc<Shared>, dir: MutexGuard<'_, DirAuthority>, ops: Vec<Op>) {
    if ops.is_empty() {
        return;
    }
    let from = shared.next.fetch_add(1, Ordering::SeqCst);
    for op in &ops {
        let _ = crate::mesh::ingest_and_fanout(shared, from, op.clone()).await;
    }
    drop(dir);
    crate::mesh::push_home(shared, ops).await;
}

/// The epoch `home` joins the renewal set at, at adoption: the highest of
/// `node`'s own claims on it in the served replica, live or lapsed, so its
/// renewals, and every later boot's, keep the epoch its first boot minted; 1,
/// that first epoch, if it holds none. Not a serve's [`max_claim_epoch`] + 1:
/// every node serves `home` at once, so no claim on it is a stale one to
/// fence out.
pub(crate) fn home_epoch(store: &Store, node: &str) -> i64 {
    let claims = store.scan(HOME, G_CLAIMS, &[], node, i64::MIN);
    let claims = claims
        .iter()
        .filter_map(|op| envelope::folded(op, ServeClaim::from_cbor));
    let home = claims.filter(|claim| claim.share == HOME);
    home.map(|claim| claim.epoch).max().unwrap_or(1)
}

/// Highest claim epoch the replica has seen for `share` — live or lapsed;
/// fencing bumps over both.
pub(crate) fn max_claim_epoch(store: &Store, share: &str) -> i64 {
    let mut max = 0;
    for (origin, _) in store.heads(HOME, G_CLAIMS, &[]) {
        for op in store.scan(HOME, G_CLAIMS, &[], &origin, i64::MIN) {
            let Some(c) = envelope::folded(&op, ServeClaim::from_cbor) else {
                continue;
            };
            if c.share == share && c.epoch > max {
                max = c.epoch;
            }
        }
    }
    max
}

// How a test changes the grant fold at run time, as a runtime route would
// (plan Step 4.3; `share.revoke`, or re-registration on a signal, neither
// built): through the directory authority, saved, then the view replaced and
// every admitted subscription checked again, then the records published. The
// section is test-only, one braced conditional module.
#[cfg(test)]
pub(crate) mod testing {
    use std::io;
    use std::sync::Arc;

    use crate::registry::Record;
    use crate::server::{refresh_policy, Shared};

    /// Append `records` under the adopted node's own chain and make the
    /// registry's grant fold the one checked. Returns the view's generation.
    pub(crate) async fn accept(shared: &Arc<Shared>, records: Vec<Record>) -> io::Result<u64> {
        let state = shared
            .dir
            .get()
            .ok_or_else(|| super::other("no directory authority"))?;
        let node = state.node_id.clone();
        let mut dir = state.inner.lock().await;
        let ops = dir.accept(|registry| {
            let appended = records
                .into_iter()
                .map(|rec| super::append(registry, rec, &node));
            appended.collect::<io::Result<Vec<_>>>()
        })?;
        let generation = refresh_policy(shared, dir.boot.registry.policy()).await;
        super::publish(shared, dir, ops).await;
        Ok(generation)
    }

    /// Renew the adopted node's leases once, as its loop does each tick.
    pub(crate) async fn tick(shared: &Arc<Shared>) {
        super::renew_leases(shared).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::testing::meshed;
    use crate::mesh::who_serves;
    use crate::sysdir::boot_at;
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::{pin, Pin};
    use std::task::Poll;

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
                let c = envelope::record(&op, ServeClaim::from_cbor).unwrap();
                if c.share == share && c.node == node && c.lease_expiry_ms > max {
                    max = c.lease_expiry_ms;
                }
            }
        }
        max
    }

    /// The leases a test renews by hand on: the default lease, the renewal
    /// loop an hour off, and a checkpoint once `after` claims are superseded.
    fn by_hand(after: usize) -> Leases {
        let renew_ms = 3_600_000;
        Leases {
            renew_ms,
            checkpoint_after: after,
            ..Leases::default()
        }
    }

    /// The lines a node's checkpoints report, as its root would print them.
    type Lines = Arc<std::sync::Mutex<Vec<String>>>;

    /// A reporter that keeps its lines, and the lines it keeps.
    fn kept() -> (Lines, impl Fn(&str) + Send + Sync + 'static) {
        let lines = Lines::default();
        let keep = lines.clone();
        let report = move |line: &str| keep.lock().unwrap().push(line.into());
        (lines, report)
    }

    /// A booted node with no mesh, adopted with the renewal loop an hour
    /// off, so a test renews by hand: its served state and its instance dir.
    async fn adopted(name: &str) -> (Arc<Shared>, PathBuf) {
        let (shared, sys, _) = folding(name, CHECKPOINT_AFTER).await;
        (shared, sys)
    }

    /// [`adopted`], its claims chain folded once `after` of its claims are
    /// superseded (plan Step 4.5c): also the lines its checkpoints report.
    async fn folding(name: &str, after: usize) -> (Arc<Shared>, PathBuf, Lines) {
        let sys = fresh(&format!("{name}-sys"));
        let boot = boot_at(sys.clone(), "gianni").unwrap();
        let server = Server::open(fresh(&format!("{name}-store"))).unwrap();
        let (lines, report) = kept();
        let adopted = server.adopt_boot_tuned(boot, by_hand(after), report);
        adopted.await.unwrap();
        (server.shared.clone(), sys, lines)
    }

    /// Make every save of the instance at `sys` fail, or work again: a
    /// directory where records.json's temp file goes refuses the save's first
    /// write, and leaves records.json as it is. Not a full disk or an I/O error.
    fn refuse_saves(sys: &std::path::Path, refuse: bool) {
        let blocker = sys.join("records.json.tmp");
        if refuse {
            std::fs::create_dir(&blocker).unwrap();
        } else {
            std::fs::remove_dir(&blocker).unwrap();
        }
    }

    /// How many records the served store holds on `glade_id` that `pick`
    /// accepts, from any origin: what has been published.
    fn published(store: &Store, glade_id: &str, pick: impl Fn(&[u8]) -> bool) -> usize {
        let mut n = 0;
        for (origin, _) in store.heads(HOME, glade_id, &[]) {
            for op in store.scan(HOME, glade_id, &[], &origin, i64::MIN) {
                n += usize::from(pick(&envelope::record_bytes(&op.payload)));
            }
        }
        n
    }

    /// The entries and the claims for `share` that the served store holds.
    fn published_serve(store: &Store, share: &str) -> (usize, usize) {
        let entry = |p: &[u8]| WorkspaceEntry::decode(p).unwrap().workspace == share;
        let claim = |p: &[u8]| ServeClaim::decode(p).unwrap().share == share;
        let entries = published(store, crate::registry::G_WORKSPACES, entry);
        (entries, published(store, G_CLAIMS, claim))
    }

    /// The ops records.json at `sys` holds.
    fn saved(sys: &std::path::Path) -> Vec<Op> {
        use crate::registry::StoreApi;
        let snap = crate::registry::BlobStore::new(sys).load().unwrap();
        snap.records
            .iter()
            .map(|bytes| Op::decode(bytes).unwrap())
            .collect()
    }

    /// How many of `ops` are on `glade_id` with a record `pick` accepts.
    fn count(ops: &[Op], glade_id: &str, pick: impl Fn(&[u8]) -> bool) -> usize {
        ops.iter()
            .filter(|op| op.glade_id == glade_id && pick(&envelope::record_bytes(&op.payload)))
            .count()
    }

    /// Slice profile SP-L1 (plan Step 4.4): a serve whose save fails folds,
    /// saves, marks and publishes nothing, so the retry mints the entry and
    /// the claim, saves them, and publishes both.
    #[tokio::test]
    async fn a_serve_whose_save_fails_is_retried_in_full() {
        let (shared, sys) = adopted("sp-l1-serve").await;
        refuse_saves(&sys, true);
        assert!(serve_workspace_on(&shared, "ws-a", "a").await.is_err());
        assert_eq!(published_serve(&*shared.store.lock().await, "ws-a"), (0, 0));
        refuse_saves(&sys, false);
        assert!(
            serve_workspace_on(&shared, "ws-a", "a").await.unwrap(),
            "the retry mints"
        );
        assert_eq!(published_serve(&*shared.store.lock().await, "ws-a"), (1, 1));
        let claim = |p: &[u8]| ServeClaim::decode(p).unwrap().share == "ws-a";
        assert_eq!(count(&saved(&sys), G_CLAIMS, claim), 1, "and saved");
    }

    /// SP-L1: a renewal whose save fails is neither folded nor published, and
    /// the next renewal is saved and published on the chain's next seq, which
    /// the served store takes without a gap.
    #[tokio::test]
    async fn a_renewal_whose_save_fails_is_not_published() {
        let (shared, sys) = adopted("sp-l1-renew").await;
        assert!(serve_workspace_on(&shared, "ws-a", "a").await.unwrap());
        refuse_saves(&sys, true);
        renew_leases(&shared).await;
        let claims = |store: &Store| published_serve(store, "ws-a").1;
        assert_eq!(
            claims(&*shared.store.lock().await),
            1,
            "the refused renewal reached no one"
        );
        refuse_saves(&sys, false);
        renew_leases(&shared).await;
        assert_eq!(claims(&*shared.store.lock().await), 2, "the next one lands");
        let claim = |p: &[u8]| ServeClaim::decode(p).unwrap().share == "ws-a";
        assert_eq!(count(&saved(&sys), G_CLAIMS, claim), 2, "and is saved");
    }

    /// SP-L1: a principal whose record fails to save is not published, and
    /// the next Hello naming it mints the record, saved and published.
    #[tokio::test]
    async fn a_principal_whose_save_fails_is_not_published() {
        let (shared, sys) = adopted("sp-l1-principal").await;
        refuse_saves(&sys, true);
        note_principal(&shared, "alice").await;
        assert!(
            !knows_principal(&*shared.store.lock().await, "alice"),
            "reached no one"
        );
        refuse_saves(&sys, false);
        note_principal(&shared, "alice").await;
        assert!(knows_principal(&*shared.store.lock().await, "alice"));
        let alice = |p: &[u8]| PrincipalRecord::decode(p).unwrap().principal == "alice";
        assert_eq!(count(&saved(&sys), G_PRINCIPALS, alice), 1, "and saved");
    }

    /// Pending-future cancellation (plan Step 4.4): a serve cancelled while it
    /// waits for the served store's lock has folded, saved and published
    /// nothing, so the next serve publishes the entry and the claim. It does
    /// not test a cancellation after the save, when the records are durable
    /// and not yet published.
    #[tokio::test]
    async fn a_serve_cancelled_before_its_save_leaves_nothing_behind() {
        let (shared, _sys) = adopted("cancel-serve").await;
        {
            let _held = shared.store.lock().await;
            let pending = serve_workspace_on(&shared, "ws-a", "a");
            let waited = tokio::time::timeout(Duration::from_millis(20), pending).await;
            assert!(waited.is_err(), "the serve waits for the served store");
        }
        assert!(serve_workspace_on(&shared, "ws-a", "a").await.unwrap());
        assert_eq!(published_serve(&*shared.store.lock().await, "ws-a"), (1, 1));
    }

    /// Poll `mint` once, outside tokio's cooperative budget, so it takes every
    /// lock it can: the test decides where each mint stops, not the scheduler.
    async fn step<F: Future + Unpin>(mint: &mut F) -> Poll<F::Output> {
        let once = std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut *mint).poll(cx)));
        tokio::task::unconstrained(once).await
    }

    /// The seqs of this node's `dir.claims` chain that the served store
    /// holds, and that records.json at `sys` holds.
    async fn claims_chain(shared: &Arc<Shared>, sys: &std::path::Path) -> (Vec<i64>, Vec<i64>) {
        let node = &shared.dir.get().unwrap().node_id;
        let held = {
            let store = shared.store.lock().await;
            store.scan(HOME, G_CLAIMS, &[], node, i64::MIN)
        };
        let served = held.iter().map(|op| op.seq).collect();
        let ours = |op: &&Op| op.glade_id == G_CLAIMS && &op.origin == node;
        let saved = saved(sys).iter().filter(ours).map(|op| op.seq).collect();
        (served, saved)
    }

    /// Plan Step 4.4's question 5 (owner, 2026-09-24): a renewal racing a
    /// serve on the node's `dir.claims` chain. The test holds the router's
    /// lock, so a serve of `ws-b` stops after it has landed its workspace
    /// entry and before its claim, and then polls a renewal, which mints the
    /// chain's next two claims. Before the fix the serve had let the
    /// directory lock go by then: the renewal's claims reached the served
    /// store first and were refused as a gap, as was every later claim until
    /// the next boot. Now the renewal waits for the serve to land, and the
    /// served store holds the chain as records.json does. The held lock and
    /// polling each mint by hand force the interleaving, not timing. It does
    /// not force two Hellos (a principal mint lands one record, with no await
    /// a test can hold between the release and the append), and it does not
    /// reach a peer.
    #[tokio::test]
    async fn a_renewal_racing_a_serve_reaches_the_served_store_in_chain_order() {
        let (shared, sys) = adopted("in-order").await;
        assert!(serve_workspace_on(&shared, "ws-a", "a").await.unwrap());
        let router = shared.router.lock().await;
        let mut serve = pin!(serve_workspace_on(&shared, "ws-b", "b"));
        assert!(step(&mut serve).await.is_pending(), "the serve waits");
        let stopped = published_serve(&*shared.store.lock().await, "ws-b");
        assert_eq!(stopped, (1, 0), "stopped between its entry and its claim");
        let mut renew = pin!(renew_leases(&shared));
        let renewed = step(&mut renew).await.is_ready();
        drop(router);
        assert!(serve.await.unwrap());
        if !renewed {
            renew.await;
        }
        let (served, saved) = claims_chain(&shared, &sys).await;
        assert_eq!(served, saved, "the served store holds records.json's chain");
        renew_leases(&shared).await;
        let (served, saved) = claims_chain(&shared, &sys).await;
        assert_eq!(served, saved, "and does after the next renewal");
    }

    /// An instance whose node holds its presence and a claim on `home` at
    /// epoch 1, leased until `lease_expiry_ms`: records.json written as the
    /// node writes it, signed. A first boot leases `home` for the node's
    /// lease; this lets a test shorten it, or start from one that lapsed
    /// while the node was stopped. Returns the instance dir and the node's id.
    fn instance_holding_home(name: &str, lease_expiry_ms: i64) -> (PathBuf, String) {
        use crate::registry::{BlobStore, StoreApi};
        let sys = fresh(&format!("{name}-sys"));
        let boot = boot_at(sys.clone(), "gianni").unwrap();
        let (identity, node) = (boot.identity().unwrap(), boot.node_id.clone());
        drop(boot);
        let mut records = Registry::sealed(identity);
        let presence = crate::sysdata::NodeRecord {
            node_id: node.clone(),
            operator: "gianni".into(),
        };
        records.append(Record::Node(presence), &node).unwrap();
        let claim = ServeClaim {
            node: node.clone(),
            share: HOME.into(),
            lease_expiry_ms,
            epoch: 1,
        };
        records.append(Record::Serve(claim), &node).unwrap();
        BlobStore::new(&sys).save(&records.snapshot()).unwrap();
        (sys, node)
    }

    /// The epochs of `node`'s claims on `home` in `store`, in chain order.
    fn home_epochs(store: &Store, node: &str) -> Vec<i64> {
        let claims = store.scan(HOME, G_CLAIMS, &[], node, i64::MIN);
        let claims = claims
            .iter()
            .map(|op| envelope::record(op, ServeClaim::from_cbor).unwrap());
        let home = claims.filter(|claim| claim.share == HOME);
        home.map(|claim| claim.epoch).collect()
    }

    /// The lane owner's ruling of 2026-09-25: `home` joins the renewal set at
    /// adoption, like any served share. A node whose `home` claim was leased
    /// for 300 ms, adopted on 300 ms leases renewed every 100 ms, holds a
    /// claim on `home` still live three leases past the first one's end, in
    /// the served store and in records.json, every one at epoch 1. Before,
    /// nothing renewed the claim a boot had minted, so it lapsed.
    #[tokio::test]
    async fn the_home_claim_is_renewed_while_the_node_runs() {
        const LEASE: i64 = 300;
        let first = now_ms() + LEASE;
        let (sys, node) = instance_holding_home("home-renewed", first);
        let boot = boot_at(sys.clone(), "gianni").unwrap();
        let server = Server::open(fresh("home-renewed-store")).unwrap();
        let leases = Leases {
            lease_ms: LEASE,
            renew_ms: 100,
            ..Leases::default()
        };
        let adopted = server.adopt_boot_tuned(boot, leases, |_: &str| {});
        adopted.await.unwrap();
        let shared = server.shared.clone();
        let past = first + 3 * LEASE;
        let renewed = |st: &Store| max_lease(st, HOME, &node) > past;
        let what = "a claim on home live three leases past the first";
        wait_store(&shared, renewed, what).await;
        let st = shared.store.lock().await;
        assert_eq!(who_serves(&st, HOME, past), Some(node.clone()));
        let epochs = home_epochs(&st, &node);
        let kept = epochs.iter().all(|epoch| *epoch == 1);
        assert!(epochs.len() > 3 && kept, "{epochs:?}");
        drop(st);
        let live = |p: &[u8]| {
            let claim = ServeClaim::decode(p).unwrap();
            claim.share == HOME && claim.lease_expiry_ms > past
        };
        assert!(count(&saved(&sys), G_CLAIMS, live) > 0, "and saved");
    }

    /// The same ruling at a later boot: a node whose `home` claim lapsed
    /// while it was stopped takes the claim up at adoption, at its epoch, and
    /// renews it at once, before any tick (the loop here is an hour off). The
    /// served store and the adopted registry, which the start line reads,
    /// then hold it live, and the renewal carries epoch 1, where a serve's
    /// rule would have minted epoch 2. Before, the claim stayed lapsed.
    #[tokio::test]
    async fn a_later_boot_renews_its_lapsed_home_claim_at_once_at_its_epoch() {
        let (sys, node) = instance_holding_home("home-lapsed", now_ms() - 1);
        let boot = boot_at(sys, "gianni").unwrap();
        assert_eq!(boot.registry.who_serves(HOME, now_ms()), None, "lapsed");
        let server = Server::open(fresh("home-lapsed-store")).unwrap();
        let adopted = server.adopt_boot_tuned(boot, by_hand(CHECKPOINT_AFTER), |_: &str| {});
        adopted.await.unwrap();
        let shared = server.shared.clone();
        let st = shared.store.lock().await;
        let serves = who_serves(&st, HOME, now_ms());
        assert_eq!(serves, Some(node.clone()), "live at once");
        assert_eq!(home_epochs(&st, &node), [1, 1], "renewed at its epoch");
        drop(st);
        let dir = shared.dir.get().unwrap().inner.lock().await;
        let serves = dir.boot.registry.who_serves(HOME, now_ms());
        assert_eq!(serves, Some(node), "in the adopted registry too");
    }

    /// F1 (question 32 (a), the owner's ruling of 2026-09-27): the node's
    /// lease and its renewal are its settings, and by default a claim lives
    /// five minutes and is renewed every 100 s, a third of that, where they
    /// were 30 s and 10 s. The assembled root's settings start from these,
    /// and the binary's entry point hands both roots the same.
    #[test]
    fn the_default_lease_is_five_minutes_renewed_every_100_s() {
        let leases = Leases::default();
        assert_eq!((leases.lease_ms, leases.renew_ms), (300_000, 100_000));
        assert_eq!(leases.checkpoint_after, 1_000, "plan Step 4.5c's threshold");
        let settings = crate::assembly::Settings::default();
        assert_eq!(settings.leases, leases, "the assembled root's settings");
    }

    /// Plan Step 4.6 (question 2, the owner's ruling of 2026-09-30):
    /// `--lease-ms <n>` sets the lease to n ms, renewed at a third of it as
    /// the default is, and leaves the checkpoint threshold, a setting with no
    /// flag, at the default's. Both ends of 3,000 to 3,600,000 ms are taken,
    /// and the leases read as the line a root prints after `node`.
    #[test]
    fn the_lease_flag_sets_the_lease_renewed_at_a_third() {
        let leases = Leases::from_flag("12000").unwrap();
        let set = (leases.lease_ms, leases.renew_ms, leases.checkpoint_after);
        assert_eq!(set, (12_000, 4_000, 1_000));
        let line = leases.to_string();
        assert_eq!(line, "leases 12000 ms, renewed every 4000 ms");
        let ends = [("3000", 3_000, 1_000), ("3600000", 3_600_000, 1_200_000)];
        for (value, lease_ms, renew_ms) in ends {
            let leases = Leases::from_flag(value).unwrap();
            let set = (leases.lease_ms, leases.renew_ms);
            assert_eq!(set, (lease_ms, renew_ms), "{value}");
        }
    }

    /// Plan Step 4.6: any other value, out of range or not a whole number of
    /// milliseconds, is refused, and the refusal names the flag and the range.
    #[test]
    fn the_lease_flag_refuses_any_other_value_naming_the_range() {
        for value in ["2999", "3600001", "12s", "-1", ""] {
            let refusal = Leases::from_flag(value).unwrap_err();
            assert_eq!(refusal.kind(), io::ErrorKind::InvalidInput, "{value:?}");
            let said = refusal.to_string();
            assert!(said.starts_with("--lease-ms "), "{value:?}: {said}");
            assert!(said.contains("from 3000 to 3600000"), "{value:?}: {said}");
        }
    }

    /// Plan Step 4.1b (D8): an instance whose records.json and served store
    /// hold its `home` records unsigned, as every node wrote them before the
    /// step (here by an unsealed registry, and into the journal unchecked): an
    /// exchange binding its app file has since dropped, the principal `alice`,
    /// and a live claim on `ws-x` at epoch 3, beside a client's app data on
    /// `ws-x`. The served store's `open` sets the node's `home` journal
    /// aside, as boot sets records.json's records aside, so adoption seeds
    /// signed records alone: the dropped binding routes no exchange, `alice`
    /// is minted again, `ws-x` is claimed at epoch 1, as `home` is renewed at
    /// adoption, and `who_serves` answers the node from both stores. Every
    /// `home` record held verifies, the app data stays, and the old journal
    /// is kept beside the new one.
    #[tokio::test]
    async fn adoption_after_the_unsigned_home_journal_is_set_aside_serves_signed() {
        use crate::registry::StoreApi;
        let (sys, at) = (fresh("unsigned-sys"), fresh("unsigned-store"));
        let node = boot_at(sys.clone(), "gianni").unwrap().node_id;
        let mut before = Registry::new();
        let dropped = crate::sysdata::BindingDecl {
            app: "x".into(),
            glade_id: "x.gone".into(),
            shape: "exchange".into(),
            authority: "share".into(),
            zone: "commons".into(),
            retention: "latest".into(),
        };
        before.append(Record::Binding(dropped), &node).unwrap();
        let alice = PrincipalRecord {
            principal: "alice".into(),
        };
        before.append(Record::Principal(alice), &node).unwrap();
        let live = ServeClaim {
            node: node.clone(),
            share: "ws-x".into(),
            lease_expiry_ms: now_ms() + 60_000,
            epoch: 3,
        };
        before.append(Record::Serve(live), &node).unwrap();
        crate::registry::BlobStore::new(&sys)
            .save(&before.snapshot())
            .unwrap();
        for bytes in &before.snapshot().records {
            crate::store::testing::journal(&at, &Op::decode(bytes).unwrap());
        }
        let note = Op {
            share: "ws-x".into(),
            glade_id: "notes".into(),
            origin: "client".into(),
            payload: b"kept".to_vec(),
            ..Op::default()
        };
        crate::store::testing::journal(&at, &note);

        let boot = boot_at(sys, "gianni").unwrap();
        assert_eq!(boot.set_aside.as_ref().map(|aside| aside.records), Some(3));
        let server = Server::open(&at).unwrap();
        let aside = server
            .set_aside()
            .await
            .expect("the unsigned journal set aside");
        assert!(
            aside.starts_with(
                "set aside 1 journal(s) of the served store's home share (3 record(s))"
            ),
            "{aside}"
        );
        let adopted = server.adopt_boot_tuned(boot, by_hand(CHECKPOINT_AFTER), |_: &str| {});
        adopted.await.unwrap();
        let shared = server.shared.clone();
        assert!(serve_workspace_on(&shared, "ws-x", "x").await.unwrap());
        note_principal(&shared, "alice").await;

        let st = shared.store.lock().await;
        let routes = crate::exchange::declared_exchange(&st, "x.gone");
        assert!(!routes, "the dropped binding routes no exchange");
        let minted = st.scan(HOME, G_PRINCIPALS, &[], &node, i64::MIN);
        assert_eq!(minted.len(), 1, "alice, minted again");
        let claims = st.scan(HOME, G_CLAIMS, &[], &node, i64::MIN);
        let epochs: Vec<i64> = claims
            .iter()
            .map(|op| envelope::record(op, ServeClaim::from_cbor).unwrap().epoch)
            .collect();
        assert_eq!(
            epochs,
            [1, 1, 1],
            "the home claim, its renewal at adoption, then ws-x's, at epoch 1"
        );
        assert_eq!(who_serves(&st, "ws-x", now_ms()), Some(node.clone()));
        for (share, glade_id, key) in st.zones().into_iter().filter(|(share, ..)| share == HOME) {
            for (origin, _) in st.heads(&share, &glade_id, &key) {
                for op in st.scan(&share, &glade_id, &key, &origin, i64::MIN) {
                    assert_eq!(envelope::verify(&op), Ok(()), "{glade_id} {origin}");
                }
            }
        }
        let notes = st.scan("ws-x", "notes", &[], "client", -1);
        assert_eq!(notes.len(), 1, "app data stays");
        drop(st);
        let dir = shared.dir.get().unwrap().inner.lock().await;
        assert_eq!(
            dir.boot.registry.who_serves("ws-x", now_ms()),
            Some(node.clone())
        );
        let hexed = |s: &str| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
        let journal = at.join(hexed(HOME)).join(format!("{}.log", hexed(&node)));
        let legacy = format!("{}.legacy-{}", journal.display(), crate::sysdir::today());
        let kept = std::path::Path::new(&legacy).exists();
        assert!(kept, "the old journal is kept beside the new one");
    }

    // ---- signed checkpoints (plan Step 4.5c's part 3) ----------------------

    /// How many of `node`'s claims `ops` holds, and how many of its
    /// checkpoints.
    fn folded_chain(ops: &[Op], node: &str) -> (usize, usize) {
        use crate::registry::G_CHECKPOINTS;
        let ours = |glade_id: &str| {
            let ours = |op: &&Op| op.glade_id == glade_id && op.origin == node;
            ops.iter().filter(ours).count()
        };
        (ours(G_CLAIMS), ours(G_CHECKPOINTS))
    }

    /// The line a checkpoint at `base` reports, having dropped `dropped`
    /// claims and carried none.
    fn folded_at(base: i64, dropped: usize) -> String {
        let dropped = format!("{dropped} superseded claim(s) dropped");
        format!("checkpoint: dir.claims folded at seq {base}, {dropped}, 0 carried")
    }

    /// Plan Step 4.5c: a node that serves `home` and `ws-a`, adopted with a
    /// threshold of 4, renews by hand. Its third tick folds its claims chain,
    /// and its sixth again, each saying so. From the first fold on,
    /// records.json holds one checkpoint and at most 4 + 2 of the node's
    /// claims, and so does the served store's journal of the node; after
    /// every tick, the registry and the served store name the node as
    /// serving `home`.
    #[tokio::test]
    async fn a_node_folds_its_claims_once_n_are_superseded() {
        use crate::store::testing::journal_of;
        let (shared, sys, lines) = folding("folds", 4).await;
        let root = std::env::temp_dir().join("glade-claims-folds-store");
        let node = shared.dir.get().unwrap().node_id.clone();
        assert!(serve_workspace_on(&shared, "ws-a", "a").await.unwrap());
        for tick in 1..=7 {
            renew_leases(&shared).await;
            if tick >= 3 {
                let (claims, checkpoints) = folded_chain(&saved(&sys), &node);
                let said = format!("tick {tick}: records.json holds {claims} of the node's claims");
                assert!(claims <= 4 + 2, "{said}");
                assert_eq!(checkpoints, 1, "tick {tick}: records.json");
                let journal = journal_of(&root, HOME, &node);
                let (claims, checkpoints) = folded_chain(&journal, &node);
                let said = format!("tick {tick}: the journal holds {claims} of the node's claims");
                assert!(claims <= 4 + 2, "{said}");
                assert_eq!(checkpoints, 1, "tick {tick}: the journal");
            }
            let now = now_ms();
            let dir = shared.dir.get().unwrap().inner.lock().await;
            let serves = dir.boot.registry.who_serves(HOME, now);
            assert_eq!(serves, Some(node.clone()), "tick {tick}: the registry");
            drop(dir);
            let serves = who_serves(&*shared.store.lock().await, HOME, now);
            assert_eq!(serves, Some(node.clone()), "tick {tick}: the served store");
        }
        assert_eq!(*lines.lock().unwrap(), [folded_at(6, 7), folded_at(12, 6)]);
    }

    /// Plan Step 4.5c (section 4): a tick that folds publishes its ops in the
    /// order it appended them: the claims it carries, then its renewals, then
    /// its checkpoint, so no reader takes the checkpoint before the claims
    /// that keep every fold's answers. The push sends the same ops, in one
    /// frame, in that order. A node whose records.json also holds a claim on
    /// `ws-gone`, a share it no longer serves, is adopted with a threshold of
    /// 2 and renews twice. A session subscribed to its claims and its
    /// checkpoints receives `home`'s renewal, then `ws-gone`'s claim carried,
    /// `home`'s next renewal and the checkpoint, and the line counts the one
    /// claim carried.
    #[tokio::test]
    async fn the_tick_publishes_its_carried_claims_and_renewals_before_its_checkpoint() {
        use crate::frame::Frame;
        use crate::registry::{StoreApi, G_CHECKPOINTS};
        let (sys, node) = instance_holding_home("carried", now_ms() + LEASE_TTL_MS);
        let mut boot = boot_at(sys, "gianni").unwrap();
        let gone = ServeClaim {
            node: node.clone(),
            share: "ws-gone".into(),
            lease_expiry_ms: now_ms() - 1,
            epoch: 1,
        };
        boot.registry.append(Record::Serve(gone), &node).unwrap();
        boot.store.save(&boot.registry.snapshot()).unwrap();
        let server = Server::open(fresh("carried-store")).unwrap();
        let (lines, report) = kept();
        let adopted = server.adopt_boot_tuned(boot, by_hand(2), report);
        adopted.await.unwrap();
        let shared = server.shared.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = shared.next.fetch_add(1, Ordering::SeqCst);
        shared.out.lock().await.insert(session, tx);
        {
            let mut router = shared.router.lock().await;
            router.subscribe(session, HOME, G_CLAIMS, &[]);
            router.subscribe(session, HOME, G_CHECKPOINTS, &[]);
        }
        renew_leases(&shared).await;
        renew_leases(&shared).await;
        let mut landed = Vec::new();
        while let Ok(bytes) = rx.try_recv() {
            let Ok(Frame::Ops(ops)) = Frame::from_bytes(&bytes) else {
                panic!("expected the tick's ops");
            };
            for op in ops.ops {
                let share = match op.glade_id == G_CLAIMS {
                    true => envelope::record(&op, ServeClaim::from_cbor).unwrap().share,
                    false => String::new(),
                };
                landed.push((op.glade_id, op.seq, share));
            }
        }
        let at = |stream: &str, seq: i64, share: &str| (stream.to_string(), seq, share.to_string());
        let (renewed, carried) = (at(G_CLAIMS, 3, HOME), at(G_CLAIMS, 4, "ws-gone"));
        let (again, folded) = (at(G_CLAIMS, 5, HOME), at(G_CHECKPOINTS, 0, ""));
        assert_eq!(landed, [renewed, carried, again, folded]);
        let dropped = "3 superseded claim(s) dropped, 1 carried";
        let line = format!("checkpoint: dir.claims folded at seq 3, {dropped}");
        assert_eq!(*lines.lock().unwrap(), [line]);
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
        let leases = Leases {
            lease_ms: 1_500,
            renew_ms: 300,
            ..Leases::default()
        };
        let adopted = b.adopt_boot_tuned(boot_b, leases, |_: &str| {});
        adopted.await.unwrap();

        meshed(&a, id_a).await;
        let at_b = meshed(&b, id_b).await;
        a.connect_peer(at_b).await.unwrap();

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
                        let e = envelope::record(&op, WorkspaceEntry::from_cbor).unwrap();
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
                    let record = envelope::record(&op, PrincipalRecord::from_cbor).unwrap();
                    if record.principal == principal {
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
                let record = envelope::record(&ops.ops[0], PrincipalRecord::from_cbor).unwrap();
                assert_eq!(record.principal, "alice");
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
                let record = envelope::record(&ops.ops[0], PrincipalRecord::from_cbor).unwrap();
                assert_eq!(record.principal, "bob");
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
