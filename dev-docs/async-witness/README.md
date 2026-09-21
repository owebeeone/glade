# The async witness — `async_witness`

**A harness workspace, not production code.** Nothing here is a production Cargo
installation, a repository-wide architecture-gate adoption, or a proposed Glade
API. It is one bounded experiment, following the precedent of
`glade/dev-docs/di-eval/` exactly: a separate Cargo workspace with its own
tracked `Cargo.lock`, never referenced from `glade/node/Cargo.toml`.

Dependency flows one way only: the witness path-depends on `glade-node` and
`glade-wire`; neither gains anything. No module is added to `glade/node/src/`,
no public contract changes, and the demo is untouched.

The controlling document is
[`glade-wz/dev-docs/arch1/AsyncWitnessPlan.md`](../../../dev-docs/arch1/AsyncWitnessPlan.md).
The question it asks is whether the chosen wiring — **Shaku** for assembly
(GDL-048) over **sdax-rs** for lifecycle (GDL-049) — survives a real async port,
start-up and cleanup, against the acceptance criteria DI-E01..DI-E04 and AR-08.
The owner records the outcome on the decision graph; this workspace does not.

## Members

| Member | Role | Dependencies | Purpose |
|---|---|---|---|
| `ports` (`async-witness-ports`) | contract | `glade-wire` only | `ClockPort`, `CarrierPort`, `StorePort` and deterministic fakes. **Zero framework dependencies — this crate is the DI-E04 wall.** |
| `fast` (`async-witness-fast`) | harness | ports, `shaku =0.6.3` | DI-E01, DI-E02, DI-E03. No tokio, no iroh, no sdax. Milliseconds. |
| `real` (`async-witness-real`) | harness | ports, shaku, `sdax`, `sdax-tokio`, `glade-node`, `glade-wire`, `iroh`, tokio; dev: `sdax-testkit`, `glade-lifecycle-api` | DI-E04 and AR-08 against the real iroh endpoint. Slow, run separately. |

The three members exist from Phase 0; `fast` and `real` are filled in by Phases
1–3. Phase 0 adds no witness logic.

Every dependency and dev-dependency the later phases need is already declared
and locked, so **no phase after this one has to touch a manifest or
`Cargo.lock`** to resolve something. Adding an `[[example]]` target or a feature
for Phase 1's compile-fail fixtures is a target-table edit and changes neither
resolution nor the lockfile; adding a new *dependency* is a dependency-posture
change and needs the plan's §5 read again first.

## Pins

`sdax`, `sdax-tokio` and `sdax-testkit` all come from Git at the **same** rev,
`ccf06e76a90e22a454471a71f0cf6f5cb878baac` on
`https://github.com/owebeeone/sdax-rs`; a mismatch between the three is a
failure case (`sdax-rs/RELEASE.md:70`). Shaku is pinned exactly at `=0.6.3`, the
evaluation's measured baseline — a caret range would silently retest a different
library. `tokio` is declared as `"1"` and is expected to **unify** at the
`=1.53.1` that `sdax-tokio` pins; the tracked `Cargo.lock` records what it
actually resolved to.

The effective MSRV is **1.91**, from iroh 1.2.0's `rust-version`, not shaku's
1.88.

## Run

From this directory:

```sh
sh check.sh            # architecture gate, tests, fmt and clippy for all three
sh check.sh ports      # one member: ports | fast | real
sh check.sh --list all # the packages a selector names, without running them
```

`check.sh` deliberately omits `--all-features --all-targets`. From Phase 1 the
`fast` member carries examples that **must fail** to compile, exactly as
`di-eval/README.md` records; sweeping them into the gate would invert their
meaning. Run those by hand and inspect the diagnostics, not just the exit codes.

Two commands to avoid from this workspace, both because they reach outside it:

- **`cargo fmt --all`** formats a package's local **path dependencies** too,
  which from here means `glade-wire`, `glade-node` and the contracts. They are
  not rustfmt-clean and are not ours to reformat. Select packages by name, as
  `check.sh` does.
- **`cargo build`, `cargo test` or `cargo check` with `glade/node` as the
  working directory.** The owner's desk runs `glade/node/target/debug/glade-node`.
  Building the witness is safe and leaves every artefact, `glade-node` included,
  in this workspace's own `target/`.

## What the gate checks, and what it cannot

`check.sh` runs the `syn`-based lint at
`glade-discover/tools/architecture-check/`, which compares each workspace
library's **declared** dependencies against the allowlist in
`architecture-policy.json`. It is an allowlist, so `shaku` or `sdax` appearing in
`async-witness-ports` is an `ARCH-002` error without anyone having thought to
forbid it; it sees inactive optional, target, build and dev dependencies, whose
kind is part of the key; and it is manifest-level, so no `#[cfg]` can bypass it.

Two limits are named rather than hidden:

- The checker shells `cargo metadata --no-deps`, so it classifies **workspace
  members only**. `glade-wire` and `glade-decl` cannot be listed in this policy
  file — Cargo refuses a workspace member that is not hierarchically below the
  workspace root, and a path dependency outside the workspace never appears in
  `--no-deps` metadata. Listing them would make the gate fail `ARCH-001 stale
  classification`. `check.sh` therefore asserts `glade-wire`'s purity directly
  against the resolved graph instead, and `glade-decl` is not reachable from this
  workspace at all.
- `--no-deps` also means **transitive** framework reachability is invisible to
  the checker. `check.sh` adds the second, equally cheap assertion the plan asks
  for: `cargo tree --invert` over `shaku`, `sdax` and `tokio` must list no
  contract-role package.

Do not relax a classification or an allowlist to make a check pass
(`glade-wz/AGENTS.md:31-32`). Record it and get it reviewed.
