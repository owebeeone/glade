# Glade directory minimal — engineering notes (Lane R step 3)

The s-discovery golden path made real: workspace listing from the local
replica (`dir.workspaces`), claim-based routing of a subscribe to the serving
node, and the timeout/absence case. Executable spec:
`ggg-viz/src/scenario/discovery.ts` phases A/C/E (DO NOT modify; the
gwz-exchange phase D and grazel-attach parts of C belong to Lane R4). Design
refs: `dev-docs/glade/GladeWorkspaceDirectory.md` (GDL-032, RATIFIED),
GDL-038 (management surface = ordinary bindings), `GladePeerSyncNotes.md`.

Normative language per AGENTS.md: MUST / SHOULD / MAY.

## Scope

IN: the peer accept loop wired into the node (`mesh.rs` — R2 left the carrier
as a library capability); home-share convergence at connect (a pull each way);
`dir.workspaces` served to client sessions through the ORDINARY subscribe path
from the node's own replica; the C2 claim-routing decision + interest
forwarding to the claim holder; the absence case as STATUS data. OUT: grazel
attach + gwz EXCHANGE forwarding (Lane R4); iroh discovery/pkarr (the trace's
E3/E4 "try the last host anyway" step); claim renewal/takeover + workspace-lock
fencing (WD P4.S2); cross-node LIVE directory updates after connect (see #7);
crypto (still the stubbed-but-structure-real seams).

## How dir.workspaces is addressed

`share = "home"`, `glade_id = "dir.workspaces"` (`registry::HOME`,
`registry::G_WORKSPACES`), key = ∅ (the commons zone), shape = log; payloads
are taut `WorkspaceEntry` records, ops origin-attributed to the writing node.
A subscribe rides the SAME `Subscribe`/`Heads`/`Ops` frames as any share
(GDL-038: reads are subscriptions to system shares — no registry RPC exists).

## The routing decision (C2), in order

Per subscribe, judged at the reader's clock (`who_serves` evaluates
`lease_expiry_ms > now` at read time; time never enters the fold):

1. no mesh enabled (legacy `glade-node <port> [dir]`) → **local** — the
   pre-mesh contract, behaviorally unchanged;
2. `share == home` → **local** (every node replicates the directory);
3. live claim held by self → **local**;
4. live claim held by a peer with a live link → **forward** the interest;
5. live claim, no link to the holder → **absent** ("claim holder unreachable");
6. no live claim but the directory KNOWS the share (a `WorkspaceEntry` or any
   lapsed claim) → **absent** ("no live ServeClaim") — the lease-lapse-at-read
   case, trace E2;
7. share unknown to the directory → **local** (plain app-share serving).

Absent = an `Error` frame, `code = UnknownShare`, the reason in `message`,
`share`/`glade_id` echoed — the trace's STATUS step (E5): data with a reason,
never a hang; the session stays usable.

## Ambiguities and smallest-reasonable resolutions

1. **Where the served directory lives.** The boot `Registry` (records.json)
   and the served `Store` (op journal) are two engines. Resolution: at node
   assembly the boot registry snapshot is SEEDED into the served store
   (`Server::seed_registry`, idempotent — same ops, same chains), and the
   STORE is the runtime authority the subscribe path and the routing fold read.
   Runtime-arriving directory ops land in the store journal only; on reboot
   the registry re-seeds its (boot-time) ops and the journal still holds the
   rest — no divergence, because both are the same attributed op-chains.
   Collapsing to one engine is the WD P2 fold-backed RegistryApi follow-up.

2. **One identity, two renderings.** Directory records carry
   `hex(sha256(node.key))`; the peer HELLO carries raw `sha256(key)` bytes.
   R2 derived the HELLO identity from the IROH key, which would never match a
   folded `ServeClaim.node`. Resolution: a booted node binds the endpoint with
   `PeerEndpoint::bind_with(Boot::identity())` — the HELLO identity derives
   from `node.key`, the iroh key stays transport-only. Claim routing looks a
   folded node id up in the live-links map keyed by the HELLO'd id.

