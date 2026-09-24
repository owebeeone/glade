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

*Amended 2026-09-24:* on the client path the node holds a session (the
Session row above) as one websocket connection, and its op statuses, the heads
the node keeps for it and its replay's completeness end with that connection
(§6's session answers, R1, R3 and R7).

Replication model (the OrbitDB/Merkle-CRDT spine per
`GladeRustOrbitStrategy.md`):

- Each participant appends ops only to its **own** per-origin log. Appending
  never blocks on the network.
  - *Amended 2026-09-24:* after a refused op, a client's next append on that
    chain fails until a subscribe has caught the chain up (answer 4, in §6's
    session answers). On a share another node serves, a node places a
    client's op only through the claim holder, and none while the holder is
    unreachable; the client still appends without blocking (W1, W5).
- Shared state = `fold(merge(logs))`: union of ops, partial order from causal
  refs, deterministic linearization (lamport + origin id) for tiebreaks.
- Convergence comes from everyone folding the same op-set the same way —
  never from coordination. Leases/roles are optimizations, not correctness
  mechanisms.
  - *Amended 2026-09-24:* on a share another node serves, the live claim, a
    lease, picks the one node that admits a zone's writes, and fixes the
    zone's SWMR writer and its shape (W1, W2 and W7).
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
| `log` | causal interleave, append-only | from-cursor (`windowed` dropped 2026-09-23, R2(a)) | replay; trivially convergent |
| `swmr` | canonical single-writer snapshot/delta/reset | from-cursor | generation-coherent assembly; see `GladeSwmrAdapter.md` |
| `crdt` | canonical causal operation merge | from-cursor | multi-writer; payload profile selected explicitly; see `GladeCrdtAdapter.md` |
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
| AsyncTap | `value` / `log` / `swmr` / `crdt` | keyed; authority split per §3 |
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
  - *Amended 2026-09-24:* after a refused op, a client's next append on that
    chain fails until a subscribe has caught the chain up (answer 4, in §6's
    session answers). On a share another node serves, no node takes a
    client's op while the claim holder is unreachable: the op is answered
    `UnknownShare`, and the client keeps it and sends it again (W1, W5).
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
  - *Amended 2026-09-24:* the client path needs a session's frames to arrive
    in the order the node queued them, or a live op can overtake its replay
    (R4 and R7). The forward needs its frames kept in order too, or a chain's
    op is refused as a gap (W6).
- Three jobs, separated:
  1. **Route**: subscription table `(share, glade id, key?) → sessions` built
     from interest frames; fan out ops to subscribers minus origin; forward
     directed frames (live channels, exchanges) 1:1 with correlation ids.
  2. **Store**: per `(share, origin)` append logs, compacted per declared
     retention; opaque cached folds for late-joiner snapshot + tail.
  3. **Resume**: heads exchange, ship the gaps, both directions.
     - *Amended 2026-09-24:* on the client path resume runs one way: the node
       ignores a client's `Heads`, leaves `Subscribe.from` unread, and never
       asks for a tail it lost (R2, R4 and R8). On the forward it runs one way
       too: nothing ships the claim holder an op it lacks (W4).
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
  - *Amended 2026-09-24:* a session's outbound stays first in, first out, as
    §12 records: a scheduler that let live ops jump a replay would overtake
    it, and conflating values would drop ops the ack promises (R4 and R7).
- Frames MUST be carrier-count agnostic: addressed by `(share, glade id,
  key)`, scoped to the session (never the socket), resume/heads state on the
  session, one lane per binding so per-origin ordering holds. A second bulk
  lane (slow remote links) is then a transport change with no protocol
  change. One socket per `(tap, key)` is ruled out: it pushes routing into
  the TCP table, multiplies auth/resume state machines, and forfeits
  scheduling control.
  - *Amended 2026-09-24:* on the client path the node's session is one
    connection: its op statuses, the heads the node keeps for it and its
    replay's completeness end with it (R1, R3 and R7). A replay on a second
    lane would break the ack's cut, so such a lane is a protocol change (R4).

### Session answers (client path)

**Ruled 2026-09-24; R1 to R6 built.** The client-writes plan
(`GladeClientWritesPlan.md`) builds these rules. Its Phase 2 has built the
node's part: Step 2.1 (glade `bc606f4`) R1 to R3, with R2's `Retention` point,
and Step 2.2 (glade `e89335a`) R4 to R6, in the `Ops` arm
(`node/src/server.rs:280-337`), the subscribe arm (`:220-279`) and
`node/src/session.rs:36-103`. R7 waits for the plan's Phase 3 (Steps 3.1 to
3.4), in which both clients read the answers; until then no shipped client
reads a status or an ack's heads. R8 holds as ruled. `GladeNodeAssembly.md`,
"Client answers (client-writes plan Phase 2)", records what was built, and one
departure from the plan: a lock of its own, `cut`, orders fan-out against a
subscribe, not the store's lock (R4). The *Amended 2026-09-24* notes in these
rules point to the cross-node writes below, which are ruled, not built.

