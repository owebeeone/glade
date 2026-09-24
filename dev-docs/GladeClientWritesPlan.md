# Glade client writes — the plan for the two client-library gaps

Plan, 2026-09-24, for the owner. It answers the ruling on Step 4.4: "the client
libraries' two gaps (no answer to an append, the heads ack dropped) get a plan
of their own" (`dev-docs/GladeFirstSlicePlan.md:828`). Nothing here is
implemented, built or committed.

The code was read in the working trees on 2026-09-24. No git command was run, so
no revision is named. The glade tree holds 4.3's part 1 (glade `e0100dc`,
`GladeFirstSlicePlan.md:807`). Another agent was editing `glade/node/` during the
read, so its line numbers may move.

Paths are from the glade-wz root. `gryth-wz/` is the sibling workspace: read,
never written. Short names:

- `server.rs`, `session.rs`, `store.rs`, `mesh.rs`, `exchange.rs` and `frame.rs`
  are in `glade/node/src/`;
- `client.rs` is `glade/client-rs/src/client.rs`;
- `client.ts` is `glade/client-ts/src/client.ts`;
- `glade.taut.py` is the wire IR, `taut/ir/glade.taut.py`.

## Summary

- **The gaps.** A client cannot tell an op the node accepted from one it refused.
  Nor can it tell when a subscribe's replay is complete (§1).
- **Recommended: option (A) with (a), which needs no wire-IR change** (§4).
  - The node answers every op a client sends with one `Error` frame. Its code is
    `Ok` if the node holds the op, and the refusal's code if not. Its `corr` is
    the op's hash.
  - The node makes the subscribe ack a cut and fills in each head's hash. It
    answers a refused subscribe with a `Heads` that names no zone, then the
    reason.
  - The clients read all of it (§4).
- **Four phases, nine steps,** about 2,300 lines, 850 of them production code
  (§7). The phases are: the contract (1.1), the node (2.1, 2.2), the two clients
  (3.1-3.4) and the two suppliers (4.1, 4.2).
- **The code showed four more problems** (§2):
  - a refused op counts as held by its sender;
  - the subscribe ack is not a cut;
  - a subscribe to an absent share hangs both clients;
  - glade-gwz still has glade-gyld's restart bug.
- **Seven questions** come first (§5). gryth-ui needs no change (§10).

## 1. The two gaps

**Gap 1: the fire-and-forget append.**

- The node answers an op it accepts with nothing (`server.rs:294-301`).
- It answers an op it refuses with an `Error` whose `corr` is `None`
  (`session.rs:37-66`; for `home`, `server.rs:137-145`).
- Both clients' `append` returns once the frame is written (`client.rs:246-258`,
  `client.ts:165-169`).
- Neither client reads `Error` frames (`client.rs:108`; `client.ts:97-136` has no
  branch for them).

**Gap 2: `subscribe()` drops the heads.**

- The node answers a `Subscribe` with a `Heads` frame naming each writer's last
  seq in the zone. It then sends the ops the session lacks, in one `Ops` frame
  (`server.rs:249-269`).
- Both clients resolve `subscribe()` on that frame and drop its body
  (`client.rs:86-90`, `client.ts:113-114`).
- Both send `from: None` (`client.rs:235`, `client.ts:159`), which the client
  path never reads. The node ships what lies above two things: the heads in the
  session's Hello, and the ops the session has sent (`server.rs:199-204`,
  `:250`, `:288-291`).

**Where it bit.**

- glade-gyld's supplier writes under one origin for the life of its data
  directory (`glade-gyld/src/supplier.rs:212`).
- After a restart its session is empty, so its first append takes seq 0 on a
  chain the node holds. The node refuses it as an equivocation, and nothing
  reads the refusal (`supplier.rs:1241-1251`). The tests are at
  `glade-gyld/tests/integration.rs:3794-3870` and `:3872-3971`.
- glade-gyld now subscribes first, then waits until 150 ms pass with no op, for
  at most 5 s (`supplier.rs:1230-1239`, `:1261-1290`).

## 2. What reading the code added

**F1. A refused op counts as held by its sender.**

- The `Ops` arm records the op's seq in the session's heads before it appends
  (`server.rs:288-291`, then `:292`).
- A later subscribe ships only what lies above those heads (`session.rs:26-33`).
- So it skips the node's own op at that seq. That is the op a refused writer
  needs in order to recover.

**F2. The subscribe ack is not a cut.**

- The session is registered for fan-out (`server.rs:249`) before the heads and
  the gap are read (`:251-254`).
- Every fan-out path appends, releases the store, and only then routes
  (`server.rs:292-299`; `mesh.rs:495-507`). So a live op can reach a subscriber
  before its ack.
- Both client stores take a chain's first op at any seq, then drop the earlier
  ones (`glade/client-rs/src/session.rs:83-96`;
  `glade/client-ts/src/store.ts:50-61`). So that early op can cost the client
  the start of a chain.
- The peer path registers first too (`mesh.rs:309`, before `:329-332`),
  although its comment says a live op cannot overtake the gap (`:325-326`).

**F3. A subscribe to an absent share hangs both clients.**

- The absent route sends an `Error` alone (`server.rs:232-243`). The client
  waits for a `Heads` that never comes, and the next `Heads` resolves the wrong
  call.