3. **Connect-time sync scope.** `serve_sync` (R2) offers EVERY zone the store
   holds; wiring that raw would bulk-replicate app content at connect and
   bypass claim routing entirely. Resolution: connect-time anti-entropy is
   scoped to the HOME share (the directory replicates everywhere, WD §3
   ladder 1); app-share content moves by INTEREST only. The mesh's serve side
   filters `store.zones()` to `home` — exactly the drop-in filter point
   GladePeerSyncNotes §Scope named for ACL zone-filtering. The R2
   `serve_sync`/`pull_sync` primitives are unchanged (their tests stand); the
   mesh reimplements the same heads/gap loop with two node-level differences:
   home scoping, and ingest fans out to local subscribers.

4. **Stream anatomy per connection.** HELLO on stream 0 (the R2 seam), then
   stream 0 becomes the dialer's home pull; the acceptor opens its own pull
   stream (convergence = a pull each way, GladePeerSyncNotes §4); every later
   stream is dispatched by its FIRST frame: `Heads` = a sync pull to serve,
   `Subscribe` = a forwarded interest. No new frame types; the wire IR is
   untouched (taut not touched).

5. **Forwarded interest = a Subscribe with `from` heads.** The A-side sends
   the existing `Subscribe` frame with `from = its replica's heads` (a wire
   field that existed unused); the holder registers the peer stream as an
   ORDINARY subscriber session (router + out-map), ships ack + gap against
   `from`, and the normal fan-out feeds it live — C3's "the keyed entry map IS
   the routing table". Ack + gap ride the same outbound queue as fan-out so a
   live op can never overtake the resume gap. Ingest on the A side is scoped
   to the subscribed zone (the holder can't push other zones down the stream),
   lands through the same chain checks as any carrier, and local subscribers
   are fed from the REPLICA (C5→C6). One stream per zone regardless of local
   subscriber count (`forwarded` dedup); the forward lapses with the stream
   and the next subscribe retries.

6. **Absence rides `Error`/`UnknownShare`, not a new STATUS frame.** The trace
   draws a STATUS frame; the wire has an `Error` frame with `share`/`glade_id`
   context and no fitting-but-distinct status type. Riding the existing frame
   avoids a wire IR change under the "prefer existing frames" constraint. If a
   richer status surface is wanted (e.g. last-eligible-host, retry hints), that
   is a deliberate taut addition later.

7. **Directory liveness across nodes stops at connect.** The home share
   converges when a link is established (and continuously WITHIN a node via
   ordinary fan-out — a pull ingest reaches live `dir.workspaces`
   subscribers, the B9 step). A directory record written on B AFTER the link
   is up does not flow to A until a re-pull. The natural follow-up is standing
   home-share interest over the same forwarded-subscribe mechanism (or
   periodic anti-entropy); deferred — the golden path's observables don't
   need it.

8. **Trace E3/E4 (resolve + dial the last host anyway) are skipped.** They
   require iroh discovery (pkarr/DNS), which the carrier deliberately disables
   (`presets::Minimal`, GladePeerSyncNotes §3). The absence answer is produced
   directly from the fold (E2) and surfaced as E5's STATUS. Same observable
   shape at the client, minus the speculative dial.

9. **`--peer <endpoint-id>@<ip:port>`** is the bin's dial form (repeatable,
   booted profiles only); a booted node prints `peer <endpoint-id> <ip:port>`
   as its own dial target. Any booted profile binds the endpoint and accepts —
   profiles stay deployment labels, not protocol types; the legacy positional
   form never binds a peer endpoint and never touches `~/.glade`.

## Live minting (GLP-0006 P0.S2 — audit F1+F2 fused)

The audit's F1: nothing production minted `WorkspaceEntry`/`ServeClaim` — only
tests did, via the registry API. Closed by `node/src/claims.rs`.

**The workspace↔share association is a `.glade` declaration**:
`workspace <share> <name>` (see `apps/grazel-app.glade`). Chosen over a CLI
flag per GDL-037 — the file IS the legible app surface, and the plan's stage-1
idiom (P1 chat groups PRE-DECLARED in grazel-app.glade) is the same shape.
Registration mints an ordinary `WorkspaceEntry` with the REGISTRANT as the
eligible host: whoever loads the file serves it — deployment picks the loader,
the file stays data, nothing hardcodes grazel. Two nodes loading the same file
each append their own entry; `replicas_of` stays LWW (latest entry wins) —
the eligible-hosts union is an open fold question, unchanged here.

