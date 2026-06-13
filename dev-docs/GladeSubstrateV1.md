# Glade Substrate V1 — Simplified Share Substrate Contract

Status: working draft — **distilled design direction, not yet a contract**

Purpose: capture the simplified V1 cut of the Glade substrate so it can be
built now. The full Glade spec (root `dev-docs/glade/*`) remains the north
star; this document selects what V1 MUST carry in its bones (because it cannot
be retrofitted) and what is deliberately layered on later. Distilled from the
review of the `codex/glial-stumbling-wip2` grok instrumentation experiment
(2026-06-12/13).

## 1. Premise

- The grok-instrumentation experiment proved the seam (taps can declare
  persistable state; capture can be an attachment; followers can suppress
  execution). Its projector mechanism is superseded — see §9.
- Single-writer sessioned sync is commodity. **Multi-writer convergence is the
  problem V1 exists to solve.** It is the primitive, not a layer.
- Persistence is the degenerate case of sharing (a session with a local
  backend), not a separate mechanism.

## 2. Core model

| Term | Meaning |
| --- | --- |
| Share | A replicated state domain with stable identity. |
| Glade ID | Stable share-space address of a binding. Declared and runtime-neutral; decoupled from Grip-local identity (grip keys, context paths, class names). |
| Binding | One shared surface inside a share: `(glade id, shape, authority, retention)`. Declared, not discovered. |
| Destination | Any replica a session's op stream replicates to: local store, glade server, mesh peer. All speak the same heads/ops protocol. |
| Session | The unit a peer holds: identity + set of bindings + destinations. Owns transport, resume, and origin identity. |
| Op | One attributed change: `(origin id, per-origin seq, prev-hash, causal refs, payload)`. |
| Fold | Deterministic function from a merged op-set to materialized state, selected by the binding's declared shape. |

Replication model (the OrbitDB/Merkle-CRDT spine per
`GladeRustOrbitStrategy.md`):

- Each participant appends ops only to its **own** per-origin log. Appending
  never blocks on the network.
- Shared state = `fold(merge(logs))`: union of ops, partial order from causal
  refs, deterministic linearization (lamport + origin id) for tiebreaks.
- Convergence comes from everyone folding the same op-set the same way —
  never from coordination. Leases/roles are optimizations, not correctness
  mechanisms.
- Snapshots are cached folds, not the primitive.

Causal-ref encoding (resolved 2026-06-13, GQ-9 — **hybrid**):

- Ops are keyed by `(origin, seq)`; cross-log causal refs are `(origin, seq)`
  pairs (version-vector style sync, cheap compaction).
- Each op carries `prev-hash` — the hash of its predecessor in its own log.
  The per-origin hash chain gives tamper evidence and equivocation detection
  (two ops claiming the same `(origin, seq)` are provably a forked log).
- `HEADS` exchange = version vector + per-log chain head hash. Compaction =
  signed per-log checkpoint, prune below it.
- Full content addressing (Merkle-DAG, untrusted-relay anti-entropy) MAY be
  layered later by promoting the chain hash to entry id; the Rust port keeps
  the OrbitDB spine's clock/ordering/conflict-resolution logic but not the
  content-addressed entry store.

Normative — V1 MUST carry from day one (brutal to retrofit):

- stable share / glade / op identity, with glade IDs decoupled from Grip-local
  identity,
- per-origin monotonic sequencing and causal refs,
- a recorded ownership/origin epoch (even where a single writer is declared).

V1 MAY defer (genuinely additive later): signatures and capability proofs,
interest aggregation beyond §7, the distributed control plane, provisioning,
scale modes.

## 3. Shapes

A shape = **payload type + fold + retention**, declared in taut
(`TautPlan.md`). The fold registry is open: new shapes are new declared folds,
not new substrate.

| Shape | Fold | Retention | Notes |
| --- | --- | --- | --- |
| `value` (SWMR/LWW or MV register) | replace; MV surfaces conflicts as data | latest | whole-value, no partials |
| `log` | causal interleave, append-only | from-cursor / windowed | replay; trivially convergent |
| structured `message` | per-field merge annotations (taut `merge`) | latest | field-level lww / set-union / counter; lists/text later |
| `stream` | none (ephemeral) | none | live channel; never replicated (read/write asymmetry per `GladeTerminalSliceProposal.md` §3) |
| `exchange` | none (directed) | per diagnostics policy | request/response routed over the session |

