# AGENTS.md - Glade Module Rules

## Scope

This repository owns Glade: the stable share substrate for G*.

Glade owns:

- declaration packages and generated bindings
- canonical records
- exchange, live-channel, and append-log semantics
- provider claims, leases, routes, and diagnostics
- transport-facing substrate behavior
- substrate control-plane mechanics

Glade does not own:

- Grip/Grok local UI graph execution
- Grip Share adapter behavior
- Glial application composition policy
- product-specific workflows

## Workflow

1. Keep implementation and contract changes small.
2. Use tests before implementation changes.
3. Keep spike code separate from stable contracts.
4. Promote stable plan output into `dev-docs/`.
5. Promote public support guarantees into `docs/`.
6. Keep temporary analysis in `scratch/`.

## Documentation

- `dev-docs/` is for internal engineering contracts.
- `docs/` is for public/end-user support promises.
- `scratch/` is ignored and non-authoritative.

Use explicit normative language in specs: `MUST`, `SHOULD`, `MAY`.
