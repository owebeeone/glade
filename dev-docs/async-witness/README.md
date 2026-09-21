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

## Negative fixtures

Phase 1 files four of them, as `fast` examples behind the `negative` feature so
nothing sweeps them into the gate. **Three MUST fail to compile and one MUST
compile.** Inspect the diagnostics, not the exit codes; these are manual
reproduction commands, not newly installed CI gates. Run them from this
directory, each prefixed with `cargo check --locked --offline --features
negative --example`:

| Fixture | Expected result | What it decides |
|---|---|---|
| `missing_binding` | **E0277** — "the trait bound `MissingPorts: HasComponent<(dyn Clock + 'static)>` is not satisfied", the same for `dyn Carrier`, and a third E0277 falling out of those two ("`MissingPorts` cannot be shared between threads safely") | DI-E03: a port no one bound is refused at the composition, not at the first request |
| `construction_cycle` | **E0275** — "overflow evaluating the requirement `Cyclic: HasComponent<(dyn Admission + 'static)>`" | DI-E03: a constructor cycle is refused before startup. The diagnostic names a bound, not the loop — caveat 1 of the plan's §8.4 |
| `ambiguous_role` | **E0599** at the natural call site — "no method named `resolve` found for struct `Ambiguous`", because a multibound interface gets no `HasComponent` impl at all — then **E0277** where the bound is named explicitly | DI-E03: selecting one of two occurrences requires its key. The plan predicted only the E0277 |
| `cfg_scanner_gap` | **compiles, and prints** `compiled: seen, unseen and an entire module the scanner skipped` | The §4.5 gap below, demonstrated rather than described. `cargo run` it |

`missing_binding` and `ambiguous_role` report the same *kind* of failure for
opposite causes — nothing bound and two things bound. Shaku's diagnostics do not
distinguish absence from ambiguity.

### The `#[cfg]` scanner gap, measured

`architecture-check`'s `fn conditional` (`src/lib.rs:97-102`) tests whether an
attribute's path is `cfg` or `cfg_attr` and never evaluates the condition, so an
always-true `#[cfg(all())]` is skipped exactly like a never-true `#[cfg(any())]`.
Pointing the real checker at `cfg_scanner_gap.rs` as a scratch contract package
gives both directions:

- **Fails closed.** With `"traits": {"PartlyScannedPort": ["seen", "unseen"]}` it
  reports `ARCH-003 cfg-gap-fixture: PartlyScannedPort must expose required
  methods {"seen", "unseen"} unconditionally` — the conditional method is
  invisible, so a *required* item is reported missing.
- **Silent.** With `"traits": {"PartlyScannedPort": ["seen"]}` it reports
  `PASS`, although the file also contains a whole module — a public trait, a
  public type and an impl — that the scanner never examined, because an
  ordinary `allow(dead_code)` wrapped in `cfg_attr` was enough to skip it.
- **The guard it defeats.** `#[path = "hidden.rs"] mod hidden;` is refused with
  `ARCH-003 … #[path] modules need explicit checker support; cannot silently
  skip them`. Wrapping the same attribute as `#[cfg_attr(all(), path =
  "hidden.rs")]` compiles the identical code and the checker reports `PASS`:
  the `!conditional(&m.attrs)` guard sits in front of that refusal
  (`src/lib.rs:211-214`).

This does not touch the dependency half of the gate, which is read from `cargo
metadata` and cannot be reached by any `#[cfg]`; DI-E01's and DI-E04's manifest
claims are unaffected. It is why the owner's standing rule puts conditional
compilation inside an explicit boundary instead of on a single declaration.

## What the gate checks, and what it cannot

`check.sh` runs the `syn`-based lint at
`glade-discover/tools/architecture-check/`, which compares each workspace
library's **declared** dependencies against the allowlist in
`architecture-policy.json`. It is an allowlist, so `shaku` or `sdax` appearing in
`async-witness-ports` is an `ARCH-002` error without anyone having thought to
forbid it; it sees inactive optional, target, build and dev dependencies, whose
kind is part of the key; and it is manifest-level, so no `#[cfg]` can bypass it.