- 4.3's ruled refusal form exists to avoid this same hang
  (`glade/dev-docs/GladeNodeAssembly.md:899-927`).
- The client-rs `Supplier` awaits every subscribe
  (`glade/client-rs/src/supplier.rs:164`, `:197`, `:266`).

**F4. glade-gwz still has glade-gyld's restart bug.**

- glade-gwz writes under a fixed origin (`glade-gwz/src/supplier.rs:97`), and
  numbers its runs from 1 in each process (`:124`, `:162`).
- A restarted supplier's `run-1` therefore lands on an earlier `run-1`'s chain.
  Every record that differs from the one already stored there is refused, and
  nothing reads the refusal (`:244-248`).
- The desk subscribes by that run id
  (`gryth-wz/gryth-ui/packages/plugins/gwz/src/live.ts:120-122`), so it is
  shipped the earlier run's output.
- glade-gyld avoids this by minting run ids from its session
  (`glade-gyld/src/supplier.rs:340-342`). None of glade-gwz's five tests covers
  a restart (`glade-gwz/tests/integration.rs:181-378`).

**F5. gryth-ui has everything option (A) uses.**

- Its vendored IR differs from `taut/corpus/glade.ir.json` only in `Shape`, which
  lacks `swmr` and `crdt` (`gryth-wz/gryth-ui/packages/glade/src/glade.ir.json`).
- It takes `@glade/client-ts` from glade-wz by a `file:` dependency
  (`gryth-wz/gryth-ui/packages/glade/package.json`).
- The installed copy is hard-linked: `client.ts` is the same inode in both
  trees. So an edit made in place reaches the desk at once, but a new file
  arrives only after `pnpm install` there.

## 3. What the wire already offers

| Field (`glade.taut.py`) | What the IR says | What the node does today |
| --- | --- | --- |
| `Error.corr` (`:187-192`) | optional text | always `None` (`session.rs:64`, `server.rs:143`, `:240`) |
| `ErrorCode.ok` (`:55-57`) | the first code, `ok=0` | never sends it |
| `Head.hash` (`:66-69`) | "the per-origin chain head used for resume + equivocation detection" (`:63-65`) | sends `None` in every client ack (`server.rs:262`). The store computes the hash (`store.rs:224`) |
| `Heads` (`:146-148`) | "Heads exchange (resume / anti-entropy), both directions" | sends it as the subscribe ack. It ignores a `Heads` from a client (`server.rs:306`) |
| `Subscribe.from` (`:129-134`) | "resume cursor" | reads it on the peer path (`mesh.rs:327-328`), never on the client path |
| `Hello.protocol`, `Welcome.protocol` (`:103-112`) | an integer | always 1 (`client.rs:215`, `client.ts:147`, `server.rs:213`). Nothing reads it |

## 4. Options

### Gap 1: how the node answers an op

**(A) One `Error` status per op, with no IR change.**

- The node answers each op in a client's `Ops` frame with one `Error`, in the
  order the ops were sent.
  - The code is `Ok` if the node holds the op (appended now, or held byte for
    byte), and the refusal's code otherwise.
  - `corr` is the op's hash in lower-case hex, the hash the chain already uses
    (`glade/node/src/chain.rs:11`, `glade/client-rs/src/hash.rs:12-14`,
    `glade/client-ts/src/hash.ts:9-11`).
- Existing clients ignore `Error` frames (`client.rs:108`, `client.ts:97-136`),
  and every copy of the IR has `ok`.
- Cost: good news arrives in a frame named `Error`, under a code the IR
  defines. And every client gets one more frame per op, on the default path.

**(B) A `Heads` reply to each `Ops` frame, negotiated, with no IR change.**

- A session whose `Hello` says `protocol: 2` gets a `Heads` after each `Ops`
  frame. It names the new head (seq and hash) of each chain the frame touched.
  Refusals come as `Error` frames, with `corr`.
- It must be negotiated. Both clients match each `Heads` to the oldest waiting
  subscribe (`client.rs:56-58`, `:86-90`; `client.ts:56`, `:113-114`). An
  unasked `Heads` therefore resolves the wrong subscribe, in gryth-ui's desk
  too.
- Cost:
  - a session flag in the node;
  - one ordered queue of replies per client. A single missing answer, as in F3,
    throws off every reply after it;
  - replies are matched by order, which the substrate calls not load-bearing
    (`glade/dev-docs/GladeSubstrateV1.md:190-194`);
  - a session without a `Hello` cannot opt in. That includes grazel's probes
    (`GladeNodeAssembly.md:1226`) and a client-rs `Supplier` with no principal
    (`glade/client-rs/src/supplier.rs:222-231`).

**(C) A wire amendment: `Ops.corr` and a status frame.**

- It gives clean names, and one status per frame instead of per op.
- Cost: an IR edit, and regeneration of `generated.rs`, `cbor.rs`,
  `taut/corpus/glade.ir.json` and the golden vectors.