Authority is per binding:

- `authority: share` — the share (server/provider) is the source of record;
  writes go through an exchange to it.
- `authority: external(source)` — the share is a replicated cache of external
  truth (meteo, coinbase); one origin executes the fetch per key, the result
  is published into the share. This is the Interest Spec / Source Binding
  concept made concrete.

## 4. Grip integration — no new tap hierarchy

Share semantics attach to the **existing** tap classes. Consumers (`useGrip`,
drips, resolver, contexts) are untouched; query bindings still swap mock ↔
shared with zero consumer changes.

Sharability is a **base Tap feature** in grip-core (resolved 2026-06-13,
GQ-5): any tap MAY declare `share: (glade id, shape, authority?)` and
thereby **advertises as sharable** via grok enumeration (GDL-029 landing
point). grip-core carries only the protocol-free surface — the declaration,
uniform capture/apply hooks with per-class defaults (export local mutations
as attributed changes; apply remote changes without echo), and the
advertisement. The binder that connects advertised taps to sessions, plus
sessions, folds, destinations, and wire, live in `grip-share` /
`glade-client-ts`; grip-core never imports glade types. An app with no
session attached pays nothing.

Sharing a tap requires a **glade ID**. A default MAY be derived from declared
stable inputs (package id + grip key) — never from runtime artifacts
(constructor names, context paths, registration order). Once a glade ID has
been persisted or shared it is frozen; renames are alias/migration records,
not new IDs. Generated defaults SHOULD be pinned into a checked-in manifest so
drift surfaces as a diff (GQ-6).

| Tap | Allowed shapes | Semantics |
| --- | --- | --- |
| Atom / MultiAtom | `value` | whole-value register |
| FunctionTap | none by default | deterministic compute over shared inputs converges free; share output only when expensive/non-deterministic (origin-primary) |
| AsyncTap | `value` / `log` (CRDT open — GQ-3) | keyed; authority split per §3 |
| StreamTap | `log` or ephemeral `stream` | SWMR/CRDT structurally ruled out |

Two levels of identity (resolved 2026-06-13):

- **Glade ID = the announcement unit.** One per tap; what the tap declares and
  what grok advertises as sharable. Controls *whether/what* is shared. Stable,
  declared, runtime-neutral.
- **`(glade id, key)` = the stream/share ID.** The actual replicated unit —
  its own log, heads, fold, subscriber set. Routing, `SUBSCRIBE`, and `HEADS`
  are all per `(glade id, key)`; the glade id alone appears only in
  advertisement and as the stream prefix.

An unkeyed tap (atom) has one stream under its glade id (the null/default
key). A keyed tap (async/stream) announces one glade id fronting many streams,
one per distinct key.

Keyed taps (async/stream):

- **Keys MUST be canonical across peers**: deterministic CBOR of the declared
  param shape (taut), never app-built strings.
- Destination params *select* which `(glade id, key)` stream a consumer
  attaches to; they do not multiply data. N destination contexts whose params
  resolve to the same key collapse to one stream = one replicated copy, paid
  once (the same dedup that lets two viewers of one region share one
  reassembly, §7).
- The keyed entry map IS the routing table — inbound ops route by
  `(glade id, key)` map lookup. No graph scans.
- Execution role (who runs the fetcher) is per `(glade id, key)`, an
  optimization only.
- Keyed shared caches MUST declare retention (TTL / latest-only /
  from-cursor); the async-tap cache TTL is promoted into the declaration.

## 5. Sessions, destinations, and persistence

- Session = `(identity, bindings, destinations)`. The op stream fans out to
  all destinations; each reconciles by the same heads-exchange protocol. A
  local store is a replica with zero latency and no fan-out — **local session
  persistence is a share destination, not a separate mechanism.**
- The glade server MUST remain optional: an application with no share server
  runs local-destination-only through the same code path. This preserves the
  rapid-dev constraint (`RapidDevEnvironment.md`): in-memory mode with no
  mandatory mesh or hosted server.
