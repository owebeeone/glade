# glade-decl: the declaration surface as a taut schema (SKELETON).
# Design: glial-dev/dev-docs/glade/GladeDeclSurface.md (GDL-035).
# Rule: declaration ONLY — if a field needs a network or a store to mean
# anything, it belongs in glade (kernel) or glial (client runtime), not here.
#
# NOTE: sketch pending the taut builder wiring (mirror taut-shape/ir/regen.py
# bootstrap when this gains its first generated consumer).

SKETCH = """
enum Shape { value, log, message, stream, exchange, window }   # text_crdt later (consolidation P4)

enum Authority { share, external }        # external carries a source name

message GladeId {
  id: STRING                              # stable, runtime-neutral; frozen once shared (GQ-6)
  # derivation: package id + grip key; pinned in a checked-in manifest;
  # renames are alias/migration records, never new ids.
}

message BindingDecl {                     # a "surface", in GladeZones terms
  glade_id: GladeId
  shape: Shape
  authority: Authority
  source: STRING?                         # when authority == external
  domain: STRING                          # which replicated world (-> wire share); zones vocab
  zone: STRING                            # commons | private(self) | future axes (-> wire key)
  retention: STRING                       # declared retention (ttl / latest / from-cursor)
}

message AdvertisementRecord {             # what grok enumeration emits (GDL-029)
  binding: BindingDecl
  package: STRING
  grip_key: STRING
}

# canonical-key INTERFACE (signature only; implementations live below):
#   canonical_key(param_shape_ir, params) -> BYTES   # deterministic CBOR
"""
