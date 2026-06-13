//! Glade node (GLP-0005, P1) — the localhost glade server.
//!
//! A replica with better uptime, plus a router (GladeSubstrateV1 §6). Built in
//! steps: the per-(share,origin) log store (P1.S1), WS carrier + resume
//! (P1.S2), subscription routing + priority (P1.S3), per-origin chain
//! verification (P1.S4), and the echo provider (P1.S6). Conforms to the frozen
//! wire IR + corpus (`glade-wire`) and the fold oracle.

pub mod chain;
pub mod echo;
pub mod frame;
pub mod router;
pub mod server;
pub mod session;
pub mod store;
pub mod ws;