**Sources.** The owner ruled the plan's seven questions "all recommended"
(`GladeClientWritesPlan.md:296-299`; "answer N" below), and the plan with them
(root `dev-docs/GladeFirstSlicePlan.md:833`). The plan answers the two gaps
that Step 4.4 recorded and handed on (`GladeFirstSlicePlan.md:830-831`). Step
4.3's ruling fixes the refusal form, "an empty `Heads` and then
`Error{Unauthorized}`", and puts the websocket grant check behind a switch that
is off by default (`:810`). Code paths are from this repository's root, and
`server.rs`, `session.rs`, `store.rs`, `mesh.rs`, `exchange.rs`, `router.rs`
and `chain.rs` are in `node/src/`. `taut/ir/glade.taut.py` is from the glade-wz
root. Lines were checked again on 2026-09-24, with Step 2.2 landed (glade
`e89335a`); `node/` may be edited next, so its lines may move.

**Scope.** A session here is one client websocket connection, from its upgrade
to its close. The node keeps nothing of it afterwards (`server.rs:342-348`). An
upgrade whose `Origin` is not loopback gets `403`, and no session
(`GladeFirstSlicePlan.md:832`). A zone is `(share, glade_id, key)`; a subscribe
with no key names the empty key (`server.rs:221-225`). Peers, exchanges and
channels are outside these rules. No frame or field changes: `Error`
(`glade.taut.py:187-192`), `ErrorCode.ok` (`:56`), `Head.hash` (`:69`), `Heads`
(`:147-148`) and `Subscribe.from` (`:134`) are all in the IR.

*Amended 2026-09-24:* one peer path comes under R1 to R3: the forward that
carries a client's op to the claim holder, which answers each op on it as it
answers a client (W2, W3).

**R1. One status per op,** always on, for every session, with no negotiation
(answer 1).

- The node answers each op in a client's `Ops` frame with one `Error` frame, in
  the order the ops were sent. Its `corr` is the op's hash (`chain.rs:10-13`) in
  lower-case hex. Its `share` and `glade_id` are the op's.
- The code is `Ok` if the node holds the op: appended now, or a byte-identical
  op already held at its seq, as Step 4.4 ruled for a re-delivery
  (`GladeFirstSlicePlan.md:830`). Otherwise the code names the refusal
  (`session.rs:80-103`, `server.rs:144-147`): `Equivocation` if a different op
  holds the seq; `Protocol` for a gap, a chain break, a SWMR envelope that does
  not decode, a second SWMR writer or a shape conflict; `Unauthorized` for an
  op on `home` (H-R3, `GladeFirstSlicePlan.md:809`); `Internal` for an I/O
  error.
- A client matches a status to its op by `corr`, never by its place among other
  frames. Statuses keep the order of the session's ops, so an op sent twice
  gets its two statuses in that order.
- Fails: a status may never come. A node from before Phase 2 sends no `Ok` and
  no `corr`, and nothing tells it apart: `Welcome.protocol` stays 1
  (`server.rs:218`). A frame the node cannot decode is dropped unanswered
  (`server.rs:174-177`). So a client may not count on a status arriving (R7).

*Amended 2026-09-24:* on a share another node serves, an op's status is the
claim holder's, relayed by this node, and `UnknownShare` means that the op was
not placed, not that it was refused (W3, W5). A session's statuses keep its
order within a zone only (W6).

