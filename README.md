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

Status: pre-release (glade-node `0.0.0`). The node in `node/` boots, loads the
app files described in [`docs/AppFileFormat.md`](docs/AppFileFormat.md) and
serves shares to clients and peers.

## Layout

| Path | Purpose |
| --- | --- |
| `docs/` | Public support contracts and user-facing documentation. |
| `dev-docs/` | Internal engineering design and implementation contracts. |
| `scratch/` | Ignored local notes, experiments, and temporary analysis. |

## Current Focus

The current plan is the first slice (`dev-docs/GladeFirstSlicePlan.md` at the
glade-wz workspace root): the declaration contract v1 and its app-file header
`glade-app v1`, then a node assembled from the contract ports in `contracts/`
behind one gate (`node/check.sh`), then real adapters and a fixed-peer route over
iroh.

Stable design from the root `glial-dev` plan documents will be promoted here
when it becomes module-owned Glade design.