- Offline-first falls out: ops append to the local destination
  unconditionally; remote destinations reconcile heads when reachable.
- Hydration = load cached fold + tail from the nearest destination, then
  reconcile heads with the rest. The previous `replaceSnapshot("collapse")`
  becomes a meaningful operation: persist the cached fold and prune replayed
  ops per retention.
- Echo control by attribution: inbound applies carry the origin; capture
  filters by origin. There is no global suppression state.

## 6. The glade server

The share server is the **glade server**: a replica with better uptime, plus
a router. Optional per §5; app-agnostic by construction.

Deployment (resolved 2026-06-13): the gryth node is the canonical glade
server instance — a rust+iroh process on the user's machine. grazel attaches
to it as an authority provider session serving workspace bindings (tree,
build status, errors, dirtiness as values/logs; builds as exchanges).
Browsers are not p2p peers and run no wasm: the SPA holds a full TS glade
session (own origin log, local destination, built-in folds — **the browser
folds**) and connects to its node over a websocket carrying the same frames
that iroh carries node-to-node.

- One multiplexed connection per session (websocket in v1); frames addressed
  by `(share, glade id, key)`. Transport ordering is not load-bearing — ops
  carry `(origin, seq, causal refs)` — so the carrier is swappable
  (libp2p/iroh) without semantic change. The hub is the degenerate star of
  the p2p-first topology.
- Three jobs, separated:
  1. **Route**: subscription table `(share, glade id, key?) → sessions` built
     from interest frames; fan out ops to subscribers minus origin; forward
     directed frames (live channels, exchanges) 1:1 with correlation ids.
  2. **Store**: per `(share, origin)` append logs, compacted per declared
     retention; opaque cached folds for late-joiner snapshot + tail.
  3. **Resume**: heads exchange, ship the gaps, both directions.
- The glade server MUST stay payload-agnostic: it never folds. Authorities
  are not the server — an `authority: share` provider (griplab/gryth backend)
  connects as a privileged session that serves exchanges and appends to its
  own log. Server-side materialization, if ever needed, is a provider session
  subscribing like any other.
- Frame vocabulary (taut-defined): `HELLO`/resume, `SUBSCRIBE`/`UNSUBSCRIBE`,
  `APPEND`/`OPS`, `HEADS`, `EXCHANGE` request/response, `CHANNEL`
  open/data/close.
- Head-of-line blocking: v1 is **one websocket** with chunked frames
  (size-capped) and a strict-priority scheduler shaped by the declared shape
  (streams/exchanges preempt log backfill; values conflate to latest-only in
  queue). Sufficient for the dominant localhost hop (~0.5 ms per 64 KB chunk).
- Frames MUST be carrier-count agnostic: addressed by `(share, glade id,
  key)`, scoped to the session (never the socket), resume/heads state on the
  session, one lane per binding so per-origin ordering holds. A second bulk
  lane (slow remote links) is then a transport change with no protocol
  change. One socket per `(tap, key)` is ruled out: it pushes routing into
  the TCP table, multiplies auth/resume state machines, and forfeits
  scheduling control.

## 7. Reassembler layer (delta-heavy surfaces)

Share/reassembly logic MUST NOT live in UI consumers. For patch-shaped
surfaces (file views, terminal screens):

- One **reassembler** per document per process consumes the replicated patch
  log, maintains the materialized model **once**, and serves regions.
- Viewers declare an **interest region** as destination params and receive the
  assembled region as a plain whole value. Viewers never see deltas.
- Same-region viewers collapse to the same key (assembly paid once). The union
  of live region params is the reassembler's effective interest, forwarded
  upstream as its subscription (interest aggregation, GDL-002 at this grain).
- Grip core already supports this (dest params, `produceOnDestParams`,
  per-destination publish). The missing piece is destination-roster
  bookkeeping (live set of `(destination, params)` + connect/disconnect
  events); V1 provides it as a library base class (`ReassemblerTap`), not a
  grok change.

The grip-core API change V1 requires is therefore small: the tap-side binding
seam (attributed ops out, patches/ops in) plus canonical-key derivation.
Drip/consumer contracts do not change.

