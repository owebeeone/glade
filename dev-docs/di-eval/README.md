# Isolated dependency-injection evaluation

2026-09-09. Shaku **0.6.3**, Dill **0.17.0**; transitive versions in `Cargo.lock`.
This is a harness/tool workspace, not a production dependency or architecture-gate
adoption. Its `ports` crate is a dependency-free witness contract plus deterministic
fixtures. It does not replace Glade's existing interfaces. No real network, database,
cryptography, wall-clock sleep or async runtime is used by the probes.

The workzone report is `dev-docs/arch1/DependencyInjectionEvaluation.md`. This
workspace remains runnable from a standalone Glade repository clone. The only
existing Glade dependency is the lifecycle contract, used by a negative example.

## Reproduce

Run from this directory:

```sh
cargo test --locked --features shaku --test shaku
cargo test --locked --features dill --test dill
cargo fmt --all --check
cargo clippy --locked --features shaku,dill --tests -- -D warnings
cargo tree -p di-eval-ports
```

Nine Shaku and fourteen Dill characterization tests pass. **A passing test that
reproduces undesirable behavior is evidence of that behavior, not Glade acceptance.**
In particular, the Dill ambiguity/cycle/cache/sibling probes intentionally assert
observed limitations. An upstream change may require reevaluating these assertions.
The concurrency probe uses two test threads and bounded coordination, not a proposed
Glade threading architecture. The cycle probe validates but NEVER resolves the cycle.

Four examples MUST fail compilation for the indicated reason:

| Command | Expected diagnostic |
|---|---|
| `cargo check --locked --features negative-shaku --example missing_shaku` | E0277, missing `HasComponent<dyn Clock>` |
| `cargo check --locked --features negative-shaku --example cycle_shaku` | E0275, recursive trait-bound overflow |
| `cargo check --locked --features negative-shaku --example foreign_port_shaku` | E0277, foreign `dyn Clock` does not implement Shaku `Interface` |
| `cargo check --locked --features negative-dyn --example native_dyn` | E0038, existing Glade `ManagedResource` is not dyn compatible |

Inspect diagnostics, not just nonzero exit codes. Do not use `--all-features
--all-targets`: it deliberately includes the invalid examples. These checks are
manual reproduction commands, not newly installed CI gates.

## Recorded evidence

- RED: test imports failed before the shared witness contracts/fixtures existed.
  Further compile probes exposed foreign-trait and constructor-cycle restrictions;
  they were retained as negative examples rather than hidden by changing Glade APIs.
- GREEN: the 23 runtime probes pass; four expected compile failures were checked.
  Rustfmt and Clippy with warnings denied pass for the positive test targets.
- Rust 1.96.0, `aarch64-apple-darwin`, debug profile. One sequential clean-target,
  offline sample per framework, then one unchanged warm repeat:
  Shaku 3.49s / 0.03s; Dill 5.32s / 0.03s wall time, including Cargo.
  Test execution was reported as 0.00s (rounded, not literally zero).
- The clean builds used new temporary target directories with downloaded sources
  already cached. This is not first-install/network timing, a statistical benchmark,
  equal-sized workload comparison, or a forecast for a full Glade build.
- Production manifests, runtime implementations, public traits, dependency allowlists
  and normal test selectors were not changed. The demo was not rebuilt or retested.

## How the fixtures keep framework coupling local

`tests/shaku.rs` supplies assembly-only facade traits extending the independent
ports and Shaku's `Interface`; stable trait upcasting returns the independent port.
`tests/dill.rs` annotates an assembly-owned fake-provider wrapper. Neither makes
`ports` depend on a DI crate. This proves that small witness boundary only; generic
async Glade interfaces and real resource startup/cleanup remain separate work.
