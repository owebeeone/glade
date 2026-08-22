# Glade shape dispatch and declaration inventory

Status: Step 0.3 implementation record

Decision source: `../../dev-docs/TautShapeCatalogDecision.md`

## Runtime capability boundary

Glade preserves the existing numeric `Shape` values for wire decoding. That
compatibility surface does not grant runtime capability.

| Path | Accepted name | Behavior |
| --- | --- | --- |
| Binding declaration | `value`, `log` | Accepted and registered |
| TypeScript/Rust op and fold | `value`, `log` | Exact adapter dispatch |
| Glial delivery assembly | `value`, `log` | Exact adapter dispatch |
| Service declaration/provider | `exchange` | Dedicated correlated request/response path |
| Binding declaration | `message`, `stream`, `exchange`, `window` | Rejected before registration |
| Op/fold dispatch | every other name | Rejected before chain/store mutation |

`exchange` is intentionally absent from every fold registry. `message` and
`window` remain reserved legacy decode values. `stream` is a canonical Taut
engine name, but Glade must reject it until a versioned adapter capability and
tests exist.

## Checked-in application inventory

`apps/grazel-app.glade` is the only checked-in `.glade` declaration in this
repository.

| Declaration | Shape | Retention token | Classification |
| --- | --- | --- | --- |
| `ws.tree` | `value` | `latest` | Compatible materialized-winner policy |
| `ws.files` | `log` | `from-cursor` | Compatible replay/cursor policy |
| `ws.diff` | `log` | `from-cursor` | Compatible replay/cursor policy |
| `term.log` | `log` | `windowed` | Ambiguous legacy token; owner decision required |
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
