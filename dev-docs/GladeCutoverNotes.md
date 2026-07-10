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

- (pending)
