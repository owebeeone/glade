# glade-decl — the declaration surface (skeleton)

> **Superseded.** This skeleton is superseded by the `glade-decl` contract
> repository, a glade-wz member: its schema is `glade-decl/ir/glade_decl.taut.py`,
> rendered by `glade-decl-rs`, `glade-decl-ts` and `glade-decl-py`. It is kept
> for history only. Nothing builds from it, and where the two differ, the
> contract is the answer.

The shared LEAF module fixing the grip→glial→glade arrows:

```text
grip-core ──▶ glade-decl ◀── glial
                  ▲
                  │ implements
               glade kernel
```

Contents (declaration only — no runtime, no wire, no folds, no persistence):
`GladeId` (+ GQ-6 derivation/pinning), `Shape`, `Authority`, `BindingDecl`,
`AdvertisementRecord`, and the canonical-key *interface*.

Authored as a taut schema (`glade_decl.taut.py`) so Rust/TS/Python agree by
generation. Design: `glial-dev/dev-docs/glade/GladeDeclSurface.md` (GDL-035).

Status: SKELETON — schema sketch only; generated `rs/`/`ts/` land with the
first consumer swap (grip-core's inline share-decl types).
