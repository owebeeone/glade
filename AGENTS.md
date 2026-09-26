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

## No process globals

Production code MUST keep no process-global mutable state:

- no `static mut`, and no statics with interior mutability or lazy initialisation
  (`Atomic*`, `Mutex`, `OnceLock`, `LazyLock`, …);
- no `thread_local!`;
- no environment, working-directory or home-directory reads where they are used;
- no process-wide hooks (panic hook, global logger, C signal handlers);
- no child process that inherits the live environment.

A program reads its arguments and environment once, at its entry point, and passes
them down. It spawns a child with `env_clear()` plus an explicit environment.

`scripts/checks/check_process_globals.py` enforces this against
`scripts/checks/process_globals_allowlist.json`. `node/check.sh` runs it as its
`process-globals` component, and `client-rs/tests/process_globals.rs` runs it with
client-rs's tests. It fails on anything new and on any entry that no longer matches, so
the allowlist only shrinks.

- Agents MUST NOT add or loosen an allowlist entry to make the check pass. Restructure
  the code instead.
- A read at a program's entry point is recorded as `permanent`; the owner ruled that
  kind of entry on 2026-09-26. Any other new entry needs the owner's approval, with a
  disposition (`debt` or `permanent`) and a reason.
- Paying a debt deletes its entry in the same change.
- A child spawned with `env_clear()` plus an explicit environment stays listed, as
  `permanent`. The checker lists every `Command::new` whether or not it clears the
  environment, so only the tests show that it does. gwz-core records its own clean
  spawn the same way.
- The checker is gwz-core's, vendored byte for byte, and the allowlist's `source` names
  the commit. Update every repository's copy together.

The plan that pays the debt down is `dev-docs/ProcessGlobalsPlan.md` in the glade-wz
workspace.

## Documentation

- `dev-docs/` is for internal engineering contracts.
- `docs/` is for public/end-user support promises.
- `scratch/` is ignored and non-authoritative.

Use explicit normative language in specs: `MUST`, `SHOULD`, `MAY`.