**Serving**: the booted bin, after registering `--app` files, calls
`serve_workspace(share, name)` for each declared workspace — mints the first
`ServeClaim` (epoch = the SERVED replica's max for that share + 1, so a
restart or takeover fences out any stale claim) and RENEWS on a cadence
(TTL 30s, renew 10s; `adopt_boot_tuned` shortens both for tests). A renewal is
an ordinary ServeClaim append — data, never a heartbeat protocol.

**adopt_boot**: the server adopts the boot instance; the boot `Registry`
stays the single chain authority for this node's own directory writes, so
records.json always matches the journal and later boots can never fork the
chains the runtime extended (renewals persist as ordinary records; the
records.json growth this implies is a GAP-10 retention question, noted, not
solved). The instance lock lives as long as the server.

**Push (B9)**: directory records minted AFTER connect-time anti-entropy reach
peers by a home-scoped Ops push on a fresh link stream (`mesh::push_home` →
the `Frame::Ops` opener arm, home ops only, scoped ingest). Self-minted
records only; receivers never re-push — transitive gossip deferred. Best
effort: a lost push (or a push racing ahead of the connect pull) is dropped by
the chain-gap check and healed at the next connect-time pull.

## Target-routed creation (audit F2 — s-create D1–D3, BUILT per P00-c)

`workspace.create` is a RESERVED system glade id the NODE answers itself,
never a supplier (the glade-sys direction, GDL-038 — wired built-in; the
`glade-sys.glade` file itself is still not needed). Creation is the one routed
operation that cannot consult a ServeClaim: it MAKES the thing claims will be
about, so the request names its TARGET node explicitly.

**The wire is untouched.** `ExchangeReq` has no target field and needs none:
the target rides the opaque exchange payload as a node-local taut message
(`WorkspaceCreateReq {workspace, name, target}` /
`WorkspaceCreateRes {workspace, node, created}` in `node/ir/sysdata.taut.py`,
regenerated with `--legacy-codec`). Routing in `exchange::handle_create`:
target == self → perform locally (`claims::create_workspace` = the same
`serve_workspace` ceremony: mint entry + claim under our own origin, join
renewal); target == a linked peer → forward the frame unchanged over the peer
link (corr preserved 1:1; the target's `serve_peer_exchange` re-enters the
handler and hits the self arm); anything else → `ExchangeRes{ok:false}` with
the reason — an unlinked target fails as DATA. Re-create is idempotent by
diff: `created:false`, no new entry, no epoch bump.

**The gwz-core seam**: disk materialization (manifest + member clones +
workspace.lock, trace D3) is deliberately EXTERNAL — the ceremony creates the
glade-side records only; grazel/glade-gwz hooks gwz-core around it (P1). The
lock-precedes-claim discipline therefore binds at that layer, not here.

Resolved smallest-faithful calls: the create payload's cross-language shape
(a TS client minting creates) waits for sysdata IR to grow a TS target —
node-side cargo E2E is the stage-1 gate; malformed create payloads share the
codebase-wide fail-open legacy-codec posture (empty payload IS guarded; the
fail-closed migration lands with taut v0.10); stage-2 asks which principals
may create on which nodes (GDL-016) — the check slot is `handle_create`.

## Principals minimal (GLP-0006 P0.S7 — identity, NOT management)

`Hello.principal` (a wire field frozen since P1, unused until now) is honored:
a session whose Hello names a principal is BOUND to it (`Shared::principals`,
session-scoped — the attribution seam P1 suppliers read); a session without
one keeps origin-as-identity, byte-for-byte (grip-share's suite is the
regression). An UNKNOWN principal auto-appends a minimal
`PrincipalRecord {principal}` to `dir.principals` (the stream GDL-038 names)
as an ordinary origin-attributed append through the same adopt_boot authority
as every other directory mint, served through the ordinary subscribe path (the
R3 precedent) and pushed to peers like any home record. Stage-1 posture:
identity as DATA, nothing enforced — lifecycle (enroll/attenuate/revoke) is
P2/glade-users and deliberately NOT smeared into this record; richer fields
arrive with that ceremony, not here. Store-only (legacy) nodes bind nothing
and mint nothing: there is no node chain to attribute the record to.
`client-ts` grows an optional `hello(principal?)` (resolves on Welcome);
`connect()` still sends nothing by default.