## 8. Proof targets

1. Terminal slice (`GladeTerminalSliceProposal.md`): exchange + live channel +
   append log + `TerminalScreen` reassembler.
2. Multi-viewer file region view: one patch log, N viewers, regions as
   interest, assembly cost paid once. (This is the case that forced §7.)
3. Demo-app parity: the `grip-react-demo` glial sync behavior reproduced on
   the V1 session with a local backend, deleting the projector path.

## 9. Superseded: the projector-as-seam

The `glial-stumbling-wip2` projector is replaced because (autopsy):

- `markDirty()` carried no information → snapshot-diff-by-stringify; deltas,
  ordering, and merge impossible at the seam.
- Echo control via a global suppression flag instead of op attribution.
- The shared surface was accidental (every live drip) instead of declared.
- Capture and hydrate conflated in `attach()`; one-shot global hydrate gate.
- Stringly, unstable identity (`constructor.name`) and full-graph linear scans
  on restore.

What survives: persistable-tap value export/restore (recast as op
emission/application), deterministic context naming, follower execution
suppression (recast per `(binding, key)`), and "capture as attachment"
(recast as the session consuming an attributed op stream).

## 10. Open decisions (gate the build)

| # | Question | Lean |
| --- | --- | --- |
| GQ-1 | MV-register conflicts surfaced to UI as first-class grip state, or V1 declares only conflict-free folds (lww/log/sets)? | surface as data (Grip makes rendering conflicts cheap); decides tap API surface |
| GQ-3 | CRDT shape on AsyncTap: any concrete case, or structurally permitted but unimplemented? | permit, don't implement |
| GQ-6 | Glade ID defaults: derivation recipe and pinning (checked-in manifest vs first-use pinning vs explicit-only for multi-party shares)? | derive from package id + grip key, pin in a manifest |
| GQ-7 | Late-joiner cached folds with no authority session: designated folder origin per `(binding, key)` (reusing the role machinery)? | yes — reuse roles, no new mechanism |

Resolved (2026-06-13):

| # | Decision |
| --- | --- |
| GQ-2 | Transport: **iroh**, node-to-node only. Browser↔node is a websocket carrying the same frames. wasm/browser p2p is dead (experiment concluded, not useful); libp2p (GLP-0001) was the proving ground, not the keeper. |
| GQ-4 | Substrate core in **Rust** (`GladeRustOrbitStrategy.md` spine, minus wasm). TS side is a session library: own origin log, local destination, built-in fold set — **the browser folds** — with Rust/TS fold parity pinned by the shared golden corpus. No p2p in TS. |
| GQ-5 | Sharability is a **base Tap feature** in grip-core: declared glade id (+ shape, authority) on any tap config ⇒ the tap advertises as sharable (grok enumeration). Core carries declaration + capture/apply hooks + advertisement only; binder/session/folds/wire stay in `grip-share`/`glade-client-ts`. |
| GQ-8 | Client thickness: full TS session (folds in browser), not a thin view client, preserving serverless/offline grip apps. |
| GQ-9 | Causal-ref encoding: **hybrid** — `(origin, seq)` ids + cross-log `(origin, seq)` refs + per-origin hash chain (`prev-hash`). Version-vector sync, signed checkpoints for compaction; full Merkle content addressing deferred as an additive layer. |

Bears on (root `DecisionLog.md`): GDL-002 (interest aggregation), GDL-005
(ownership control), GDL-020/021 (schema/DSL → taut), GDL-026 (handles/keys),
GDL-028 (cursors), GDL-030 (shared vs session-local inputs).

## 11. The limping milestone (M-LIMP)

Security: allow-all with retrofit seams (principal id at `HELLO`,
capability-ref slots in the envelope, no-op enforcement hooks at every frame
class) per `GladeGrythSecurityModelAnalysisPrompt.md`.

Definition of limping — all on localhost:

> Two browser (TS) sessions + one rust glade node. One `lww` value and one
> append `log` shared between the browsers through the node. Node restart
> resumes from its store (heads exchange, no data loss). A browser goes
> offline, keeps writing locally, reconciles on reconnect. `EXCHANGE` and
> `CHANNEL` proven via a trivial echo provider session attached to the node.

