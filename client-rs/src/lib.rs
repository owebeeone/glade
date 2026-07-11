//! Glade rust wire client (GLP-0006 P0.S3) — a supplier-side mirror of the TS
//! session client (`glade/client-ts`). It wraps the frozen wire (`glade-wire`)
//! and the proven node choreography (subscribe / ops / exchange; the R4 provider
//! protocol) behind a small async API a rust SUPPLIER attaches through, with no
//! node internals (P00-a: suppliers depend on the wire + a client lib only).
//!
//! - [`client::GladeClient`] — the connection + session: connect / hello /
//!   subscribe / append / send_ops / exchange, the provider loop
//!   (`on_exchange_req` + `respond_exchange`), fan-out `on_ops`, and reattach
//!   (`on_drop` + `reconnect`).
//! - [`supplier::Supplier`] — the thin authority helper (serve_exchange /
//!   serve_share / reattach-on-drop) mirroring the glial kit.
//! - [`session::Session`] — per-origin chain store + lww/log folds, the rust
//!   twin of the node's store + the taut fold oracle.
//!
//! Conforms byte-for-byte to the wire codec + op-hash oracle (`glade-wire`).

pub mod client;
pub mod hash;
pub mod session;
pub mod supplier;
pub mod ws;

pub use client::{ExchangeOutcome, GladeClient};
pub use supplier::{Backoff, ShareController, Supplier, SupplierConfig, SupplierSurface};
