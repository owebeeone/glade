# GC-3 per-binding cutover notes (Lane T step 3b)

Working log for the M-LIMP binder migration ruled 2026-07-10 (GC-3,
`glade-wz/dev-docs/glial/GlialClientRuntime.md`): each demo binding moves from
the grip-share binder's direct tap↔glade coupling to a glial mount
individually, verified per binding; grip-share shrinks binding-by-binding until
its session coupling is deleted. NO compatibility shim / strangle layer.

## The binding list (from demo/src/manifest.ts)

| # | glade id | shape | domain/zone | payload codec | write path today |
| - | --- | --- | --- | --- | --- |
| 1 | `app:status` | value | account/commons | JSON (Text) | `STATUS_TAP.set` |
| 2 | `app:notes` | value | doc/commons | JSON (Text) | `NOTES_TAP.set` |
| 3 | `app:selection` | value | doc/private | JSON (Text) | `SELECTION_TAP.set` |
| 4 | `app:activity` | log | doc/commons | taut `ChatLine` | `postActivity` → `binder.appendLog` |

Cutover order: 1 → 2 → 3 → 4 (simplest value surface first; the typed log
last), then the grip-share binder deletion.

## Wire-byte compatibility (the invariant, per binding)

A cut-over binding must produce byte-identical ops: same `(share, key)` (from
the SAME `manifestScope`), same payload bytes (same codec: JSON default / taut
ChatLine), same op chain (the SAME client-ts `Session` mints them). Evidence is
a per-binding interop test in `grip-share/test/glial_cutover.test.ts`: a
binder-era participant and a glial-era participant converge BOTH directions
through the real rust node, plus absolute payload-byte asserts.

## Decisions / ambiguities hit

- **`link:` not `file:` for `@owebeeone/glial-runtime`.** pnpm's `file:`
  protocol copies the package into `.pnpm`, which (a) breaks glial's own
  relative `file:../glade-decl-ts` dev link and (b) puts glial's `.ts` sources
  under a `node_modules` realpath, where node's `--experimental-strip-types`
  refuses to strip. `link:` symlinks to the real directory: glial resolves its
  own deps, and its sources stay strippable/type-checkable. Same for
  `@owebeeone/glade-decl` (type-only import in the demo). Resolved within the
  glade repo's package.jsons only; glial untouched.
- **demo lockfile:** `package-lock.json` (npm era) replaced by pnpm's
  `pnpm-lock.yaml` — the repo rule is pnpm-only going forward.
- **Pre-existing red:** the demo did not type-check at the starting commit
  (4d190a2): grip-core's dist types `ShareDecl.shape`/`domain` as the
  glade-decl unions (step 3a era) while grip-share's structural `ShareDecl` is
  stringly. The demo predates that retype. Interim fix: one `as never` at the
  `share()` helper in taps.ts — the helper dies with the cutover.
- **One shared session during the transition.** glial's `SessionDestination`
  and the shrinking grip-share binder must mint ops on the SAME client-ts
  `Session` (same origin chain, one resync source). `demo/src/glial.ts` owns
  session/client/bus; glade.ts's binder borrows them until it dies. Both apply
  inbound ops to the session (SessionDestination for its route, binder for
  its); `Session.applyRemote` dedups, so the double-apply is harmless and
  temporary.
- **The `OpBus` carrier adapter** (`ClientBus` in demo glial.ts / test helper):
  `publish → client.sendOps`, `client.onOps → deliver`. This is glial's own
  seam ("the WS carrier in production"), not a compat shim — it survives the
  cutover as the production carrier wiring.
- **BindingDecl anchors:** the manifest's domain names map to glade-decl
  `DomainAnchor`s as data: `doc → document`, `account → account`. The fill
  carries the concrete ids (`doc`/`user`; private zone key = `user`).

## Per-binding log

- **1/4 `app:status`** — glial mount via `glialSurface()` in taps.ts
  (decl/fill/codec/dest all manifest-derived data). Interop test
  `glial_cutover.test.ts` "cutover 1/4": binder-era tab and glial-era tab of
  the same user converge both directions through the real node; the glial op's
  `(share, key, shape, payload)` asserted byte-identical to the binder era
  (JSON payload bytes). Live demo check: browser (glial) wrote
  `account:alice`/`app:status`; a headless client-ts probe read it back and
  wrote a reply that appeared in the browser input. Note: grip-share's test
  runner moved `--experimental-strip-types` → `--experimental-transform-types`
  (glial's `SessionDestination` uses TS parameter properties, which strip-only
  mode rejects; transform mode is the same built-in loader).
