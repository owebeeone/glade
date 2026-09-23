# Glade shape dispatch and declaration inventory

Status: exact capability record; SWMR and CRDT/text adapters built 2026-08-29

Decision source: `../../dev-docs/TautShapeCatalogDecision.md`

## Runtime capability boundary

Glade preserves the existing numeric `Shape` values for wire decoding. That
compatibility surface does not grant runtime capability.

| Path | Accepted name | Behavior |
| --- | --- | --- |
| Binding declaration | `value`, `log`, `swmr`, `crdt` | Accepted and registered |
| TypeScript/Rust durable op | `value`, `log`, `swmr`, `crdt` | Exact adapter dispatch; SWMR action envelope validated; CRDT causal heads carried in `Op.refs` |
| TypeScript/Rust fold | `value`, `log` | Exact local fold dispatch; SWMR and CRDT are not reinterpreted |
| Glial durable assembly | `value`, `log`, `swmr`, `crdt` + explicit `text_crdt` profile | value/log folds; SWMR delegates to released `SwmrNode`; CRDT delegates to released `CrdtNode` and text projection |
| Service declaration/provider | `exchange` | Dedicated correlated request/response path |
| Binding declaration | `message`, `stream`, `exchange`, `window` | Rejected before registration |
| Op/fold dispatch | every other name | Rejected before chain/store mutation |

`exchange` is intentionally absent from every fold registry. `message` and
`window` remain reserved legacy decode values. `stream` is a canonical Taut
engine name, but Glade must reject it until a versioned adapter capability and
tests exist. SWMR is specified by `GladeSwmrAdapter.md`; CRDT/text is specified
by `GladeCrdtAdapter.md`.

## Checked-in application inventory

`apps/grazel-app.glade` is the only checked-in `.glade` declaration in this
repository.

| Declaration | Shape | Retention token | Classification |
| --- | --- | --- | --- |
| `ws.tree` | `value` | `latest` | Compatible materialized-winner policy |
| `ws.files` | `swmr` | `from-cursor` | Canonical single-writer generation; file window is an application projection |
| `ws.diff` | `log` | `from-cursor` | Compatible replay/cursor policy |
| `term.log` | `log` | `from-cursor` | Was `windowed`; decided 2026-09-23 (R2(a)): the window is an application projection, the history is `from-cursor` |
| `gwz.output` | `log` | `from-cursor` | Compatible replay/cursor policy |
| `chat.msgs` | `log` | `from-cursor` | Compatible replay/cursor policy |
| `chat.groups` | `value` | `latest` | Compatible materialized-winner policy |
| service `grazel` / `gwz.ops` | `exchange` service | n/a | Dedicated exchange path, not a binding |

No checked-in application binding declares `message`, `stream`, `exchange`, or
`window` as a delivery shape.

## Retention follow-up

`term.log`'s free-form `windowed` token names neither a bound nor an expiry and
recovery policy. This implementation deliberately does not reinterpret or
narrow it. The terminal owner must choose an explicit record/byte/age bound and
cursor-expiry behavior before the declaration can migrate. All other current
tokens can remain while retention is separated from shape identity in a later
declaration revision.

*Decided 2026-09-23 (`GladeDeclReconciliation.md` R2(a)): `windowed` is not a
retention; `term.log` is `from-cursor`, with the window kept in the app. A
storage bound for the history (records, bytes or age) and cursor expiry are
still open: retention is declarative and unenforced until GC-4.*
