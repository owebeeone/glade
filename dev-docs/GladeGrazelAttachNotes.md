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

Line-oriented text, hand-parsed, zero new deps:

```text
glade-app v0                                        # header, first decl line
app grazel                                          # exactly once, first
binding <glade_id> <shape> <authority> <zone> <retention>
service <name> <exchange-glade-id>
seed <principal> <share> <verb[,verb...]>
# comments + blank lines anywhere; `#` starts a comment
```

Why this over JSON/CBOR-of-a-taut-message: the file is the LEGIBLE app
surface (GDL-037's surviving de-noising value) and is hand-edited, so
line-diagnostics and diff-friendliness are load-bearing; the node has no JSON
dep and the wire discipline is zero-dep; and the file is a *rendering* only —
what registers is taut records (`BindingDecl`/`ServiceDefinition` in
`node/ir/sysdata.taut.py`), so the cross-language contract lives in taut, not
in this text form. A structured rendering can replace it later without
touching anything downstream of `parse()`. Validation: unknown shape /
authority / directive, duplicate glade id (frozen-once-shared, GQ-6), and
missing header/app are refused with line numbers.

`BindingDecl` is app-static: no share/key in the record — the ServeClaim
selects the node, the mount fills domain/zone/key (GladeDeclSurface). shape /
authority / zone / retention ride as STRINGS so the record evolves additively.

## Registration (s-app-register RL/RC)

`appdecl::register(decl, registry, origin)` appends each declaration as an
ordinary home-share record on `dir.bindings` / `dir.services`, and COMPILES
each seed to a `CapabilityGrant` on `dir.grants` — the same record kind
s-grant appends by hand, under the REGISTRANT's chain. Idempotence is by DIFF:
a record whose (glade_id, payload bytes) already exist in the fold is skipped,
so re-loading appends nothing and can never clobber a later runtime revocation
(`reregistration_cannot_clobber_a_runtime_revocation` is the regression).
Nothing in base glade names grazel; the loader registers any app
(`registration_appends_ordinary_attributed_records` uses a non-grazel app).

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
whose glade id is DECLARED an exchange surface (a `dir.services` record, or a
`dir.bindings` record with shape `exchange`, folded from the served store).
The node registers the session in `Shared::providers` and acks with an empty
`Heads` — "the keyed entry map IS the routing table" applied to the directed
leg; no new frame. On disconnect the provider entry drops with the session.

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
   yet; retention/timeout policy per declaration is a decl-surface question.

8. **`who_serves == self` requires a booted mesh.** On a mesh-less (legacy)
   node every declared exchange routes Local — the provider map alone decides.
   Unchanged legacy behavior: nothing is declared on a legacy node anyway.