- **2/4 `app:notes`** — same pattern (doc domain, commons). Interop test
  "cutover 2/4": a LATE glial joiner hydrates binder-era stored bytes off the
  node's subscribe replay, then both directions converge live; byte assert on
  the glial op. Test-helper lesson: mount BEFORE subscribe (the demo's order —
  registerAllTaps then startGladeSync), else the subscribe replay races the bus
  handler. Live demo: reloaded page folded the pre-cutover store value
  (existing-store compat), browser→probe and probe→browser both converged.
- **RELOAD-RESUME GAP found during 4/4 (live-demo verification, 2026-07-10).**
  With the binder's `applyRemote` gone, a tab reload (same stable origin,
  fresh session) broke the README-promised "reload a tab and the node resyncs
  it" in TWO ways: (a) the participant's OWN prior state no longer displayed
  (glial's `SessionDestination` echo-guards own-origin ops out of the node
  replay, and the in-memory store engine starts empty), and (b) the fresh
  session restarted its chain at seq 0 — the node correctly treats the next
  write as a forked chain and DROPS it (observed live: a `selected README.md`
  op silently lost). The binder era masked both by applying every inbound op
  to the session. Resolution — two DESIGNED seams, no glial edit, no
  grip-share coupling:
  1. carrier wiring (demo glade.ts): `client.onOps` feeds EVERY inbound op to
     the session before the bus — truthful heads/resume vectors + own-chain
     resume (session store dedups; SessionDestination's per-route
     `applyRemote` double-apply is harmless);
  2. a demo-grade `BrowserStoreEngine` (sessionStorage, per-tab) in glial's
     GC-4 `StoreEngine` slot — rule-1 persistence proper, so own state
     survives reload locally.
  Regression test: glial_cutover "reloaded tab resumes its chain" (asserts
  seq continues at 1 and a witness converges to the post-reload write).
  **Glial should own this natively** (report filed with the cutover): a
  session-hydration story for `attachGlade`/SessionDestination (own-origin
  replay must reach the session it minted from, even though it must not
  re-fold into the instance) + the real GC-4 persistent engine. Residual until
  then: offline-from-boot writes after a reload (no node replay to hydrate
  seq) can still fork — same hole the binder era had.
- **3/4 `app:selection`** — private zone: the `self:{user}` key rides the
  glial route (from the same manifest scope). Interop test "cutover 3/4":
  binder-era alice + glial-era bob each pick privately, isolation holds both
  ways, key bytes asserted identical to the binder era (`self:bob` utf8) +
  JSON payload bytes. Live demo: alice's (glial) pick landed under
  `self:alice` on the node; a bob probe's pick stayed under `self:bob`;
  neither crossed. (Browser-driving aside: preview_click didn't reach React's
  onClick; element.click() via eval did — app behavior itself fine.)
- **4/4 `app:activity`** — the typed log: taut ChatLine codec rides the glial
  mount (`codecFor`); `postActivity` writes through the glial controller
  (`ACTIVITY_TAP` handle grip, `append` = one op per entry); the demo-side
  grip-share binder wiring is gone (glade.ts no longer imports the binder).
  Interop test "cutover 4/4": binder-era and glial-era participants interleave
  ChatLines through the real node to the same ordered decoded list; glial op
  payload asserted byte-identical to `taut encode(ChatLine)`. Live demo: 22
  binder-era taut entries hydrated + decoded; post + selection-click entries
  landed (chain seq 0→1→2 across three page lives, verified in the node op
  dump); probe append appeared in the browser.
- **Pre-existing (not introduced here, not fixed):** two tabs of the SAME
  browser profile share the localStorage `glade-origin`, so concurrent writes
  from both tabs can fork that origin's chain — binder era had the identical
  hole (the README's two-tab flow works because replay usually hydrates before
  a write, and distinct surfaces have distinct chains).
