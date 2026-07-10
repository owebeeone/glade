# Glade node↔node peer sync — engineering notes (Lane R step 2)

The iroh carrier, the node↔node HELLO seam, and heads/gap replica sync
(the s-sync trace minus AZ-12 checkpoint bootstrap). Executable spec:
`ggg-viz/src/scenario/sync.ts` (DO NOT modify). Design refs:
`GladeSubstrateV1.md` §2 (GQ-9), `GladeZones.md` (D8), `GladeSystemDataSeamNotes.md`.

Normative language per AGENTS.md: MUST / SHOULD / MAY.

## Scope

IN: iroh QUIC carrier (dial + framed protocol); node↔node HELLO seam
(structure real, crypto stubbed); HEADS exchange over per-(origin, zone) chains;
chunked, size-capped, resumable gap streaming; verify-as-ingest; reject-suffix +
re-fetch on chain failure; equivocation detection with the proof persisted as a
record. OUT: AZ-12 checkpoint bootstrap; ACL zone-filtering enforcement; real key
exchange / ed25519; equivocation-proof UI; server/bin wiring of the peer accept
loop (the carrier is a library capability this step; the WS client path is
untouched).

## The chain unit is per-(origin, zone) — already true, confirmed

The store already keys chains as `(share, glade_id, key, origin)` (`store.rs`),
i.e. **per-(origin, zone)** — the D8 refinement the reframe demands. `heads()`
is scoped to a zone `(share, glade_id, key)` and returns per-origin heads within
it; `scan()` / equivocation / chain-break are all per-chain. **No store re-keying
was needed.** `StreamHeads {share, glade_id, key, heads:[Head{origin,seq,hash}]}`
is exactly the (origin, zone) version-vector unit, so HEADS rides the existing
wire type. Building per-origin-only sync — the mistake the reframe warns against —
was structurally impossible against this store.

## Wire additions (taut `ir/glade.taut.py`, regenerated — never hand-edited)

Two messages + two `FrameType` values, appended so existing wire values stay
frozen (byte-parity of the pre-existing corpus preserved; synth adds one vector
each; wire-rs corpus tests stay green):

- `FrameType.node_hello = 13`, `FrameType.node_welcome = 14`.
- `NodeHello  { node_id: bytes, protocol: int, sig: bytes? }`
- `NodeWelcome { node_id: bytes, protocol: int, sig: bytes? }`

Regen: `cd taut && PYTHONPATH=src python3 -m taut.corpus.glade_build` — rewrites
`glade/wire-rs/src/generated.rs`, `taut/corpus/glade.ir.json`, `glade.golden.json`
together. `frame.rs` (hand-written `Frame` enum) gains the two arms.

## Ambiguities and smallest-reasonable resolutions

1. **Node identity: reuse `Hello` or add messages?** The client `Hello` carries a
   session + `principal`/`capability`; a peer link carries a *node* identity. Per
   clean-seams, added dedicated `NodeHello`/`NodeWelcome` rather than overloading
   the client frame. `node_id = sha256(node key)`; the node key is the iroh
   endpoint's ed25519 public-key bytes, so the id is genuinely `sha256(key)`
   (GladeSystemDataSeamNotes posture). Real ed25519 swaps in behind `verify_peer`
   with no wire change.

2. **HELLO trust.** The seam is structure-real, check stubbed: `verify_peer`
   parses the claimed `node_id`, carries `sig`, and ACCEPTS unconditionally. This
   matches the s-sync gate note — *sync integrity never trusts the carrier or the
   handshake*; it is end-to-end from origin chains.

3. **Carrier reachability.** `presets::Minimal` (relay AND discovery disabled) +
   direct dial by `(EndpointId, 127.0.0.1:port)`. No n0 relay/DNS egress; tests
   are pure localhost QUIC. `PeerEndpoint` is `Clone` (Arc-backed) and MUST
   outlive every `PeerLink` — dropping the last endpoint handle closes live
   connections (learned the hard way; documented on the type).

4. **Sync shape: one-directional pull primitive.** `serve_sync` (reads one
   `Heads`, streams the peer's gap, closes) + `pull_sync` (sends `Heads`, ingests
   until EOF). Bidirectional convergence = a pull each way. Stream close (`finish`
   / EOF) is the terminator — resumable by construction since heads advance as
   ops land. Matches the trace's local2→local1 pull.

5. **Chunking granularity.** Gap is streamed as multiple size-capped `Ops`
   batches (bulk priority), NOT the `Chunk` frame (that is for a single oversized
   payload, out of scope). Cap = max ops per batch.

6. **Tamper detection under a STUBBED origin signature.** With `verify_origin_sig`
   stubbed-accept, a payload-only tamper of op N is caught at op N+1 (its `prev`
   no longer matches `hash(stored N)`), so a corrupted op could momentarily land
   before its successor is rejected. A `prev`-field tamper is caught AT N (before
   apply). Both reject the suffix from that peer for that (origin, zone) chain and
   permit re-fetch elsewhere. Closing the one-op window is EXACTLY what a real
   per-op origin signature (the seam) buys — the reason the seam exists. Tests
   drive the deterministic `prev`-tamper case.

7. **Equivocation proof persistence.** Two signed ops in one `(origin, zone, seq)`
   slot are detected in `store.append` (which holds both ops) and the proof — both
   ops + the chain id — is persisted under `<root>/proofs/` and kept in memory
   (`Store::equivocation_proofs()`). "Persisted as a record, no UI." Promoting the
   proof to a *replicated* record (its own append-log zone, so it "replicates like
   any record" per SY4) is deferred.
