# Signed checkpoints: the directory stops growing (plan Step 4.5c)

Design note, 2026-09-27, written before any code, against glade `b4e890f` (glade-wz root
`276953c`). A document only: no code was changed, built or run. The red runs and the measured
figures are filled in as each part is built.

The spec is plan Step 4.5c (`dev-docs/GladeFirstSlicePlan.md:908-921` at the glade-wz root) and
the ruling it builds, AZ-12 (`dev-docs/glade/GladeAuthzModel.md:341`, ruled 2026-07-05):
origin-signed checkpoints, with the checkpoint head also appended to the `home` share, so that a
rewrite becomes fold-detectable; countersigning by replication, no new protocol; explicit
countersigning deferred until a threat model demands it. The owner scheduled the step on
2026-09-27 (question 32 (c), plan `:852`). F1 (glade `befa93c`, plan `:928-931`) is the stopgap
until it lands.

Paths: `node/src/…` is `glade/node/src/…`, with line numbers at glade `b4e890f`;
`dev-docs/…` alone is the glade-wz root's. The design was read against `d051b7e`, and plan Step
4.5 part 1 landed as `b4e890f` meanwhile. The files this step changes most, `claims.rs`,
`registry.rs`, `store.rs`, `envelope.rs`, `records_file.rs` and `peer.rs`, are the same in both.
The node lane goes on with 4.5, so lines in `mesh.rs`, `iroh_carrier.rs`, `transport.rs`,
`sysdir.rs`, `assembly.rs` and the roots may move again before this step is built.

This step adds no listener, dial or network call, and reads no environment. The wire IR, the
frames, the contracts and the dependencies do not change. The node's own IR gains one record
kind.

## Summary

**The recommendation.**

- **The record.** A checkpoint is a new `home` record, `ChainCheckpoint {node, stream, seq,
  hash}`, on a new stream, `dir.checkpoints`, in the origin's own chain, sealed by the origin-op
  envelope like every `home` record. It says: my chain on `stream` is folded at `seq`, whose op
  hashes to `hash`. Being a `home` record, it is also its own anchor in the `home` share, which
  is AZ-12's "head also appended to the home share".
- **What it covers.** A chain's whole prefix, folded to its state. In the checkpoint's own
  acceptance the origin first appends again, above the base, every record of the prefix that the
  stream's fold still reads; then every op at or below the base leaves. Only `dir.claims` has
  such a rule in this step: its state is every claim that no later claim of the same node and
  share dominates, with an epoch at least as high and an expiry at least as late. So what leaves
  is the superseded renewals and nothing else, and every claims fold answers as before, at every
  reader's clock. Every other stream stays whole; grants and revocations are the audit log B5
  keeps.
- **Who, and when.** The origin alone, inside a renewal tick's acceptance, once at least N of
  its claims are superseded. N is 1,000 by default, about 14 hours of the desk's renewals: a
  setting in `claims::Leases`, from the entry point, as F1's are.
- **The chain rules.** A compacted chain has a floor: its first op is the one above the base,
  whose `prev` must be the checkpoint's hash, and an op at or below the floor is taken as seen
  and not held. `dir.checkpoints` is a register: the newest checkpoint of a stream supersedes
  the older ones, and a node takes a newer one across any gap.
- **Peers.** A peer that holds the prefix checks the op it holds at the base against the
  checkpoint's hash, keeps a mismatch as a fork proof, and otherwise drops the prefix. A peer
  that is behind, or has never seen the chain, takes the checkpoint first and then the chain
  from its floor. D9 and pull on a gap work unchanged. The serve sends `dir.checkpoints` first.
- **What it buys.** records.json then holds the directory's content, one checkpoint and at most
  about N claims: about 300 KB, where a week's renewals are 3.5 MB today and a year's about
  180 MB. Boot checks at most about 1,000 records in each store, about 0.1 s in all, where a
  week costs 1.1 s today and a year about 58 s. The served store's `home` journals are
  rewritten at each checkpoint.
- **Compatibility.** One-way, like D8 and 4.1c: once a node has made a checkpoint, a build from
  before this step refuses its instance with 4.1b's message. `PROTOCOL` moves to 4, so an older
  peer fails at connect, not mid-sync.

**The split**, in `.rs` lines with doc comments, estimated (section 12):

