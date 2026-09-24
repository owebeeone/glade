# Glade cross-node writes — the plan for a write on a share another node serves

Plan, 2026-09-24, for the owner. It answers the item added on 2026-09-24: a
client's write to a share another node serves stays on the node the client
reached (`glade/dev-docs/GladeSubstrateV1.md:285-287`, `:295-297`;
`dev-docs/GladeFirstSlicePlan.md:937`; `dev-docs/GladeProgramStatus.md:37`).
Nothing here is implemented, built or committed. The code was read in the working
trees on 2026-09-24 with no git command, so no revision is named; another agent
was editing `glade/node/` and `glade/dev-docs/GladeNodeAssembly.md` (slice Step
4.1a), so line numbers there may move.

Paths are from the glade-wz root; bare `.rs` names are in `glade/node/src/`. "The
substrate document" is `GladeSubstrateV1.md` (R1–R8 are its §6 session answers);
"slice 4.3" is a step of `dev-docs/GladeFirstSlicePlan.md` (the slice plan); "CW
2.1" is a step of `glade/dev-docs/GladeClientWritesPlan.md`, and "CW answer 4" its
§5 answer 4. This plan's steps are X1.1–X4.3; its rules W1–W8.

**The letters.** A holds the live `ServeClaim` for share S; B is linked to A;
client c is connected to B. The node's tests use them the other way round: in
`s_discovery_golden_path_end_to_end` (`mesh.rs:691-807`) B serves and A forwards.

## Summary

- **Today** c's write on S lands in B's store and reaches B's other clients, and
  nothing else. The two sides fold different op sets for good (§1).
- **Recommended: option A, the write goes through the claim holder** (§2, §3).
  B sends c's op to A on the zone's forward, the stream that already brings A's
  ops to B. A decides it as it decides a client's op and answers with R1's
  status. B relays the status and keeps only what A accepted. So `Ok` means A
  holds the op, and B does too. If B cannot reach A, it keeps nothing and answers
  `UnknownShare`, and the client keeps the op and sends it again. No wire change.
- **Not a first-slice step.** 4.6's route moves directory records, which cross by
  the directory's own pull and push (§7). Recommend the rules document now and the
  node steps after 4.6.
- **Four phases, eleven steps,** about 2,700 lines of code, some 620 of them
  production, plus ~120 lines of text (§5).
- **Eight questions** (§4). Rule two first: whether the end-to-end app has writers
  on two nodes (if not, this plan is off the publish path), and A over B.
- **The reading also found** (§1): shares the directory never heard of cross no
  node at all, reads included; a first write on B can make B refuse A's ops for a
  SWMR or CRDT zone for good; the peer ack is not a cut, so B's copy of a chain can
  lose its start; and a forward is never reopened after its link drops.

## 1. What happens today

1. **Route.** c subscribes to S at B. `route_subscribe` (`mesh.rs:121-142`) finds
   A's live claim in B's copy of `home` (`who_serves`, `:512-523`) and a link to
   A: `Forward(A)` (`:130-137`).
2. **Subscribe.** B registers c (`server.rs:249`), acks with its own heads, ships
   the gap from its own replica (`:250-269`), and forwards the interest
   (`:270-272`).
3. **The forward.** One stream per zone (`mesh.rs:360-376`, deduped at
   `:363-365`). `run_forward` sends `Subscribe{from: B's heads}` (`:396-407`) and
   then only reads, landing each op of the zone in B's store and fanning it out at
   B (`:409-424`; `ingest_and_fanout`, `:495-507`).
4. **At A.** `serve_peer_subscribe` registers the stream as a subscriber, acks,
   ships the gap, and lets A's fan-out feed it (`mesh.rs:299-344`). It reads B's
   half only to see it close, discarding every frame (`:347`).
5. **c writes O.** B's `Ops` arm asks no route (`server.rs:276-305`): O is not on
   `home` (`:283-286`), so B counts it in c's heads (`:288-291`), appends it
   (`:292`), and fans it out to its subscribers but c (`:294-300`;
   `router.rs:46-53`). B's router has no entry for A: the forward is an ingest
   loop, not a session.
6. **Nothing else carries O.** A node pushes only its own `home` records
   (`mesh.rs:271-293`, called only by `claims::publish`, `claims.rs:298-308`); a
   pushed `Ops` frame is ingested only for `home` (`mesh.rs:258-266`); the pull
   each way at connect is `home` only (`:432-458`, `:466-490`); `serve_sync` has no
   production caller (`peer.rs:285-313`; its callers are tests,
   `iroh_carrier.rs:178-304`).