- Cost: a flag day. Old and new peers break each other:
  - the generated Rust panics on a missing map key
    (`glade/wire-rs/src/cbor.rs:22-31`), and on an unknown frame tag
    (`glade/wire-rs/src/generated.rs:57`, reached from `client.rs:73`);
  - the node does not catch a decode panic (`frame.rs:58-78`,
    `server.rs:172-175`), so the session dies before its cleanup runs
    (`server.rs:310-316`);
  - client-ts throws on an unknown tag (`client.ts:99`,
    `glade/client-ts/src/taut/schema.ts:90-94`).
- So every client must move on one day, or taut's generator must learn to decode
  a missing key. The new frame still needs negotiating.
- gryth-ui must re-vendor its IR, which is the owner's change.
- It needs the amendment review that the first-slice plan excludes
  (`GladeFirstSlicePlan.md:38-42`, `:931`), as D4 did for signatures
  (`glade/dev-docs/GladeNodeSigning.md:155-160`).

**(D) A client-side barrier. Not taken.**

- The client sends a `Hello` with no principal after its ops, and reads its
  `Welcome` as "all handled". Such a `Hello` changes nothing
  (`server.rs:198-214`).
- A refusal still names no op, so one barrier covers only one op. And it turns
  `Hello` into a ping.

### Gap 2: how a client learns the heads

- **(a) Return the ack, and resolve after the replay.** The clients return the
  ack's heads, and resolve once every op up to them has arrived. The node fills
  in `Head.hash` and makes the ack a cut (F2). No IR change.
- **(b) Honour `Subscribe.from` on the client path,** as the peer path does
  (`mesh.rs:327-328`). It saves re-sending a chain the client already holds.
  Neither gap needs it.
- **(c) An end-of-replay marker.** Not needed: the heads already say where the
  replay ends.

### What each costs

| | (A) + (a) | (B) + (a) | (C) + (a) |
| --- | --- | --- | --- |
| Node | ~110 production lines in `server.rs`, `session.rs` and `mesh.rs`; ~380 test lines; six existing tests updated (2.1, 2.2) | as (A), less the status. Adds a session flag, a `Welcome` naming 2, a `Heads` per `Ops` frame, and tests for both protocols | the IR edit and regeneration, plus tolerant decoding or a flag day. Then (A)'s work |
| client-rs | reads `Error` frames; ~330 production and ~400 test lines (3.1, 3.2) | as (A), plus one ordered reply queue in place of `sub_acks`, and a `Hello` at connect | regenerated types, then (A)'s work |
| client-ts | as client-rs; ~300 production and ~370 test lines (3.3, 3.4) | as client-rs | the new IR JSON, then (A)'s work |
| glade-gyld's supplier | the resume loses its settle loop, and writes go through one helper: ~80 production and ~150 test lines (4.1) | the same | the same, rebuilt against the new IR |
| gryth-ui's vendored copy | nothing required. Every field is in its IR copy (F5). The new client arrives at its next `pnpm install`. A refused subscribe resolves instead of hanging | nothing required, but it gets no answers to its ops until its runtime sends `protocol: 2` | must re-vendor its IR before it meets a new node. Otherwise the node drops its session at its first write |

### Recommendation

Choose (A) with (a):

- no IR change, as the first-slice plan requires (`GladeFirstSlicePlan.md:38-42`,
  `:931`);
- nothing an unmodified client can trip on;
- answers matched by `corr`, not by order (`GladeSubstrateV1.md:190-194`);
- the cheapest option for every consumer.

Its one cost is the name: an `Error` with code `Ok` carries good news.

## 5. Questions for the owner

Rule these before Phase 2. Step 1.1 records the answers.

1. **(A), (B) or (C)?** Recommend (A), always on, with no negotiation.
2. **What "an empty `Heads`" means** in the refusal ruling
   (`GladeFirstSlicePlan.md:808`).
   - Recommend `Heads{streams: []}`, which names no zone, followed by the
     `Error`.
   - An accepted ack always names its zone, even an empty one
     (`server.rs:255-265`, `exchange.rs:89-91`). So a new client can tell a
     refusal at the ack, and an old client still resolves.
   - Recommend the absent route answer the same way, with `UnknownShare` (F3).
   - The alternative, `Error` first, also works but changes the ruled order.
3. **The client API.** Recommend:
   - `append`, `send_ops` / `sendOps` and `subscribe` keep their signatures.
     `subscribe` returns after the replay. A refused subscribe resolves as an
     empty zone, as ruling (b) intends (`GladeNodeAssembly.md:919-921`).
   - New calls return the node's answer as data: `append_outcome`,
     `send_ops_outcome` and `subscribe_outcome` (in TypeScript,
     `appendOutcome`, `sendOpsOutcome` and `subscribeOutcome`). They follow
     `ExchangeOutcome` (`client.rs:29-35`), where failure is data
     (`client.rs:268-269`, `client.ts:197-199`).
   - `on_refused` / `onRefused` reports every refusal.
   - Why the old signatures stay:
     - glial's `SupplierSession` types `subscribe` as `Promise<void> | void`
       and `append` as synchronous (`glial/src/supplier/index.ts:134`, `:147`);
     - gryth-ui writes through `sendOps` and goes offline if a subscribe throws
       (`gryth-wz/gryth-ui/packages/glade/src/runtime.ts:66-68`, `:170-172`).
