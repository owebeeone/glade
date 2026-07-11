//! The supplier helper (GLP-0006 P0.S3/S4, rust) — the authority-side
//! counterpart of a tap, mirroring the glial kit's semantics
//! (`glial/src/supplier/index.ts`; `GladeSupplierModel.md` §2) in ONE module
//! over a [`GladeClient`]. Two node mechanisms, one seam:
//!
//! * **exchange** surfaces → [`Supplier::serve_exchange`]. A Subscribe on a
//!   DECLARED exchange glade id registers the session as THE provider
//!   (`attach_provider`); the supplier answers each `ExchangeReq`, corr 1:1. A
//!   thrown handler is failure-as-DATA (`ok:false`), never a hang.
//! * **value / log** surfaces → [`Supplier::serve_share`]. NO provider entry:
//!   "serving" is APPENDING ops into the surface's stream (which fold + replicate
//!   to subscribers); the claim-holding authority is the NODE (F1). A subscribe
//!   only RECEIVES inbound ops so the source can fold them.
//!
//! Reattach-on-drop with backoff: on link loss the supplier re-Hellos and
//! re-Subscribes every serving; the per-surface answer/op loops persist across
//! reconnects (their receivers outlive the connection), so only the wire
//! attachment is re-established.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use glade_wire::generated::{ExchangeReq, ExchangeRes, Op};

use crate::client::GladeClient;

/// The declared surface a supplier stands behind, as addressed on the wire:
/// `(share, glade_id, shape, key)`. An absent/empty `key` is the commons zone.
#[derive(Clone, Debug)]
pub struct SupplierSurface {
    pub share: String,
    pub glade_id: String,
    /// `"value" | "log" | "exchange" | …` — routes the serve act (op vs answer).
    pub shape: String,
    pub key: Option<Vec<u8>>,
}

impl SupplierSurface {
    pub fn new(share: &str, glade_id: &str, shape: &str) -> Self {
        SupplierSurface { share: share.into(), glade_id: glade_id.into(), shape: shape.into(), key: None }
    }
    pub fn keyed(share: &str, glade_id: &str, shape: &str, key: Vec<u8>) -> Self {
        SupplierSurface { share: share.into(), glade_id: glade_id.into(), shape: shape.into(), key: Some(key) }
    }
    fn key_slice(&self) -> Option<&[u8]> {
        self.key.as_deref().filter(|k| !k.is_empty())
    }
    fn want_key(&self) -> Vec<u8> {
        self.key.clone().unwrap_or_default()
    }
}

/// Reattach backoff shape: `min(max_ms, initial_ms * factor^attempt)`.
#[derive(Clone, Debug)]
pub struct Backoff {
    pub initial_ms: u64,
    pub factor: u64,
    pub max_ms: u64,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff { initial_ms: 250, factor: 2, max_ms: 10_000 }
    }
}

impl Backoff {
    fn delay(&self, attempt: u32) -> Duration {
        Duration::from_millis(self.initial_ms.saturating_mul(self.factor.saturating_pow(attempt)).min(self.max_ms))
    }
}

/// Supplier composition: principal for attribution (§4) + reattach backoff.
#[derive(Clone, Default)]
pub struct SupplierConfig {
    pub principal: Option<String>,
    pub backoff: Backoff,
}

/// The publish controller for a value/log surface (`serve_share`). `set` is the
/// value op (lww whole-value refresh); `append` is the log op (one entry). Each
/// APPENDS into the surface's stream — which is what "serving" a value/log
/// surface IS. Wrong-shape use errors.
#[derive(Clone)]
pub struct ShareController {
    client: GladeClient,
    surface: SupplierSurface,
}

impl ShareController {
    pub async fn set(&self, payload: Vec<u8>) -> io::Result<Op> {
        if self.surface.shape == "log" {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("serve_share {}: set() is a value-shape op; use append() for a log", self.surface.glade_id)));
        }
        self.publish(payload).await
    }
    pub async fn append(&self, payload: Vec<u8>) -> io::Result<Op> {
        if self.surface.shape != "log" {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("serve_share {}: append() is a log-shape op; use set() for a value", self.surface.glade_id)));
        }
        self.publish(payload).await
    }
    async fn publish(&self, payload: Vec<u8>) -> io::Result<Op> {
        self.client.append(&self.surface.share, &self.surface.glade_id, &self.surface.shape, payload, self.surface.key_slice()).await
    }
}

struct State {
    principal: Option<String>,
    backoff: Backoff,
    servings: Mutex<Vec<SupplierSurface>>,
    helloed: AtomicBool,
    detached: AtomicBool,
}

