# Grazel attach — engineering notes (Lane R step 4)

The s-app-register trace made real, plus the gwz-exchange leg R3 left behind:
`grazel-app.glade` LOADED as runtime data (GDL-037), registration compiling
its ACL seeds to ordinary grant records, an authority session serving a real
declared binding, and a directed gwz exchange routed to the authority per the
fan-out-asymmetry rule. Executable specs: `ggg-viz/src/scenario/register.ts`
(s-app-register), `discovery.ts` phase D + the timeout posture, `fanout.ts`
(s-fanout-exchange). Design refs: GDL-037/038 (ratified),
`dev-docs/glade/GladeDeclSurface.md`, `GladeDirectoryNotes.md` (R3).

Normative language per AGENTS.md: MUST / SHOULD / MAY.

## Scope

IN: the `<app>.glade` file format + parser/validator (`node/src/appdecl.rs`);
registration as diff-idempotent attributed record appends (BindingDecl /
ServiceDefinition / seed→CapabilityGrant); the `--app` flag on the booted bin
form; authority-provider attach + EXCHANGE routing (local, forwarded, absent);
the E2E. OUT: capability ENFORCEMENT on exchange/provider attach (the seams
stay stub-allow-all, matching every other gate); claim renewal/takeover;
channel (`ChannelOpen/Data/Close`) routing to providers (still echo);
glade-sys.glade (base glade's own app file — nothing needs it yet).

## The `<app>.glade` format (the serialization call)

Line-oriented text, hand-parsed, zero new deps. The user-facing page for the
format is [`glade/docs/AppFileFormat.md`](../docs/AppFileFormat.md), which
carries the same grammar and what each token means:

```text
glade-app v0                                        # header, first decl line
app grazel                                          # exactly once, first
binding <glade_id> <shape> <authority> <zone> <retention> [ttl=<duration>] [shape-profile=<profile>]
service <name> <exchange-glade-id>
seed <principal> <share> <verb[,verb...]>
workspace <share> <name>                            # makes declared surfaces routable
# comments + blank lines anywhere; `#` starts a comment
```

A binding line's tail (R11(a)) is optional `key=value` entries after its five
tokens, in any order: `ttl=<duration>` (a whole number above zero and one unit
of `ms`, `s`, `m`, `h` or `d`, such as `ttl=10m`, on a line whose retention is
`ttl`) and `shape-profile=<profile>` (`text_crdt`, required on `crdt`;
`snapshot_delta`, optional on `swmr`). An author learns the tail exists only
from a grammar like this one: a valid five-token line is never refused, so the
node's template, which shows the tail, never reaches an author whose line is
right.

The in-file grammar comment is carried by the three authored app files
(`grazel/apps/grazel-app.glade`, its byte-identical twin
`glade/apps/grazel-app.glade`, and `grazel/apps/gyld-app.glade`) and not by the
two test fixtures (`glade-gyld/tests/fixtures/gyld-test-app.glade`,
`glade-gwz/tests/fixtures/gwz-test-app.glade`).

Why this over JSON/CBOR-of-a-taut-message: the file is the LEGIBLE app
surface (GDL-037's surviving de-noising value) and is hand-edited, so
line-diagnostics and diff-friendliness are load-bearing; the node has no JSON
dep and the wire discipline is zero-dep; and the file is a *rendering* only —
what registers is taut records (`BindingDecl`/`ServiceDefinition` in
`node/ir/sysdata.taut.py`), so the cross-language contract lives in taut, not
in this text form. A structured rendering can replace it later without
touching anything downstream of `parse()`. Validation: unknown shape /
authority / directive, duplicate glade id (frozen-once-shared, GQ-6),
missing header/app, and a malformed tail (an entry that is not `key=value` or
stands where a positional token goes, an unknown or repeated key, a bad
duration or profile, `ttl=` without the retention `ttl`, a profile that does
not fit the shape, `crdt` without one) are refused with line numbers.

`BindingDecl` is app-static: no share/key in the record — the ServeClaim
selects the node. The author writes the zone (the binding line's `<zone>` token,
which has no default), and a mount does not override it: on the grip-share path
`manifestScope` reads `decl.zone ?? spec?.zone ?? ""`, so the declared zone wins
and the manifest's surface spec is only a fallback; on the glial path the
mount's zone fill enters only a local instance key and never reaches the wire
(GladeDeclSurface, `Domain` / `Zone`). shape / authority / zone / retention ride
as STRINGS so the record evolves additively. The tail is parse data and rides
no record: a new `BindingDecl` field would change the bytes of every stored
binding record, so a key that must reach a consumer takes a record kind of its
own, keyed by glade id (the contract's `ShapeProfileDecl`).

## Registration (s-app-register RL/RC)

`appdecl::register(decl, registry, origin)` appends each declaration as an
ordinary home-share record on `dir.bindings` / `dir.services`, and COMPILES
each seed to a `CapabilityGrant` on `dir.grants` — the same record kind
s-grant appends by hand, under the REGISTRANT's chain. Idempotence is by DIFF:
a record whose (glade_id, payload bytes) already exist in the fold is skipped
(a binding: against the live fold, per `(app, glade_id)`, below),
so re-loading appends nothing and can never clobber a later runtime revocation
(`reregistration_cannot_clobber_a_runtime_revocation` is the regression).
Nothing in base glade names grazel; the loader registers any app
(`registration_appends_ordinary_attributed_records` uses a non-grazel app).

**Changed and deleted lines (R9 (b2) + (a), ruled 2026-09-23).** `parse()`
normalises `from-cursor` to `from_cursor` on the way into the record, so the
store holds the contract's spelling whichever spelling the file uses.
`dir.bindings` is folded by `glade_id`, newest wins (`BindingFold` in
`node/src/registry.rs`, read by `RegistryApi::bindings_of` and by
`declared_exchange` in `node/src/exchange.rs`), and `register` diffs the
parsed file's bindings against that fold per `(app, glade_id)`: a changed line
appends its new `BindingDecl`, and each binding the file's `app` registered
before and no longer declares gets a `BindingRetraction` on
`dir.binding-retractions`. "Newest" is the highest `(lamport, origin)` across
both streams, which the registry keeps as one lamport clock over the two, so a
line put back after a retraction declares its surface again; and a retraction
takes down only its own app's declaration. So a file not loaded on a boot
retracts nothing (gyld's surfaces stay declared while grazel's gyld leg is
off), and a declaration made by a different app file is never in scope. R9
governs `dir.bindings` only: deleting a `service` or `workspace` line retracts
nothing, so a retired exchange stays routable, and `seed`'s remove half is a
runtime revocation (item 4 below). A client that folds `dir.bindings` itself
must fold `dir.binding-retractions` with it the same way; none does yet.
`glade/docs/AppFileFormat.md` states the same rule for authors.

## Resolved ambiguities (smallest reasonable call)

1. **`sysdata.rs` regen uses `--legacy-codec`.** taut ≥ v0.8.0 defaults to the
   fail-closed Rust codec (`from_cbor -> Result`, needs `cbor::DecodeError` /
   `try_*`), which `glade-wire`'s frozen cbor runtime does not expose — and
   wire-rs is read-only for this step. `--legacy-codec` reproduces the exact
   pre-v0.8.0 style already in tree (additive diff only). The flag is removed
   at taut v0.10.0: migrating sysdata.rs to the fail-closed codec (with the
   wire runtime growing `try_*`) is a follow-up owned by the wire/corpus gate.

2. **Where the file lives: `glade/apps/grazel-app.glade`** — outside `node/`
   on purpose (base glade is app-agnostic; the file is DATA the bin points at
   via `--app`), inside the repo so the E2E and the demo path can load it.

3. **ServiceDefinition = `{app, name, glade_id}`.** The trace shows "1
   service" without fields; the minimum that lets routing work is the exchange
   glade id the provider answers. Instantiation/launch config is deliberately
   absent (ephemeral endpoint management is base-glade record-driven work,
   not stage 1).

4. **Seeds registered even if identical grant was revoked.** The diff skips
   only byte-identical records; `grants_for` applies revocation-wins at
   `(principal, share)` regardless of order, so even a NON-identical re-seed
   cannot resurrect access. Fold authority holds both ways.

## The exchange leg (discovery.ts phase D, fanout.ts asymmetry)

No wire change: `ExchangeReq`/`ExchangeRes` frames existed frozen since P1
(taut NOT touched). What was missing was routing — the server echoed every
exchange locally.

**Provider attach.** An authority session SUBSCRIBEs to a `(share, glade_id)`
whose glade id is DECLARED an exchange surface: a `dir.services` record, folded
from the served store, whose authored form is a `service` line. (A
`dir.bindings` record with shape `exchange` would also count — the fold honours
one if it exists — but it is not authorable: the parser refuses
`binding … exchange …` and names `service` instead.) The node registers the
session in `Shared::providers` and acks with an empty `Heads` — "the keyed
entry map IS the routing table" applied to the directed leg; no new frame. On disconnect the provider entry drops with the session.

**Request routing** (`ExchangeReq`), in order:

1. glade id NOT declared an exchange surface → the legacy echo provider
   answers (the pre-R4 contract, byte-for-byte — grip-share/demo unaffected).
2. Declared: the C2 decision (`route_subscribe`) judges the SHARE —
   - **Local** (claim held by self / mesh-less): look up the provider; found →
     forward the frame to it, remember `corr → requester` in
     `Shared::pending`; none attached → `ExchangeRes{ok:false, error}` NOW.
   - **Forward(peer)**: open a fresh stream on the claim-holder's link, send
     the `ExchangeReq`, await the `ExchangeRes` bounded (10s) → relay to the
     requester; timeout/link-drop → `ExchangeRes{ok:false, error:"timeout…"}`.
   - **Absent(reason)** (no live claim / holder unreachable) →
     `ExchangeRes{ok:false, error:reason}` immediately.
   The replica NEVER answers a declared exchange — even with a warm cache of
   the share's streams (the s-fanout-exchange rule).

**Response routing.** An inbound `ExchangeRes` from any session resolves
`pending[corr]` and is delivered to the recorded requester; unknown corr is
dropped. On the holder side a forwarded exchange gets a synthetic session id
whose outbound channel IS the QUIC stream, so provider→requester delivery is
the same `pending` lookup everywhere.

**Failure = data, never a hang** (the E-phase posture): every arm answers with
an `ExchangeRes{ok:false}` carrying the reason and the correlation id — the
session stays usable, mirroring R3's `Error/UnknownShare` call for subscribes
(exchanges have a response frame with an error slot, so absence rides IT).

### Exchange-leg ambiguities

5. **Correlation ids are node-scoped.** `pending` is keyed by `corr` alone;
   two live requests with the same corr on one node would collide (last one
   wins). The trace treats corr as 1:1 and preserved; per-session namespacing
   is a wire-visible question (does the forwarded corr get rewritten?) —
   deferred, noted here rather than invented.

6. **Provider attach is last-writer-wins, unauthenticated.** A second session
   subscribing to the same exchange surface replaces the provider entry; any
   session may attach. This is the stub-allow-all posture every seam has —
   the capability check slot exists at SUBSCRIBE (C3's gate) and covers this
   the day enforcement lands.

7. **Exchange timeout is a node constant (10s).** Not declared per-binding
   yet; a per-declaration timeout is still a decl-surface question. Retention
   per declaration no longer is: it is the binding line's `<retention>` token
   (`latest` / `from-cursor` / `ttl`), glossed in `glade/docs/AppFileFormat.md`
   and in the `Retention` row of `dev-docs/glade/GladeDeclSurface.md`, and
   declarative — nothing enforces it yet (GC-4).

8. **`who_serves == self` requires a booted mesh.** On a mesh-less (legacy)
   node every declared exchange routes Local — the provider map alone decides.
   Unchanged legacy behavior: nothing is declared on a legacy node anyway.
