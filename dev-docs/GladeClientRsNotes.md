# GladeClientRsNotes — the rust wire client (GLP-0006 P0.S3)

`glade/client-rs` (`glade-client`): a rust mirror of the TS `client.ts`
choreography for SUPPLIERS. Build notes + the ambiguities resolved (smallest
faithful call), so a later reader sees why each judgement landed.

## What it is

- `client::GladeClient` — one WS connection + session: `connect` / `hello`
  (S7) / `subscribe` (Heads-acked) / `append` / `send_ops` / `exchange`
  (requester), the provider loop (`on_exchange_req` + `respond_exchange`, corr
  1:1), fan-out `on_ops`, and reattach (`on_drop` + `reconnect`).
- `supplier::Supplier` — the thin authority helper mirroring the glial kit
  (`glial/src/supplier/index.ts`): `serve_exchange` (subscribe + answer),
  `serve_share` (op-append + fold inbound), reattach-on-drop with backoff — ONE
  module, as ruled.
- `session::Session` — per-origin chain store + lww/log folds, the rust twin of
  the node store + the taut fold/hash oracles.

Deps: `glade-wire` (path) + `sha2` + `tokio` only. **No `glade-node`
dependency** — P00-a: suppliers depend on the wire + a client lib, zero node
internals. `ws.rs` is the client half of the node's `ws.rs`, behavior-ported.

## Ambiguities → resolutions

1. **Test harness = the SPAWNED binary, not an in-process `Server`.** Faithful
   to the brief's "spawned glade-node", keeps client-rs dependency-clean (no
   node dep), and is the only way to test a real NODE RESTART (kill + respawn).
   The harness builds `../node/target/debug/glade-node` once if absent.
   - **Exchange tests** need a DECLARED exchange surface (an undeclared id hits
     the echo fallback, not a provider), so the node is BOOTED with
     `apps/grazel-app.glade` (`service grazel gwz.ops` + `workspace ws-razel`)
     under a temp `GLADE_HOME`/`HOME` — NEVER the real `~/.glade`.
   - **Share + reattach tests** use the LEGACY serve form (`glade-node <port>
     <store_dir>`): every subscribe is Local (allow-all, no mesh), and the store
     PERSISTS to the dir, so a restart on the same dir re-folds (§6).
   - Alternative not taken: `glade-node` as a dev-dependency + in-process
     `Server`. Rejected — it would couple client-rs to node internals (against
     P00-a) and can't drop live client connections to model a restart (per-conn
     tasks are detached).

2. **Fan-out via mpsc receivers.** `on_ops` / `on_exchange_req` / `on_drop`
   each return a FRESH `UnboundedReceiver` and the read loop broadcasts to all —
   the idiomatic rust mirror of the TS client's multi-listener callback fan-out
   (the client-ts `addOpsListener` seam). Receivers outlive a reconnect, so a
   supplier's answer/op loops keep running across reattach; only the wire
   attachment (subscribe) is re-issued.

3. **`append` fails fast when disconnected, WITHOUT advancing the chain.** A
   supplier must not build phantom ops the node can't reconcile after a reattach
   (a stale local seq becomes a gap the restarted node rejects). So `append`
   returns `NotConnected` before touching the session if there is no writer; the
   first append after reconnect is contiguous. The offline outbox (buffer +
   replay while down) is a SEPARATE rider (GAP-11), deliberately not built here.

4. **Exchange handler = `Fn(&ExchangeReq) -> Result<Vec<u8>, String>`.**
   `Ok(payload)` → `ok:true`; `Err(reason)` → `ok:false` data — failure as DATA
   (§4/§6), never a hang. Chosen over porting the TS `{ok?, payload?, error?}`
   answer struct: `Result` is the idiomatic rust shape and captures the same two
   arms exactly.

5. **`Supplier::detach_all` closes the session + stops reattaching; the
   per-surface answer/op loops are left to drain.** They hold only receivers
   (no work once the channel is idle); the process owns their lifetime, as with
   any spawned task in stage-1. A join-all teardown is deferred (not needed for
   correctness).

6. **Folds exposed as `fold_value` / `fold_log`** (two typed methods) rather
   than the TS union return (`Uint8Array | Uint8Array[] | null`) — rust has no
   ergonomic union; the caller knows the surface shape.

7. **`Cargo.lock` untracked; `/target` + `/Cargo.lock` gitignored** — the
   sibling convention (node, wire-rs, grip-share do not commit `Cargo.lock`).

8. **No `node/src` changes.** No `pub(crate)`→`pub` widening was needed: the
   client talks to the node purely over the wire.

## Gates (this crate)

`cargo test -p glade-client` = 6 green: 3 lib (op-hash oracle match; value lww
fold; log order + zone isolation) + 3 integration against the spawned node
(exchange round-trip both roles + failure-as-data; value/log serve → subscriber
fold; reattach after a node restart). Requires the node binary (auto-built if
absent). Never touches the real `~/.glade`.