Three limits are named rather than hidden, the third measured above under
[the `#[cfg]` scanner gap](#the-cfg-scanner-gap-measured):

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

## Phase 3 — the real async port

Phase 2 decided AR-08 over fakes so that a lifecycle failure could never be
mistaken for an injector failure (`AsyncWitnessPlan.md` §8.1). Phase 3 swaps the
fakes for the node's own `glade_node::iroh_carrier::PeerEndpoint`, unmodified,
and keeps the two questions apart in the same way.

| Step | What it shows | Where |
|---|---|---|
| 3.1 | Two witness nodes bind localhost QUIC endpoints, one dials the other, both complete the node<->node HELLO seam, and one real glade `Frame` crosses the witness's `CarrierPort` — all driven by an sdax plan, with every external effect inside `cx.hold(...)` | `real/src/peer_carrier.rs`, `real/src/peer_plan.rs`, `real/tests/peer_carrier.rs` |
| 3.2 | **AR-08 for real**: after `report.is_clean()`, every recorded UDP port re-binds within the bound — and two variants in which one endpoint clone, or one link `Connection`, deliberately escapes keep that port bound for the whole bound, then free it when the escapee is dropped | `real/tests/peer_release.rs` |
| 3.3 | Shaku assembles over the **already-acquired** handle from inside an sdax step that `.needs` it, resolves the engine's own carrier, puts a frame across it, and constructs no provider of its own; the module step is ordered before every endpoint's release | `real/src/shaku_bridge.rs`, `real/tests/shaku_assembly.rs`, `real/tests/shaku_registration.rs` |
| 3.4 | **The differential**: the same plan with and without the module step. Both runs are `is_clean()`, both free every port, and nothing observable diverges — in either running order, over three repeated pairs | `real/tests/differential.rs` |

Reproduce from this directory:

```sh
cargo test --locked --offline -p async-witness-real --test peer_carrier
cargo test --locked --offline -p async-witness-real --test peer_release -- --nocapture
cargo test --locked --offline -p async-witness-real --test shaku_assembly
cargo test --locked --offline -p async-witness-real --test shaku_registration
cargo test --locked --offline -p async-witness-real --test differential -- --nocapture --test-threads 1
```

### The plan shape, and why it is this shape

```text
Acceptor  <-- Served  <-- Exchange
    ^                        |
    +---- Dialed  <----------+
           ^
        Dialer
```

The arrows are `needs`; cleanup is their reverse, derived by sdax from the typed
edges. `Dialed` needs **both** endpoints — its own to dial from and the
acceptor's `addr()` to dial to — which makes every endpoint the parent of every
link that can reach it. `release_order().before(...)` is asserted statically
over that declaration, with no runtime and no socket, exactly as Step 2.1 did
over the fakes.

### Giving a handle back by value

`PeerEndpoint::close(self)` consumes the handle, because iroh frees the UDP
socket only once every clone is gone. An sdax release body receives an `Arc<T>`,
and an **async** release leaves that `Arc` in the engine's slots rather than
taking it out (`sdax/src/host/bodies.rs:404-421` takes the value only for a
`by_drop` release). Closing a *clone* would therefore leave one live clone in
the slots for as long as the run's storage lives.

So `WitnessEndpoint` owns its `PeerEndpoint` as a `Mutex<Option<..>>` and the
release body **takes** it out; the `Arc` the engine keeps afterwards is an empty
shell. `WitnessCarrier` does the same for the link's `SendStream`, `RecvStream`
and `Connection`, and that half is settled by measurement rather than by
reading a driver.

**A correction, and what replaced it.** This passage used to derive the link's
by-value release from quinn's endpoint driver. **iroh 1.2.0 does not use
quinn**: `cargo tree -p iroh --depth 1` lists `noq`, `noq-proto` and `noq-udp`,
and `cargo tree --invert quinn` answers "package ID specification `quinn` did
not match any packages" — it is not in this workspace's graph at all. The crate
that is there stops its endpoint driver when the connection map is empty **and**
either the handle count is zero **or** `close` has been called
(`noq-1.3.0/src/endpoint.rs:471-476`), so after a close a remaining handle does
not keep the *driver* alive, and the driver's exit condition settles nothing
about the socket in either direction.

So the witness measured the socket instead, with the direction unknown in
advance:
`tests/peer_release.rs::an_escaped_connection_holds_the_port_as_an_endpoint_clone_does`
runs the ordinary plan with one clone of the **served** link's `Connection`
escaping the composition. The acceptor's port stays bound for the whole two
second bound, the dialer's port — the side nothing cloned — comes back in about
a hundred microseconds, and the acceptor's comes back in tens of microseconds
the moment the escaped `Connection` is dropped. The escapee is a clone of the
handle the release then closes, so what is measured is precisely the
counterfactual: a **closed** `Connection` value left alive in a slot. Taking the
link's handles by value is therefore load-bearing, not tidiness — the claim
stands, but it now rests on the socket rather than on a crate this workspace
does not compile.

Those `Mutex`es are interior mutability inside the **provider**, which is where
`ports/src/lib.rs` already puts it ("an implementation owns its own interior
mutability"). No Glade contract is touched, nothing is made `Sync` to please a
container, and no public future is boxed that the port did not already box.

### The socket is the only honest witness

A leaked handle is **invisible to the report**. The escaped-clone variant is
`report.is_clean()`, `report.incomplete` is empty and `TokioRuntime::tracked()`
is 0 — and the acceptor's UDP port is still bound two seconds later. Nothing
sdax can see is wrong, because nothing sdax can see *is* wrong: the release ran,
the obligation was discharged, and a clone the engine never knew about outlived
it. That is why the plan chose a port whose leak is observable from outside the
process, and it is what makes §8.4 caveat 3 decidable rather than rhetorical.

The release check is itself falsifiable: `the_release_check_can_answer_still_bound`
holds a port of its own and requires the helper to spend the whole bound saying
so, so a `Some(..)` elsewhere cannot mean the check was vacuous.

### Where Shaku meets the lifecycle

```text
sdax-rs acquires  ->  Shaku assembles over the acquired handles  ->  sdax-rs releases, in reverse
```

The order is forced, not chosen: `build()` is synchronous and
`with_component_override` takes an already-constructed value, so Shaku cannot
acquire a socket. The `Module` step therefore `.needs` the carrier the engine
acquired three nodes earlier, and hands it to the builder as an override.

- **The facade traits are declared in `real`**, not imported from `fast`.
  `architecture-policy.json` does not let `async-witness-real` depend on
  `async-witness-fast`, and widening an allowlist to make a check pass is
  forbidden. "Depends on 1.1" means the pattern; a facade is local to one
  assembly anyway.
- **A plain `with_component_override` is enough.** The override is consulted
  before the registered build function, which Phase 1 measured, so
  `BindingCarrier::build` never runs and the counter reads 0.
  `with_component_override_fn` on a `#[lazy]` registration would also work and
  is not needed — there is nothing to defer when the value already exists.
- **`AcquiredCarrier` is what makes the plain override possible.**
  `with_component_override` takes `Box<I>` and the engine's value is an `Arc`,
  so the box is a delegating handle rather than a copy of anything.
- **The counter is paid for.** `tests/shaku_registration.rs` is a separate test
  binary that builds the same module with nothing overridden and watches the
  registration construct itself, so the zero read in `tests/shaku_assembly.rs`
  is not the zero of a registration that does not exist.
- **The `Module` step `.needs` the `Exchange` step.** Without that edge the two
  would be `unordered` and would contend for the same stream; a witness whose
  result depended on which body reached the lock first would be evidence of
  nothing.

**The sharp risk at the seam, measured.** Plan §4.2 warns that "the Shaku module
holds `Arc` clones of things derived from the endpoint, and an escaped clone
keeps the UDP socket bound". It does not here:
`a_module_that_outlives_the_step_holds_no_socket` stashes the built module in
the harness so it survives the whole run, and both ports still free in
microseconds. The reason is the by-value release above — a module that outlives
the step holds a carrier that owns nothing. Contrast the endpoint clone of Step
3.2, a handle the engine never owned, which does hold the port.

### The differential, and what it may not compare

`observe(with_module: bool)` is one function and the `bool` reaches exactly one
place, `PeerHarness::with_shaku_module`. Everything else — plan, bodies,
budgets, runtime, the order of the observations — is shared, so a difference in
the result can only have come from the module step.

**The first version of it compared too much, and that is worth recording.** It
compared the *sequence* in which releases completed, and the two sides
"diverged": `["Served", "Dialed", "Acceptor", "Dialer"]` against `["Dialed",
"Served", "Dialer", "Acceptor"]`. The sequence also varied between runs of the
**same** configuration, which is what gave it away. Nothing was wrong: `Served`
and `Dialed` share no `needs` path, and neither do `Acceptor` and `Dialer`, so
`release_order()` reports both pairs `unordered` and the engine is free to
finish them in either order. That is AR-08's third clause — "concurrent
independent cleanup can progress" — observed live rather than derived from
`unordered_pairs()`, and three distinct total orders showed up across six runs.

So the differential compares the **partial** order the plan declares: which
releases completed, and `released_before(child, parent)` for each of the three
pairs that are genuinely ordered.
`the_uncompared_pairs_are_the_ones_the_plan_declares_unordered` asserts
statically that the pairs left out are exactly the ones `release_order()` calls
unordered, so the exclusion is reading the plan rather than excusing a
difference.

The general lesson for anyone writing the next differential here: **a total
order is not an observable of this system.** Compare what the declaration
constrains.

### Measured

Toolchain `rustc 1.96.0 (ac68faa20 2026-05-25)`, macOS 26.6 on Apple silicon,
12 cores, no `RUSTC_WRAPPER` and no compiler cache, `dev` profile. Phase 4's
Step 4.1 owns the measured table for the result document; these are Phase 3's
working figures.

| Measurement | Result |
|---|---|
| `sh check.sh` (all three members, warm; 67 tests) | 4.3–4.5 s |
| Cold build of the whole workspace (`--lib --tests --no-run`, empty `CARGO_TARGET_DIR`) | 33.0 s, 1.6 GB of artefacts |
| Warm incremental rebuild of the `real` lib after one touched file | 1.1 s |
| `cargo test -p async-witness-real --lib --tests` (warm, 42 tests) | 2.88–2.94 s over five consecutive runs |
| `tests/peer_carrier.rs` alone (4 tests, two real endpoints per run) | 0.04 s |
| `tests/peer_release.rs` (4 tests) | 2.04 s, of which two deliberate 2 s bounds |
| `tests/shaku_assembly.rs` (4 tests, two real endpoints per run) | 0.04 s |
| `tests/differential.rs` (5 tests, 22 real endpoints) | 0.09 s in parallel, 0.21 s single-threaded |
| Port free after a clean run | 4.7–15.2 µs, over three runs of both endpoints — already free at the first poll |
| Port free after the escaped clone was dropped | 23–108 µs |
| Port with an escaped clone still alive | still bound at 2.004–2.006 s, i.e. the whole bound |
| Port with an escaped link `Connection` still alive | still bound at 2.004–2.005 s; free 41–66 µs after it was dropped, while the uncloned dialer side was free in 108–143 µs |
| Under load: 4 parallel copies of every `real` test binary while a whole-workspace `cargo test` ran | all green; the slowest suite went from 0.04 s to 2.2 s and stayed green |

The clean-run figure is three orders of magnitude below the node's own 6–10 ms
(`iroh_carrier.rs:218-221`), and the difference is not a faster machine: the
node's test asks immediately after `close` resolves, whereas a run has a second
endpoint to release and a report to assemble in between, so iroh's driver has
already wound down by the time the check happens. The bound stays at two
seconds regardless — it is there for the case where it has not.
