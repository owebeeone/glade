# System-data seam (GDL-036) — build notes, Lane R step 1

Implementation notes for the seam spec (`glial-dev` /
`dev-docs/glade/GladeSystemDataSeam.md`, RATIFIED 2026-07-07) as landed in
`glade/node/`. Records the small calls made where the spec left room. The
s-boot atlas trace (`ggg-viz/src/scenario/boot.ts`) is the executable spec;
nothing here changes an existing trace.

## What landed

| Piece | File |
| --- | --- |
| `SystemSnapshot` + WD §2 record kinds (taut) | `node/ir/sysdata.taut.py` → `node/src/sysdata.rs` (generated) |
| `StoreApi` (persistence seam) + `BlobStore`/`MemStore` engines | `node/src/registry.rs` |
| `RegistryApi` (queries-over-fold + attributed appends) + `Registry` fold | `node/src/registry.rs` |
| On-disk layout, profiles, instance lock, load-validation ladder | `node/src/sysdir.rs` |
| CLI: `--profile`/`--name`/`--operator`, boot-before-serve | `node/src/bin/glade-node.rs` |
| Conformance gate: blob engine ≡ mem (future-fold) engine | `registry.rs::blob_impl_equiv_future_fold_impl` |

## Resolved ambiguities (smallest reasonable call)

1. **`SystemSnapshot` home + record encoding.** The seam wants records to be
   the ONE wire `Op` (no second op type) *and* the snapshot to be a
   self-contained taut message, but the wire vocabulary lives in the separate
   `taut` repo. Resolution: `SystemSnapshot{records: [bytes], heads: [bytes]}`
   where each `records[i]` is a CBOR-encoded wire `Op` and each `heads[j]` a
   CBOR-encoded wire `StreamHeads`. Keeps a single `Op` type, keeps the schema
   inside the glade repo (mirrors `demo/ir/workspace.taut.py`), and is
   consistent with the substrate's own opaque-bytes layering. Op-granular sync
   later reuses the SAME record bytes.

2. **At-rest format of `records.json`.** Spec: "JSON is the at-rest rendering;
   hashes over records are canonical CBOR." For step 1 the file holds canonical
   CBOR of the `SystemSnapshot` (extension kept as `.json` per the trace/layout
   table). This makes at-rest == hashing-basis trivially uniform for
   verify-as-ingest. JSON-text rendering is a deferred cosmetic; the seam does
   not depend on it (nothing above `StoreApi` sees the bytes).

3. **Crypto is stubbed (M-LIMP), structure is real.** Matching the codebase's
   "security seams present but unenforced" posture: `NodeId = hex(sha256(node.key))`
   (a deterministic stand-in for the ed25519 pubkey); the class-3 `local.json`
   self-signature is structural. REAL: the `node.key` permission check (refuse
   group/world-readable), the class-2 per-origin chain verification (shared with
   the live wire store), and the fail-closed load ladder shape. The crypto swaps
   in without touching the seam.

4. **`grants_for` revocation granularity.** "Set-union; revocation wins" is
   implemented at `(principal, share)` granularity: a matching
   `CapabilityRevocation` clears all verbs for that pair (policy fails closed).
   Per-verb attenuation is a later refinement behind the same query.

5. **`instance.lock` is advisory.** O_EXCL create + remove-on-Drop (the
   `workspace.lock` precedent, WD §4: "filesystem lock is ground truth"). A
   crash / SIGTERM-kill leaves a stale lock for a human to clear — a graceful
   SIGTERM handler (release on shutdown) and dead-PID reclaim are follow-ups,
   out of step-1 scope.

6. **`operator` identity.** Passed via `--operator` (default `"local"`); the
   real principal/operator model is owned by the security analysis. It only
   labels this node's `NodeRecord` for `nodes_of(operator)`.

7. **Sysdir boot is OPT-IN — the legacy serve form is unchanged.** (Regression
   fix: the first cut booted unconditionally, colliding concurrent test spawns
   on the global `~/.glade/sys/glade-local/instance.lock` and writing the real
   `$HOME` — breaking the grip-share integration suite.) The two forms:

   | Invocation | Sysdir boot | Touches on disk |
   | --- | --- | --- |
   | `glade-node <port> [store_dir]` (legacy, no flags) | none | only `store_dir` (default: a temp dir); NEVER `~/.glade` |
   | `glade-node --profile P [--name N] [--operator O] [port] [store_dir]` | yes | `$GLADE_HOME` else `$HOME/.glade`, at `sys/<name>/` (+ `store_dir`, default `sys/<name>/cache/store/`) |

   `--name` alone also opts in (profile defaults to `local`). Tests that boot
   set `GLADE_HOME` to a temp dir; the legacy form needs no isolation because
   it never looks at the sysdir at all.

## Regenerating `sysdata.rs`

Never hand-edit the generated file. From the gwz workspace:

```
cd taut && PYTHONPATH=src python3 -m taut.cli gen \
    ../glade/node/ir/sysdata.taut.py -o /tmp/sysdata-gen -l rust --api-only
cp /tmp/sysdata-gen/rust/api.rs ../glade/node/src/sysdata.rs
```

The generated file emits `use crate::cbor::Cbor;`; `node/src/lib.rs`
re-exports the wire crate's runtime as `crate::cbor` (`pub use glade_wire::cbor;`)
so it resolves against the one proven codec, no second copy.