4. **What a client does with its own refused op.** Recommend:
   - the client drops the refused op, and every later op it sent on that chain;
   - the next append on that chain fails until a subscribe has caught the chain
     up. So the session never builds on, or folds, a refused op;
   - a TS session that a binder owns is only told. That is a session with the
     `onOps` field set (`client.ts:59-61`, `:110-111`), and glial decides
     (§10).
5. **`Subscribe.from`.** Recommend leaving it unread on the client path until a
   client keeps its store across restarts.
6. **glade-gwz's run ids (F4).** Recommend fixing them here, as Step 4.2.
7. **Order against 4.3.** The glade checkout allows one agent at a time
   (`GladeFirstSlicePlan.md:927`). Recommend:
   - 1.1 at once;
   - Phase 2 before 4.3's websocket enforcement. 2.2 builds the refusal helper
     that the enforcement calls, and both edit the `Subscribe` arm
     (`server.rs:215-275`) and `s_discovery_golden_path_end_to_end`
     (`mesh.rs:691-807`);
   - then Phase 3;
   - Phase 4 alongside, in its own repositories.

**Ruled, owner, 2026-09-24 ("all recommended"):** 1 (A), always on, with no negotiation; 2 `Heads{streams: []}`,
then the `Error`, and the absent route likewise, with `UnknownShare`; 3 and 4 as recommended; 5 `from` stays
unread; 6 glade-gwz's run ids are fixed here, as Step 4.2; 7 1.1 at once, Phase 2 before 4.3's websocket
enforcement, then Phase 3, and Phase 4 alongside in its own repositories.

**Ruled, owner, 2026-09-24, from Step 1.1's open point:** an op below the first seq the node holds on its chain
is answered `Retention`, not `Ok`, and a client treats it as settled, not as a refusal. Step 2.1 builds the
node's half; Steps 3.1 and 3.3 the clients'.

**Ruled, owner, 2026-09-24, from the cross-node writes plan (its answer 4):** `UnknownShare` answering an op means
"not placed", not "refused": the client keeps the op and its chain, and sends them again. Steps 3.1 and 3.3
build it (`GladeCrossNodeWritesPlan.md`).

## 6. The session contract

Step 1.1 writes these rules, as ruled, into `glade/dev-docs/GladeSubstrateV1.md`
§6. They assume the recommendations in §5.

- **R1. One status per op.** Each op in a client's `Ops` frame gets one
  `Error`, in the order the ops were sent.
  - `corr` is the op's hash in lower-case hex. `share` and `glade_id` are the
    op's.
  - The code is `Ok` if the node holds the op (appended now, or held byte for
    byte). Otherwise it is one of:
    - `Equivocation`;
    - `Protocol`: a gap, a chain break, or a SWMR or shape conflict;
    - `Unauthorized`: an op on `home`;
    - `Internal`: an I/O error.
- **R2. What `Ok` promises** (LBT-006,
  `dev-docs/LibraryBoundaryAndTestingPolicy.md:37`).
  - The op is in this node's served store, written to its log without fsync.
    It survives a crash of the node process, but not an OS crash or power loss
    (`GladeNodeAssembly.md:405-412`).
  - It was handed to every local subscriber of its zone registered at that
    moment.
  - It promises nothing about any other node.
- **R3. A refused op is not held by its sender.** The node adds an op to the
  session's heads only once it holds it. So a later subscribe ships the node's
  own op at that seq (F1).
- **R4. The ack is a cut.**
  - No op of the zone reaches the session before its ack.
  - After the ack, the gap and the live ops carry every op above two things:
    what the session announced in its `Hello`, and what it sent that the node
    holds.
- **R5. The ack names each origin's head,** by seq and hash.
- **R6. A refused subscribe** gets `Heads{streams: []}`, then an `Error` with
  the reason and no `corr`. The absent route uses it from Step 2.2 (F3). 4.3's
  grant refusal will use it when built.
- **R7. What a client may conclude.**
  - An op is accepted when its `Ok` arrives, and refused when its refusal
    arrives. Its fate is unknown if the connection ends first.
  - A replay is complete when, for each origin in the ack, the session has an op
    at or above that seq on this connection. The op may have been received, or
    sent and accepted.
  - For a share the node forwards, "complete" means complete against this
    node's replica (`server.rs:244-248`).
- **R8.** `Subscribe.from` stays unread on the client path.

## 7. Phases and steps

**Rules for every step** (`GladeFirstSlicePlan.md:44-58`):

- One commit per step through gwz: the member first, then the root lock. No
  attribution trailer.
- Braced control-flow bodies, and `#[cfg]` only inside `cfg_if!` or a module.
  The TypeScript has unbraced bodies (`client.ts:92`, `:105`;
  `glade/client-ts/src/store.ts:60`, `:63`). A step braces the lines it touches.
  Bracing the rest is a migration: recorded here, and not done.
- The node gate's rustfmt and clippy counts are ratchets (`glade/node/check.sh`).
- A behaviour change starts with a failing test (LBT-007).
- Pure-library tests start no node, socket or clock (LBT-008).
- A change to a public contract runs its consumers' suites (LBT-011).

**Done when, for every step:**

- its new tests were seen red, then green;
- its gate passes (for the node: 8 of 8 components, on both roots);
- its consumers pass unchanged.

The steps below add only what is particular to them.

**The node binary.**