**R2. What `Ok` promises** (LBT-006, root
`dev-docs/LibraryBoundaryAndTestingPolicy.md:37`).

- The op is in this node's served store, written to its log before any fan-out,
  with no fsync (`store.rs:156-159`, `:381-387`). It survives a crash of the
  node process, not an OS crash or power loss, as Step 4.4 left it
  (`GladeNodeAssembly.md:405-412`, "Durable store and restart").
- An op appended now was queued, before its `Ok`, for every other session then
  subscribed to its zone on this node, a peer's forwarded interest included
  (`router.rs:46-53`, `mesh.rs:309`). A repeat is not sent again.
- It promises nothing about delivery, or about any other node. An op on a share
  this node forwards stays here: the forward only reads, and the node pushes
  only its own `home` records (`mesh.rs:395-426`, `:277-293`).
  - *Amended 2026-09-24:* on a share another node serves, the node sends the
    op to the claim holder, and its `Ok` promises that both nodes hold it
    (W3, W4).
- Fails: an OS crash can lose an op after its `Ok`. The next ack then names a
  lower head for its origin, the client's next op on that chain is refused as
  a gap, and the lost op returns only if the client sends it again.
- An op below the first seq the node holds on its chain, which the store takes
  as seen without holding it (`store.rs:322`, `:328`), is answered `Retention`,
  not `Ok` (owner, 2026-09-24; `server.rs:323-331`). A client treats
  `Retention` as settled, not as a refusal: it drops nothing, and its chain
  goes on.
- An op on a share this node forwards stays here: see R2's third point. The
  owner added cross-node writes as a planned item on 2026-09-24
  (`GladeCrossNodeWritesPlan.md`).
  - *Amended 2026-09-24:* the cross-node writes below place such an op at the
    claim holder, or answer it `UnknownShare` and leave it with the client
    (W1, W5).

**R3. A refused op is not held by its sender.** The node adds an op's seq to
the session's heads only once it holds the op, and keeps the highest seq.
Before Step 2.1 it added the seq before the append, so a later subscribe
skipped the node's own op at that seq (the plan's F1); now only the two `Ok`
arms add it (`server.rs:301-302`, `:319-320`, `:352-364`). Under R3 the gap
carries that op, which a refused writer needs in order to recover (answer 4).
R3 adds no failure: the refusal itself arrives under R1.

**R4. The ack is a cut.**

- A session not yet subscribed to a zone receives none of its ops before the
  ack.
- After the ack, the gap and then the live ops carry every op of the zone that
  the node holds, or comes to hold, above two things: what the session
  announced in its `Hello`, which only ever raises a head (owner, 2026-09-24;
  `server.rs:200-209`), and what it sent that the node holds. The session's
  own ops are not echoed back.
- One lock of its own, `cut`, orders a subscribe against every fan-out
  (`server.rs:32-37`). A fan-out holds it from its append until its ops are
  queued: the `Ops` arm (`:296-334`), and `mesh::ingest_and_fanout` for the
  peer paths, the forward and the node's own records (`mesh.rs:492-511`). A
  subscribe holds it while it registers, reads its heads and gap, and queues
  both (`server.rs:252-273`). The plan named the store's lock; Step 2.2 built
  `cut` instead (`GladeNodeAssembly.md:2276-2286`).
- This covers the local and the forwarded route (`server.rs:247-251`). A
  subscribe to a declared exchange attaches a provider instead: its ack names
  the zone with no origins, and nothing replays (`exchange.rs:87-93`).
- It assumes the carrier delivers a session's frames in the order the node
  queues them, as the websocket and today's first-in, first-out outbound do.
  Reordering them, as the priority scheduler or a second bulk lane above would,
  or conflating values, breaks R4 and R7.
- Fails: a `Hello`'s heads are taken on the client's word, and the ops at or
  below them are never shipped. A connection that ends mid-replay leaves the
  replay incomplete (R7).

