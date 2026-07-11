//! Glade node (GLP-0005, P1) — the localhost glade server.
//!
//! A replica with better uptime, plus a router (GladeSubstrateV1 §6). Built in
//! steps: the per-(share,origin) log store (P1.S1), WS carrier + resume
//! (P1.S2), subscription routing + priority (P1.S3), per-origin chain
//! verification (P1.S4), and the echo provider (P1.S6). Conforms to the frozen
//! wire IR + corpus (`glade-wire`) and the fold oracle.

pub mod appdecl;
pub mod chain;
pub mod echo;
pub mod exchange;
pub mod frame;
pub mod iroh_carrier;
pub mod mesh;
pub mod peer;
pub mod registry;
pub mod router;
pub mod server;
pub mod session;
pub mod store;
pub mod sysdata;
pub mod sysdir;
pub mod ws;

// Re-export the wire CBOR runtime as `crate::cbor` so the taut-generated
// `sysdata.rs` (which emits `use crate::cbor::Cbor;`) resolves against the ONE
// codec runtime the wire crate owns — no second, hand-edited copy.
pub use glade_wire::cbor;