- Every consumer suite spawns `glade/node/target/debug/glade-node`
  (`glade/client-rs/tests/integration.rs:31-49`,
  `glade/client-ts/test/integration.test.ts:18`,
  `glade-gyld/tests/integration.rs:61-62`, `glade-gwz/tests/integration.rs:33-34`,
  `grazel/tests/integration.rs:29-31`).
- client-rs's harness builds the binary only when it is absent.
- So every step after Phase 2 first rebuilds it:
  `cargo build --offline --locked --manifest-path glade/node/Cargo.toml --bin glade-node`.

### Phase 1 — The contract written

**Milestone:** the rules every later step builds are in the substrate document,
as ruled. The node work and both clients' pure logic can then start at once.

**Step 1.1 — The session answers, in the substrate document**

- **Goal:** R1-R8 as ruled, in a new subsection "Session answers (client path)"
  of `glade/dev-docs/GladeSubstrateV1.md` §6 (`:176-220`), with the rulings
  cited.
- **Files:** that document only. `glade.taut.py` is untouched.
- **Tests:** none, since it is a document. It names the tests the steps write.
- **Proves:** nothing about the code. It fixes the rules the steps share.
- **Gate:** the lane owner reads it against the rulings.
- **Done when:** each rule states its guarantee and its failure modes (LBT-006).
- **Size:** ~120 lines of text.
- **Depends on:** the rulings.

### Phase 2 — The node answers

**Milestone:** every client op gets a status, and every subscribe an ack that is
a cut and carries hashes. The absent route no longer hangs a client. No shipped
client changes.

**Step 2.1 — A status for every client op**

- **Goal:** R1-R3.
- **Files:**
  - `server.rs`: the `Ops` arm (`:276-305`) and `home_refused` (`:137-145`);
  - `session.rs`: `error_frame` (`:37-66`) takes the op.
- **Changes:**
  - Each op's status goes out once its fan-out is queued.
  - The session's heads take an op's seq only when the node holds that op, and
    keep the highest seq. The code at `server.rs:288-291` moves into the two
    `Ok` arms.
- **Tests,** in `server.rs`:
  - `every_client_op_gets_one_status_named_by_its_hash`.
    - One frame carries five ops: a new op, its repeat, a different op at a held
      seq, an op past a gap, and an op on `home`.
    - They must get `Ok`, `Ok`, `Equivocation`, `Protocol` and `Unauthorized`,
      in order, each with the op's hash as `corr`.
    - Red today: nothing comes back for the first two, and three `Error` frames
      with `corr: None` for the rest.
  - `a_refused_op_is_not_held_by_its_sender`.
    - Session 1 writes `(w, 0)`. Session 2's different `(w, 0)` is refused, and
      session 2 then subscribes. Its gap must carry session 1's op.
    - Red today: the gap is empty (F1).
  - Updated tests:
    - `a_client_op_on_home_is_refused_and_never_stored`: `corr` is the hash
      (`:536`);
    - `a_client_op_on_any_other_share_still_lands`: two `Ok` statuses (`:560`);
    - `end_to_end_over_websocket`: reads past its status (`:389`, `:418`);
    - `grazel_attach_end_to_end` (`exchange.rs:687`, `:743`);
    - `forked_op_surfaces_error_frame_not_silent` (`session.rs:144`).
- **Proves:** the node's answer on the client path, on both roots.
- **Does not prove:** that a client reads it; the peer paths, which get no
  status; any durability beyond R2.
- **Gate:**
  1. While working:
     `cargo test --offline --locked --manifest-path glade/node/Cargo.toml --lib -- server:: session::`.
  2. `sh glade/node/check.sh`.
  3. With the binary rebuilt, the suites of client-rs, client-ts, grip-share,
     grazel, glade-gwz and glade-gyld, all unchanged. The commands are under
     3.1 and 3.3.
- **Size:** ~50 production lines, ~220 test lines.
- **Depends on:** 1.1.

**Step 2.2 — The subscribe ack: a cut, with hashes, and one refusal form**

- **Goal:** R4-R6.
- **Files:**
  - `server.rs`: the `Subscribe` arm (`:215-275`), and the `Ops` arm's fan-out;
  - `mesh.rs`: `ingest_and_fanout` (`:495-507`);
  - `session.rs`: heads with hashes, computed as `Store::all_heads` computes them
    (`store.rs:215-230`), and a helper that builds a refused subscribe's two
    frames.
- **Changes:**
  - The `Subscribe` arm takes the store lock first. While it holds the lock, it
    registers the session, reads the heads and the gap, and queues the ack and
    the gap.
  - Every fan-out path holds the store lock from its append until its fan-out is
    queued.
  - This nesting cannot deadlock. No site holds the router or the outbound map
    while it takes the store lock (`server.rs:128`, `:151`, `:249`, `:295`,
    `:310-311`; `mesh.rs:308-309`, `:348-349`, `:499`; `exchange.rs:244`,
    `:253`).
  - The absent route sends R6's two frames.