/// A supplier: one authority session, several served surfaces. Compose it and
/// `serve_exchange` / `serve_share` per surface; it reattaches every serving on
/// a link drop with backoff. `detach_all` is the clean teardown.
#[derive(Clone)]
pub struct Supplier {
    client: GladeClient,
    state: Arc<State>,
}

impl Supplier {
    /// Attach a supplier over an authority session and start the drop-watcher.
    pub fn attach(client: GladeClient, config: SupplierConfig) -> Supplier {
        let sup = Supplier {
            client,
            state: Arc::new(State {
                principal: config.principal,
                backoff: config.backoff,
                servings: Mutex::new(Vec::new()),
                helloed: AtomicBool::new(false),
                detached: AtomicBool::new(false),
            }),
        };
        let watcher = sup.clone();
        tokio::spawn(async move { watcher.watch_drops().await });
        sup
    }

    /// Serve a DECLARED EXCHANGE surface: Subscribe registers this session as
    /// THE provider; each inbound `ExchangeReq` runs `handler` and is answered
    /// with `corr` preserved. `Ok(payload)` → `ok:true`; `Err(reason)` →
    /// `ok:false` data.
    pub async fn serve_exchange<H>(&self, surface: SupplierSurface, handler: H) -> io::Result<()>
    where
        H: Fn(&ExchangeReq) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    {
        self.ensure_hello().await;
        self.state.servings.lock().await.push(surface.clone());
        self.client.subscribe(&surface.share, &surface.glade_id, surface.key_slice()).await?;

        let mut rx = self.client.on_exchange_req().await;
        let client = self.client.clone();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                // A client may multiplex several providers over one session —
                // only answer requests routed to THIS surface (be defensive).
                if req.share != surface.share || req.glade_id != surface.glade_id {
                    continue;
                }
                let res = match handler(&req) {
                    Ok(payload) => ExchangeRes { corr: req.corr, ok: true, payload: Some(payload), error: None },
                    Err(e) => ExchangeRes { corr: req.corr, ok: false, payload: None, error: Some(e) },
                };
                let _ = client.respond_exchange(res).await;
            }
        });
        Ok(())
    }

    /// Serve a VALUE / LOG surface: returns the publish [`ShareController`] and
    /// delivers the surface's inbound ops to `on_op` (any session may append in
    /// stage-1). This is op-publishing, NOT a provider attach — the claim /
    /// authority lives with the NODE (F1).
    pub async fn serve_share<F>(&self, surface: SupplierSurface, on_op: F) -> io::Result<ShareController>
    where
        F: Fn(Op) + Send + Sync + 'static,
    {
        self.ensure_hello().await;
        self.state.servings.lock().await.push(surface.clone());
        self.client.subscribe(&surface.share, &surface.glade_id, surface.key_slice()).await?;

        let mut rx = self.client.on_ops().await;
        let surf = surface.clone();
        tokio::spawn(async move {
            let want = surf.want_key();
            while let Some(ops) = rx.recv().await {
                for op in ops {
                    if op.share == surf.share && op.glade_id == surf.glade_id && op.key == want {
                        on_op(op);
                    }
                }
            }
        });
        Ok(ShareController { client: self.client.clone(), surface })
    }

    /// Stop reattaching and close the session. Servings' answer/op loops are
    /// left to drain naturally (they hold only receivers) — the process owns
    /// their lifetime, as with any spawned task in stage-1.
    pub async fn detach_all(&self) {
        self.state.detached.store(true, Ordering::SeqCst);
        self.client.close().await;
    }

    async fn ensure_hello(&self) {
        if self.state.helloed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(p) = &self.state.principal {
            // Attribution is best-effort in stage-1: a hello failure must not
            // wedge the supplier (identity as data, nothing enforced — §4).
            let _ = self.client.hello(Some(p)).await;
        }
    }

    async fn watch_drops(self) {
        let mut drops = self.client.on_drop().await;
        while drops.recv().await.is_some() {
            if self.state.detached.load(Ordering::SeqCst) {
                break;
            }
            // A fresh connection re-Hellos; then reconnect + re-subscribe all.
            self.state.helloed.store(false, Ordering::SeqCst);
            self.reattach().await;
            // Drain any drop signals that piled up during the backoff loop.
            while drops.try_recv().is_ok() {}
        }
    }

    async fn reattach(&self) {
        let mut attempt = 0u32;
        loop {
            if self.state.detached.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(self.state.backoff.delay(attempt)).await;
            if self.reattach_once().await.is_ok() {
                return;
            }
            attempt += 1;
        }
    }

    async fn reattach_once(&self) -> io::Result<()> {
        self.client.reconnect().await?;
        self.ensure_hello().await;
        let servings = self.state.servings.lock().await.clone();
        for s in &servings {
            self.client.subscribe(&s.share, &s.glade_id, s.key_slice()).await?;
        }
        Ok(())
    }
}