Build items, in dependency order (2–4 parallel after 1):

1. **glade-wire**: `glade.taut.py` IR — op envelope (GQ-9 hybrid) + frame
   vocabulary (§6) — golden corpus, generated Rust/TS codecs, plus **fold
   conformance vectors** (same op-sets → byte-identical folded state in Rust
   and TS).
2. **glade-node** (rust): WS carrier, per-(share, origin) log store, heads
   resume, subscription routing, opaque cached folds. Boring storage. iroh
   carrier added only after localhost limps.
3. **glade-client-ts**: session, own origin log (seq + prev-hash), local
   destination (memory, then IndexedDB), folds `lww` + `log`, WS client.
4. **grip-share** (TS): bindings for AtomValueTap (`value`) and a log-shaped
   tap; glade IDs + pinned manifest (GQ-6 first real test).
5. Echo provider session (rust) for the exchange/channel leg.

Explicitly NOT in M-LIMP (next, in rough order): keyed async/stream bindings
and canonical key derivation, reassembler base + interest regions, iroh
multi-node, grazel authority session, MV folds, security enforcement.

GQ-1 is sidestepped, not decided: M-LIMP declares only conflict-free folds
(`lww`, `log`). MV is an additive fold kind plus an optional conflicts-grip;
nothing in M-LIMP forecloses either answer.

## 12. M-LIMP reached (retro, 2026-06-14)

Built on branch `gladev2` (GLP-0005), tags `gladev2/p0-start` →
`gladev2/p4-mlimp`. The §11 scenario passes as a single scripted acceptance
test (converge lww+log → node restart resume → offline-write/reconnect
reconcile → echo EXCHANGE), and a live React demo (`glade/demo`, the gryth
workspace panel) converges two participants through the real node in a browser.
The substrate exists: rust node + glade wire + TS client folds + grip-share
binder + grip-core base-tap `share`, all over the frozen wire/fold/hash oracles.

What landed, by layer: `taut/ir/glade.taut.py` + corpus + fold + op-hash
oracles (byte-parity Rust/TS/Python); `glade/node` (store, resume, routing,
GQ-9 chain verify, echo); `glade/client-ts` (session, lww+log folds, WS,
exchange, browser-safe sync sha256); `glade/grip-share` (binder, value+log
bindings, resync); grip-core `share` decl + `listSharedTaps` (GQ-5);
`glade/demo`.

Deviations / decisions folded from the build (see plan `Decisions.md`):
- **D8** authoritative log is per `(share, origin)`; the wire `StreamHeads`
  (per-stream) is reinterpreted as share-scoped origin heads for M-LIMP.
- **D9** node logic built carrier-first; the WS socket is one adapter.
- **D10** op-hash = `sha256(canonical_cbor(op))`; cross-language for free off
  the wire corpus. TS uses a sync pure-JS sha256 (Web Crypto is async-only).
- **GQ-5** sharability is a base-tap feature (resolved). **GQ-7** late-join uses
  full gap-ship; the opaque cached-fold optimization stays deferred (ratified).
- Resume over WS reconciles by re-shipping ops (idempotent dedup), not yet a
  heads-vector exchange on reconnect — sufficient for M-LIMP, tighten later.

Known gaps (M-LIMP-acceptable, recorded honestly):
- The priority `OutQueue` (interactive preempts bulk) is unit-tested but the WS
  server's outbound is FIFO mpsc — not wired. Localhost control RTT is
  sub-ms (p50 0.13ms / p90 0.32ms / max 0.88ms over 50), so FIFO is fine here.
- IndexedDB client destination deferred (memory + node-backed persistence
  cover the demo); MV folds, keyed bindings, reassembler, iroh, security
  enforcement are all post-LIMP per §8 non-goals.

Post-LIMP order (unchanged): keyed async/stream bindings + canonical keys →
reassembler + interest regions → iroh carrier/mesh → grazel authority session →
security model (per `GladeGrythSecurityModelAnalysisPrompt.md`). Each is an
addition on these rails, not a redesign — the M-LIMP premise.
