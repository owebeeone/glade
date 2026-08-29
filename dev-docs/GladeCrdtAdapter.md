# Glade CRDT adapter v1

Status: **Built 2026-08-29** — GLP-0006 P4.S1/P4.S2 vertical slice

Canonical engine: `crdt.oracle/v1` from released `@owebeeone/taut-shape`

First payload profile: `text_crdt.profile/v1`

## Boundary

Glade persists and routes attributed CRDT operations; it does not merge the
opaque payload. Glial maps the Glade envelope into the canonical Taut core:

| Glade field | Canonical CRDT field |
| --- | --- |
| `Op.origin` | `CrdtOp.origin` |
| per-zone `Op.seq` | positive, per-origin `CrdtOp.seq` |
| `Op.refs[]` stream frontier | normalized `CrdtOp.deps` version vector |
| `Op.payload` | opaque `CrdtOp.payload` |

The Glade wire `Shape` enum appends `crdt=4`. The declaration contract appends
`crdt=7`; these are separate enums with separately frozen numeric histories.
Existing values MUST NOT be renumbered. `text_crdt` is a profile over the CRDT
engine and MUST NOT be encoded as another delivery shape.

## Text identity and cursor rule

The first editor profile emits insert/delete payloads accepted by
`text_crdt.profile/v1`. Every inserted editor element has the public identity
`{actor_id,counter}`; deletes name the stable element identity and retain a
tombstone. A cursor or selection endpoint MUST store an element identity plus
`before`/`after` affinity. A consumer MUST capture those anchors before applying
a remote projection and resolve them afterward. It MUST NOT retain a raw string
offset across a remote delta.

## Normative requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| `GCA-01` | Dispatch MUST resolve the exact `crdt` capability. CRDT MUST NOT fall through to `value` or `log` folding. | `client-ts/test/session.test.ts`; `client-rs/src/session.rs`; `glial/test/shapes.test.ts` |
| `GCA-02` | A CRDT append MUST copy the zone-surface causal frontier into `Op.refs`, sorted by origin. | both client session suites |
| `GCA-03` | A CRDT zone-surface MUST accept several writer origins and MUST reject mixing with another durable shape before store mutation. | `node/src/store.rs` |
| `GCA-04` | A Glial CRDT mount MUST select an explicit supported payload profile. Unknown or absent profiles MUST fail before opening an instance store. | `glial/test/shapes.test.ts` |
| `GCA-05` | A text-profile operation MUST pass strict UTF-8/JSON insert/delete validation before instance-store mutation. | `glial/test/text_crdt_mount.test.ts` |
| `GCA-06` | Glial MUST replay candidate operations through released `CrdtNode` and project text through released `text_crdt` semantics. It MUST NOT invent a second merge rule. | `glial/src/text_crdt.ts`; `glial/test/text_crdt_mount.test.ts` |
| `GCA-07` | Concurrent delivery permutations and offline exchange MUST converge on the same text and operation identity set. | `glial/test/text_crdt_mount.test.ts`; upstream Taut CRDT corpora |
| `GCA-08` | Cursor and selection endpoints MUST be element-ID anchored with affinity and MUST resolve at the tombstone position after deletion. | `glial/test/text_crdt_mount.test.ts` |
| `GCA-09` | The demo MUST expose a declared `crdt` surface through a plaintext contenteditable editor, not a controlled whole-value input. | `demo/test/files.test.ts`; `demo/src/CollaborativeTextEditor.tsx` |

## Deliberate remainder

This vertical slice does not implement causal compaction/checkpoint
acknowledgement, saved-file compare-and-replace, or private presence/cursor
publication. Those remain required before a production editing supplier can
claim the complete H-P4 contract. The local selection anchor is private UI
state and is not published by this demo.
