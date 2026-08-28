# Glade SWMR adapter v1

Status: **Built 2026-08-29** — GLP-0006 P3.S1 transport/assembly vertical slice

Canonical engine: `swmr.oracle/v1` from released `@owebeeone/taut-shape`

Glade capability: `glade.swmr.adapter/v1`

## Boundary

Glade persists and routes attributed operations. It does not interpret file
bytes. Glial maps those operations into the canonical Taut `SwmrNode` and owns
assembly. The authenticated `Op.origin` is the canonical `writer_id`.

The Glade wire `Shape` enum appends `swmr=3`. The declaration contract appends
`swmr=6`; these are separate enums with separately frozen numeric histories.
Existing values MUST NOT be renumbered.

## Operation envelope

Every SWMR `Op.payload` MUST begin with this two-byte envelope:

| Byte | Meaning | Values |
| --- | --- | --- |
| `0` | adapter version | `1` |
| `1` | action | `0` snapshot, `1` delta, `2` reset |
| `2..` | opaque application body | zero or more bytes |

The exact mapping to `swmr.oracle/v1` is:

| Glade action | Canonical input |
| --- | --- |
| snapshot | `snapshot_push { writer_id: Op.origin, payload: body }` |
| delta | `delta_push { writer_id: Op.origin, payload: body }` |
| reset | `reset { writer_id: Op.origin, reason: producer_requested, detail: body? }` |

An empty reset body maps to `detail: null`. No other canonical SWMR input is
authorable through adapter v1.

## Normative requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| `GSA-01` | Dispatch MUST resolve the exact `swmr` capability. SWMR MUST NOT fall through to `value` or `log` folding. | `client-ts/test/session.test.ts`; `client-rs/src/session.rs`; `glial/test/shapes.test.ts` |
| `GSA-02` | A node or client MUST reject a short, wrong-version, or unknown-action envelope before store, chain, lamport, or callback mutation. | `wire-rs/src/lib.rs`; `node/src/store.rs`; both client session suites |
| `GSA-03` | `Op.origin` MUST be the canonical writer id. A zone-surface `(share, glade_id, key)` MUST accept at most one writer origin. | `node/src/store.rs`; `glial/test/swmr.test.ts` |
| `GSA-04` | A SWMR surface MUST NOT mix with a `value` or `log` surface at the same zone address. | `node/src/store.rs` |
| `GSA-05` | Glial MUST replay candidate state through released `SwmrNode` before mutating its instance store. Canonical diagnostics MUST be failures. | `glial/src/swmr.ts`; `glial/test/swmr.test.ts` |
| `GSA-06` | A delta before a snapshot, a second writer, and malformed bytes MUST reject the entire incoming batch. | `glial/test/swmr.test.ts` |
| `GSA-07` | Reset MUST advance the canonical epoch and MUST make prior-generation bytes unavailable until a new snapshot lands. | `glial/test/swmr.test.ts`; `glial/test/grip_adapter.test.ts` |
| `GSA-08` | A file window MUST be a projection over SWMR state, never a `window` delivery shape. A projection MUST select bytes from exactly one `(epoch, seq)` generation. | `glial/src/swmr.ts`; `demo/test/files.test.ts` |
| `GSA-09` | The checked-in `ws.files` declaration MUST use `swmr`. The Glade and grazel declaration copies MUST remain byte-identical. | `apps/grazel-app.glade`; `node/src/appdecl.rs` |

## First file profile

The demo profile treats every snapshot and delta body as a complete UTF-8 file
image. Its Grip projection exposes the first 4096 bytes with revision
`<epoch>:<seq>`. Reset exposes zero bytes, so bytes from two epochs cannot be
spliced into one view.

This is the P3.S1 vertical slice, not the complete `glade-files` supplier. It
does not yet provide path-addressed instances, viewport control messages,
background bulk backfill, large/binary blob handoff, or authoritative write
acknowledgements. Those remain P3.S1/P3.S2/P3.S3 work and MUST preserve the
adapter and generation rules above.
