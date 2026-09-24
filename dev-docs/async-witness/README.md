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

Update 2026-09-24: `glade-node`, a path dependency, gained the four Step 3.1
port crates, `shaku` and the sdax crates at the first-slice plan's Steps 3.2
and 3.3, which left this lockfile stale for `--locked`. It was refreshed with
`cargo update -p glade-node --offline`: the four port crates were added and
`glade-node`'s dependency list extended; no version already locked moved. Every
measurement below predates the refresh.

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

Phase 3 files one that the gate itself runs: `arch002-fixture.sh` injects
`shaku` into the contract crate's manifest **on a copy** and requires the gate
to refuse it with one exact `ARCH-002`. It is described under
[Step 3.5](#step-35--the-wall-with-a-real-provider-standing-behind-it), it fails
closed rather than on any non-zero exit, and `check.sh` calls it. Run it alone
with `sh arch002-fixture.sh`.

Phase 1 files four more, as `fast` examples behind the `negative` feature so
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
  for: `cargo tree --invert` over `shaku`, `sdax`, `sdax-tokio`, `sdax-testkit`,
  `tokio` and `iroh` must list no contract-role package. Step 3.5 below records
  what those inversions answer now that a real provider is behind the port.

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
| 3.5 | **DI-E04**: the contract crate still has one dependency while the real iroh-backed provider fills its `CarrierPort`; the gate is seen to refuse an injected `shaku`, on a copy; no framework reaches it transitively or through a public signature | `arch002-fixture.sh`, `check.sh` |
| 3.6 | **The two clocks**: the engine times a budget on a controllable clock of this crate's, the port reads `ClockPort::now_ms` from the contract crate's `FakeClock`, and one test shows they tell the same story while another advances them apart and shows they do not | `real/src/two_clocks.rs`, `real/tests/two_clocks.rs` |

Reproduce from this directory:

```sh
cargo test --locked --offline -p async-witness-real --test peer_carrier
cargo test --locked --offline -p async-witness-real --test peer_release -- --nocapture
cargo test --locked --offline -p async-witness-real --test shaku_assembly
cargo test --locked --offline -p async-witness-real --test shaku_registration
cargo test --locked --offline -p async-witness-real --test differential -- --nocapture --test-threads 1
cargo test --locked --offline -p async-witness-real --test two_clocks
sh arch002-fixture.sh
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

### Step 3.5 — the wall, with a real provider standing behind it

DI-E04 asks whether "the selected real async Glade port remains usable without
framework imports in its contract/pure libraries". Phase 0 could only answer it
about an empty crate. It is answered here against the state that now exists:
`WitnessCarrier`, over the node's own `PeerEndpoint`, is a live `CarrierPort`
implementation that binds real UDP sockets and drives real QUIC — and the port
it implements still has one dependency.

**The declaration.** `cargo tree -p async-witness-ports` is two lines:

```text
async-witness-ports v0.0.0 (…/dev-docs/async-witness/ports)
└── glade-wire v0.0.0 (…/glade/wire-rs)
```

**The inversion**, which is what the manifest check cannot see. From each of
the six frameworks, the workspace members reached are:

| `cargo tree --invert` | reaches |
|---|---|
| `shaku` | `async-witness-fast`, `async-witness-real` |
| `sdax` | `async-witness-real` (directly, and through `sdax-tokio` and the dev-dependency `sdax-testkit`) |
| `sdax-tokio` | `async-witness-real` |
| `sdax-testkit` | `async-witness-real`, as `[dev-dependencies]` |
| `tokio` | `async-witness-real` and `glade-node`; everything else on the path is iroh's own transitive graph |
| `iroh` | `async-witness-real` and `glade-node` |

`async-witness-ports` appears in none of them. `check.sh` asserts that, and
separately asserts that `glade-wire` — the pure crate the port's types come from
— is still a one-line tree of its own.

**The gate is seen to refuse.** `arch002-fixture.sh` injects `shaku` into the
contract crate's manifest and requires this exact diagnostic:

```text
ARCH-002 async-witness-ports: undeclared dependency normal:shaku
```

It **fails closed**, and each branch was exercised rather than assumed:

| If | the fixture says |
|---|---|
| the untouched copy does not report `PASS` first | "the untouched copy does not pass the gate, so nothing below decides anything" |
| `cargo metadata` could not run | "the checker could not run, so its non-zero exit decides nothing" |
| the gate accepts the injected entry | "the gate ACCEPTED a framework dependency in the contract crate" |
| the gate refuses with some other `ARCH-002` | "the gate refused, but not with the diagnostic this fixture is about", printing expected and actual |

The last three were each produced on purpose from a scratch copy of the script —
with the injection removed, with the sibling symlinks removed, and with `tokio`
injected in place of `shaku` (`actual: ARCH-002 async-witness-ports: undeclared
dependency normal:tokio`). A non-zero exit on its own never passes this fixture.

**It runs on a copy, never on the live tree.** The manifests, `Cargo.lock` and
`architecture-policy.json` here are never edited, not even for the length of one
command, so an interrupted run cannot leave the workspace changed. The copy sits
at the same depth below a `glade` root that the real tree has, so the members'
`../../../` path dependencies resolve, and the three siblings they name
(`wire-rs`, `node`, `contracts`) are symlinks the fixture only reads through. It
costs 0.6–0.8 s warm, is entirely offline, and `check.sh` runs it.

**What the fixture does and does not prove**, in the terms of the plan's §4.5.
It proves the *manifest* half fails closed: an allowlist violation in the
contract crate is an error without anyone having thought to forbid that
particular crate, and dependency kind is part of the key, so `dev:shaku` would
be caught as surely as `normal:shaku`. It does not prove anything about
transitive reachability, because the checker shells `cargo metadata --no-deps`
and therefore sees workspace members only — that is what the inversion table
above is for. And it says nothing about the trait half of the gate, which the
`#[cfg]` scanner gap above measures and which is why that gap is recorded rather
than described.

**The public signatures, by inspection.** §8.2's DI-E04 row has a second clause:
no framework type may appear in a contract crate's public signature either.
`ports/src/lib.rs` names `shaku`, `sdax` and `tokio` exactly three times, all in
doc comments saying the crate must not name them. Every type in a public
signature is `std` or `glade-wire`: `FrameType`, `i64`, `String`, `Vec<u8>`,
`Pin<Box<dyn Future + Send>>`, `Result`, and the crate's own `CarrierError` and
`StoreError`. The `std::sync::Mutex` inside `FakeCarrier` and `FakeStore` is a
private field, not a signature.

**And nothing was added to please a container.** `git log -- ports/` is **two
commits**, both Phase 0: `b2b6779` (the skeleton) and `8a7ed44` (the ports and
fakes). Nothing has touched the crate since, so no `Sync`, no lock and no boxed
public future was added to it for Phase 1's bridge, Phase 2's lifecycle or Phase
3's real provider. Two things that were there from the start should still be
said out loud rather than counted as absences, and Phase 4 should say them:

- `ClockPort`, `CarrierPort` and `StorePort` all require `Any + Send + Sync`,
  and the crate's own doc says the `Sync` is what lets Phase 1's facade
  (`trait Carrier: CarrierPort + shaku::Interface`) compile. The bound is not
  *only* Shaku's: an injected port is shared as `Arc<dyn CarrierPort>` across
  tasks on a multi-thread runtime, which needs `Sync` whatever assembles it, and
  `Any` is a `'static` bound any `'static` type already satisfies. All three are
  `std`. But the coincidence is a coincidence, and it is honest to record that
  the port was written knowing what `shaku::Interface` demands.
- `PortFuture` is a boxed public future. It is boxed to stay dyn-compatible —
  `impl Future` in return position is not (E0038) — and it is the **witness's
  own** port, not a Glade contract. `glade-lifecycle-api`'s `ManagedResource`
  still returns `impl Future` and was not touched.

§8.2 records `selection_reopened` for "adding `Sync`, a lock or a boxed public
future **to a Glade contract**". No Glade contract was changed, so neither of
these is that. They belong in the Phase 4 write-up as stated facts.

### Step 3.6 — the two clocks

Plan §4.4: "There are two clocks and the witness must not conflate them." sdax
measures every backoff, `within` deadline and shutdown budget on its **own**
injected `Clock`; Glade's clock port is a separate, Glade-owned thing, and
"using sdax's `Clock` as Glade's clock port would put a framework type in a
contract crate and fail DI-E04 by construction".

**Which engine clock, and why.** Not `sdax_testkit::FakeClock` — the plan's own
update, item 2, rules it out: its `sleep` answers `Pending` without registering
a waker (`sdax-testkit/src/clock.rs:59-71`), so a sleeper on a multi-thread
runtime never wakes, and every witness test is multi-thread because
`TokioRuntime::new` panics on a current-thread handle. The update offers a
scaled real clock or a clock that records wakers; this step takes the **second**
and declares `WitnessClock` in `real/src/two_clocks.rs`. A scaled clock still
measures wall time, so "advance both clocks and see whether they agree" would
become "sleep and hope", and the agreement observed would be a fact about the
machine's load. `WitnessClock` moves only when a test moves it, so the whole
suite runs in 0.00 s and says nothing about the scheduler.

It adds one thing `sdax_testkit::FakeClock` does not have, and the step turns on
it: `advance` returns **how many registered deadlines that advance crossed**. A
test can then say *where* the engine's budget expired, not merely that it
eventually did.

**The observable.** One step asks the engine for `cx.timeout(500 ms, pending)`
— timed by the engine, on the engine's clock — and reads `ClockPort::now_ms`
from the contract crate's `FakeClock` on either side of the wait. The run's
export is the port's own measure of the engine's budget. The two clocks start at
different origins on purpose, the engine at zero and the port at a
wall-clock-shaped instant, so what they must agree about is an **interval**,
which is the only thing two clocks can honestly agree about.

| Test | What it shows |
|---|---|
| `the_engine_deadline_falls_where_the_port_clock_says_it_should` | both clocks advanced in ten equal ticks; the first nine cross **no** registered deadline and the tenth crosses exactly one, and the port's reading of the same wait is exactly 500 ms. `engine.asked_for() == [500 ms]` first, so the deadline advanced past is the body's and not some other wait the engine took out |
| `advancing_the_two_clocks_apart_makes_them_disagree` | the same code path with the port clock frozen (the engine's budget expires in full, the port reports **0 ms**) and with the port clock at twice the rate (**1000 ms**), alongside the lockstep value the agreement test asserts. The agreement is therefore falsifiable, by construction, from the same `observe` |
| `the_two_clocks_are_two_types_from_two_crates` | `WitnessClock` is `async_witness_real::…` implementing sdax's `Clock`; `FakeClock` is `async_witness_ports::…` implementing `ClockPort`. The load-bearing enforcement is the gate, not this test |

**Two things worth passing on.** `Running::poll` is what launches the engine
(`sdax-tokio/src/running.rs:339`, `me.launch()`), so a test that advances a
clock must do it in a **second branch of the same await** — awaiting the run
first and ticking afterwards hangs, because nothing has registered a deadline to
advance past. And the wait for "the body is now waiting" is for the **clock's**
registration, not for an announcement from the body: the deadline is fixed when
`Clock::sleep` is called and the waker exists one poll later, so waiting for the
waker is what removes the race with the first tick.

**No socket.** The plan draws a fakes-or-real boundary and this step is on the
fakes side: the two-clock plan holds no resource, binds nothing, and takes no
wall time.

### Measured

Toolchain `rustc 1.96.0 (ac68faa20 2026-05-25)`, macOS 26.6 on Apple silicon,
12 cores, no `RUSTC_WRAPPER` and no compiler cache, `dev` profile. Phase 4's
Step 4.1 owns the measured table for the result document; these are Phase 3's
working figures.

| Measurement | Result |
|---|---|
| `sh check.sh` (all three members, warm; 71 tests, `arch002-fixture.sh` included) | 4.9–5.9 s |
| `sh arch002-fixture.sh` alone, warm (a copy, two checker runs, offline) | 0.64–0.79 s |
| Cold build of the whole workspace (`--lib --tests --no-run`, empty `CARGO_TARGET_DIR`) | 33.0 s, 1.6 GB of artefacts |
| Warm incremental rebuild of the `real` lib after one touched file | 1.1 s |
| `cargo test -p async-witness-real --lib --tests` (warm, 46 tests) | 2.84–2.86 s over five consecutive runs |
| `tests/two_clocks.rs` (3 tests, five simulated seconds of engine time) | 0.00 s, and 0.00 s over 40 consecutive runs — the engine clock consumes no wall time at all |
| `tests/peer_carrier.rs` alone (4 tests, two real endpoints per run) | 0.04 s |
| `tests/peer_release.rs` (5 tests) | 2.05 s, of which three deliberate 2 s bounds |
| `tests/shaku_assembly.rs` (4 tests, two real endpoints per run) | 0.04 s |
| `tests/differential.rs` (5 tests, 22 real endpoints) | 0.09 s in parallel, 0.21 s single-threaded |
| Port free after a clean run | 4.7–15.2 µs, over three runs of both endpoints — already free at the first poll |
| Port free after the escaped clone was dropped | 23–108 µs |
| Port with an escaped clone still alive | still bound at 2.004–2.006 s, i.e. the whole bound |
| Port with an escaped link `Connection` still alive | still bound at 2.004–2.005 s; free 41–66 µs after it was dropped, while the uncloned dialer side was free in 108–143 µs |
| Under load: 4 parallel copies of every `real` test binary while a whole-workspace `cargo test` ran | all green, 44 binaries and 184 tests; `peer_carrier` went from 0.04 s to 1.12 s, the suites with a 2 s bound stayed at 2.05–2.10 s, and `two_clocks` stayed at 0.01–0.07 s |
| Under load: 4 parallel copies of `arch002-fixture.sh` beside the same load | all four refused the injection with the same diagnostic; no temp directory left behind |

The clean-run figure is three orders of magnitude below the node's own 6–10 ms
(`iroh_carrier.rs:218-221`), and the difference is not a faster machine: the
node's test asks immediately after `close` resolves, whereas a run has a second
endpoint to release and a report to assemble in between, so iroh's driver has
already wound down by the time the check happens. The bound stays at two
seconds regardless — it is there for the case where it has not.

## Measured — Step 4.1

This is the consolidated measurement the plan's Step 4.1 asks for, per
`LibraryBoundaryAndTestingPolicy.md:66`: "Each adopting project MUST record its
fast command, machine/toolchain context, and measured budget. Measure test
execution, warm incremental build plus tests, and cold build separately. Do not
describe an unmeasured target as an achieved performance guarantee."

**Conditions**, one set, for every figure below.

| | |
|---|---|
| Date | 2026-09-22 |
| Tree | glade `5f2658f`, 71 tests: 7 `ports`, 18 `fast`, 46 `real` |
| Toolchain | `rustc 1.96.0 (ac68faa20 2026-05-25)`, `cargo 1.96.0 (30a34c682 2026-05-25)`, host `aarch64-apple-darwin` |
| Machine | Apple M3 Pro, 12 cores, 36 GiB memory, macOS 26.6.2 (build 25G83) |
| Profile | `dev`; no `RUSTC_WRAPPER` and no compiler cache |
| Fast command | `sh check.sh fast` for the fast member; `sh check.sh` for the whole gate |

**The machine was not idle, and these are wall times on a loaded machine.** The
owner's own application instance was running throughout on ports 5173, 8080 and
9099 and was not touched, alongside a browser, an editor and Spotlight
indexing; none of that load is the witness's. One-minute load averages are
recorded beside every set and ranged from 13.9 to 89.4 across the measurement
window on a 12-core machine. Read each figure as an upper bound under real
desktop load, not as a best case and not as a guarantee.

| Measurement | Repeats | min | median | max | 1-min load, before → after |
|---|---|---|---|---|---|
| `cargo test --locked --offline -p async-witness-ports --lib --tests`, warm (7 tests) | 5 | 0.09 s | 0.09 s | 0.21 s | 16.17 → 15.03 |
| `cargo test --locked --offline -p async-witness-fast --lib --tests`, warm (18 tests) | 5 | 0.11 s | 0.11 s | 0.14 s | 15.03 → 15.03 |
| `cargo test --locked --offline -p async-witness-real --lib --tests`, warm (46 tests) | 5 | 2.95 s | 3.04 s | 3.39 s | 15.03 → 13.92 |
| `sh check.sh`, warm — all three members, 71 tests, `arch002-fixture.sh` included | 5 | 4.92 s | 7.47 s | 8.05 s | 13.92 → 52.61 |
| Warm incremental: `touch fast/src/lib.rs`, then rebuild and run (18 tests) | 5 | 2.99 s | 3.05 s | 3.35 s | 67.90 → 49.42 |
| Warm incremental: `touch real/src/lib.rs`, then rebuild and run (46 tests) | 5 | 11.12 s | 11.25 s | 11.72 s | 49.42 → 24.76 |
| Cold build of `fast` alone, `--lib --tests --no-run`, empty `CARGO_TARGET_DIR` | 1 | — | 7.10 s | — | 53.89 → 53.18 |
| Cold build of `real` alone, same command | 1 | — | 42.88 s | — | 53.18 → 89.38 |

Cold artefacts: `fast` 71 MiB (72,764 KiB), `real` 1.6 GiB (1,665,924 KiB). Both
cold builds used a scratch `CARGO_TARGET_DIR` outside the workspace, which was
deleted afterwards; this workspace's own `target/` was not used for them and not
disturbed by them. Free space was checked before and after and never fell below
50 GiB.

The three things the policy line asks to be kept apart are kept apart above:
**test execution** is the first four rows, where nothing recompiles; **warm
incremental build plus tests** is the two `touch` rows, which rebuild the
library and every test binary and then run the tests; **cold build** is the last
two rows.

**None of this is a budget.** The plan sets no seconds threshold for the witness
and the policy "deliberately does not impose a universal seconds threshold"
(`LibraryBoundaryAndTestingPolicy.md:68`). These are measurements, not achieved
performance guarantees. The plan's §9.2 stop trigger "The real target's build
cost makes Phase 3 impractical" did not fire: Phase 3 completed on the real
target and the WebSocket runner-up was never needed.

### Where this disagrees with Phase 3's working figures above

The Phase 3 table stays where it is; it is that phase's own record. **This
section is the current one.** The test count is identical in both (71), so every
difference is load, with one exception that is a different measurement
altogether:

- `sh check.sh`. Phase 3 recorded 4.9–5.9 s; this set measured 4.92–8.05 s over
  five runs. The minimum agrees almost exactly. The median and maximum are
  higher because the one-minute load rose from 13.9 to 52.6 while the five runs
  were going, from work that is not the witness's.
- Cold build. Phase 3 recorded 33.0 s and 1.6 GB for the **whole workspace**;
  this set built `real` **alone** in 42.9 s for the same 1.6 GiB, at a load of
  53 rising to 89. The artefact size matches because `real` is what dominates
  it — it pulls iroh, `glade-node` and the two sdax crates — and adding `ports`
  and `fast` costs little on top. The wall time is higher for load, not for a
  larger build.
- `cargo test -p async-witness-real`. Phase 3 recorded 2.84–2.86 s over five
  consecutive runs; this set measured 2.95–3.39 s over five. The `peer_release`
  binary's three deliberate two-second bounds dominate the figure in both, which
  is why load moves it so little.
- `sh arch002-fixture.sh` alone. Phase 3 recorded 0.64–0.79 s warm; measured
  once here at 1.06 s, at a load above 50. Same cause.
- Warm incremental. Phase 3's 1.1 s is **not** the same measurement as the row
  above: it timed `cargo build` of the `real` library after one touched file.
  The row above rebuilds that library and all ten of its test binaries and then
  runs 46 tests, which is what the policy line means by "warm incremental build
  plus tests". The two numbers are not comparable and neither supersedes the
  other.
