# glade-decl — the declaration surface (skeleton)

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