| Party | Sees O? |
| --- | --- |
| c | no answer if B took O (`server.rs:294-301`), an `Error` with no `corr` if not (`:302`). After CW 2.1, B's `Ok`, which "promises nothing ... about any other node" (substrate document `:285-287`) |
| B's store and B's other clients | yes: held (R2), fanned out (`server.rs:294-300`), and in every later gap (`:250-254`; `session.rs:26-33`) |
| A and A's clients | never |
| a third node C | never: C's fold routes S to A (`mesh.rs:121-142`), so C gets only what A holds, and no node forwards S to B |

A `value` zone can show a different winner on each side. O reaches A today only
if a TypeScript client that holds it connects to A's node, since such a client
re-ships its whole session on connect (`glade/demo/src/glade.ts:56-57`; gryth-wz
`gryth-ui/packages/glade/src/runtime.ts:167-168`). Across machines it cannot: a
node's websocket binds 127.0.0.1 (`bin/glade-node.rs:219`).

**Also found.**

- **Unclaimed shares cross nothing.** A share the directory never heard of routes
  `Local` everywhere (`mesh.rs:140`, `:529-545`), reads included. glade/demo's
  shares (`doc:<doc>`, `account:<user>`, `chat`) run with no directory at all
  (`GladeNodeAssembly.md:1232`). An app on two nodes must claim its shares: a
  `workspace` line in the host's app file and not the reader's, which would claim
  the share too (`GladeNodeAssembly.md:1303-1307`), or `workspace.create`
  (`exchange.rs:147-193`). Two nodes that load one app also meet the binding
  family's cross-registry order (slice plan `:871-879`).
- **A first write on B can split a zone.** A store takes a zone's first op as its
  shape and, for SWMR, its one writer (`store.rs:159-185`). If c's op is the first
  B holds of a zone where A has another SWMR writer, or another shape with SWMR or
  CRDT on either side (`:169-182`), A's ops for the zone are refused at B
  (`SwmrWriterConflict`, `ShapeConflict`) and dropped unannounced
  (`mesh.rs:497-498`). B stops following A for that zone.
- **The peer ack is not a cut.** The stream is registered (`mesh.rs:309`) before
  the heads and gap are read (`:329-332`), so a live op can reach B first. B's
  store takes a chain's first op at any seq (`store.rs:302`) and the gap's earlier
  ops as already seen (`:308`): B's copy loses the chain's start. CW 2.2 leaves
  this path to slice 4.3 (`GladeClientWritesPlan.md:490-493`).
- **A forward is not reopened.** It ends with its stream; "a later subscribe
  retries" (`mesh.rs:354-359`, `:372-375`). After a link drop, B's subscribers get
  nothing more from A.
- **A serves any forward.** `serve_peer_subscribe` checks neither route nor claim
  (`mesh.rs:299-352`).

## 2. The options

### Option A: the write goes through the claim holder (recommended)

```
 c (on B)            B                                   A (holds S's claim)
   |-- Ops[O] ------>| route: Forward(A); O pending
   |                 |-- Ops[O], on S's forward -------->| takes O as a client op, the forward's
   |                 |                                   | session its origin; appends; fans out
   |                 |<-- Error{Ok, corr = hash(O)} -----| to all but that session, then answers
   |                 | lands O; fans it out, not to c
   |<-- Error{Ok} ---|
```

B places a write where its subscribe would be placed. A decides with its client
path. B relays A's answer and lands only what A accepted. §3 states it as W1–W8.

### Option B: replicate the zone both ways

B takes c's op as today and answers `Ok`; its fan-out also sends the op up the
forward, and when a forward opens B ships what A's ack heads lack ("both
directions", substrate document `:201`). It is the substrate's own model:
appending never blocks on the network, convergence is by fold, "never from
coordination" (`:37-43`), and writes work offline
(`dev-docs/glade/GladeAuthzModel.md:26`).

Its flaw: A can refuse what B already stored, answered `Ok` and fanned out, for
reasons B cannot see: a grant (grants are per node, `GladeNodeAssembly.md:855-856`);
SWMR's first writer or a zone's first shape, raced between nodes
(`store.rs:169-182`); one origin writing through two nodes; a gap. Nothing takes an
op back, so the zone then differs between A and B for good.