| Part | What | Production | Tests |
| --- | --- | --- | --- |
| 1 | The record and records.json: the IR, `checkpoint.rs` (the record's check, the placement rule), floors and the register in the registry, boot's two-pass load, `accept`'s change count | 250-330 | ~300 |
| 2 | The served store and sync: floors and the register in `Store`, the journal's rewrite, `open`'s two passes, the serve order | 200-280 | ~350 |
| 3 | Making them: the dominance rule, the threshold, the tick's checkpoint, the line, `PROTOCOL` 4, the simulated week, the desk's replay | 130-200 | ~400 |

In that order, each gated and landed on its own through gwz. Parts 1 and 2 read checkpoints and
make none, so nothing a running node does changes until part 3.

**Questions for the owner**, each with a recommendation at the end:

1. AZ-12 for `home` chains: one record is both the checkpoint and its anchor.
2. What a checkpoint covers: the whole prefix with its state carried forward; `dir.claims` only.
3. The trigger: a count of superseded claims, 1,000 by default, as a setting.
4. `dir.checkpoints` as a register, so records.json holds one checkpoint.
5. A peer behind the base drops its prefix unchecked.
6. `PROTOCOL` 4.
7. The one-way moment on the desk: automatic, with no copy of what the first checkpoint drops.
8. Contradicting checkpoints kept as fork proofs.
9. The leases once this lands: F1's defaults stay, and 4.6 picks what its expiry check needs.
10. Three parts.

## 1. The problem, measured

- A renewal is an ordinary `ServeClaim` append, the same epoch with a fresh expiry, one per
  served share per tick (`node/src/claims.rs:316-344`). Nothing ever removes one. Signed, it is
  291 bytes (`glade/dev-docs/GladeNodeAssembly.md:5126-5128`).
- records.json holds them all and is rewritten whole at each save (`node/src/registry.rs:626-638`,
  `node/src/records_file.rs:145-177`). The served store's `home` journal holds them again,
  append-only (`node/src/store.rs:507-518`).
- Every start checks every `home` record twice, in records.json and in the journal
  (`GladeNodeAssembly.md:4019`): 46 µs a check in the desk's debug build, with the three crypto
  crates optimised (`:4142`).
- At F1's defaults the desk mints 1,728 renewals a day. After a week each save rewrites about
  3.5 MB once every 100 s, and boot's checks take about 1.1 s (plan `:931`). A month is about
  15 MB and 4.8 s; a year about 180 MB and 58 s.
- The same history also grows three other costs: each tick clones the registry and encodes every
  op (`registry.rs:626-638`); each save reads records.json whole to find its revision
  (`GladeNodeAssembly.md:4800-4806`); and every routing decision scans every claim
  (`node/src/mesh.rs:903-915`).

## 2. AZ-12, read for `home` chains

AZ-12 was ruled for chains in general. The s-sync trace draws a new replica bootstrapping from
"chain (peer1·commons), seq 12,000, signed by origin", with the "checkpoint head ALSO in the
home-share fold" (`ggg-viz/src/scenario/sync.ts:67-69`). GQ-9 names the mechanism: "Compaction =
signed per-log checkpoint, prune below it" (`glade/dev-docs/GladeSubstrateV1.md:66-67`). For an
app chain that is two records: the checkpoint in the chain, and its head in `home`, where every
node that folds the directory holds it.

The chains this step compacts are `home` chains, so one record can be both. The recommendation is
a `ChainCheckpoint` in the origin's own `home` chain (question 1). It replicates wherever the
origin's `home` records go: records.json, the served store's journals, every pull and push. A
rewrite then contradicts what those holders fold (section 7).

"No new protocol" holds: no frame, message or exchange is added, and the checkpoint rides the
existing `home` sync as a record. `PROTOCOL` moves to 4 only as a version gate (section 10).

GD56R2-11 (`dev-docs/glade/GladeDiscoveryDesign-Review56-2.md:227-241`) set out what a safe
compaction of lease history needs. Point by point:

| It asks for | Here |
| --- | --- |
| a signed checkpoint with a base seq and hash | `ChainCheckpoint {seq, hash}`, signed by the origin (section 5) |
| peer acknowledgement, or a safe-retention rule | a safe-retention rule: only what no fold reads leaves (section 3), so no peer needs anything a checkpoint drops |
| how gaps cross a checkpoint | the floor and the register (section 6) |
| where fork evidence remains | at the base, above it, and between checkpoints; given up below the base for a node that did not hold the op there (section 7) |
| no independent wall-time horizons | only the origin folds, by a count, never by the clock (section 4) |

The discovery kernel keeps its own v1 rule, chains retained
(`dev-docs/glade/GladeDiscoveryDesign.md:380-384`). Its records are another family
(`GladeNodeSigning.md`, D4), so nothing here reaches it.

## 3. What a checkpoint covers (point 1)

**Recommendation: a chain's whole prefix, folded to its state, with the state carried forward.**
A checkpoint at base B covers every op of its chain at or below B. What of that prefix the
stream's fold still reads is appended again above B, in the checkpoint's own acceptance, the same
record bytes in new ops. Then the prefix leaves.

**Only `dir.claims` in this step.** A stream can be compacted only once it has a rule for what its
fold still reads. `dir.claims` gets one:

- A claim is **superseded** when a later claim in the same chain (a higher seq), naming the same
  node and share, has an epoch at least as high and a lease expiry at least as late.
- Every claim not superseded is state, and is carried forward.

**Why every fold keeps its answers.** Each reader of `dir.claims` asks one of three things of a
node's claims on a share:

| Reader | What it asks |
| --- | --- |
| `Registry::who_serves` (`registry.rs:648-655`), `mesh::who_serves` (`mesh.rs:903-915`) | is a claim live at the reader's clock, and the highest epoch among the live ones |
| `max_claim_epoch` (`claims.rs:383-394`), `home_epoch` (`:372-379`) | the highest epoch, live or lapsed |
| `directory_knows` (`mesh.rs:920-937`) | whether any claim names the share |

A superseded claim is live only while the claim that supersedes it is live, with an epoch at
least as high. The highest epoch is always held by a claim that nothing supersedes, and a share
with any claim keeps one. So every answer is the same, at every clock: a peer whose clock runs
behind or ahead sees what it saw before.

**On the desk,** every earlier claim on `home` or `ws-razel` is superseded by the latest renewal:
a restart raises `ws-razel`'s epoch, and `home` keeps epoch 1 (`claims.rs:372-379`). So nothing is
carried, and what leaves is exactly the renewals. A claim is carried only for a share the node no
longer renews, or after a clock step back or a shorter lease left an older claim live longer.

**Alternatives weighed.**

- **(a) The renewals only, the originals kept below the base.** The checkpoint lists the hashes
  of the claims that stay, and they keep their places. Nothing is appended again, but chains get
  holes: the store and the registry must place a sparse op by the list, `scan` and the heads step
  over holes, and a peer can check a held op against the list only where it names a seq. More
  code where the risk is, and no fold gains.
- **(b) The whole prefix, its state carried forward.** Recommended. Chains stay contiguous above
  their floor, and every fold reads the same records it read before.
- **(c) The state inside the checkpoint.** The record lists the live claims as values. Every fold
  would then read checkpoints as well as ops, and the record grows with the state.
- **(d) Every stream, each with its own rule.** Grants and revocations are the audit log: "the
  audit log is the storage, and coup attempts are permanent, attributable data"
  (`dev-docs/glade/GladeAuthzModel.md:122-123`), and B5 keeps legacy records as history
  (`:141-146`). Nodes, workspaces, bindings, services, transport records and the recovery key do
  not churn: re-registering an unchanged file appends nothing. Principals grow with browser tabs,
  but a principal once seen must stay known, or its next Hello mints it again
  (`claims.rs:279-312`). So (d) gains little and risks the one history the model says to keep. A
  stream can opt in later, with its own rule.
- **(e) No checkpoints**: leases outside the fold, or renewals decoupled from op minting
  (`dev-docs/GladePersistenceReview.md:661-665`, options (b) and (c)). The owner ruled AZ-12's
  checkpoints (question 32 (c)).

**What stays verifiable afterwards.**

- Every op a node holds: by its own signature, and by its chain from the floor, since the first
  op above the floor must name the checkpoint's hash as its `prev`.
- The checkpoint: by its signature, which covers its base, and by its place in the origin's
  checkpoint chain wherever the one before it is held.
- The base: vouched for by the checkpoint alone once the op at the base is gone, and checked
  against that op by every node that still held it when the checkpoint arrived.
- Every fold's answers, as above.

**What history is given up.**

- The superseded renewals: when each lease was renewed, and how often. The latest claim of each
  share stays.
- Serving the covered prefix: no node can give it to anyone.
- Fork evidence inside the covered range, for a node that had not received the op at the base:
  it drops its held prefix unchecked (section 7).
- Older checkpoints: only the newest of a stream is kept, so a contradiction between two of them
  is provable only by a node that held both.
- 4.4's first restart promise, "A restart loses nothing it acknowledged"
  (`GladeNodeAssembly.md:468-470`), becomes: every acknowledged record, or its effect on every
  fold, survives a restart.

## 4. Who makes one, and when (point 2)

**The origin alone** (AZ-12). A node makes checkpoints of its own chains only, and applies the
origin's to its copies of others'. Each chain then has one horizon, the origin's, so no two
holders can fold one chain differently: the hazard GD56R2-11 names for independent wall-time
compaction.

**Inside a renewal tick's acceptance.** `renew_leases` (`claims.rs:316-344`) runs each tick under
the directory lock, as one acceptance: staged, saved, then folded (SP-L1, `:103-113`). When a
checkpoint is due, that acceptance does, on the staged registry:

1. B is the claims chain's tip before the tick, and H its op's hash.
2. The claims at or below B that are not superseded, counting this tick's renewals, are appended
   again, in their order.
3. The tick's renewals are appended, as today.
4. `ChainCheckpoint {node, "dir.claims", B, H}` is appended on `dir.checkpoints`.
5. The registry, taking its own checkpoint, drops its claims at or below B and its previous
   checkpoint (section 6).

Then one save, the fold, and `publish` (`claims.rs:354-364`) in the order the ops were appended:
the carried claims, the renewals, the checkpoint last (section 7 says why). A failed save folds
and publishes nothing, and the next tick tries again, as every tick does.

```text
before     dir.claims of N:        0 1 2 … 999 1000          tip 1000
the tick   carries nothing; renews 1001 1002; checkpoint c = (dir.claims, 1000, hash(op 1000))
after      dir.claims of N:        1001 1002                 floor (1000, H); 1001.prev = H
           dir.checkpoints of N:   c                         c-1 dropped
```

**The trigger: a count.** A checkpoint is due when at least N of the node's own claims above its
floor are superseded. N is a new field, `Leases.checkpoint_after`, 1,000 by default. `start()`
builds it with the lease and the interval and hands it to both roots, as F1 does
(`node/src/bin/glade-node.rs:210`), with no flag, file line or environment read; tests set a
small one. With two served shares the desk supersedes two claims a tick, so a checkpoint comes
every 500 ticks, about 14 hours. The count is checked at every tick, adoption's renewal included
(`claims.rs:179`), so a node that starts on a long chain folds it before anyone connects.

The line each checkpoint prints goes through a reporter the root hands to adoption, as each root
hands the door its reporter (`node/src/transport.rs:327`, `:369-371`): stdout on the hand-written
root, the console on the assembled root. No static, no hook.

Alternatives weighed (question 3):

- **Size of records.json.** Much the same here, since claims are about 291 bytes each, but it
  depends on the encoding and the other streams. The count is the quantity that grows.
- **Time, say daily.** Leases are settings and tests shorten them. A count bounds the records
  whatever the rate, and is deterministic under test.
- **At boot only.** A node that runs for weeks keeps growing, and the desk does run for weeks.
- **Every tick**, N equal to the shares served. records.json stays minimal, but each tick adds a
  checkpoint record, a journal rewrite and a pushed record, and a renewal's fork evidence lives
  one tick.
- **Countersigned, or folded by any holder.** AZ-12 defers countersigning; any holder folding
  brings back independent horizons.

## 5. The record (point 3)

**The IR change.** One message in the node's own IR, `node/ir/sysdata.taut.py`, after
`NodeRecoveryKey` (`:158`), regenerated with `--legacy-codec`
(`dev-docs/GladeProgramStatus.md:29`). The wire IR does not change.

```python
    # ---- signed checkpoints (plan Step 4.5c; AZ-12) --------------------------
    # Node N's chain on `stream`, a directory stream this build compacts
    # (dir.claims alone), is folded at `seq`, whose op hashes to `hash`: every
    # op of that chain at or below `seq` is covered, and what of it the stream's
    # fold still reads was appended again above it. A record in N's own chain on
    # dir.checkpoints, sealed by the origin-op envelope, which is its proof. The
    # newest for a stream supersedes the older ones.
    Msg("ChainCheckpoint",
        F("node", 1, STR),
        F("stream", 2, STR),
        F("seq", 3, INT),
        F("hash", 4, BYTES)),
```

- `G_CHECKPOINTS = "dir.checkpoints"` joins the stream ids (`registry.rs:49-69`),
  `Record::Checkpoint` the record enum, and the kind table (`node/src/envelope.rs:192-206`) takes
  it as `[Text, Text, Int, Bytes]`. The directory profile then hosts it, through
  `envelope::directory_stream` (`node/src/assembly.rs:536-538`).
- **Its own check**, after the envelope's seven rules (`envelope.rs:146-173`): `node` is the op's
  origin; `stream` is one this build compacts, `dir.claims`; `seq` is 0 or more; `hash` is 32
  bytes. Anything else is refused with a new reason, `Refused::Checkpoint`, and drops nothing.

**What it signs.** It has no signature of its own and no new tag, like 4.1c's `NodeRecoveryKey`
(`GladeNodeAssembly.md:5158-5176`). The envelope's signature is the origin's, under
`glade/v1/origin-op\0`, over the op's ten fields with the record as payload
(`envelope.rs:63-71`). It binds who (the origin), which chain (the stream), the base (its seq and
hash), and the checkpoint's own place in the origin's checkpoint chain (its seq and `prev`). It
verifies alone, since the op carries everything it covers, so a checkpoint could later travel
apart from its chain: a blob bootstrap, say (`dev-docs/IrohGladeMapping.md:199-202`).

**How its head is also appended to `home`.** The record is a `home` record in the origin's own
chain (section 2).

**`format` looks inside.** 4.1b refuses a start on a `home` record this build cannot read, rather
than quarantining it and dropping it at the next save (`GladeNodeAssembly.md:4202-4233`). A
checkpoint of a stream this build does not compact is such a record: a later build's, which,
quarantined here, would leave that stream's chain stranded above its floor. So `envelope::format`
(`envelope.rs:104-119`) calls it `Unknown`.

**Alternatives weighed.**

- A checkpoint op inside the chain it compacts would put two kinds on one stream, where each
  stream holds one kind (`envelope.rs:159-162`).
- One cumulative record listing every compacted stream needs a list of structures, deeper than
  the checked decoder reads (two levels, `envelope.rs:232-240`). One record per stream, the newest
  winning, does the same.
- An inner signature under a new tag, as the transport records carry
  (`GladeNodeAssembly.md:3886-3897`), buys nothing the envelope does not.

## 6. The chain rules: a floor, and a register

One function, in a new module `node/src/checkpoint.rs`, decides both rules for the registry and
the served store alike.

**The floor.** For each (stream, origin), the newest checkpoint held for it gives the chain a
floor, (B, H):

- The chain's first op is the one at B+1, and its `prev` must be H. For a chain with a floor,
  that replaces the rule that a `home` chain starts at seq 0 (`registry.rs:499-501`,
  `store.rs:416-424`).
- An op at or below B is covered: taken as seen and not held, which is the served store's
  existing `Append::BelowRetained` (`store.rs:37-42`) and the registry's new `Ingested::Covered`.
  It is not judged, since nothing it chains to is held. The one exception is an op at B whose
  hash is not H, which contradicts the checkpoint: a fork (section 7).
- A chain's head is its last op, or (B, H) while nothing above the floor is held
  (`store.rs:343-361`), so a pull asks for exactly what lies above.

**The register.** `dir.checkpoints` is not a log whose ops must follow one another. Per origin and
named stream, a node keeps the newest checkpoint:

| A checkpoint arrives; the newest held for its stream is… | Taken as |
| --- | --- |
| none | placed, at any seq: it anchors itself |
| at a lower seq, with a lower base, or the same base and hash | placed; the held one leaves |
| the same op | a duplicate |
| at the same seq, another op | a fork: both kept as its proof |
| at a higher seq | seen, not held |
| at a lower seq, with a higher base, or the same base and another hash | a rewrite: refused, both kept as its proof |

The envelope's rule still holds: seq 0 names no predecessor and a later seq names one
(`envelope.rs:156-157`). The `prev` is checked wherever the checkpoint before it is held. B5's
strict predecessor governs grant, revoke, name-claim and membership ops
(`dev-docs/glade/GladeAuthzModel.md:135-138`); a checkpoint is none of these, and never covers
them. With one compacted stream the register holds one op, so records.json holds one checkpoint
however long the node runs.

**Placing a checkpoint** at a node that holds some of its chain:

1. It must pass section 5's check and the register.
2. If the node holds the op at B, that op's hash must be H, or the checkpoint is a fork
   (section 7).
3. The node's ops at or below B leave, and the chain's floor becomes (B, H).

**Three places the code as it stands must change:**

- **Load and open take checkpoints first.** records.json and a journal list ops in the order they
  were written, and a checkpoint is written after the ops above its floor (section 4).
  `Registry::load` (`registry.rs:408-432`) and `Store::open` (`store.rs:171-226`) make two
  passes, checkpoints first, whatever the order. `Registry::snapshot` (`registry.rs:712-739`)
  also writes them first, so that `seed_registry` (`node/src/server.rs:129-139`) seeds a fresh
  served store from the floor.
- **`accept` counts changes, not ops.** It saves only when the staged copy is longer than the
  fold, since "the fold only grows" (`registry.rs:633-636`). A checkpoint can leave the copy no
  longer, or shorter. `accept` keeps a change count instead, and saves when it moved.
- **The D8 set-aside must know floors.** `open` renames a `home` journal aside, whole, if any op
  in it does not verify as appended in order from seq 0 (`store.rs:201-209`, `:440-449`). A
  journal rewritten from a floor would be set aside at the next open. `verifies` takes the two
  passes and the floors, and skips a covered op rather than failing it.

## 7. Peers (point 4)

**A peer that holds the prefix, and is current.** The origin's push carries the carried claims,
the renewals and the checkpoint, in that order (`claims.rs:354-364`, `mesh.rs:385-400`). The
peer holds the chain to B, so the claims above B land in order, and then the checkpoint. The peer
holds the op at B, so its hash is checked against H before anything is dropped. A match proves
the whole covered prefix the same as the origin's, since each op's hash covers its `prev`. The
peer then drops its ops at or below B, and its served store rewrites that origin's journal
(section 8). No gap or pull arises. With the checkpoint last, the peer never sees the origin's
claims missing, and nor does the origin's own served store, which `publish` feeds in the same
order.

**A peer behind the base,** holding the chain only to some k below B, because it missed pushes or
was offline. The claims above B arrive as a gap: refused, and noted for a pull. The checkpoint,
on its own chain, is placed. The peer cannot compare its held ops with H, since it does not hold
the op at B, so it drops them unchecked and takes the floor (question 5). The pull that the gap
starts (`mesh.rs:746-772`) announces the floor as its head and brings the chain from B+1, whose
`prev` must be H. Until that pull lands, the peer's fold lacks the origin's claims; it was stale
already. A push lost whole heals the same way, at the next tick's push.

**A peer that has never seen the chain,** at its pull at connect. It announces no head for the
origin, and the serve sends the origin's newest checkpoint first, then the chain from its floor.
`serve_home` (`mesh.rs:570-595`) and the library's `serve_sync` (`node/src/peer.rs:311-352`) send
`dir.checkpoints` before every other zone. Byte order already puts `dir.checkpoints` before
`dir.claims`, since both serve `Store::zones` in order through `missing_for`
(`node/src/session.rs:27-34`), but a stream compacted later could sort first, so the order is
made explicit.

**D9.** A checkpoint is a `home` record like any other: taken only from this node or one it has
met this run, and otherwise deferred with the rest of that origin's chains (`mesh.rs:681-704`).
What a node holds is checked again at open without the met-this-run rule, as ruled (plan `:776`).

**Pull on a gap.** Its rule is unchanged (`GladeNodeAssembly.md:4366-4384`): only a push refused
as a gap starts a pull. A covered op is not a gap; it is taken as seen. A checkpoint refused as a
fork or a rewrite is not one either: a pull would bring the same op. The pull's "healed" test
reads the heads (`mesh.rs:851-857`), which report the floor.

**The sync drivers.** The mesh's rounds and the library's `pull_sync` (`peer.rs:358-402`) both land
ops through `Store::append`, which applies the floor and the register, so neither changes beyond
the serve order. A covered op counts as applied, as a duplicate does.

**What a rewrite looks like** (AZ-12's fold-detectable rewrite, and where fork evidence remains):

| The origin signs | Who sees it | What follows |
| --- | --- | --- |
| two checkpoints at one `dir.checkpoints` seq | any node holding one when the other arrives | a fork: the store's existing proof, both ops (`store.rs:264-268`) |
| a checkpoint naming at B a hash other than that of the op it signed at B | any node that held the op at B | a fork: the held op and the checkpoint kept as its proof; nothing dropped |
| a later checkpoint with an earlier base, or another hash at the same base | any node holding the earlier checkpoint | a rewrite: refused, both kept as its proof |
| a second op at B+1, or one whose `prev` is not H | any node holding the chain from the floor | a fork or a chain break, as today |
| two versions of a covered op | nobody, once both are covered | given up |

The proofs log keeps "two validly-shaped ops signed into the SAME `(origin, zone, seq)` slot"
(`store.rs:45-49`). A checkpoint names a slot, B of its stream, by that op's hash, so the op held
there and the checkpoint are such a pair, and `slot()` answers the held op's. A rewrite pair is
two checkpoints (question 8).

No peer signs anything about another's checkpoint, and no acceptance is recorded: countersigning
stays deferred, as AZ-12 rules.

## 8. records.json and the served store's `home` journal (point 5)

**records.json keeps** every record of the node's own streams but `dir.claims`, whole; its newest
checkpoint; and its claims above the floor, which are the carried ones and then the renewals
since.

- A save still writes the whole snapshot: the persistence contract is a snapshot store
  (`records_file.rs:1-11`), and 4.4's atomic replace depends on it. What stops is the history
  inside it. The bound is the directory's content plus N claims: about 1,030 records and 300 KB on
  the desk at N = 1,000, where the file now grows by 0.5 MB a day.
- **The revision** keeps its one role: each save is one compare-and-swap at the next revision
  (`records_file.rs:145-177`), and a checkpoint lands in its tick's save like any mint. Nothing
  about a checkpoint is keyed on the revision. The revision is local and restarts at 1 on a
  downgrade (`GladeNodeAssembly.md:4692-4706`), while a checkpoint's identity, its seq on
  `dir.checkpoints` and its base, replicates. Nor do revisions trigger one: saves also count
  registrations and principals.
- Each save still reads records.json to find its revision; that read is now bounded too.

Alternatives weighed: an append-only records.json, a journal with a periodic snapshot, is "the
served store as record host", 400-650 lines, deferred on 2026-09-24 (`GladeNodeAssembly.md`,
"Durable store and restart"). It would end whole rewrites but not the growth. A side file for
claims is not atomic with the snapshot, which 4.4's rule forbids.

**The served store's `home` journal**, one file per share and origin (`store.rs:507-518`), keeps
the same ops as the registry for the node's own chain, and for a peer's chain what that peer's
checkpoints leave.

- **Rewritten at each checkpoint it places:** the node's own journal at its own checkpoint, a
  peer's when that peer's arrives. The retained ops of that origin's `home` share go, checkpoints
  first, to a temporary file, which is synced and renamed over the journal, and the directory
  synced: 4.4's order, through `entry_sync`'s braced platform modules (`registry.rs:280-295`), so
  no new platform branch. The temporary name does not end in `.log`, so `open` never replays one
  that a crash left. The rewrite runs under the store's lock, once per checkpoint: about twice a
  day on the desk, over a bounded file.
- **At open,** a journal that still holds covered ops, from a crash between the checkpoint's
  append and the rewrite, loads without them, is not set aside, and is rewritten then. Whatever
  else a crash loses comes back as it does today: adoption seeds the node's own records from
  records.json (`server.rs:129-139`), and a peer's come with the next pull.

Alternatives weighed: segment files, one per checkpoint with the old ones deleted, which need a
manifest and a new journal layout; or pruning in memory only, which leaves the disk growing.

## 9. Boot (point 6)

**What it checks.** The same things as today, over a bounded set:

1. records.json through the checked reader, with 4.1b's format classes (`node/src/sysdir.rs:384-414`):
   a checkpoint of a stream this build compacts is sealed; any other is unknown and refuses the
   start (section 5).
2. The registry's load, checkpoints first: each checkpoint's envelope, its own check and the
   register; then every other record, each chain from seq 0 or from its floor, every signature
   checked (`registry.rs:408-432`).
3. At the served store's open, each `home` journal the same way, in two passes.
4. At adoption, the seed lands records.json's records in the served store, checkpoints first; a
   repeat is taken as held without a check (`store.rs:243-245`).
5. Adoption's renewal checks the trigger, so a node that starts on a long chain folds it there,
   before the endpoint binds or the listener opens.

**In what bounded time.** Per node, a boot checks its content C (every stream but `dir.claims`),
one checkpoint, the carried claims K (at most the shares it has ever served), and at most N + S
claims since the checkpoint (S shares served), whatever its uptime; then the same again at open,
with each peer's chain bounded by that peer's own checkpoints. On the desk at N = 1,000 that is
about 1,030 records: about 50 ms of checks in records.json at 46 µs each, and as much in the
journal, so about 0.1 s, where a week costs 1.1 s today. Estimated from 4.1b's measurements
(`GladeNodeAssembly.md:4140-4142`); part 3 measures it.

The first start on this build still checks the whole history once, as today, and then folds it.

Alternatives weighed: a snapshot signed whole by the node and checked with one signature would end
class 2's rule that records verify on load "exactly as on the wire", which makes tampering
self-DoS, never forgery (`dev-docs/glade/GladeSystemDataSeam.md:104-113`), and records.json would
still grow. Trusting the node's own records without a check drops the same rule.

## 10. Compatibility (point 7)

**An older build on a folded instance.** A build from 4.1b part 2 up to the one before this
step's part 1 meets `dir.checkpoints`, a stream it does not know, in records.json, and refuses the
start before it writes anything (`envelope.rs:123-130`; `GladeNodeAssembly.md:4202-4233`):

```text
…/records.json holds a home record this build cannot read (dir.checkpoints of node <id> at seq <n>): its format is newer than this build's, or it is damaged; start the build that wrote it, or move …/records.json aside
```

The served store's open refuses its journal the same way. Going back means moving records.json
and the `home` journals aside, and the node then mints again, as D8 describes.

So this is another **one-way upgrade**, like D8's set-aside at 4.1b and 4.1c's recovery key
(`GladeNodeAssembly.md:5289-5303`), with one difference: nothing is set aside. The superseded
renewals are dropped, since a copy of them would be the history the step exists to remove
(question 7). Parts 1 and 2 read checkpoints and make none: a downgrade from either to an older
build is harmless, and one from part 3 to either starts, and only stops folding.

**An older peer.** It would refuse the checkpoint, `not a directory stream`, and keep the rest, as
it does 4.1c's commitment (`GladeNodeAssembly.md:5299-5301`). But once behind a floor it could
never follow the chain again: every push past its gap would be refused, and every pull would
bring the same gap. That is the mid-sync failure D4 and D6 moved the protocol to prevent. So
`peer::PROTOCOL` becomes 4 (`peer.rs:37-41`), and an older node fails at connect: at HELLO, which
checks the number (`peer.rs:174-175`), and, while the mesh is still on `PeerEndpoint`, at the
ALPN, which becomes `glade/node/4` (`node/src/iroh_carrier.rs:50-53`). No mesh runs yet beyond
the lane owner's test machines, and the desk has no peer (question 6).

**Clients.** None reads or writes `home` (`GladeNodeSigning.md`, D4), and 4.3 refuses a client's
`home` write. A session subscribed to `dir.claims`, as the node's own tests are, receives the
chain from its floor.

**What the owner's desk sees at its first restart on part 3:**

- The first start takes as long as today's, since it checks the whole history once: about 1.1 s
  for each week of renewals at F1's rate since its records were last set aside, and ten times
  that for each week before F1.
- During adoption, before `registry ready`, one new line: `checkpoint: dir.claims folded at seq
  <B>, <M> superseded claim(s) dropped, 0 carried`.
- records.json falls to a few kilobytes, and the served store's `home` journal with it. The node
  and endpoint ids, the apps, `ws-razel`'s epoch and every other line are as before.
- Later starts check about 1,000 records at most, about 0.1 s. While it runs, the desk prints the
  line about twice a day.
- From then on, a build before this step refuses the instance, with the message above.

## 11. Tests, each begun red (point 8)

Each test is run first against the code with the part it guards switched off, in a scratch copy
of the sources, and the message recorded, as in the steps before.

**The done-when, made concrete:**

1. **A simulated week.** A node that serves `home` and one workspace renews for a week at F1's
   defaults, 6,048 ticks and 12,096 renewals, at the default threshold, through the registry's
   tick with an in-memory engine, then saves once to records.json and boots from it. records.json
   then holds at most C + N + S + K + 1 records, with K 0 here, so C + 1,003, where a week without
   checkpoints holds C + 12,096. The boot checks at most that many, and the served store's journal holds as many
   after adoption. Every claims fold answers as it does over a registry that kept every renewal.
   The bound on time is asserted as the count of records checked, which is deterministic; the
   measured time is recorded, not asserted.
2. **A peer accepts the checkpointed chain.** Over real iroh: a linked peer takes the origin's
   pushes across its checkpoints and folds alike; a peer that never saw the chain takes it from
   the checkpoint; a peer behind the base drops its prefix and heals by the pull.
3. **Both roots**, as processes: a node that starts on a long chain folds it at adoption and says
   so, and a second start does not.

**Part 1.**

| Test | Proves | Red against |
| --- | --- | --- |
| `checkpoint`: `a_checkpoint_is_taken_only_for_its_own_nodes_claims` | the record's check: another node's id, `dir.principals`, an unknown stream, a negative seq, a 31-byte hash, each refused `Checkpoint`; a good one taken | a check that answers `Ok` |
| `envelope`: `format_calls_a_checkpoint_of_a_stream_this_build_does_not_compact_unknown` | a later build's checkpoint refuses the start rather than being quarantined | `format` reading the kind's shape alone: `Sealed` |
| `registry`: `a_sealed_registry_keeps_its_claims_from_its_checkpoints_floor` | after its own checkpoint at B: no claim at or below B in the fold; the next claim above the tip; the snapshot lists the checkpoint first; a sealed reload quarantines nothing and has the same tips | a chain that must start at seq 0: the reload quarantines every claim |
| `registry`: `a_covered_op_is_seen_and_a_newer_checkpoint_crosses_a_gap` | an op at or below the floor is `Covered`, not a fork; a checkpoint two seqs ahead is placed; an older one is seen; one at the held seq with another hash is a fork; one whose base moves back is refused | `dir.checkpoints` as an ordinary chain: `Gap { expected: 1, got: 2 }` |
| `registry`: `a_change_that_drops_as_many_as_it_appends_is_saved` | `accept` saves a checkpoint that leaves the fold no longer | the length test (`registry.rs:633-636`): the engine's snapshot unchanged |
| `sysdir`: `a_boot_loads_a_checkpointed_records_json_in_any_order` | a records.json whose checkpoint follows the claims above its floor boots with nothing quarantined | a load in file order: `quarantined N` |

**Part 1, as built** (2026-09-28, on `b4599ef`). 310 production `.rs` lines added and 32 removed,
against 250-330: `checkpoint.rs` 147 (the reader, `check`, and the two rules as pure functions,
`against` for the floor and `place` for the register, which part 2's store calls as the registry
does); `registry.rs` +107 −19; `envelope.rs` +28 −12, where `verify` makes the record's check after
its seven rules; `sysdata.rs` 26, generated. Tests: 335 lines, the six above, each first shown red
in a copy against its named form, and the kind added to two tests that list every kind. As built,
the register keeps its checkpoints apart from the fold's ops, per (stream, origin), and a snapshot
lists them first; a floor with nothing above it is its chain's tip; a checkpoint's `prev` is checked
against the held one, which, with one compacted stream, is the one before it; a fork at the base is
`Equivocation` at B, a rewrite the new `RegistryError::Rewrite`, an older checkpoint `Covered`. The
lines part 1 cites moved a little and hold. For part 2: `mesh.rs`'s readers (`:903-937`) are now
`:1250-1287`, `serve_home` (`:570-595`) is `:870`, on 4.5b's conversations, and `serve_sync` and
`pull_sync` are `peer.rs:404` and `:451`. The gate passed 9/9, 431 tests on each path, and the
desk replay printed `b4599ef`'s lines.

**Part 2.**

| Test | Proves | Red against |
| --- | --- | --- |
| `store`: `a_checkpoint_moves_a_chains_floor_and_rewrites_its_journal` | a peer's chain of ten claims, then its checkpoint at 7: `scan` yields 8 and 9; the journal holds the retained ops, checkpoint first, and a reopen holds the same; the heads name 9, or (7, H) with nothing above; an op at 5 is `BelowRetained`, a repeat of 8 a duplicate | the checkpoint placed and nothing dropped: the journal still holds all ten |
| `store`: `open_takes_a_rewritten_journal_and_one_the_rewrite_never_reached` | a journal that starts with a checkpoint opens with nothing set aside; one holding the covered ops, the checkpoint after them, opens without them, is not set aside, and is rewritten | 4.1b's `verifies`: `set aside 1 journal(s) of the served store's home share …` |
| `store`: `a_checkpoint_that_contradicts_its_base_is_a_fork_and_drops_nothing` | a held op at B with another hash: refused as a fork, the pair in the proofs log, nothing dropped; a base moving back: refused, the pair kept | no comparison: the prefix dropped |
| `mesh`: `the_serve_sends_checkpoints_before_every_other_home_zone` | pure: the serve order of the `home` zones puts `dir.checkpoints` first, given one that sorts before it | the zones in `Store::zones` order: `dir.binding-retractions` first |
| `peer`: `pull_sync_takes_a_chain_from_its_checkpoints_floor` | the library's driver: a puller holding nothing takes the checkpoint, then the chain from its floor, and rejects nothing | a store with no floor: `rejected` names the claims chain |
| `mesh`: `a_peer_behind_a_checkpoint_takes_it_first_and_the_chain_from_its_floor` | over real iroh: B holds A's claims 0 to 3; A holds its checkpoint at 7 and claims 8 and 9, made by a test helper; B's pull leaves B holding the checkpoint, 8 and 9, and routing A's share to A | a store with no floor: `refused 2 home record(s) of node <A> on dir.claims from peer <A>: a gap: expected seq 4, got 8` |
| `mesh`: `a_pushed_checkpoint_prunes_a_current_peer_with_no_gap` | B holds A's claims to 7; A pushes 8, 9 and its checkpoint at 7; B holds 8, 9 and the checkpoint, no pull starts, and B's journal for A holds those three | the push taken and nothing dropped |

**Part 3.**

| Test | Proves | Red against |
| --- | --- | --- |
| `checkpoint`: `only_superseded_claims_leave_and_every_claims_fold_answers_alike` | the rule, over a history with restarts that raise the epoch, a shorter lease, a clock step back, an exact repeat, a lapsed share and two shares: at instants across every expiry, both `who_serves`, `max_claim_epoch`, `home_epoch` and `directory_knows` answer as over the whole history | the latest claim kept per share: at the clock step, `who_serves` left `None`, right `Some(<node>)` |
| `claims`: `a_node_folds_its_claims_once_n_are_superseded` | adopted with short leases and a threshold of 4: records.json holds one checkpoint and at most 4 + 2 claims, the served store's journal the same, and `who_serves(home)` names the node throughout | no trigger: records.json holds every claim |
| `claims`: `the_tick_publishes_its_carried_claims_and_renewals_before_its_checkpoint` | the order `publish` lands and pushes | the checkpoint first |
| `tests/durable`: `a_simulated_week_leaves_records_json_and_the_boot_bounded` | the done-when's first clause, above, in a temp directory | no trigger: C + 12,096 records where at most C + 1,003 were expected |
| `mesh`: `a_linked_peer_and_a_new_one_take_the_checkpointed_chain` | the done-when's second clause, over real iroh with a threshold of 4: B, linked, follows A across two checkpoints, its journal for A bounded, routing alike; C, met afterwards, takes A's chain from the checkpoint | a peer that never drops: B's copy holds every claim |
| `tests/assembled_path`: `both_roots_fold_a_long_claims_chain_at_adoption_and_say_so` | on each root: an instance whose records.json holds 2,000 renewals, written under its key by a test helper, prints the line before `registry ready`, and records.json then holds a bounded count; a second start prints no such line | no check at adoption's renewal |
| `peer` (and `iroh_carrier` while the mesh is on `PeerEndpoint`): `a_protocol_3_node_fails_at_connect` | an older node fails at HELLO, and at the ALPN | `PROTOCOL` at 3 |

**What they do not prove:** Windows and Linux, which the lane owner runs on dabeest and the Pi; a
year, only extrapolated from the week; a crash inside the journal's rewrite, only its two end
states; a clock step on a live node, only in the pure rule; an older peer beyond the protocol's
refusal; four or more nodes.

**The gate** (`glade/node/check.sh`) must pass every component: the node's tests on both paths;
rustfmt at or below its baseline, with no deviation in a line this step writes; clippy at its
baseline; process-globals with nothing new, since the threshold comes from the entry point and
the reporter from the root; confinement with no new crate; the contracts gate unchanged. Beside
it: the six downstream suites against the rebuilt binary, and a replay on a stand-in of the desk
at each part. Parts 1 and 2 must print today's lines. At part 3:

1. a stand-in laid out as grazel lays out the desk, first booted by today's binary;
2. a week of renewals written into it, under its key, by a test helper;
3. today's binary started on it, timed to `listening`;
4. part 3's binary: the line, records.json's size, and the time;
5. part 3's binary again: no line, and the time;
6. today's binary on the folded instance: refused, exit 1, with the message;
7. part 3's binary once more, unchanged.

## 12. Size and the split (point 9)

Estimated, in `.rs` lines with doc comments. Recent steps have run over their estimates (4.1c ran
about 1,000 lines against 250, `GladeNodeAssembly.md:5369-5371`), so each part is kept well under
500.

| Part | Production | Tests |
| --- | --- | --- |
| 1: `checkpoint.rs` about 120 (new: the record's check, the placement rule); `registry.rs` about 110 (the stream and the kind, floors, `Covered`, the two-pass load, the snapshot's order, the change count); `envelope.rs` about 20 (the kind, `format`, `Refused::Checkpoint`); `sysdata.rs` 20, generated | 250-330 | ~300 |
| 2: `store.rs` about 180 (floors, the register, placing a checkpoint, the proof, the journal's rewrite, `open`'s two passes, the heads); `mesh.rs` and `peer.rs` about 20 (the serve order) | 200-280 | ~350 |
| 3: `checkpoint.rs` about 60 (the dominance rule, the tick's checkpoint); `claims.rs` about 60 (`Leases.checkpoint_after`, the tick, the order, the line); the roots about 25 (the threshold and the reporter); `peer.rs` and `iroh_carrier.rs` about 5 (`PROTOCOL` 4) | 130-200 | ~400 |

About 580 to 810 production lines in all, hence the split. The order is foundational first: part
1's rules are what part 2's store and part 3's tick call. Parts 1 and 2 could be one commit of
about 460 lines (question 10).

Where it meets other work: 4.5 and 4.5b edit `mesh.rs`, `peer.rs`, `iroh_carrier.rs` and the
roots, which parts 2 and 3 touch in a few places: the serve order, `PROTOCOL`, the threshold's
path from the entry point. The plan runs 4.5c after 4.5b in the one node lane (plan `:1027`), so
those lines land on 4.5b's code.

## Named gaps

- **Fork evidence below a base** is given up for a node that did not hold the op at the base
  (section 7).
- **Principals keep growing,** one record per new principal, and each desk tab presents a random
  one (`GladeNodeAssembly.md:794`). The other streams grow too, at their own slow rates. Only
  `dir.claims` is compacted.
- **Only the newest checkpoint** of a stream is kept, so a contradiction between two older ones is
  provable only by a node that held both.
- **A rewrite pair** goes into the proofs log, whose pairs were one slot's two ops; `slot()` then
  names the earlier checkpoint's.
- **A peer behind the base** lacks the origin's claims between the checkpoint and its pull
  (section 7).
- **A session subscribed to `dir.claims`** sees the chain start at its floor, and a client that
  checked chains from seq 0 would call that a break. No shipped client reads `home`.
- **The first start** on this build checks the whole history once.
- **The journal's rewrite** holds the store's lock while it writes and syncs, once per checkpoint.
- **App-share chains** are not compacted: R2-11 stays deferred, and a `ChainCheckpoint` names a
  `home` stream only. An app chain's checkpoint needs its share and zone key, and its head in
  `home`: AZ-12's two-record form (section 2).
- The legacy files of 4.1a and 4.1b stay, pruned by hand.

## Default-path changes

At part 3; parts 1 and 2 change nothing a node does, since no build makes a checkpoint before
part 3.

1. A node folds its `dir.claims` chain once 1,000 of its claims are superseded, in a renewal
   tick's save: about twice a day on the desk, and at the first start on this build.
2. Each fold prints `checkpoint: dir.claims folded at seq <B>, <M> superseded claim(s) dropped,
   <K> carried`, through the root's reporter.
3. records.json and the served store's `home` journals stay bounded. A journal is rewritten at
   each checkpoint it takes, the node's own and each peer's.
4. `PROTOCOL` is 4: a node of protocol 3 fails at connect.
5. Once a node has folded, a build from before this step refuses its instance.
6. 4.4's first restart promise reads: every acknowledged record, or its effect on every fold,
   survives a restart.

## What this changes for 4.5b and 4.6

- **4.5b.** Once the mesh rides `glade/carrier/1` (`iroh_carrier.rs:374-376`), the ALPN no longer
  names the node's protocol, and HELLO's `protocol` check (`peer.rs:174-175`) is the only version
  gate. 4.5b must keep it on the port, since this step's move to 4 rides on it. The serve order
  of section 7 goes into whatever serves `home` once 4.5b moves the sync driver onto link frames.
  Nothing else: no frame changes, and a push carries one more op at each checkpoint.
- **4.6.** Every claims fold answers as before (section 3), so "expired entries are excluded"
  reads the same claims. Both nodes run protocol 4. 4.6's "restart outcomes are honest" asserts
  4.4's restated promise. With growth bounded, the storage reason for F1's five-minute lease is
  gone, so 4.6's expiry check may shorten the lease, which is a setting, if it needs to (question
  9; plan `:931`). 5.1's gap "boot verification grows with lease renewals" (plan `:983`) closes,
  and this note's named gaps take its place.

## Questions for the owner

1. **AZ-12 for `home` chains** (section 2). Recommend one record, the `ChainCheckpoint` in the
   origin's own `home` chain, as both the checkpoint and its anchor. The alternative is AZ-12's
   two-record form, a checkpoint op in the compacted chain and a head record in `home`, which for
   a `home` chain duplicates the record and puts two kinds on one stream.
2. **What a checkpoint covers** (section 3). Recommend the whole prefix, folded to its state, the
   state carried forward above the base, with `dir.claims` the only stream given a rule, so only
   superseded claims leave. Alternatives: the originals kept below the base, with a list of those
   that stay (chains with holes); the state inside the record; every stream with a rule of its
   own (grants and revocations are the audit log B5 keeps).
3. **The trigger** (section 4). Recommend a count, at least 1,000 superseded claims, checked at
   every tick with adoption's included, as `Leases.checkpoint_after` from the entry point, with no
   flag, file line or environment read; tests set a small one. Alternatives: records.json's size,
   a daily time, boot only, every tick.
4. **The register** (section 6). Recommend that the newest checkpoint of a stream supersede the
   older ones, so records.json holds one and a node takes a newer one across a gap. The
   alternative keeps every checkpoint as an ordinary chain: forks between any two stay provable,
   and records.json grows by one record per checkpoint, about 12 a week.
5. **A peer behind the base** (section 7). Recommend that it drop its prefix unchecked.
   Alternatives: keep the prefix until it can be linked to the base, which it never can once the
   origin has dropped the op there; or refuse the checkpoint, and the chain stalls at that peer
   for good.
6. **`PROTOCOL` 4** (section 10). Recommend the move, so that an older node fails at connect,
   since one behind a floor could never follow the chain again. The alternative is 4.1c's way, no
   move: an older peer refuses the checkpoint, keeps the rest, and stalls once it falls behind a
   floor.
7. **The one-way moment on the desk** (section 10). Recommend automatic, at the first tick past
   the threshold, adoption's included, keeping no copy of what the first checkpoint drops.
   Alternatives: the first checkpoint only by a one-shot command, as with 4.1c's recovery key, so
   the owner picks the moment; or a `records.legacy-<date>.json` of the dropped claims, which a
   downgrade still could not use without the old journal.
8. **Contradictions as proofs** (section 7). Recommend refusing a checkpoint that contradicts the
   op held at its base, or whose base moves back, and keeping the pair in the proofs log. The
   alternative refuses and reports only, keeping no evidence.
9. **The leases once this lands** (section "What this changes for 4.5b and 4.6"). Recommend
   keeping F1's defaults, a five-minute lease renewed every 100 s, and letting 4.6's design pick
   what its expiry check needs. The alternative returns to 30 s and 10 s now: 17,280 claims a day,
   a fold every 1.4 hours at the default threshold, and a synced save every 10 s.
10. **The split** (section 12). Recommend three parts, about 250-330, 200-280 and 130-200
    production lines, each gated and replayed. The alternative merges parts 1 and 2, about 460
    lines.

**Ruled, owner, 2026-09-27 ("all recommended"):** 1 one record, `ChainCheckpoint`, is both the checkpoint and
its anchor in `home`; 2 a checkpoint covers the whole prefix with its state carried forward, and only `dir.claims`
is compacted; 3 the trigger is 1,000 superseded claims, a setting from the entry point checked at every tick; 4
`dir.checkpoints` keeps the newest checkpoint of a stream; 5 a peer behind the checkpoint drops its old records
unchecked; 6 `PROTOCOL` moves to 4; 7 the desk's first checkpoint comes automatically at the first tick past the
threshold, keeping no copy of the dropped claims; 8 a contradicting or backward checkpoint is refused and the pair
kept in the proofs log; 9 F1's leases stay, and 4.6 chooses what its expiry check needs; 10 three parts, in order.