**R5. The ack names each origin's head,** by seq and hash. For each origin
with an op in the zone on this node, it gives the last seq, and in `Head.hash`
the 32 bytes of that op's hash, as `Store::zone_heads`, the per-zone half of
`all_heads`, computes them (`store.rs:218-249`, `session.rs:36-43`). R1's
`corr` is the same hash in hex. An empty zone's ack names the zone and no
origin.

**R6. A refused subscribe** gets `Heads{streams: []}`, then an `Error` with the
subscribe's share and glade id, the reason's code, and no `corr` (answer 2).

- `UnknownShare` on the absent route, since Step 2.2 (`server.rs:237-246`,
  `session.rs:45-63`). Before it, that route sent the `Error` alone, so a
  client's subscribe waited, or took the next zone's ack (the plan's F3).
- `Unauthorized` for Step 4.3's grant refusal, once it is built and its switch
  is on (`GladeFirstSlicePlan.md:810`). Answer 2 reads that ruling's "empty
  `Heads`" as naming no zone; the 4.3 note's option (b) named the zone
  (`GladeNodeAssembly.md:921-923`), as an accepted empty zone's ack does.
- An accepted ack always names its zone (`session.rs:36-43`,
  `exchange.rs:89-91`), so a client knows a refusal at the ack. Its reason is
  the next `Error` with no `corr` for that share and glade id. A client that
  reads no `Error` resolves the subscribe, and sees an empty zone.
- The session is not subscribed, and no op of the zone follows.

**R7. What a client may conclude.**

- An op is accepted when its `Ok` arrives, and refused when its refusal
  arrives. Its fate is unknown if the connection ends first, or if no status
  comes (R1). A client can learn it by sending the op again: a repeat the node
  holds gets `Ok`.
- A replay is complete when, for each origin in the ack, the session has an op
  of that origin at or above the acked seq: announced in its `Hello`, received
  on this connection, or sent on it and answered `Ok`. No client announces
  heads today (`client-rs/src/client.rs:218`, `client-ts/src/client.ts:148`).
- For a share the node forwards, "complete" means complete against this
  node's replica (`server.rs:247-251`). What the claim holder holds beyond it
  arrives as live ops, while the forward runs.

**R8. `Subscribe.from` stays unread on the client path** (answer 5), until a
client keeps its store across restarts. A client's `from` changes nothing: R4
cuts the gap. The peer path reads it (`mesh.rs:327-328`).

**The client libraries** (answers 3 and 4, built in Steps 3.1 to 3.4).
`append`, `send_ops` / `sendOps` and `subscribe` keep their signatures;
`subscribe` returns after the replay, and returns a refusal as an empty zone.
New calls return the node's answer as data, as `ExchangeOutcome` does
(`client-rs/src/client.rs:29-35`): `append_outcome`, `send_ops_outcome` and
`subscribe_outcome` (in TypeScript, `appendOutcome`, `sendOpsOutcome` and
`subscribeOutcome`). `on_refused` / `onRefused` reports every refusal. A client
drops a refused op and every later op it sent on that chain, and its next
append on that chain fails until a subscribe has caught the chain up. So a
session never builds on, or folds, a refused op. A TypeScript session that a
binder owns is only told, and glial decides.

*Amended 2026-09-24:* an op answered `UnknownShare` is not placed, not
refused, so the drop does not apply to it: the client keeps it and its
chain's later ops, and sends them again in order (W5).

**Not covered.** The peer subscribe keeps F2's race until Step 4.3's peer
check (`mesh.rs:309`). A live subscription that 4.3's revocation pass ends
gets `Error{Unauthorized}` alone (`GladeNodeAssembly.md`, "How a revocation
reaches a live subscription"); these rules do not yet say how a client reads
it.

**Where each rule comes from, and what pins it.** Each step writes its new
tests red first. "The plan" is the plan as ruled (`GladeFirstSlicePlan.md:833`).

| Rule | Ruling | Steps | Tests the steps write |
| --- | --- | --- | --- |
| R1 | answer 1; H-R3; 4.4's re-delivery | 2.1; 3.1, 3.3 | `every_client_op_gets_one_status_named_by_its_hash`; updated: `a_client_op_on_home_is_refused_and_never_stored`, `a_client_op_on_any_other_share_still_lands`, `end_to_end_over_websocket`, `grazel_attach_end_to_end`, `forked_op_surfaces_error_frame_not_silent`; 3.1's pure tests (`client-rs/src/answers.rs`), `an_accepted_append_is_ok`, `a_repeated_append_is_ok`; 3.3's, in TypeScript |
| R2 | the plan, with its §10: held, not synced | 2.1 | none for durability. 4.4's `a_torn_tail_is_cut_so_the_next_append_reopens_whole` covers a torn record only; `an_op_below_the_first_seq_held_is_answered_retention` for `Retention` |
| R3 | the plan (F1) | 2.1 | `a_refused_op_is_not_held_by_its_sender` |
| R4 | the plan's option (a) for gap 2 (F2) | 2.2 | `no_op_of_a_zone_reaches_a_subscriber_before_its_ack`, `a_later_hello_never_lowers_the_sessions_heads` |
| R5 | the plan's option (a); GQ-9 (§2) | 2.2; 3.2, 3.4 | `the_ack_names_each_origin_head_with_its_hash`; `subscribe_outcome_returns_the_nodes_heads` |
| R6 | answer 2; 4.3's refusal form | 2.2; 3.2, 3.4; 4.3 | phase E of `s_discovery_golden_path_end_to_end`, turned round; 3.2's pure test that an ack naming no zone is a refusal; 4.3's `a_session_claiming_no_principal_is_refused` |
| R7 | answers 1, 3 and 4 | 3.1 to 3.4 | 3.1's and 3.2's pure tests; `a_refused_append_stops_its_chain`, `subscribe_returns_after_the_replay_is_folded`, `a_refused_chain_resumes_after_a_subscribe`; 3.3's and 3.4's, in TypeScript |
| R8 | answer 5 | none | none: the node never reads `from` on this path today (`server.rs:220-279`) |

**What these rules contradict elsewhere in this document** (found by Step 1.1;
each passage now carries a note that opens *Amended 2026-09-24*, and its older
text stands as ratified):

- §6's "transport ordering is not load-bearing": R4 and R7 need one session's
  frames to arrive in the order the node queued them.
- §6's priority scheduler and value conflation: live ops that jump the queue
  would overtake the replay, and conflation would drop ops R4 promises. The
  rules hold today because the outbound queue is first in, first out (§12).
- §6's second bulk lane "with no protocol change": a replay on its own lane
  breaks R4's cut, so it becomes a protocol change. And "scoped to the session,
  never the socket" (with §2's Session row): the node's session is one
  connection, and the statuses, R3's heads and R7's completeness end with it.
- §6's resume in "both directions": on the client path it runs one way. The node
  ignores a client's `Heads`, R8 leaves `from` unread, and the node never asks
  for a tail it lost (§12 already notes the missing exchange).
- §5's "append to the local destination unconditionally" and §2's "appending
  never blocks on the network": under answer 4 the next append on a chain after
  a refused op fails until a subscribe has caught the chain up.

### Cross-node writes (W1–W8)

**Ruled 2026-09-24; not built.** These rules place a client's write on a share
that another node serves. The cross-node writes plan
(`GladeCrossNodeWritesPlan.md`) builds the node's part after the first slice's
Step 4.6, and the client-writes plan's Steps 3.1 and 3.3 build the clients'
part of W5. Today such a write stays on the node the client reached: the `Ops`
arm asks no route (`server.rs:280-337`), the forward only reads
(`mesh.rs:395-426`), and the claim holder discards what the forward sends it
(`:347`).

**Sources.** The owner ruled the plan's eight questions "all recommended"
(`GladeCrossNodeWritesPlan.md:267-273`; "ruling N" below), and recorded ruling
4 on the client-writes plan too (`GladeClientWritesPlan.md:305-307`). The slice
plan's order places the steps (root `dev-docs/GladeFirstSlicePlan.md:934`).
Below, "the plan" is the cross-node writes plan; "answer N" and "CW 3.1" are
the client-writes plan's. Paths are as in the session answers. Lines were read
on 2026-09-24 while `node/` was being edited, so theirs may move.

**Terms.** A holds the live `ServeClaim` for share S; B, linked to A, routes S
`Forward(A)` (`mesh.rs:121-142`); client c is connected to B. The forward is
the one stream per zone that B opens to A (`forward_interest`, `:360-376`). To
place an op is to land it in a node's served store. These rules bring the
forward under R1 to R3; other peer paths, exchanges and channels stay outside.

**W1. Writes follow the read route** (ruling 5).

- A client's op on a share other than `home` is placed where a subscribe to
  its share is served, by the same C2 decision, asked once per share per
  frame. `Local`: appended here (R1 to R3). `Forward(A)`: sent to A on the
  zone's forward (W2, W3). `Absent`: answered `UnknownShare`, with the route's
  reason and the op's hash as `corr`, and kept nowhere (W5). An op on `home`
  is refused before any route, as today (H-R3).
- A client may rely on one decision placing its reads and its writes. A node
  with no mesh, and a share the directory never heard of, route `Local`
  (`mesh.rs:122`, `:140`): the legacy form and unclaimed shares are unchanged.
- Fails: a share the directory knows, with no live claim at B's clock, stops
  taking writes that land there today (ruling 5 accepts the change). B routes
  by its own replica of `home`, so until it hears that a claim moved, it sends
  ops to a node that answers `UnknownShare` (W2).

**W2. The holder decides** (rulings 2 and 6).

- A takes an op from the forward through its client path (H-R3, the chain,
  SWMR and shape checks, the append, the fan-out), with the forward's session
  as the op's origin, so the fan-out skips the forward. It answers with R1's
  status on the forward, after the fan-out it queued. It takes only the
  forward's zone, and only while its fold names it S's holder: another zone's
  op is `Protocol`, a `home` op `Unauthorized` (W8), and a node whose fold
  names another holder answers `UnknownShare`, keeping nothing.
- From Step X4.1 a write needs `write.append`, which a stored `write.*` admits
  (root `dev-docs/GladeFirstSlicePlan.md:810`). A checks it on B's node id, by
  default, and B's id needs `read.subscribe` too, for the forward. The
  client's node checks it on its session's claimed principal, behind 4.3's
  websocket switch, off by default. An op without it is `Unauthorized`.
- A client may rely on one node judging each zone, once: its chain checks,
  SWMR's writer, its shape and its grants. No node holds an op another refuses.
- Fails: A takes B's word for the op and its writer: app ops are unsigned (D5,
  `GladeNodeSigning.md:187-210`), and no client principal reaches A. After
  X4.1, a holder that grants no write verb refuses every forwarded write, and
  no shipped seed grants one. If A refuses the forward itself (4.3's refusal
  form), B answers the zone's pending and later writes `Unauthorized` until a
  forward opens again.

**W3. The forwarding node relays, and holds only what the holder accepted**
(rulings 2 and 3).

- On A's `Ok`, B lands the op through its own verify path, fans it out to its
  subscribers but the writer, adds it to the writer's heads (R3), and then
  relays the `Ok`. On any other status, B relays it and keeps nothing.
- A client may rely on B's other clients receiving its op only once A holds
  it, and on B never holding an op of its that A refused.
- Fails: where a zone split before these rules (the plan's §1), B's store can
  refuse an op that A accepted. B then does not hold it, so it answers its own
  store's code, not `Ok` (ruling 3). Nothing repairs the split (the plan's §7).

**W4. What `Ok` promises on a forwarded share** (ruling 3; LBT-006).

- R2 at A: in A's served store, not synced, and queued for every session then
  subscribed to its zone at A but the forward that carried it, other nodes'
  forwards included. And R2 at B, before B's `Ok`. Nothing about any other
  node's store: a third node gets the op by its own forward, while that runs.
- A client may rely on its op being held at A and at B, and queued for every
  session subscribed to its zone at either; as in R2, not on its delivery.
- Fails: R2's failures, at A and at B apart. B's loss heals when a forward
  reopens, since A's gap carries the op back. A's does not: B's ack still
  names the op, so the client's next op on that chain is refused as a gap,
  relayed from A, and the chain stalls at A until the client sends the lost op
  again (the plan's §7).

**W5. `UnknownShare` on an op means "not placed"** (ruling 4).

- B holds nothing of the op. Its route was `Absent`; its forward ended, or
  waited 12 s, the exchange forward's bound (`exchange.rs:42-46`), with it
  unanswered; or A, no longer the holder, answered it (W2). A holder from
  before Step X3.1 discards what the forward sends (`mesh.rs:347`), so B
  answers at 12 s.
- On a subscribe `UnknownShare` stays a refusal (R6); on an op it is not one.
  The client keeps the op and its chain's later ops, `append` goes on, and
  answer 4's drop does not apply. It sends them again, in order, after its
  next successful subscribe of the zone and on a backoff of 1 s doubling to
  30 s (Step X3.3a). A TypeScript session that a binder owns is told, and the
  client resends what it sent. A client may rely on a resend never being held
  twice: a repeat that A holds is `Ok` (R1).
- Fails: A may hold an op answered `UnknownShare`, if the forward ended or
  timed out after A took it; only a resend tells. No node keeps an outbox
  (GAP-11, `client-rs/src/client.rs:241-245`): unplaced ops live in the client
  and end with it.

**W6. Order** (ruling 7).

- A zone's forwarded ops from one node ride its one forward in the order sent,
  and A answers them in that order. A session's statuses keep its order within
  a zone only; across zones, a local status may overtake a forwarded one.
- A client may rely on `corr` (R1). An op sent twice goes to its one zone, so
  its two statuses keep their order.
- Fails: matching statuses across zones by their place mismatches them. Like
  R4, W6 assumes a carrier that keeps the forward's frames in order: a chain's
  op that overtook its predecessor would be refused as a gap.

**W7. Shapes** (ruling 2). The four op shapes (`value`, `log`, `swmr` and
`crdt`, `GladeShapeDispatch.md:15`) cross as ops. SWMR's one writer and a
zone's shape are A's to decide, from the first op A holds of the zone
(`store.rs:164-191`), so a client may rely on a SWMR zone having one writer
across nodes (GSA-03, `GladeSwmrAdapter.md:46`). A `crdt` merge stays the
clients' (GCA-07, `GladeCrdtAdapter.md:46`). `stream` has no op path
(`GladeShapeDispatch.md:23-25`). Exchanges keep their own forward
(`exchange.rs:207-228`), and live channels stay on their node
(`server.rs:182-190`). Fails: a zone split before these rules stays split (W3).

**W8. The forward carries app ops only** (ruling 2; H-R3). A refuses a `home`
op on it `Unauthorized`. `home` records keep moving by the directory's pull
and push (`mesh.rs:271-293`, `:432-490`), which slice Step 4.1b verifies. A
client sees no change: its op on `home` is refused at its own node.

**How they meet R1 to R8.** On a forwarded share an op's R1 status is A's,
relayed by B unless B cannot hold what A took (W3); `UnknownShare` joins R1's
codes as "not placed" (W5), and R1's order holds within a zone only (W6). R2's
`Ok` also means that the claim holder holds the op, for a share another node
serves (W4). R3 holds at B once B holds the op, after A's `Ok`. R4 and R5 hold
at B, and Step X2.2 closes on the forward the race that the session answers'
"Not covered" leaves to 4.3. Under R7 an op is accepted, refused, or not
placed.

**Not covered.** An op that the client sent before it learned that an earlier
op of its chain was not placed can reach A past a gap, and be refused
`Protocol`; neither plan says how the client keeps it. Nor does either say
whether B lands an op that A accepts after B has answered it `UnknownShare` at
12 s; the resend is `Ok` either way. Writes made before a share was claimed
stay where they were made.

**Where each rule comes from, and what pins it.** Each step writes its new
tests red first.

| Rule | Ruling | Steps | Tests the steps write |
| --- | --- | --- | --- |
| W1 | 5 | X2.3; X3.2 | `a_write_to_a_share_with_no_live_claim_is_not_placed`, `a_write_to_a_share_the_directory_never_heard_of_still_lands`; `a_write_on_b_reaches_a_and_every_subscriber` |
| W2 | 2, 6; H-R3 | X2.1; X3.1; X4.1 | `the_acceptance_path_answers_a_batch_without_a_socket`; `a_forwarded_op_lands_at_the_claim_holder_and_is_answered`, `a_forwarded_op_off_its_zone_or_on_home_is_refused_and_not_stored`, `a_node_that_no_longer_holds_the_claim_takes_no_forwarded_write`; `a_forwarded_write_without_a_grant_is_refused_by_its_claimed_node_id` and its granted twin, `a_session_write_without_a_grant_is_refused_when_the_switch_is_on` and off, `a_revocation_ends_a_forwarded_write_stream` |
| W3 | 2, 3 | X3.2 | `a_write_on_b_reaches_a_and_every_subscriber`, `a_write_the_holder_refuses_is_refused_at_b_and_kept_nowhere` |
| W4 | 3 | X3.1; X3.2 | `a_forwarded_op_lands_at_the_claim_holder_and_is_answered`; `a_write_on_b_reaches_a_and_every_subscriber` (each op once in each store). None for durability, as for R2 |
| W5 | 4 | X3.2; CW 3.1, 3.3; X4.2 | `writes_pending_when_the_forward_ends_are_answered_unknown_share`; Step X3.3a's pure tests, in CW 3.1 (not placed is not refused, a repeat's `Ok` settles it, a later refusal still drops the tail, the backoff's schedule), and X3.3b's, in CW 3.3 (`client-ts/test/answers.test.ts`); `a_forward_resumes_when_the_link_returns` |
| W6 | 7 | X2.1; X3.2 | `the_acceptance_path_answers_a_batch_without_a_socket`, a batch in order. None pins the order on the forward, or across zones |
| W7 | 2 | X3.2 | `a_write_on_b_reaches_a_and_every_subscriber` (`value`, `log`, `crdt`), `a_write_the_holder_refuses_is_refused_at_b_and_kept_nowhere` (a second SWMR writer; a `crdt` op on a `value` zone) |
| W8 | 2; H-R3 | X3.1 | `a_forwarded_op_off_its_zone_or_on_home_is_refused_and_not_stored` |
| R4, R5 on the forward | the plan's §1 | X2.2 | `no_op_of_a_zone_reaches_a_forwarding_node_before_its_ack`, `the_peer_ack_names_each_origin_head_with_its_hash` |

Step X4.3's journey (`node/tests/cross_node_writes.rs`, new) runs W1 to W5 and
W7 on two nodes and a third, through a stop and a restart of the holder.

**What these rules contradict elsewhere in this document** (found by Step
X1.1; lines are this document's; each passage now carries a note that opens
*Amended 2026-09-24*, and its older text stands as ratified):

- R2's third point and last bullet (`:332-334`, `:346-348`): on a forwarded
  share the op no longer stays at the node the client reached. W3 and W4
  replace them, and their notes point here, as the plan's Step X1.1 asked.
- R1 (`:301-311`) and the session answers' scope (`:286-287`): A answers a
  forward's ops under R1 (W2); R1's order holds within a zone only (W6); and
  an op's `UnknownShare` names no refusal (W5), so the client libraries' drop
  (`:435-438`) does not apply to it.
- §2's "appending never blocks on the network" (`:42-43`) and §5's
  offline-first (`:180-181`): on a forwarded share a node places a write only
  through the holder, and none while it is unreachable. The client still
  appends without blocking.
- §2's "never from coordination" and "leases/roles are optimizations, not
  correctness mechanisms" (`:51-53`): a live claim, a lease, picks the one
  node that admits a zone's writes, and fixes its writer and shape.
- §6's "transport ordering is not load-bearing" (`:208-212`) and its resume in
  "both directions" (`:223`): W6 needs the forward's frames in order, and on
  the forward resume runs one way; nothing ships A an op it lacks (W4).

Outside this document, root `dev-docs/glade/GladeAuthzModel.md:26` has a write
executed by "the receiving replica", and working offline; on a forwarded share
the holder executes it. Answer 4 (`GladeClientWritesPlan.md:276-282`) gains
W5's exception, as its ruling records (`:305-307`).

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
| GQ-3 | CRDT shape on AsyncTap: any concrete case, or structurally permitted but unimplemented? | resolved 2026-08-29: text editing profile implemented by `GladeCrdtAdapter.md` |
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