- **Tests:**
  - `no_op_of_a_zone_reaches_a_subscriber_before_its_ack`.
    - The test holds the store lock. A writer's op is sent and given time to
      queue on the lock. Then another session subscribes, and the test releases
      the lock.
    - tokio's `Mutex` grants the lock in request order, so the writer appends
      first.
    - Red today: the subscriber registered before it waited (`server.rs:249`),
      so it gets the op before its ack.
    - Green: the ack comes first, and the op arrives in the gap.
  - `the_ack_names_each_origin_head_with_its_hash`. Red today: `hash: None`
    (`server.rs:262`).
  - Phase E of `s_discovery_golden_path_end_to_end` (`mesh.rs:794-806`), turned
    round.
    - A subscribe to `ws-attic` gets `Heads{streams: []}`, then
      `Error{UnknownShare}`.
    - Red today: no `Heads` arrives within a bounded read.
- **Proves:** the order and content of the client-path ack, on both roots, and
  that the absent route no longer hangs a client.
- **Does not prove:**
  - the peer subscribe, which keeps the race (`mesh.rs:309`, before
    `:329-332`). This is a named gap, for 4.3's peer check at that line;
  - the grant refusal, which 4.3 part 2 builds on the helper;
  - behaviour over a carrier that reorders frames;
  - the cost of holding the store lock longer: one route, and one queue per
    subscriber, per append. It is not measured.
- **Gate:** as for 2.1.
- **Size:** ~60 production lines, ~160 test lines.
- **Depends on:** 1.1. In one checkout it runs after 2.1, since both edit
  `server.rs`, in different arms.

### Phase 3 — The clients read the answers

**Milestone:** each client tells accepted, refused and unknown ops apart. Each
returns a subscribe's heads once its replay has arrived. Every consumer passes
unchanged.

**Step 3.1 — client-rs: op outcomes**

- **Goal:** R1 and R7 in client-rs, and §5 questions 3 and 4.
- **Files:**
  - `client.rs`: `dispatch` reads `Error` frames (`:108`). New
    `append_outcome`, `send_ops_outcome` and `on_refused`. `append` and
    `send_ops` (`:246-266`) record what they send.
  - A new `glade/client-rs/src/answers.rs`, which is pure: it keeps sent ops by
    hash and resolves them from statuses.
  - `glade/client-rs/src/session.rs`:
    - drop a refused op and the later own ops of its chain, and mark the chain
      unresumed;
    - `append` fails on an unresumed chain;
    - a subscribe ack for the zone clears the mark. 3.2 moves this to the end of
      the replay.
  - `lib.rs`: exports.
- **Tests:**
  - Pure (LBT-008):
    - a status resolves its op;
    - a refusal resolves its op as refused, and drops the tail;
    - an unknown hash is ignored;
    - the connection's end resolves every waiting op as unknown;
    - the fire-and-forget table is bounded, because a node from before Phase 2
      never answers.
  - Integration (`glade/client-rs/tests/integration.rs`):
    - `a_refused_append_stops_its_chain`.
      - Two clients share an origin, and the second one's append at seq 0 is
        refused. After a pause for the refusal to arrive, its next append on
        that chain must fail.
      - Red today: that append returns `Ok`, and the node refuses it as a chain
        break, unseen.
      - With `on_refused` built, the test waits on it instead of pausing, and
        names `Equivocation`.
    - `an_accepted_append_is_ok` and `a_repeated_append_is_ok`.
- **Proves:** that client-rs tells accepted, refused and unknown apart against
  the real node, and never builds on a refused op.
- **Does not prove:** the suppliers' use of it (Phase 4).
- **Gate:**
  1. `cargo test --offline --manifest-path glade/client-rs/Cargo.toml`. There is
     no `--locked`, because its lockfile is untracked
     (`glade/client-rs/.gitignore`).
  2. `cargo test --offline --locked --manifest-path grazel/Cargo.toml`, and
     likewise for `glade-gwz/Cargo.toml` and `glade-gyld/Cargo.toml`.
- **Size:** ~200 production lines, ~220 test lines.
- **Depends on:** 1.1, and 2.1's binary for the integration tests.

**Step 3.2 — client-rs: the subscribe outcome**

- **Goal:** R5-R7 in client-rs.
- **Files:**
  - `client.rs`: the `Heads` arm (`:86-90`), `subscribe` (`:228-239`), and a new
    `subscribe_outcome`;
  - `answers.rs`: catching up, which compares the ack's heads with what the
    connection has received or had accepted;
  - `glade/client-rs/src/supplier.rs:164`, `:197` and `:266`: use
    `subscribe_outcome`, and turn a refusal into an error. So `serve_*` fails,
    and a reattach retries.
- **Changes:**
  - `subscribe_outcome` returns the heads with their hashes, or the refusal and
    its reason.
  - `subscribe` returns after the replay, and returns a refusal as an empty
    zone.
  - A frame the session cannot take fails the waiting subscribes of its zones.
    Today such a frame is dropped whole (`client.rs:81-83`).
  - A completed subscribe clears the unresumed mark on its chain.
- **Tests:**
  - Pure:
    - an ack with no origins completes at once;
    - ops below, at and above the acked seq;
    - the session's own accepted ops count;
    - an ack that names no zone is a refusal. Its reason is the next `Error` for
      the zone without a `corr`.
  - Integration:
    - `subscribe_returns_after_the_replay_is_folded`. A writer puts 2,000 ops on
      a chain. A fresh client's `subscribe` must return with all 2,000 folded.
      - Red today, over several runs: `subscribe` returns at the ack.
      - That red depends on timing. The pure tests cover the same logic
        deterministically.
    - `a_refused_chain_resumes_after_a_subscribe`: the client refused in 3.1
      lands at seq 1 after a subscribe.
    - `subscribe_outcome_returns_the_nodes_heads`.
