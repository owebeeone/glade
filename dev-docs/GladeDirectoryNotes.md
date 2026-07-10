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