### Side by side

| | A: through the holder | B: both ways |
| --- | --- | --- |
| `value`, `log` | A decides; every node gets O through its forward | converge, unless A refuses after B's `Ok` |
| `stream` | no op path: every client and binding refuses it (`glade/dev-docs/GladeShapeDispatch.md:19`, `:23-25`; `glade/client-ts/src/shapes.ts:26-29`); live channels answer from the local echo (`server.rs:180-188`) | the same |
| `swmr` | one writer per zone, decided once, at A (GSA-03, `GladeSwmrAdapter.md:46`) | two nodes can take different first writers and refuse each other for good; needs A's decision anyway |
| `crdt` | A fixes the zone's shape; the merge is the clients' (GCA-07, `GladeCrdtAdapter.md:46`) | merges, but a first op of another shape on either side splits the zone |
| c is told | A's status: `Ok` means A and B hold O; on a refusal c drops O and its chain's tail (CW answer 4); on `UnknownShare` c keeps O and resends | B's status; A's later refusal reaches no one |
| Order, loss | one forward per zone per node keeps a chain in order; lost in flight is `UnknownShare`, and a repeat A holds is `Ok` (R1) | the same stream; a heads exchange at reopen, still to build |
| A unreachable | B places nothing; its reads are already `Absent` (`mesh.rs:130-139`) | B takes writes; they meet A's at reconnect, when A may refuse some |
| Slice 4.2 | the holder checked is B's node id, proved at HELLO since 4.1a (`peer.rs:153-180`), bound to B's endpoint key by 4.2 | the same |
| Slice 4.3 | `write.append` on B's id, checked before O is stored | the same check, after B's `Ok`: a refusal splits the zone |
| Slice 4.1b | none: the forward carries app ops and refuses `home`, which 4.1b signs; app ops stay unsigned (D5, `GladeNodeSigning.md:187-210`), so A takes O on B's word | the same |
| Cost | a round trip and two frames per write from B; no node-side offline writes on a forwarded share | one frame; but resync, a home for A's refusals and SWMR arbitration are extra, and the split has no fix in the slice |

### What else the code suggests

- **C. The holder pulls:** A subscribes back to B, since `serve_peer_subscribe`
  checks no claim and `run_forward` already pulls (`mesh.rs:395-426`). B, pulled,
  with B's flaw.
- **D. Writes as exchanges** to a provider at A that appends under its own origin
  (substrate document `:89-90`; H-R3, `GladeAuthzModel.md:402-410`). It crosses
  nodes today (`exchange.rs:115-144`, `:207-228`), so the end-to-end app can use it
  now for effects. But the op is the provider's, each surface needs one, and each
  write is a round trip bounded at 12 s (`exchange.rs:42-46`).
- **E. Push app ops as `home` records are pushed:** one stream per push reorders a
  chain into a gap (`GladeNodeAssembly.md:1658-1665`), and gets no answer. Rejected.
- **F. Sync whole shares** (`serve_sync`, `pull_sync`, `peer.rs:285-359`): app
  shares move by interest only (`mesh.rs:14-18`), and every node would hold every
  share, against the `metadata_exposure` ruling. A repair tool for B.
- **G. The client connects to A:** a browser reaches only its own node
  (`bin/glade-node.rs:219`; substrate document `:185-188`).

### Recommendation

Choose A. One node decides each zone: H-R3, the chain checks, SWMR's writer, a
zone's shape and slice 4.3's grants are decided once, so no node holds an op
another refuses, and `Ok` keeps a meaning a client can act on (R7). It extends what
reads already do: the claim decides where a subscribe is served, and a share whose
holder is unreachable is already `Absent` at B. No wire change.

It gives up node-side offline writes on a forwarded share; the client keeps its
own. B becomes sound once every node would decide alike: policy riding the share
(`GladeAuthzModel.md:154-157`), signed app ops (D5 (a), a wire amendment), and
declared shapes enforced. Until then its offline writes may silently never count.

## 3. The rules, as recommended

X1.1 writes these into the substrate document §6, after "Session answers".