- **Proves:** that a returned subscribe has its replay, and that the heads come
  back.
- **Does not prove:**
  - the absent route end to end in client-rs. Its harness has no known,
    unclaimed share; 2.2 covers this;
  - anything about a forwarded share's claim holder (R7).
- **Gate:** as for 3.1.
- **Size:** ~130 production lines, ~180 test lines.
- **Depends on:** 3.1, which shares `dispatch`, and 2.2's binary.

**Step 3.3 — client-ts: op outcomes**

- **Goal:** 3.1, in TypeScript.
- **Files:**
  - `client.ts`: an `Error` branch in `onMessage` (`:97-136`), and
    `appendOutcome`, `sendOpsOutcome` and `onRefused`;
  - a new `glade/client-ts/src/answers.ts`, which is pure;
  - `glade/client-ts/src/store.ts` and `session.ts`: the refused tail, and
    unresumed chains.
- **Also:**
  - An outcome call fails at once when the socket is not open. Today, with no
    socket, `send` silently does nothing (`client.ts:234-236`), so `append`
    reports an op as sent when it never was.
  - A refusal that no listener takes goes to `console.warn`. The desk's console
    then shows it without a gryth-ui change.
  - A session that a binder owns is only told.
- **Tests:**
  - pure, in a new `glade/client-ts/test/answers.test.ts`;
  - integration, in `test/integration.test.ts`: an accepted op, a refused op
    (a second client with the same origin), and a repeated op. Red first, as
    in 3.1.
- **Proves:** 3.1's claims, for client-ts.
- **Does not prove:** glial's handling of a refusal (§10).
- **Gate:**
  1. `pnpm --dir glade/client-ts test`.
  2. `pnpm --dir glial test` and `pnpm --dir glial typecheck`. glial's supplier
     kit drives the real client (`glial/test/supplier.test.ts:6-9`).
  3. `pnpm --dir glade/grip-share test`.
  4. `pnpm --dir glade/demo test` and `pnpm --dir glade/demo typecheck`.
- **Size:** ~180 production lines, ~200 test lines.
- **Depends on:** 1.1, and 2.1's binary.

**Step 3.4 — client-ts: the subscribe outcome**

- **Goal:** 3.2, in TypeScript.
- **Files:**
  - `client.ts`: the `Heads` branch (`:113-114`), `subscribe` (`:155-162`), and
    a new `subscribeOutcome`;
  - `answers.ts`.
- **Changes:** as in 3.2. Also:
  - `subscribe` never rejects on a refusal;
  - a frame the client cannot decode or take fails the waiting subscribes.
    Today the decode throws inside `onMessage` (`client.ts:99-106`).
- **Tests:** the pure catching-up tests, and the 2,000-op replay, red first as
  in 3.2.
- **Gate:** as for 3.3.
- **Size:** ~120 production lines, ~170 test lines.
- **Depends on:** 3.3, and 2.2's binary.

### Phase 4 — The suppliers use them

**Milestone:** glade-gyld resumes by the heads. No supplier write is refused
unseen. A restarted glade-gwz streams its runs.

**Step 4.1 — glade-gyld: resume by the heads; report and retry a refused write**

- **Goal:** close the finding in the code where it was found.
- **Files:** `glade-gyld/src/supplier.rs` and `glade-gyld/tests/integration.rs`.
- **Changes:**
  - `resume` (`:1261-1290`) drops its settle loop and its two constants
    (`:1230-1239`). When the subscribe returns, the replay is complete. A
    refused subscribe is logged with its code.
  - One helper carries every write: the publications (`:1371-1388`),
    `append_ask` (`:1820-1835`) and `append` (`:1976-1991`). The helper:
    1. resumes the chain once, through `Resumed` (`:1305-1319`);
    2. appends with `append_outcome`;
    3. on a refusal, logs the chain, the seq and the code, forgets the chain in
       `Resumed`, resumes it, and retries once;
    4. on a second refusal, logs it and drops the write.
- **Tests:**
  - `a_publish_resumes_without_waiting_for_quiet`.
    - A third session writes every 20 ms to an ask conversation the supplier has
      answered, which the supplier is therefore subscribed to.
    - A rebuild's census must reach the mount within 1.5 s.
    - Red today: the resume waits out its 5 s deadline.
  - `a_write_the_node_refuses_is_reported`.
    - A test client puts a `crdt` op on a zone the supplier publishes as a
      value. The node then refuses the supplier's op as a shape conflict
      (`store.rs:164-176`).
    - The supplier binary runs with its stderr piped. (The existing test at
      `tests/integration.rs:2415` discards stderr.) Its stderr must name the
      refusal twice: once for the attempt and once for the retry.
    - Red today: it says nothing.
  - The restart regressions still pass (`:3805`, `:3887`).
