# Glade

Glade is the G* share substrate.

It is intended to provide the stable mechanics underneath Glial, Grip Share,
and future G* applications:

- share identity and scope
- declaration-driven surfaces
- canonical records
- bounded exchanges
- live channels
- append logs
- provider claims and leases
- routing and diagnostics

This repository is new and currently contains scaffolding only.

## Layout

| Path | Purpose |
| --- | --- |
| `docs/` | Public support contracts and user-facing documentation. |
| `dev-docs/` | Internal engineering design and implementation contracts. |
| `scratch/` | Ignored local notes, experiments, and temporary analysis. |

## Current Focus

The immediate planning focus is Phase 1:

```text
browser js-libp2p peer
  -> Rust libp2p provider peer
  -> local PTY process
  -> append-log-shaped output buffer
```

Stable design from the root `glial-dev` plan documents will be promoted here
when it becomes module-owned Glade design.