- **W1. Writes follow the read route.** A client op on a share other than `home`
  is placed by the C2 decision its subscribe would get. `Local`: appended here
  (R1–R3). `Forward(A)`: sent to A. `Absent`: answered `UnknownShare`, with the
  reason and the op's hash as `corr`, and kept nowhere. A node with no mesh routes
  everything `Local` (`mesh.rs:122`), so the legacy form is unchanged.
- **W2. The holder decides.** A takes a forwarded op through its client path, the
  forward's session as origin, and answers with R1's status on the forward, behind
  the fan-out before it. It takes only the forward's own zone, and only while its
  fold names it S's holder: another zone's op is `Protocol`, a `home` op
  `Unauthorized` (H-R3), and a node whose fold names another holder answers
  `UnknownShare`, keeping nothing. From X4.1, the forwarding node needs
  `write.append`.
- **W3. The forwarding node relays, and holds only what the holder accepted.** On
  `Ok`, B lands the op, fans it out to its subscribers but the writer, counts it
  as the writer's (R3), then relays. It relays a refusal and keeps nothing.
- **W4. `Ok` on a forwarded share** is R2 at A (in A's served store, not synced;
  queued for every session subscribed at A, other nodes' forwards included) and R2
  at B; nothing about any other node's store. It replaces R2's third point and
  last bullet (substrate document `:285-287`, `:295-297`).
- **W5. Not placed.** `UnknownShare` on an op means B could not place it: no live
  claim, holder unreachable, or the forward ended, or waited 12 s, with the op
  unanswered. It is not a refusal: the client keeps the op and the rest of its
  chain (not CW answer 4's drop), and sends them again in order after its next
  successful subscribe of the zone, and on a backoff. A repeat A holds is `Ok`.
- **W6. Order.** A zone's forwarded ops from one node ride its one forward in the
  order sent, and A answers in that order. A session's statuses keep its order
  within a zone; across zones a local status may overtake a forwarded one.
  Clients match by `corr` (R1), and an op sent twice goes to one zone.
- **W7. Shapes.** `value`, `log` and `crdt` cross as ops; SWMR's writer and a
  zone's shape are A's to decide; `stream` has no op path. Exchanges keep their own
  forward; live channels stay on their node.
- **W8. The forward carries app ops only.** `home` moves by the directory's pull
  and push, which slice 4.1b verifies.

For the substrate document's list of contradictions (`:397-414`): R1's order
(`:267-269`) holds per zone (W6); CW answer 4 (`GladeClientWritesPlan.md:276-282`)
gains W5's exception; and on a forwarded share a node places a write only through
the holder, against the substrate document's "appending never blocks on the
network" (`:37-38`) and §5's offline-first (`:167-168`), and against
`GladeAuthzModel.md:26`. The client still appends without blocking.

## 4. Questions for the owner

1. **The end-to-end app's topology.** Does the app that gates the publish (slice
   plan `:512`) have writers on two nodes, on one share? If not, this plan is off
   the publish path. Recommend yes: a node on each of 4.5's machines, a browser on
   each, one claimed share, the `workspace` line only in the host's app file.
2. **A, B, or D alone?** Recommend A (§2). D needs no node change and stays open
   for effects.
3. **What `Ok` promises on a forwarded share** (W4). Recommend: held by the holder
   and by the node the client reached.
4. **`UnknownShare` on an op** (W5). Recommend: kept with its chain and sent again.
   Rule it before CW 3.1 and 3.3 start, so they build it in and X3.3a and X3.3b
   shrink to their tests.
5. **Writes follow the read route, `Absent` included** (W1). A default-path
   change: a share the directory knows, with no live claim, stops taking writes
   that today land and stay. The live desk writes `ws-razel`, which its node
   claims, and shares the directory never heard of, so it should see no change.
   Recommend yes.
6. **The write verb and its holder.** Slice 4.3 ruled a read's verb and an
   exchange's, not a write's (slice plan `:809`). Recommend `write.append`, AZM's
   wire verb (`GladeAuthzModel.md:194-199`), admitted by a stored `write.*`;
   checked at the holder on the forwarding node's id, on by default like 4.3's
   peer paths; and at the client's node on the claimed principal, behind 4.3's
   websocket switch, off by default. A forwarded write rides the forward, so it needs
   `read.subscribe` too. No shipped seed grants a write verb, so a host admitting
   a peer's writes needs a seed such as `seed <peer id> <share> read.*,write.*`.
7. **Status order across zones** (W6). Recommend per zone; R1's order across zones
   would hold a local status behind a forwarded round trip.
8. **Order.** Recommend X1.1 now; X3.3a and X3.3b folded into CW 3.1–3.4 if
   question 4 is ruled first; the node steps after slice 4.6. The earliest they
   can go is after CW Phase 2 and slice 4.3 part 2, which edit the same functions.

**Ruled, owner, 2026-09-24 ("all recommended"):** 1 yes, the end-to-end app has writers on two nodes on one
share, so this plan is on the publish path; 2 option A, through the claim holder; 3 `Ok` means the claim holder
and the node the client reached both hold the op; 4 `UnknownShare` on an op means "not placed", and the client
keeps the op and its chain and sends them again (built into the client-writes plan's Steps 3.1 and 3.3); 5 writes
follow the read route, refusal included; 6 the verb is `write.append`, granted by a stored `write.*`, checked at
the claim holder against the forwarding node's id by default and at the client's node behind 4.3's switch; 7
statuses keep their order within a zone only; 8 X1.1 now, the node steps after the first slice's 4.6.

## 5. Phases and steps

**Rules for every step** (`GladeClientWritesPlan.md:352-363`): one commit per
step through gwz, member then root lock, no attribution trailer; braced bodies,
and `#[cfg]` only inside `cfg_if!` or a module; rustfmt and clippy counts are
ratchets; a behaviour change starts with a failing test (LBT-007); pure tests
start no node, socket or clock (LBT-008); a contract change runs its consumers'
suites (LBT-011).

**Done when, every step:** its new tests were seen red, then green; its gate
passes; its consumers pass unchanged. Each step adds only what is its own.

**The node gate:** `cargo test --offline --locked --manifest-path
glade/node/Cargo.toml --lib -- mesh:: server::` while working; `sh
glade/node/check.sh`, 8 of 8 on both roots; then, with the binary rebuilt
(`cargo build --offline --locked --manifest-path glade/node/Cargo.toml --bin
glade-node`), the client-rs, client-ts, glial, grip-share, grazel, glade-gwz and
glade-gyld suites (`GladeClientWritesPlan.md:545-550`, `:622-627`).

### Phase X1 — The rules written

Milestone: W1–W8, as ruled, in the substrate document.

**X1.1 — Cross-node writes, in the substrate document**

- **Goal:** a §6 subsection "Cross-node writes (forwarded shares)" holding W1–W8;
  R2's third point and last bullet point to it; R1's order as question 7 rules;
  §3's changes join the list at `:397-414`.
- **Files:** the substrate document. **Tests:** none; it names the steps' tests.
- **Proves:** nothing about the code. **Gate:** the lane owner reads it against
  the rulings. **Done when:** each rule states its guarantee and its failures
  (LBT-006).
- **Size:** ~120 lines of text. **Depends on:** questions 2–7; nothing in the
  slice; before CW 3.1 and 3.3 if question 4 is to be built into them.

### Phase X2 — The node's foundations

Milestone: one acceptance path for client ops; the peer ack a cut; writes routed,
with `Absent` answered. No op crosses yet.

**X2.1 — One acceptance path**

- **Goal:** the `Ops` arm's work per op, as CW 2.1 leaves it (H-R3, append, R3's
  heads, fan-out, R1's status), becomes one function taking the op's origin
  session, which X2.3 and X3.1 call.
- **Files:** `server.rs` (`:276-305`); `session.rs` or a new `node/src/accept.rs`.
- **Tests:** CW 2.1's, unchanged, on both roots. New,
  `the_acceptance_path_answers_a_batch_without_a_socket` (LBT-008): new, repeat,
  fork, past a gap and `home` get `Ok`, `Ok`, `Equivocation`, `Protocol`,
  `Unauthorized`, each with its hash, and the fan-out skips the origin session.
  No behaviour changes, so it is red only until the function exists.
- **Proves:** one path decides every client op. **Not:** anything about peers.
- **Gate:** the node gate. **Size:** ~60 production, ~100 test lines.
- **Depends on:** CW 2.1; nothing in the slice.

**X2.2 — The peer ack is a cut**

- **Goal:** R4 and R5 on the peer path: `serve_peer_subscribe` registers the
  stream and queues the ack (with head hashes) and the gap under one hold of the
  store lock, as CW 2.2 does for clients.
- **Files:** `mesh.rs` (`:299-352`).
- **Tests,** two booted nodes, the stream to A opened by hand:
  `no_op_of_a_zone_reaches_a_forwarding_node_before_its_ack` (A's store lock held,
  a client op on A queued, the `Subscribe` arrives, the lock released: the first
  frame must be the ack and the op must come in the gap; red today, since the
  stream registers first, `mesh.rs:309`); and
  `the_peer_ack_names_each_origin_head_with_its_hash` (red: `hash: None`, `:338`).
- **Proves:** B's copy keeps a chain's start. **Not:** a reordering carrier; the
  grant check slice 4.3 puts in the same function.
- **Gate:** the node gate. **Size:** ~30 production, ~150 test lines.
- **Depends on:** CW 2.2. Slice 4.3 part 2 edits the same function: the later one
  rebases, and if 4.3 closed the race, this step keeps only its tests.

**X2.3 — Writes follow the read route**

- **Goal:** W1 for `Local` and `Absent`, the route asked once per share per frame.
  `Forward` keeps today's local append until X3.2.
- **Files:** `server.rs` (the `Ops` arm).
- **Tests,** on one node with its mesh enabled and `ws-attic` known only by a
  lapsed claim, seeded as in `mesh.rs:710-723`:
  `a_write_to_a_share_with_no_live_claim_is_not_placed` (`UnknownShare` with the
  op's hash, nothing stored or fanned out; red today, the op is stored,
  `server.rs:292`); and the guard
  `a_write_to_a_share_the_directory_never_heard_of_still_lands`.
- **Proves:** W1's local half. **Not:** the forward route.
- **Measured by the step:** a route scans every claim record (`mesh.rs:512-523`),
  one more per served share every 10 s (`GladeNodeSigning.md:375-377`); a cached
  fold is a later step if the figure asks for one. Question 5's default-path
  change.
- **Gate:** the node gate. **Size:** ~40 production, ~130 test lines.
- **Depends on:** X2.1; nothing in the slice.

### Phase X3 — The write crosses

Milestone: c's write on B reaches A and every subscriber of its zone on every
linked node; c gets A's verdict; B holds only what A accepted.

**X3.1 — The holder takes writes on the forward**

- **Goal:** W2 less the grant. `serve_peer_subscribe`'s read loop (`mesh.rs:347`)
  takes `Ops` through X2.1's path, the stream's session as origin, while
  `who_serves(S)` names this node; statuses go on the stream.
- **Files:** `mesh.rs`.
- **Tests,** two booted nodes, B played by hand:
  `a_forwarded_op_lands_at_the_claim_holder_and_is_answered` (A's subscriber gets
  it; the stream gets `Ok` and not the op; red today, A discards it, `:347`);
  `a_forwarded_op_off_its_zone_or_on_home_is_refused_and_not_stored` (`Protocol`,
  `Unauthorized`); `a_node_that_no_longer_holds_the_claim_takes_no_forwarded_write`
  (a higher epoch elsewhere: `UnknownShare`). Red today: no status.
- **Proves:** A's half. **Not:** B's half; grants.
- **Gate:** the node gate. **Size:** ~70 production, ~260 test lines.
- **Depends on:** X2.1, X2.2; nothing in the slice (4.1b never meets it, W8).

**X3.2 — The forwarding node sends writes up and relays the answers**

- **Goal:** W1's forward half, W3 and W5's node half. The `Ops` arm hands a
  `Forward(A)` op to its zone's forward, opening one if none runs. The forward
  writes ops up in order and holds them pending; an `Ok` lands its op (the verify
  path, writer as origin) and is relayed; a refusal is relayed; an end, or 12 s
  without an answer (`exchange.rs:42-46`), answers the pending ops `UnknownShare`.
- **Files:** `server.rs`: the forward branch, and the session's heads (`:165`),
  which the forward must reach for R3. `mesh.rs`: `Mesh.forwarded` (`:62-64`)
  becomes a map from zone to handle; `forward_interest`, `run_forward`
  (`:360-376`, `:395-426`).
- **Tests,** on the golden path's harness plus a third node:
  `a_write_on_b_reaches_a_and_every_subscriber` (value, log and crdt ops reach
  clients on A, B and the third node; c gets `Ok` per hash and no echo; each op
  once in each store; red today, A's client times out);
  `a_write_the_holder_refuses_is_refused_at_b_and_kept_nowhere` (a second SWMR
  writer, a `crdt` op on a `value` zone; red today, B stores and fans them out);
  `writes_pending_when_the_forward_ends_are_answered_unknown_share` (A's lock
  held, the link closed; resent after relinking, `Ok` once, stored once; red: no
  status).
- **Proves:** the milestone over loopback iroh. **Not:** a relay crossing (slice
  4.5); grants; subscribers after a link drop.
- **Gate:** the node gate. **Size:** ~160 production, ~320 test lines; if over, it
  splits into the send half with its pending table, then landing and relay.
- **Depends on:** X2.3, X3.1; nothing in the slice beyond 4.1a's HELLO, landed.

**X3.3a — client-rs keeps a write that was not placed**

- **Goal:** W5 in client-rs: `UnknownShare` marks an op unplaced, kept with its
  chain's later ops while `append` goes on; unplaced ops are resent in order after
  the zone's next successful subscribe and on a backoff (1 s doubling to 30 s); a
  later `Ok` settles them; a refusal still drops the tail.
- **Files:** `glade/client-rs/src/answers.rs`, `client.rs`, as CW 3.1–3.2 leave
  them.
- **Tests:** pure (LBT-008): unplaced is not refused; a repeat's `Ok` settles it;
  a later refusal still drops the tail; the backoff's schedule. Red: under CW 3.1
  every code but `Ok` drops the chain. A known, unserved share needs a holder that
  has gone, so X4.3 runs it end to end.
- **Proves:** the client's half of W5. **Not:** the node's (X3.2).
- **Gate:** CW 3.1's (`GladeClientWritesPlan.md:545-550`). **Size:** ~60
  production, ~150 test lines.
- **Depends on:** X1.1; CW 3.1, 3.2; nothing in the slice. Folded into CW 3.1–3.2
  if question 4 is ruled first.

**X3.3b — client-ts keeps a write that was not placed**

- **Goal:** X3.3a in TypeScript. A binder-owned session is only told (CW answer
  4); glial keeps the op anyway (`glial/src/instance.ts:150`), and the client
  resends what it sent.
- **Files:** `glade/client-ts/src/answers.ts`, `client.ts`, as CW 3.3–3.4 leave
  them. **Tests:** X3.3a's, in `glade/client-ts/test/answers.test.ts`.
- **Proves / not:** as X3.3a. **Gate:** CW 3.3's (`GladeClientWritesPlan.md:622-627`).
- **Size:** ~60 production, ~140 test lines. **Depends on:** X1.1; CW 3.3, 3.4.

### Phase X4 — Grants, a returning link, and the journey

Milestone: a forwarded write needs a grant at the holder; a returning link brings
its forwards back; two nodes converge under writes from both sides, through a
restart of the holder.

**X4.1 — The write verb**

- **Goal:** question 6 as ruled. At A, `write.append` on the forwarding node's id
  through slice 4.3's `GrantPort` adapter, before X3.1's path, `Unauthorized` per
  op. When A refuses a forward (4.3's `Heads{streams: []}` then
  `Error{Unauthorized}`), B answers that zone's pending and later writes
  `Unauthorized` until a forward opens again. At B, `write.append` on the
  session's principal, behind 4.3's websocket switch.
- **Files:** `mesh.rs`, `server.rs`, the tests' app files.
- **Tests,** named for claimed identities (4.3's convention); red today, every
  write lands: `a_forwarded_write_without_a_grant_is_refused_by_its_claimed_node_id`
  and its granted twin; `a_session_write_without_a_grant_is_refused_when_the_switch_is_on`,
  and off; `a_revocation_ends_a_forwarded_write_stream` (4.3's `revoke` line and
  re-check pass).
- **Proves:** W2's grant. **Not:** that a principal is who it claims.
- **Gate:** the node gate. **Size:** ~80 production, ~260 test lines.
- **Depends on:** X3.1, X3.2; slice 4.3 part 2 (the adapter, the peer id threaded
  to `handle_peer_stream`, the switch) and its `revoke` line; question 6.

**X4.2 — Forwards come back with the link**

- **Goal:** on link-up, B reopens a forward for each zone with a local subscriber
  whose share routes to that peer. Mostly the read side; the journey needs it.
- **Files:** `mesh.rs` (`run_link`, `:202-244`).
- **Tests:** `a_forward_resumes_when_the_link_returns`: after a drop and a
  reconnect, B's subscriber gets A's next op without subscribing again, and a
  resent write lands.
  Red today: nothing arrives (`mesh.rs:372-375`).
- **Proves:** a link blip no longer silences B. **Not:** a restart of B, whose
  clients subscribe again anyway.
- **Gate:** the node gate. **Size:** ~60 production, ~200 test lines.
- **Depends on:** X3.2; nothing in the slice.

**X4.3 — Two nodes, writes from both sides**

- **Goal:** the journey the end-to-end app relies on, as one test, then, after
  slice 4.5, one run on the two machines.
- **Files:** `glade/node/tests/cross_node_writes.rs` (new).
- **Tests:** the journey. Two booted nodes; the host's app file holds S's
  `workspace` line and a seed granting the reader `read.*,write.*`. client-rs
  sessions on both write value, log and crdt ops, which fold alike on both nodes
  and a third; SWMR keeps one writer across nodes; with the host stopped B's
  writes get `UnknownShare` and are kept; restarted, forwards return, resends are
  `Ok` once, folds match; a reader without the seed is refused. Each assertion is
  first seen failing on a tree without the step it pins.
- **Proves:** the milestone over loopback. **Not:** a relay crossing, unless run
  on 4.5's machines.
- **Gate:** the node gate. **Size:** ~400 test lines, no production code.
- **Depends on:** X3.2, X3.3a, X4.1, X4.2; on two machines, slice 4.2 and 4.5.
  It can join 4.6's route script once 4.6 lands.

## 6. Order and parallelism

```
X1.1 -+- X2.1 -+- X2.3 --------+
      |        |               |
      |        +-------+       +- X3.2 -+- X4.1 -+
      |                |       |        |        |
      +- X2.2 ---------+ X3.1 -+        +- X4.2 -+- X4.3
      |                                          |
      +- X3.3a (client-rs) ----------------------+
      +- X3.3b (client-ts)

inputs:  CW 2.1 -> X2.1   CW 2.2 -> X2.2   slice 4.3 part 2 -> X4.1
         slice 4.2, 4.5 -> X4.3 on two machines
```

- **Foundations first:** the rules; one acceptance path and the peer cut; the
  route; A's half, then B's.
- **The clients** run beside the node steps in their own worktrees, sharing no
  file with them (`GladeClientWritesPlan.md:737-739`).
- **One agent at a time in the glade checkout** (slice plan `:714-716`, `:933`).
  X2.1 and X2.2 edit different files, and X4.1 and X4.2 different functions, so a
  second worktree can take one of each pair.
- **Against the other plans** (question 8): X1.1 now; the node steps after slice
  4.6, and no earlier than CW Phase 2 and slice 4.3 part 2, which edit
  `serve_peer_subscribe` and the `Ops` arm. They rely on 4.1a's signed HELLO,
  which has landed (slice plan `:746`). 4.1b, 4.1c and the pull-on-gap step
  concern `home` only, so they change nothing these steps rely on.

## 7. What this plan does not do, and how it fits beside the first slice

- **Not a first-slice step.** 4.6 shows a registration on one node discoverable
  from another (slice plan `:858-870`): records the node writes itself, crossing by
  the pull each way and the push (`mesh.rs:271-293`, `:432-490`). No client writes
  a share another node serves. Nothing here waits on 4.6 but X4.3's run on two
  machines.
- **No wire change,** and **no offline writes at a node** on a forwarded share (W1,
  W5). **No outbox:** the client keeps what was not placed (GAP-11,
  `glade/client-rs/src/client.rs:241-245`).
- **No repair.** Writes made before a share was claimed stay where they were made;
  a zone split before these rules (§1) stays split, B relaying A's `Ok` and
  reporting its own refusal. An op the holder lost to an OS crash after its `Ok`
  (R2) stays on B, unoffered; the writer's chain stalls at A until it is resent.
- **One hop,** to a holder B links to directly, as for reads (`mesh.rs:130-137`).
  Exchanges keep their forward; live channels stay local (`server.rs:180-188`).
- **Identity.** No principal reaches the holder, and the `self:` key is not
  derived from an authenticated principal (B4, `GladeAuthzModel.md:345`), so a
  private zone stays private by routing only. App ops stay unsigned (D5).
- **No change for the suppliers or the desk:** glade-gyld and glade-gwz write
  where their share is served, so they stay `Local`. **Nothing is published.**