- **Proves:** that glade-gyld resumes by the heads and logs refusals.
- **Does not prove:** that the desk shows a refusal (§10).
- **Gate:** `cargo test --offline --locked --manifest-path glade-gyld/Cargo.toml`,
  against the rebuilt node. At 4.3 part 1 the suite ran 233 tests (1 ignored),
  plus 31 more (`GladeNodeAssembly.md:836-837`).
- **Size:** ~80 production lines (about 30 removed), ~150 test lines.
- **Depends on:** 3.1, 3.2, and Phase 2's binary.

**Step 4.2 — glade-gwz: run ids that survive a restart; refusals reported**

- **Goal:** close F4.
- **Files:**
  - `glade-gwz/src/supplier.rs`: run ids minted from the session (`:124`,
    `:162`), as glade-gyld's `mint_run_id` does, and an `on_refused` listener
    that logs;
  - `glade-gwz/tests/integration.rs`.
- **Tests:** `a_restarted_supplier_streams_its_first_run`, on the harness of
  `streaming_output_visible_to_subscriber` (`:320-373`).
  - A supplier streams a run, then shuts down (`glade-gwz/src/supplier.rs:86`).
  - A second supplier, on the same node, streams a run with different output.
  - A subscriber on the second run's id must fold its lines.
  - Red today: the second run's `run-1` lands on the first run's chain, and is
    refused.
- **Proves:** that a restart no longer collides on run ids, and that refusals
  reach the log.
- **Does not prove:** that the desk shows a refusal. The desk treats run ids as
  opaque (`gryth-wz/gryth-ui/packages/plugins/gwz/src/live.ts:43-48`,
  `:120-122`), so gryth-ui needs no change.
- **Gate:** `cargo test --offline --locked --manifest-path glade-gwz/Cargo.toml`.
  At 4.3 part 1 the suite ran 9 tests, plus 5 more
  (`GladeNodeAssembly.md:837`).
- **Size:** ~25 production lines, ~100 test lines.
- **Depends on:** nothing for the run ids, which may land first. The listener
  needs 3.1.

## 8. Order and parallelism

```
§5 ruled
    |
   1.1 --+-- 2.1 -- 2.2 ---------------+-- 4.1  glade-gyld
         |                             |
         +-- 3.1 -- 3.2 ---------------+
         |                             |
         +-- 3.3 -- 3.4                +-- 4.2  glade-gwz (its run ids: any time)
```

- **Foundations first.** The contract lands first, then the node, whose binary
  every integration test uses.
- **The clients.** Each client's pure part can start once 1.1 lands. client-rs
  (3.1, 3.2) and client-ts (3.3, 3.4) share no file. Their integration tests
  wait for Phase 2.
- **Shared files.** 2.1 and 2.2 edit different arms of `server.rs`. 3.1 and 3.2
  share `client.rs`, and 3.3 and 3.4 share `client.ts`.
- **One agent per checkout.** The glade checkout takes one agent at a time
  (`GladeFirstSlicePlan.md:927`). Agents in their own worktrees can take the two
  client lines at once.
- **Other repositories.** glade-gyld and glade-gwz are separate repositories, so
  their steps can run alongside.

## 9. What changes on the default path

As each phase lands, the owner's running desk and suppliers see these changes:

1. Every op a client sends gets its status back, one frame per op (2.1).
   Existing clients ignore it.
2. A refused op no longer counts as held by its sender (2.1).
3. A subscribe to an absent share gets an empty `Heads` before its `Error`, and
   no longer hangs (2.2).
4. Acks carry head hashes, and no op of a zone reaches a session before its ack.
   Each append holds the store lock a little longer (2.2).
5. `subscribe()` returns after the replay, not at the ack (3.2, 3.4).
6. After a refusal, the next append on that chain fails until the chain is
   resumed (3.1, 3.3). The TS client logs refusals that no listener takes to the
   console.
7. glade-gyld no longer waits for quiet (4.1). glade-gwz's run ids change form
   (4.2).

## 10. What this plan does not do

- **The wire.** No IR, frame or field changes.
- **Durability.**
  - No fsync per client op. `Ok` means held, not synced (R2), as 4.4 left it
    (`GladeNodeAssembly.md:410-412`).
  - A tail the node lost is not re-sent. That would be the other direction of
    resume (`GladeSubstrateV1.md:201`).
  - No offline outbox (GAP-11, `client.rs:241-245`). `append` still fails fast
    when disconnected.
- **`Subscribe.from`** stays unread on the client path (§5, question 5).
- **Out-of-order ops.** The client stores still drop ops that arrive out of
  order. R4 keeps a websocket session in order. A carrier that reorders frames
  would need this.
- **The peer subscribe's race** (F2) is left to 4.3's peer check at
  `mesh.rs:309`.
- **glial is not changed.** Its binder keeps a refused op in its instance store
  (`glial/src/instance.ts:150`). Its supplier kit treats a refused subscribe as
  an empty one. The TS client reports both. Handling them is a follow-up in
  glade-wz.
- **gryth-wz is not touched,** and needs nothing. The owner may:
  - run `pnpm install` in gryth-ui once Phase 3 lands, before restarting the
    desk (F5);
  - make the desk show refusals. Until then they reach the console.
- **Grants.** Appends get no grant check. 4.3 checks reads, and `home` is
  refused already.
- **Publishing.** Nothing is published, and there is no publish until an
  end-to-end app runs on grip, glial and glade.
