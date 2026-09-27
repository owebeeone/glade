# The node assembly (plan Step 3.2)

Design note, 2026-09-24, written before the code, against glade `4255ef5`; the
last section was filled in once the code existed. The spec is plan Step 3.2
(`dev-docs/GladeFirstSlicePlan.md`, glade-wz root) and the owner's ruling under
it: the assembled composition root is a path in `glade-node`, chosen by
`GLADE_NODE_ASSEMBLED=1`, off by default. Unset runs today's hand-written `run()`;
`1` runs the assembled root; any other value refuses to start (exit 1, naming the
variable). Steps 3.3 (sdax) and 3.4 (the journeys) are out of scope.

## What is built

`glade/node/src/assembly.rs` declares one Shaku module, `NodeAssembly`: the six
recipes of `arch1/InjectionGraphRefinement.md:12-19`, the grant and signer
bindings of Step 3.1, and a configuration binding. One built `NodeAssembly` is one
node scope (`SharedWithin[NodeScope]`). The assembled root in
`src/bin/glade-node.rs` builds it once, resolves its participants, and composes
the unchanged `Server` API (`open`, `adopt_boot`, `enable_mesh`, `connect_peer`,
`serve_workspace`, `run`). Nothing in `server.rs`, `mesh.rs`, `ws.rs` or
`claims.rs` changes.

## The bindings

Every port is bridged by an assembly-local facade, `trait F: Port +
shaku::Interface` with `impl<T: Port + 'static> F for T {}` (`AsyncWitnessResult.md`
§5, caveat 4), so no port names Shaku.

| Binding (facade) | Port, and where it is defined | Provider on the assembled path | Fake in a test composition | Consumers |
| --- | --- | --- | --- | --- |
| `clock_binding` (`Clock`) | `ClockPort`, `glade-clock-api` | `SystemClock`: the wall clock, as `sysdir::now_ms()` reads it | one shared atomic clock (CL-001, CL-002) | `Directory` (who serves, read at the clock); `Admission` (a decision's instant) |
| `peer_carrier_binding` (`PeerCarrier`) | `CarrierPort`, `glade-carrier-api` | `IrohCarrier` (plan Step 4.2c), lent no endpoint key by either root, so fail-closed (`bind` refused, `dial` `Closed`, `accept` `Ok(None)`), as `PendingIrohAdapter` was before it; built with the record transport `Records` injects, never called | port A on a fake network (CA-001..005) | `Sessions` (peer role); the record transport |
| `client_carrier_binding` (`ClientCarrier`) | `CarrierPort`, `glade-carrier-api` | `PendingWebSocketAdapter`, the same, `#[lazy]` and never resolved | port B on the same network, a second occurrence | `Sessions` (client role) |
| `record_transport_binding` (`RecordTransport`) | `TransportPort`, node-local | `CarrierTransport` over the peer occurrence: a view, never overridden | the same view, over port A | `Records` |
| `directory_host_binding` (`RecordHost`) | `RecordHostPort`, node-local | `Records` over the booted instance (its `Registry` and `BlobStore`), lent by the root | `Records` over the node's own `Registry` and `MemStore` (the in-memory store and registry) | `Directory` |
| `directory_profile_binding` (`RecordProfile`) | `RecordProfilePort`, node-local | `DirectoryRules`: pure, the same in every composition | not overridden | `Records` |
| grant (`Grants`) | `GrantPort`, `glade-grant-api` | `PendingGrantFold`: every check `Unavailable` (GR-003), `#[lazy]` | an in-memory fold, revocation wins (GR-001..003) | `Admission` |
| signer (`Signer`) | `SignerPort`, `glade-signer-api` | `NodeSigner` (plan Step 4.1a): Ed25519 over the booted instance's key, lent as its component parameters; lent none, sign and verify `Unavailable` (SI-003); `#[lazy]` | a keyed test signer, FNV over a per-node secret (SI-001..003) | none until 4.1b |
| configuration (`Config`) | `ConfigPort`, node-local | `CommandLine`: the arguments the root parsed | fixed `Settings` | the root; 4.5's carriers |

Roles are told apart by binding occurrence: `PeerCarrier` and `ClientCarrier` are
two Shaku interfaces over one port, so each role is its own binding, overridden
on its own. Nothing is bound to `CarrierPort` itself, so a consumer that asks for
a carrier by port type alone does not compile (below). The participants are
`Directory`, `Admission`, `Sessions` and `Records`, the record host itself; each
injects only the ports in its row and hands them back as ports, never facades.

**Node-local ports.** `TransportPort` (`push` encoded records to a peer over one
link), `RecordHostPort` (`append`, idempotent on a byte-identical record;
`register` an app file; `ingest` a carried op; `who_serves` at an instant),
`RecordProfilePort` (`share`, and whether it `hosts` a stream) and `ConfigPort`
(`settings`) have no contract crate and are defined in `assembly.rs`, framework
free. They name node types (`Record`, `Op`, `AppDecl`): node seams, not public
contracts. One moves to `glade/contracts` when a second consumer needs it, as
`ConfigPort` will at 4.5.

## DirectoryRules and the cycle

Records needs the directory profile to know what it hosts; the Directory facade
needs Records to read and write. With Directory supplying the profile, that was a
constructor cycle (`InjectionGraphRefinement.md:35-40`). `DirectoryRules` is the
profile on its own: pure, nothing injected, delegating to the node's constants
(`HOME`, the nine `dir.*` stream ids). Construction runs rules, then Records,
then Directory. Records consults it in `ingest`: an op on another share, or on a
stream the profile does not host, is refused before the registry's
verify-as-ingest sees it (3.4's wrong-scope journey stands on this). That verify
path is the registry's own `Registry::ingest`, made `pub(crate)`; no rule is
copied. The cycle, rebuilt with Directory as the profile, is a negative example.

## One iroh-facing occurrence

The peer carrier is registered once. `CarrierTransport`, the record transport's
provider, injects that one `Arc<dyn PeerCarrier>` and implements `TransportPort`
over it, so within a scope the two recipes share one occurrence by construction,
not by a second registration. Tests assert it by `Arc::ptr_eq` against the
`Sessions` peer role, and that it is not the client occurrence.

## Scopes

Within one built module every resolution of a binding, and every consumer's
injected copy, is one occurrence. A sibling test node is a second build: it
shares nothing, by pointer or by behaviour, and two nodes meet only through the
fake network whose addresses they bind. A `#[lazy]` slot is Shaku's `OnceLock`,
so a concurrent first resolution constructs once per scope. Construction is
eager unless a binding is overridden or `#[lazy]`; a test composition overrides
every provider marked real above, and the recorder it binds as its construction
observer (`Constructions`, which the module binds to `Unobserved`) sees none
built. A process-wide counter did this until the process-globals plan's Step
4.1 (glade `a1f97ee`).

## The assembled path: real now, Phase 4 next, what 3.3 and 3.4 build on

The root parses the arguments (`Settings`, `run()`'s parse), loads every `--app`
file before anything is written, then boots the instance: acquisition is I/O and
belongs to the root, never to a constructor. It builds the module with the
settings and an instance slot as component parameters, prints `run()`'s lines,
answers "home served" and registers each app through `Directory`, then takes the
instance back out of the slot by value for `Server::adopt_boot`; `Records` then
answers `NotOpen`. The root still binds the iroh endpoint and the TCP listener,
and the Server runs them, as in `run()`. A real provider built without its handle
refuses; it never acquires one. The assembled root also prints one stderr line
naming itself, so a test can tell the roots apart.

Real now: the clock, the record host over the booted instance, the rules, the
configuration, since 4.1a the signer (Ed25519), and since 4.2c the iroh
`CarrierPort` adapter, lent no key. Phase 4 replaces the two remaining
`Pending*` stand-ins: 4.3 the grant fold with its serve-hop consult, and a
WebSocket `CarrierPort` adapter. 4.5 gives the iroh adapter the bind address
CA-004's re-bind needs; the mesh and the WS server then move onto the carrier
bindings, and 4.4 makes the served store the record host. Step 3.3's sdax plan takes over the root's three acquisitions
(instance, endpoint, listener) and releases them in reverse, assembling the module
over the acquired handles inside a step, as the witness did; the slot's
take-by-value is the discipline `PeerEndpoint::close(self)` also needs. Step 3.4's
journeys run on the test composition: the fake clock drives renewal and expiry,
the fake network loss and duplicates, the fake fold denied authority, and
`Records::ingest` wrong scope.

## Named gaps

- AR-03 holds for the module's consumers only. The Server's internals still read
  `sysdir::now_ms()` (claims, mesh routing, boot's presence claim) and own their
  transports; moving each behind a binding is 3.4's and Phase 4's work, one
  consumer at a time, each with a failing test first.
- CA-001..005 run on the fake network and, since 4.2c, on the iroh adapter
  over loopback; no root lends that adapter a key, so nothing binds it.
  The pending grant fold passes the fail-closed half of its suite (GR-003)
  and nothing more; the Ed25519 signer passes SI-001..003 (plan Step 4.1a).
- The node-local ports have no shared conformance suite; the fake host is the
  node's own `Registry` over `MemStore`, so its fold is the real fold.
- DI-E01's "no I/O" rests on construction (every I/O-capable provider
  overridden, the rest in memory or pure) and on review: `glade-node` links
  tokio, iroh and `std::fs`, so no manifest check excludes hidden I/O, unlike the
  witness's `fast` crate (its caveat 7).

## The negative examples (DI-E03)

`compile_fail` doctests in `assembly.rs`'s module docs, run by `cargo test` and so
by the node gate, beside one doctest that builds the complete module. Stable
rustdoc checks only that each fails, so each was also extracted verbatim and
compiled alone (rustc 1.96.0): every error it produced is of the kind below.

| Example | Error | Diagnostic |
| --- | --- | --- |
| a missing binding: `DirectoryFacade` alone | E0277 (20 errors) | the trait bound `Unbound: HasComponent<(dyn Clock + 'static)>` is not satisfied; the same for `dyn RecordHost`; then `Unbound` cannot be shared between threads safely |
| a construction cycle: a directory supplying the record profile, with `Records` | E0275 (6) | overflow evaluating the requirement `Cyclic: HasComponent<(dyn RecordProfile + 'static)>` (and `dyn RecordHost`) |
| an ambiguous role: a second provider for the peer role | E0119 (1) | conflicting implementations of trait `HasComponent<(dyn PeerCarrier + 'static)>` for type `TwoPeers` |
| a carrier asked for by port type alone | E0277 (14) | the trait bound `(dyn CarrierPort + 'static): Interface` is not satisfied; `ByPortType: HasComponent<(dyn CarrierPort + 'static)>` is not satisfied |

As in the witness (its caveats 1 and 6), E0275 names a bound, not the loop, and a
missing binding and a port-type request read alike. The ambiguous role is refused
where it is declared (E0119), each role being its own interface; the witness's
keyed roles were refused at the request (E0599).

## Lifecycle (plan Step 3.3)

Design addition, 2026-09-24, written before the code against glade `3f97a3d`
and corrected where the code taught otherwise; the spec is plan Step 3.3. The
assembled root starts one sdax plan (`src/lifecycle.rs`; `sdax`, `sdax-tokio`,
and `sdax-testkit` for tests, all at `ccf06e76…`) and awaits it where `run()`
awaits `server.run(listener)`. The hand-written root builds none and behaves as
before.

**The plan.** Resident, fail-fast, a 30 s shutdown budget, 10 s for each owner
to stop. The legacy form declares the same nodes; those it does not use hold
nothing, so each form prints the same lines as before.

| Node | Kind | Needs | Acquire, or start | Release, or stop |
| --- | --- | --- | --- | --- |
| `Instance` | resource | | `boot_at` the instance dir; prints `instance`, `node` | drops a `Boot` that was never adopted |
| `Assembly` | step | Instance | `NodeAssembly` over the acquired instance (3.2's slot); prints `registry` and `app` | |
| `Storage` | resource | Instance, Assembly | `Server::open` with owned tasks; adopts the instance by value | takes the `Server` by value; a cleanup failure if anything else still owns its state; dropping it releases the instance lock |
| `PeerCarrier` | resource | Instance, Storage | `PeerEndpoint::bind_with` into an `EndpointSlot` | takes the endpoint out: `PeerEndpoint::close(self)` |
| `ClientCarrier` | resource | Storage | `TcpListener::bind` | takes the listener out and drops it |
| `Records` | service | Storage, PeerCarrier | owner of the renewal loop and record pushes | admission closed, tasks cancelled and joined |
| `Sessions` | service | Storage, PeerCarrier, ClientCarrier | enables the mesh over the slot (`peer`); owner of links, streams, subscriptions, forwards and client sessions; runs `Server::run`'s accept loop once clients are admitted | admission and accept loop closed, tasks cancelled and joined, then every link closed by value and forwarded interests cleared |
| `Peers` | step | Storage, Sessions | `connect_peer` per `--peer` | |
| `Workspaces` | step | Assembly, Storage, Peers, Records | `serve_workspace` per declared workspace | |
| `Listening` | step | Sessions, ClientCarrier, Workspaces | prints `listening`, then admits clients | |

**The release graph** is the reverse of those edges. Four of them are the
partial order of `arch1/InjectionGraphRefinement.md:42-45`: Records→Storage and
Records→PeerCarrier, Sessions→PeerCarrier and Sessions→ClientCarrier. Records
and Sessions stop concurrently, then the two carriers are released concurrently,
then Storage, then Instance. Each owner's stop follows
`RuntimeAndAssurance.md:91-93`: admission closes, its children are cancelled at
their next await point and joined, and only then are their resources released.
`tests/release_order.rs` reproduces the witness's partial-order test on this
plan, statically and by a simulated run (sdax-testkit).

**The task-owner seam** (`src/tasks.rs`). `Shared` gains a `Tasks`, and every
production spawn becomes `shared.tasks.spawn(Site::…, future)`. `Server::open`
makes it unowned, and then the call is `tokio::spawn`: the same detached task at
the same place, and the two writers are still aborted through their handle. The
hand-written root never leaves that mode. The assembled root makes it owned, and
the task is sent to the owner of its site's role, which spawns it into its own
`JoinSet` and reaps it. After that owner stops, a send is refused and the
future is dropped: admission is closed. There is not one sdax instance per task,
for two reasons. The node spawns per stream and per push, and an sdax run keeps
history for every instance it ever spawned ("historical inspection and trace
storage still grow with churn", `sdax/src/host/engine/compaction.rs:22-23`), so
a resident node would grow without bound. And the spawns sit deep in shared
code, which would then have to carry a service's `Cx`. sdax sees the owner: its
stop is bounded by `stop_within`, and an owner that cannot finish is abandoned
and named in `report.incomplete`, while the `JoinSet` dropped with it aborts
whatever is left.

| Site | Task | Owner |
| --- | --- | --- |
| `mesh.rs:124` | the peer accept loop | Sessions |
| `mesh.rs:131, 166, 175, 178, 193` | a link's driver, its unlink watcher, its stream dispatcher, one inbound stream, the acceptor's stream 0 | Sessions |
| `mesh.rs:269`, `:328` | a served subscription's writer; a forwarded interest | Sessions |
| `mesh.rs:241` | a record push to one link | Records |
| `claims.rs:115` | the renewal loop | Records |
| `exchange.rs:134`, `:185` | a forwarded exchange; a forwarded `workspace.create` | Sessions |
| `server.rs:99`, `:120` | a client session; its writer | Sessions |

The two `exchange.rs` spawns, and `server.rs:99` and `:120`, which the plan
does not name either, come under the plan with no special handling, so none is
left as a gap. Links are acquired by the accept loop and `Peers` (HELLO
completed) and registered in the mesh's link table; once no task that could
register one is left, `Sessions` takes the table by value and closes each
connection (`glade node stopping`) and clears the forwarded interests. Served
peer subscriptions end with their streams, client ones with their sessions.

**Handles given up by value.** The mesh reaches its endpoint only through the
`EndpointSlot` it shares with `PeerCarrier`; accept and dial hold a clone only
while they run, so after the release the mesh holds nothing. Service handles
hold no node state (each serve body takes its own). `Storage`'s last-owner check
(`Arc::strong_count`) turns a leaked task into a cleanup failure in the report,
where it would otherwise show only as a socket still bound.

**Stop signal and exit status.** The assembled root installs SIGTERM and SIGINT
handlers before it starts the plan (Ctrl-C only, off Unix). Every signal asks the
run to shut down. Start-up still in flight is cancelled at once, sdax's settle
(T5). The release graph then runs within the budget, and a later signal changes
nothing, since sdax never interrupts a cleanup that has begun (INV-7). A clean
stop exits 0; clean means `report.is_clean()`: outcome `Ok`, and no fault, cleanup
failure, `incomplete` or `ambiguous` record. Anything else exits 1. Each fault's
message goes to stderr, as `run()` prints a failed start, and then the report if
it records more than faults. `tracked()` is never consulted. The hand-written
root installs no handler, so a signal still ends it by the signal itself.

**Retry loops stay above sdax.** No node declares `Retry`; the release graph runs
once. The node's retries stay in the loops the owners run (the accept loop goes
on past a bad handshake, the next renewal tick retries a failed one, a later
subscribe retries a lapsed forward). A failed release is a cleanup failure and
exit 1; nothing runs it again.

**Gate.** `glade-node` may declare the three sdax crates; confinement lets only
it see them, and the contracts none. `arch002-fixture.sh` now injects `dill` at
the `=0.17.0` `arch1/DependencyInjectionEvaluation.md` measured, the framework
that evaluation weighed against Shaku and did not select, since `sdax` would now
be accepted. The lockfile moves tokio 1.52.3→1.53.1 and tokio-util
0.7.18→0.7.19, as `sdax-tokio` pins them, for both paths; the manifest keeps
`tokio = "1"` a range and adds tokio's `signal` feature, already compiled in
through iroh's graph.

**Named gaps.** Client sessions are cancelled, not drained. The listener now
binds before the mesh is enabled, because `Sessions` needs it. The lines keep
their order, and no client is accepted before `listening`, but a start whose
port is taken now fails before printing `peer`. `TokioRuntime::tracked()` does
not count owned tasks. The Shaku module still assembles over the instance alone,
because no `CarrierPort` adapter exists before Phase 4.

## Journeys (plan Step 3.4)

Design addition, 2026-09-24, written before the code against glade `0a8733d`,
with the fast loop's figures filled in once measured; the spec is plan Step
3.4, and AR-05 (`arch1/RuntimeAndAssurance.md:123`) is the criterion.
`tests/journeys/` is a test binary of its own. Nothing in it starts a runtime,
a socket or a file: `fakes::run` polls every future, and the fake clock and
the test's own steps are the whole schedule (LBT-008).

**Two test nodes, one fixed route.** A and B are each a whole test composition
of `NodeAssembly` (3.2's, every real provider overridden), bound on one fake
network at the addresses their configuration names; A's `Config` names B as
its one peer, the fixed authorized locator (no lookup, no referral). The
registration is the node's own family, which is the one the plan's steps name
(slice profile §8 item 1): a `WorkspaceEntry` and a `ServeClaim` on `home`,
with the lease `claims.rs` mints (`LEASE_TTL_MS`, renewed every
`RENEW_EVERY_MS` with the same epoch), stamped at the node's injected clock. A
publishes through its directory's record host, which persists through a
volatile engine; A's record transport pushes the persisted ops, a frame each,
over its peer carrier; B's test code plays the session: it accepts, decodes
and hands each op to B's record host (`ingest`). The lookup is
`Directory::serves`, at each node's own clock. `claims.rs` itself is not run:
it sleeps on tokio and stamps with `sysdir::now_ms()` (the AR-03 gap above), so
the journeys write the records it writes.

**Test-only providers** (`tests/journeys/faults.rs`), beside 3.2's fakes,
which the binary shares by `#[path]`; each says what it does not prove.
`FaultyPort` wraps a fake port: a dialed link can be given one transport
failure at its k-th frame, before or after that frame arrives; the link then
ends and the sender's `send` answers `Transport`, which the carrier contract
allows ("unless the transport fails"; `send` is no acknowledgement). Passing
through, it runs CA-001..004. `VolatileStore` is the `StoreApi` engine
a host persists through and a journey reads back: the last snapshot, in memory.
`LiveGrants` is a grant fold a journey appends to between two decisions (the
contract's fixture fold, then later records); it runs GR-001..003.

**One production seam**, additive: `Records::in_memory_over(profile, transport,
store)`, the in-memory record host persisting through a given engine;
`Records::in_memory` keeps `MemStore`, and no composition path builds either.

| Journey | Drives (binding, port) | Asserts | The fakes do not prove | Phase 4 |
| --- | --- | --- | --- | --- |
| `publish` (i) | A: `directory_host_binding` `append` (entry, claim); `record_transport_binding` `push` over `peer_carrier_binding` to the configured peer. B: its peer carrier's `accept`, `ingest`. Both: `Directory::serves` at `clock_binding` | `Ok(true)` twice, `push` `Ok(2)`, both ingested; A and B answer A; B's persisted snapshot equals A's | a transport, durability, a signature (the ops are unsigned) | the iroh `CarrierPort` (4.2, 4.5); the served store as record host (4.4); signing (4.1); 4.6's route |
| `exact_retry` (i) | A: `append` of the same record, before and after its lease lapses | `Ok(false)` both times, the snapshot unchanged, the lease not extended, nothing revived; only a new stamp is `Ok(true)` | a restart: the engine is volatile | 4.4: the exact retry across a restart |
| `lost_acknowledgement` (i) | A's push over a link whose transport fails after the last frame arrives; A retries (`append` is `Ok(false)`, nothing to re-mint) and pushes the same persisted bytes; B ingests each op twice | the first `push` is `Transport`; B holds each op once, its snapshot byte-equal, answering A; each re-delivered op answers `Ok`, a duplicate (pinned as `Equivocation` until the ruling of 2026-09-24, below) | an acknowledgement: the push has none | the iroh adapter's failure evidence and TR-002's unknown outcome (4.2, 4.5); 4.4 |
| `renewal` (ii) | A: `append` of a renewal every `RENEW_EVERY_MS` of fake time, each pushed to B | each `Ok(true)`, one epoch; past the first lease both answer A; a lease after the last renewal both answer none | `claims.rs`'s loop and its clock reads | `claims.rs` on the clock binding, the `Records` owner (3.3), over iroh |
| `expiry` (ii) | A and B with a clock each, B's ahead | live at `lease - 1`, none at `lease`, at each reader's instant; B sees the lapse first; no record changes | a real clock, or a skew bound | the system clock; clock uncertainty is not decided (SP-C2) |
| `wrong_scope` (ii) | A pushes an op on another share, one on a stream the profile does not host, then one in scope; B: `ingest` via `directory_profile_binding` | `OutOfScope` twice, before verification, nothing persisted, B answering none; then `Ok` and A | a referral (none in the slice, SP-N1); a copied or forged op: ops carry no signature (SP-P3(a)); who may connect | 4.1's origin signatures; 4.2's accept-time check |
| `unknown_or_denied_authority` (ii) | `Admission` (grant binding, `GrantPort`, over `LiveGrants`) at the fake clock; A's `Directory::register`, read back through the R9 fold (`bindings_of`) | an unknown holder `NoGrant`; a grant copied to another holder, or an operator's to its node, admits nothing; a revocation between two decisions makes the second `Revoked` and a later grant stays `Revoked`; an unreadable fold `Unavailable`; an unknown authority token refuses the file; a surface's declared authority (`share` or `external`) is its live declaration's; an app's retraction withdraws its own only, and another app's later declaration stands | enforcement: no serve path consults either before 4.3; the fold's chain, issuer or persistence | 4.3's grant adapter over the node's fold, at the serve hop |
| `partial_lookup` (ii) | A pushes three stamps of its lease, one link each: the second lost, the third delivered; then the rest, from B's head | B refuses the third (`Gap`) and answers from its prefix (none, where A answers A); after the retry, A | truncation, a limit or an observed-at stamp: the node's lookup answers one node or none; clock uncertainty (slice profile §8 item 11) | 4.6's sync round over the route |

**Pinned, not endorsed.** `Registry::ingest` answers a byte-identical
re-delivery `Equivocation` (`registry.rs:283-286`), against its own comment and
the wire store's `Duplicate` (`store.rs:266-272`). The fold is unchanged either
way, and `lost_acknowledgement` pins the label; a fix changes what boot does
with a duplicated stored record, so it is the owner's call, with that pin,
turned round, as its failing test. *(Superseded: the owner ruled on 2026-09-24
to fix it in Step 4.4, and the pin is turned round; see "Durable store and
restart", below.)*

**Fast loop** (LBT-010), from the glade-wz root: `cargo test --offline --locked
--manifest-path glade/node/Cargo.toml --test journeys --test assembly`, the test
composition's two binaries, 35 tests. Measured on an Apple M3 Pro (12 cores),
Rust 1.96, with `/usr/bin/time -p` around the command (user+sys includes cargo
and both binaries), 2026-09-24:

- warm, 7 runs at load average 3.5: wall 0.14-0.18 s, CPU 0.13-0.17 s. Cargo's
  start-up and freshness check are nearly all of it; each binary reports 0.00 s.
- after touching one journey file, 5 runs at load 4.3 (rebuild the journeys
  binary, then run both): wall 0.93-1.18 s, CPU 1.17-1.32 s.

Proposed budget: warm 1.0 s wall, and 3.0 s wall after an edit to one journey
file, about six and three times the figures, for a machine other sessions
load. Compare two trees by CPU time over interleaved runs, not one wall time.

## Durable store and restart (plan Step 4.4)

Design addition, 2026-09-24, written before the code against glade `f39daa9`;
the measured figures were filled in afterwards. The spec is plan Step 4.4 and
slice profile SP-L1 (`dev-docs/glade/GladeFirstSliceProfile.md` §4): durable
local acceptance precedes any sync effect.

**The real store, on each path.** A booted node has two stores. Both
composition roots run the same `sysdir`, `claims.rs` and `Server` code, so the
stores are the same on both paths.

| Store | Where | Holds | Written by | Read by | Port it would stand behind |
| --- | --- | --- | --- | --- | --- |
| `records.json`, through `BlobStore` (`StoreApi`) | the instance directory | this node's own directory records as one snapshot: the fold's ops and heads | first-boot presence (`sysdir::boot_at`); app registration (`run()`, and on the assembled path `Records::register` in the `Assembly` step); `claims.rs`'s mints after adoption | boot (`Registry::from_snapshot`, verify-as-ingest) | the persistence port, `SnapshotStore` |
| the served store, `store::Store` | `<instance>/cache/store/`, or the second positional, which the legacy form requires (the owner's ruling of 2026-09-26; it once defaulted to a temp directory) | every op the node serves, in per-(share, origin) op logs: its own records (seeded from records.json at adoption, then published), peers' records, clients' ops | `Server` (`Frame::Ops`); the mesh (pull, push, forward); `claims::publish` | every serve path; the peer pull announces its heads | the operation-store port, `DurableOperationStore` |

On the assembled path `Records` is the record host over records.json until
`Storage` adopts the instance. In the journeys, `Records` persists through a
`StoreApi` engine: `VolatileStore` in the fast loop, and `BlobStore` over a
temp directory in the new durable target (below). No journey reaches the
served store.

**records.json against `SnapshotStore`'s rules**, as it stands after this step:

- Atomic replace: yes. The snapshot is written to `records.json.tmp` and renamed
  over `records.json`, so a reader sees the old file or the new one, whole.
- Durability: this step adds `sync_all` on the temp file before the rename, and
  on the directory after it (Unix; std issues `F_FULLFSYNC` on macOS; a save
  measured 3-10 ms here). Until now a save reached only the page cache, so an
  OS crash or power loss could drop an acknowledged save, or leave the renamed
  file without its data. With the syncs, a save survives an OS crash provided
  the file system orders the rename after the synced data and the device
  honours the flush. Nothing in this step proves that.
- Interrupted write: a crash before the rename leaves the old records.json and a
  partial temp file, which load never reads and the next save truncates. After
  the rename there is only the new file. Either way, the entire old or the
  entire new snapshot.
- Revision: none. records.json carries no revision, and `StoreApi::save`
  replaces the file unconditionally, with no compare-and-swap. One writer per
  instance is the job of the instance lock (`instance.lock`, created O_EXCL).
  A `SnapshotStore` adapter needs a revision committed atomically with the
  bytes, which is a durable-format decision (see "Blocked", below).
- Absence and corruption: a missing file loads as an empty snapshot, which is
  established absence. A corrupt file panics in the decoder at boot. The node
  stops, and never takes the file as empty, but there is no typed `Corrupt`
  error.
- Concurrent handles: not linearized. Two `BlobStore`s on one directory share
  one temp name, so two saves in flight can tear the file: one renames a temp
  file the other is still writing. Two sequential saves from two handles lose
  the first save, and nothing reports a conflict. The node never opens two
  handles, because of the instance lock.
- Capacity: none is enforced. A full disk fails the temp write, and records.json
  is left unchanged.

**The served store against the operation-store port**: the suite cannot run,
and no fixture stands in for it.

- Types: the suite is typed on discovery's `SignedOp`, a map of eleven fields,
  the last being the signature. The node's `Op` has ten (SP-P3 (a)), and `Swmr`
  and `Crdt` have no discovery shape (SP-P3 (b)). Running the suite would also
  need a dependency on a glade-discover crate, which this step does not add.
- Semantics: OS-001..007 assume a content-addressed store with no admission. It
  has `load(hash)`, and it keeps distinct signed variants side by side at one
  record id (OS-003, OS-006). The served store instead addresses ops by chain
  (share, stream, zone key, origin) and seq, and admits or refuses them (gap,
  chain break, SWMR, shape). A second op at an occupied slot is an
  equivocation: kept as a proof, and never in the log. So the store differs
  from the suite in meaning as well as in encoding.
- It is recorded as an explicit blocker under SP-P3 (a) and (b), with the
  semantic mismatch beside it.
- Durability profile, stated as OS-002 requires: an append writes the op to its
  (share, origin) log before indexing and before any fan-out, with no fsync. It
  survives a process crash once the write returns, but not an OS crash or
  power loss. The node's own directory records survive either way: records.json
  is synced, and adoption re-seeds the served store from it at every boot. A
  peer's records come back with the next connect-time pull. A client's ops come
  back only if the client re-sends them. An fsync per client op is left out on
  purpose: it would change the latency of every op on the default path.
- Interrupted append: the length prefix and the op bytes went in two writes, so
  a crash between them left a torn tail. Open skips a torn tail, but the next
  append lands after it, so the following open misreads the log and panics.
  This step writes each record in one write, and truncates a torn tail at open,
  so the next append starts on a record boundary (failing test first). What
  loads is unchanged: the same ops load, and a well-formed log is not touched.

**The durable-acceptance order** (SP-L1). Every write the node makes to its own
directory records goes through the same four steps:

1. The change is built on a staged copy of the fold.
2. The copy's snapshot is saved through the engine.
3. Only if the save succeeds does the copy become the fold.
4. Only then is the change published (fanned out to local subscribers and
   pushed to peers), or answered `Ok`.

A refusal or a failed save leaves the fold and the stored snapshot as they
were, so a retry is judged against what was durably accepted. The staged copy
is a clone of the registry. That costs O(n), as does the save itself, which
re-encodes every op. One helper, `Registry::accept`, carries the order for
every call site. A change that appends nothing (an unchanged registration, or a
duplicate) saves nothing.

| Call site | Before | After |
| --- | --- | --- |
| `assembly::Records::append` | fold, then save: after a failed save the fold already holds the record, so a retry is `Ok(false)`, saving nothing | staged, saved, committed: after a failed save the retry is `Ok(true)` and saved |
| `Records::register` | `appdecl::register` into the fold, then save: the retry reports the lines unchanged | staged: the retry appends them |
| `Records::ingest` | verify into the fold, then save: after a failed save the redelivery is refused (the pinned duplicate label, below) | staged: the redelivery lands |
| `claims::serve_workspace_on` | entry and claim folded and the share marked served, then saved. A failed save left the share marked served, so a retry answered `Ok(false)` with nothing saved, and the renewal loop renewed an unsaved claim | staged and saved; the share is marked served only after the save, then published |
| `claims::renew_leases` | appended; a save error ignored; published anyway | staged. A failed save publishes nothing and the next tick retries |
| `claims::note_principal` | appended; a save error ignored; published anyway | as renewal. The next Hello naming the principal retries |

Two sites are unchanged. First-boot presence (`sysdir::boot_at`) and the
hand-written root's registration both fold and then save, but a failed save
ends the boot or the process with the fold dropped and nothing sent. The served
store already writes its log before it indexes or fans out.

The new order also closes a cancellation hazard. `serve_workspace_on` folded the
workspace entry before awaiting the served store's lock. A call cancelled there
left the entry folded but unsaved and unpublished, and the next call diffed the
entry away and published the claim alone.

**What "a node restarted mid-round resumes from its heads" means here.** Two
texts decide it: plan 4.4's sentence, and the M-LIMP definition it grew from,
"Node restart resumes from its store (heads exchange, no data loss)"
(`GladeSubstrateV1.md` §11; §6 describes Resume as "heads exchange, ship the
gaps, both directions"). A node's heads are, per (stream, origin), the chain
tip's seq and hash. records.json keeps them as `SystemSnapshot.heads`, and the
served store's are `Store::all_heads`. Here the sentence means four things,
which the restart journey and its companion (below) assert:

1. A restart loses nothing it acknowledged, and keeps nothing it failed to save.
   Every record whose append or ingest answered `Ok` is in the fold after the
   restart; a record whose save failed is not.
2. After the restart, the heads are those of what the node durably accepted,
   read back from its store through verify-as-ingest.
3. The interrupted round resumes from those heads. The peer ships only what lies
   past them, and each op lands once, with no gap, refusal or duplicate. The
   lookup answers as if the restart had not happened.
4. The node's own next record extends its reloaded chain (the next seq, with
   `prev` the head's hash), so a restart never forks the chain. An exact retry
   of a record the node holds is still `Ok(false)` across the restart.

Three things are not decided, so they are not promised. Each comes with options:

- (a) Restart after the process crashed. The instance lock is an O_EXCL file. A
  crash leaves it behind, and the next boot refuses (`AddrInUse`) until someone
  removes it (`sysdir.rs:79-81`). The journey restarts cleanly. Options: keep
  the lock (honest, but not automatic); use an OS lock that the kernel releases
  when the process exits (`File::try_lock`, in std since Rust 1.89, so no new
  dependency); or check the pid that the lock file holds. Recommendation: the OS
  lock, as a separate change with its own failing test. Built on 2026-09-24:
  see "Hardening (after Step 4.4 and 4.3 part 1)", fix 2.
- (b) Finishing the interrupted round's sends. The node keeps no outbox. A push
  lost to a link failure or a restart is healed only by the peer's next
  connect-time pull (`mesh.rs:271-276`). Discovery's driver commits its effects
  with each state delta (SP-L1 step 3); the node does not. Options: rely on the
  pull (today); a durable outbox committed with each accepted record; or
  re-push past the peer's heads when a link comes up. Recommendation: the pull
  for the slice. An outbox belongs with 4.6's route.
- (c) The served store's restart. On a real node the pull announces the served
  store's heads, not records.json's, while the journeys' record host holds
  records.json. The adapter tests below cover the served store's reopen; the
  journey does not.

**The restart journey**, `restart_mid_round` (in `tests/journeys/restart.rs`,
which both test binaries run):

1. A serves `WS` and renews twice: four records on two streams.
2. A pushes them to B, over a link that fails after the second frame arrives.
3. Both nodes restart. Each closes its peer carrier, drops its composition, and
   is rebuilt over the same engine; its fold is reloaded by `Records::over`.
4. B's heads, read from its stored snapshot (`SystemSnapshot.heads`), name A's
   entry and first claim, with A's hashes.
5. A's exact retry is `Ok(false)` and saves nothing.
6. A pushes what lies past B's heads. B ingests both, `Ok`. B's snapshot is
   byte-equal to A's, and both answer A past the first lease.
7. A renews once more. Its record is seq 3, with `prev` the reloaded head's
   hash, and B ingests it.

A companion test, `retry_after_a_failed_save`, covers the known-failure path
and is written failing first:

1. A's engine refuses to save (a volatile engine told to refuse; on disk, a
   directory where the temp file goes). A's append is `Err(Io)`, nothing is
   persisted, and A still answers none.
2. Once saves work again, the same append is `Ok(true)` and persisted.
3. The same holds for B's ingest and for A's `register`.
4. After a restart, the record is held.

**How the journeys run over the real store.** The journey files are unchanged.
The harness they share moves to `tests/journeys/node.rs`, and each test binary
supplies the engine:

- `journeys`, the fast loop: `VolatileStore`.
- `durable`, a new target: `BlobStore` over records.json, in a fresh temp
  directory per node, removed once the test drops its last handle on it. It
  includes the same journey files by `#[path]` and runs outside the fast loop.

The record host is `Records::over(profile, transport, engine)`. It loads the
fold from its engine through `Registry::from_snapshot`, as boot loads
records.json. It replaces `Records::in_memory_over`. A journey reads back what
the node persisted: the last snapshot in memory, or records.json read through a
second `BlobStore`.

Beyond the fast loop, the durable runs prove one thing: every journey's records
round-trip, byte-equal, through the node's real engine (encode, temp write,
sync, rename, read, verify-as-ingest) on this machine's file system. They do not
prove:

- crash safety or fsync: a reopen reads what this process wrote, from the page
  cache (the contracts README's warning);
- the served store, which the journeys never reach;
- iroh;
- signatures.

`faults.rs`'s two conformance tests run in both binaries.

**The adapter tests the contracts README requires.** Tests on records.json are
in `tests/durable/`; the others are marked.

| Requirement | Test | Proves | Cannot prove |
| --- | --- | --- | --- |
| recovery after a known failure | A directory where the temp file goes makes a save fail. records.json keeps its bytes, and once the directory is removed, the next save lands. The same through `Records` (`retry_after_a_failed_save`). Through `claims.rs` (lib tests): a failed save in `serve_workspace_on`, in a renewal or in noting a principal publishes nothing, and the retry mints and publishes | a known failure changes neither the stored snapshot nor the fold, and the retry is honest | ENOSPC or EIO part-way through the write or at the sync, which fail at other points |
| interrupted writes | A partial records.json.tmp, as a killed save leaves it: load returns the previous snapshot, and the next save replaces both files. Served store (lib test): a torn tail on a log is cut at open, and after the next append a reopen loads every op (failing first: that reopen panics) | load never reads a temp file; a leftover one blocks nothing; a torn log tail no longer poisons the log | that a real crash leaves only these states. The test writes the files itself and simulates neither rename ordering nor power loss |
| concurrent handles | Two handles on one directory, in turn: the second's save replaces the first's, and no conflict is raised | that the single-writer premise rests on the lock (`sysdir::tests::instance_lock_is_single_writer` proves the lock) | the tearing race on the shared temp name. It depends on timing, and is stated from the code |
| pending-future cancellation | `StoreApi` and `RecordHostPort` are synchronous, so there is no future to cancel. A future is pending around a save in `claims.rs` (lib test): a `serve_workspace_on` cancelled while it waits for the served store's lock leaves nothing folded, saved or marked served, and the next call publishes both the entry and the claim (failing first: the entry was never published) | a mint cancelled before its save leaves nothing half-done | cancellation after the save, when the records are durable but not yet published. The next boot's seed and the peer's next pull heal it; not tested |
| capacity limits | none: neither store enforces a capacity, and `StoreApi` has no capacity answer | nothing | behaviour on a full disk, which cannot be produced without a size-limited file system, and not on this machine's |

**Blocked, for the owner.**

- PS-001..008 on the real store. glade-node does not depend on
  `glade-persistence-api`, so running the suite needs a dependency (normal, and
  dev with `conformance`), a lockfile entry and a node policy change. An honest
  adapter would also need a revision committed with the bytes, and a typed
  corruption error in place of the decoder's panic (PS-005). Options for the
  revision:
  - (i) a versioned container around records.json, with today's bare snapshot
    read as revision 1. An older node cannot read a file a newer node wrote.
  - (ii) an optional revision field in `SystemSnapshot` (`sysdata.taut.py`, the
    node's durable IR, not the wire IR). An older node reads the new file (it
    reads keys 1 and 2) and drops the revision when it saves. The generated
    legacy decoder panics on a missing key, so reading today's files needs a
    tolerant decode.
  - (iii) a sidecar revision file. It is not atomic with the bytes, so it fails
    the rule.
  Recommendation: (ii), as its own step after 4.4. Its acceptance test is that
  every existing records.json loads unchanged.
- The operation-store suite: blocked, as described above.
- "The served store as record host" (3.4's Phase 4 entry for `publish`): not in
  this step (deferred by the lane owner on 2026-09-24, not an owner ruling).
  What it needs is set out below, as a piece of work of its own.

**The duplicate labelled as a fork: ruled, and fixed in this step.** Owner,
2026-09-24: "fix it in 4.4". Before the ruling, `Registry::ingest` answered a
byte-identical re-delivery `Equivocation`, and the wire store answered it
`Duplicate`.

- The fix: `Registry::ingest` answers an op its chain already holds, byte for
  byte (the same op hash at the same stream, origin and seq), with a new
  outcome, `Ingested::Duplicate`, and appends nothing. A new op is
  `Ingested::Appended`. A different op at a position already held is still
  `Equivocation`. The held op is found by scanning the fold's ops, since the
  registry keeps no index by seq. The scan runs only for an op at or below its
  chain's tip, which is to say only for a re-delivery or a fork.
- Through the record host: `Records::ingest` answers a duplicate `Ok`, and saves
  nothing, because `Registry::accept` now skips the save when a change appended
  nothing (the fold only grows, so a copy of the same length is the fold). A
  duplicate is therefore `Ok` even while saves fail: what it repeats was
  already accepted, durably.
- At boot: `from_snapshot` takes a repeat as a duplicate. It quarantines
  nothing, and the rest of that chain loads. Before, it quarantined the repeat
  together with every later record of the chain (that stream, from that
  origin). A snapshot holding a record twice therefore loads more records and
  reports fewer quarantined, and the next save writes the record once. The
  node's own saves never write a repeat: `snapshot()` writes the fold, which
  never holds one. So an instance written only by the node loads as before.
- The tests, each run failing first:
  - `delivery::lost_acknowledgement`, its pin turned round: B answers each
    re-delivered op `Ok`. Before the fix it failed with both answers
    `Err(Rejected(Equivocation { origin: "a", seq: 0 }))`.
  - `registry::tests::a_repeat_is_a_duplicate_and_a_different_op_at_a_held_seq_a_fork`:
    a repeat is `Ok(Ingested::Duplicate)` and appends nothing, and a different
    op at seq 0 is `Equivocation`. Before the fix the repeat answered
    `Err(Equivocation { origin: "n1", seq: 0 })`. The red run asserted only
    `is_ok()`, since `Ingested` did not exist yet; the assertion now names the
    variant.
  - `sysdir::tests::boot_takes_a_record_held_twice_as_one_and_keeps_the_rest_of_its_chain`:
    a records.json holding a peer's claim twice boots with nothing
    quarantined, the claim after the repeat folded, and the claim saved once.
    Before the fix, boot quarantined 2 records: the repeat and the claim after
    it.
- 3.4's "Pinned, not endorsed" paragraph is marked superseded, and its journey
  row now says that each re-delivered op answers `Ok`.

**The served store as record host: a piece of work of its own.** The lane
owner deferred it from this pass on 2026-09-24; the owner has not ruled on
it. The move would make
the journeys' record host the store a real node serves, and the store its peer
rounds resume from, which closes (c) under "resume" above. It needs five
things:

- An asynchronous record-host port, or a synchronous served store.
  `RecordHostPort` is synchronous, while the served store sits behind a
  `tokio::sync::Mutex` in `Shared` that 13 production sites lock (2 in
  `claims.rs`, 1 in `exchange.rs`, 6 in `mesh.rs`, 4 in `server.rs`), and more
  in tests. Either the port's four methods
  return futures, as `TransportPort`'s do, which reaches `Directory`, the
  harness and the lifecycle's `assemble`; or the served store moves to a
  `std::sync::Mutex`, and each of those sites is checked for holding it across
  an await.
- A journal the fast loop can keep in memory. `store::Store` writes its own logs
  and proofs, at five file touch points (open, append, proof). A journal seam
  with a file implementation and an in-memory one keeps `tests/journeys` pure.
- The host's writes routed through `claims.rs`'s path. `append` and `register`
  accept into records.json first, since the registry stays the chain authority,
  and then land in the served store and publish. `ingest` goes to
  `Store::append`, and `who_serves` to `mesh::who_serves`. Since this step, a
  re-delivery answers alike on both stores.
- Refusals in the served store's vocabulary. `HostError` has to carry
  `store::StoreError` (gap, chain break, equivocation, SWMR, shape, I/O), and
  the journeys that match `Rejected(RegistryError::Gap { .. })`
  (`partial_lookup`) move over to it.
- A new start-up order. `Assembly` registers apps through the host before
  `Storage` opens the served store and adopts the instance. A host over the
  served store needs `Storage` first, which changes `lifecycle.rs`'s edges and
  `tests/release_order.rs`.

Size, roughly: 250-400 lines of production code (the port or the mutex, 80-150;
the host over the served store, 80-120; the memory journal, 80-120; the order
and the errors, 50-70), and 150-250 lines of tests: 400-650 lines in all. That
is about one step's budget. If it runs over, it splits in two: the journal and
the port first, then the host and the start-up order.

**The two findings from the witness period.** No document records them; the
plan names them. Both come from glade-gyld's supplier (`glade-gyld` `65da8cb`:
`src/supplier.rs:1234-1289`, `tests/integration.rs:3801`, `:3885`), and both
lie on the WebSocket client path.

- The fire-and-forget append. The node answers an accepted client op with
  nothing. It answers a refused one with an `Error` frame whose `corr` is `None`
  (`server.rs:262-282`, `session.rs:59-65`). Both clients' `append` returns once
  the frame is sent (`client-rs/src/client.rs:246-258`,
  `client-ts/src/client.ts:165-169`), and neither reads `Error` frames
  (`client.rs:108`). A client therefore cannot tell an accepted op from a
  refused one. glade-gyld's restarted supplier had its ops silently refused as
  equivocations.
- `subscribe()` discarding heads. The node answers a Subscribe with a `Heads`
  ack of its per-origin seqs, then ships the gap (`server.rs:201-261`). Both
  clients resolve on the ack and drop its body (`client.rs:86-90`,
  `client.ts:113-114`). They also send `from: None`, which the node's client
  path ignores anyway: it uses the Hello's heads and the session's own ops. So
  a client cannot tell when the replay of a resumed chain is complete, and
  glade-gyld waits for quiet, with a 5 s deadline.
- Neither affects a journey. The journeys run the directory's record path
  (`RecordHostPort`, `TransportPort`), never the WebSocket client path. The peer
  path's own best-effort push is what `lost_acknowledgement` pins. Nothing is
  fixed here.

**Retention.** R2 was ruled (a), with row 18b (ii)
(`dev-docs/glade/GladeDeclReconciliation.md` §3). The vocabulary is `{latest,
from_cursor, ttl}`, with `windowed` refused, and the token is validated: a
warning for one release, then an error. The ruling fixes a vocabulary and its
validation. It does not make any store enforce a retention: its own text says
"Nothing reads any of it as a policy", and enforcement is GC-4's future work.
The node stores the token and never enforces it. So `latest` enforcement stays
out of this step.

**Named gaps.**

- The proofs log writes an equivocation's two ops in one write, but a crash in
  the middle of that write can still leave the first op without the second.
  `read_proofs` drops that op, and the next proof pairs wrongly. The log is
  evidence, not state.
- The legacy form's store directory has no lock. Two processes on one directory
  can interleave their appends there, as they could before.
- `claims.rs` publishes after releasing its lock. Two mints on one chain (two
  Hellos naming new principals, or a renewal racing a serve) can therefore
  reach the served store out of order. The served store refuses the later one
  as a gap, and then refuses every later record on that chain the same way,
  until the next boot's seed fills the hole. This predates the step, and
  publishing under the lock would close it. It is not changed here, because no
  deterministic test shows it. Closed on 2026-09-24, with such a test: see
  "Hardening (after Step 4.4 and 4.3 part 1)", fix 3.
- Off Unix, the directory is not synced after the rename.

**Behaviour changes on the default (hand-written) path.** Nothing changes in
the wire, in any durable format, in the dependencies or in the node policy.

1. A records.json save now syncs the temp file and the directory. Each save
   takes longer: two `F_FULLFSYNC`s on macOS, 3-10 ms here. A node saves at
   first boot, once per app it registers, once per workspace it serves, once
   per renewal tick (every 10 s) and once per new principal.
2. `claims.rs`'s mints (serve, renewal, principal): a mint whose save fails is
   not folded, not marked served and not published, and its retry mints again.
   Before, a failed serve left the share marked served, so the retry answered
   `Ok(false)` with nothing saved, and the renewal loop went on renewing an
   unsaved claim; renewals and principals were published unsaved.
3. A serve cancelled before its save now leaves nothing behind. Before, the
   entry was folded unsaved, and the next serve published the claim alone.
4. The served store writes each record in one write, and an equivocation proof's
   two records in one. At open, it cuts a torn tail from a log. What loads is
   unchanged. A log with a torn tail is now repaired, where before it became
   unreadable after the next append. A torn tail on a log the node cannot write
   now fails the open; before, it was only skipped.
5. Boot takes a record that records.json holds twice as one (the ruling above).
   The rest of its chain loads, where before the repeat and every later record
   of that chain from that origin were quarantined, and counted in the
   `quarantined N record(s) at load` line. The next save writes the record once.
   The node's own saves never write a repeat, so its own instances load as
   before.

On the assembled path, `Records::register` in the `Assembly` step also saves
before it folds, and saves nothing when the registration changes nothing. This
is visible only if the save fails, and that fails the start, as before.

**Measured**, 2026-09-24, Apple M3 Pro, Rust 1.96, `/usr/bin/time -p`, on the
tree with the duplicate fixed:

- The fast loop is `--test journeys --test assembly`, now 37 tests (25 and 12).
  Warm, over 7 runs at load average 4.5: wall 0.14-0.15 s, CPU 0.14-0.15 s.
  After touching one journey file, over 5 runs: wall 0.93-1.15 s, CPU
  1.22-1.32 s. Both are within the 1.0 s and 3.0 s budgets.
- `--test durable`: 15 tests, 0.24 s of test time (the syncs dominate), and
  0.44 s wall warm. No scratch directory is left behind.
- The gate (`glade/node/check.sh`) passes all 8 components, with 205 node tests
  on each path across 15 test binaries (181 across 14 before the step).
  rustfmt reports 337 hunks, one below the old baseline of 338: the rewritten
  `note_principal` dropped an older deviation, and the baseline is lowered to
  337 (`check.sh`, the `glade-node` row of `style_dispositions`). Clippy
  reports 11 warnings, at its baseline.
- Code, first pass: production +213/−102 lines (`assembly.rs`, `claims.rs`,
  `registry.rs`, `store.rs`); tests +838/−156, of which 150 lines moved from
  `tests/journeys/main.rs` into `node.rs`. The duplicate fix adds about 40
  lines of production code (`registry.rs`, `assembly.rs`) and about 55 of
  tests, and removes about 15 of test (the pin).

## Grant check at the serve hop (plan Step 4.3)

Design addition, 2026-09-24, written before any code against glade `18b8524`
(the code as at `f5d055f`). The spec is plan Step 4.3 and its two RULED lines;
the preconditions carried from SUR-P3-4 and SUR-P3-6
(`dev-docs/glade/GladeDeclAmendment-RemPlan.md:121-133`, confirmed by the owner
at `:19`); slice profile SP-T1 to SP-T3 and SP-P1; and three findings the lane
owner relayed from Step 4.1's note (`GladeNodeSigning.md` D5, D11, F2).

**Stopped before code.** Three of the brief's stop conditions hold, so this
section was the step's whole output, and nothing below was built when it
stopped. One piece has landed since, on the lane owner's word: see "Part 1
(landed)", below. The node side of the rest of part 1 followed, on the owner's
rulings: see "Part 1, landing (i)".

1. No route revokes a seeded grant (precondition 1).
2. The verb vocabulary and the principal vocabulary are undecided. The check
   cannot be written without inventing both.
3. Every client flow outside the directory would be refused: the Gyld desk,
   glade/demo, and the glade-gwz and glade-gyld suites. No seed or documented
   grant can allow them. The desk and the demo present a random principal per
   tab, the suites' clients present none, and the demo's node has no grant
   fold at all.

What follows is what 4.3 builds once the owner rules, and what each ruling
decides. The questions come last, each with a recommendation.

### Part 1 (landed): client writes to `home` refused

Built on 2026-09-24, on the lane owner's word, against glade `d838bd0`. It
answers question 6, and it is the first piece of part 1. Ruling H-R3 already
covers it, and it depends on nothing undecided.

- **What changed** (`node/src/server.rs`, the `Frame::Ops` arm).
  - A client's op on `home` is refused before any of it is kept. It is not
    appended, not fanned out, and not counted in the heads the client has
    announced.
  - The session gets the node's usual answer to a refused op: an `Error` naming
    the share and the stream, here with the code `Unauthorized`
    (`home_refused`).
  - The frame's other ops are handled as before.
  - The node's own home records still reach the served store through
    `claims::publish`.
- **The only client write path.** `Ops` is the one frame through which a
  client appends. A Hello's principal and a `workspace.create` are both
  written by the node, under its own origin.
- **Tests** (`server.rs`).
  - `a_client_op_on_home_is_refused_and_never_stored` sends a forged grant on
    `home/dir.grants`. On the old code it failed: "the client's op on home was
    stored: [("home", "dir.grants", [])]".
  - `a_client_op_on_any_other_share_still_lands` sends one frame with an op on
    `sh` and one on `home-notes`. Both are stored, and no error comes back.
    It passed before and after: it guards the refusal's scope.
- **The default-path change.** A client's op on `home` was appended to the
  served store and fanned out to the share's subscribers. Now it is refused. No
  shipped client sends one (see "Client writes to `home` (H-R3)").
- **Not in it.** No grant is checked. There is no `Origin` check and no
  switch. Peers still write `home`, through the push and the pull, until
  4.1b: a named gap.
- **Still waiting on the owner.** Questions 1 to 5, 7 and 8. They hold back the
  rest of part 1 (the revocation route, the seed lines, the fixtures, the
  warning, the format page) and all of part 2.
- **Measured** on 2026-09-24:
  - The gate passes all 8 components. There are 207 node tests on each path,
    across 15 test binaries (205 before).
  - rustfmt stays at its baselines: glade-node 337 hunks, glade-wire 43. None
    of the new lines is a deviation.
  - clippy stays at 11 warnings for glade-node and 7 for glade-wire.
  - The fast loop runs 37 tests in 0.14-0.19 s wall, warm.
  - Against the rebuilt default binary: grazel 26 + 3, glade-gwz 9 + 5,
    glade-gyld 233 (1 ignored) + 31.
- **Size.** 23 production lines and 109 test lines.

### Part 1, landing (i): the `revoke` line, the seed warning and the page

Built on 2026-09-25 against glade `0bef8cd`, on the owner's rulings of
2026-09-24 (below, "Questions for the owner"). It is the node side of the rest
of part 1. No app file changes in it. The owner's desk re-reads
`grazel/apps/grazel-app.glade` at every restart, and a node built before this
landing refuses a `revoke` line, so the corrected lines wait for landing (ii),
once this build is the default binary.

- **What changed** (`node/src/appdecl.rs`).
  - `revoke <principal> <share>` is a directive. It parses to a
    `CapabilityRevocation` (`AppDecl::revocations`), after `app`, with exactly
    two tokens, and `register` appends it as an ordinary record on
    `dir.revocations`, under the registrant's chain, after the seeds. It is
    diffed like a seed: loaded again, it appends nothing.
  - The fold needed no change. `grants_for` already answers no verb for a pair
    that any revocation names, whichever came first (`registry.rs:512-533`).
  - `load_all` checks the seeds of one start's files together (precondition
    4). A seed whose share no `workspace` line in any of them declares is
    warned on its line, after the file's own warnings. Both roots print what
    `load_all` returns, so neither root changed. A `revoke` line is not
    checked. Each seed's line is kept as parse data (`AppDecl::seed_lines`).
  - The warning, as the node prints it:
    `` <file>: warning: line N: no loaded `workspace` line declares the share `S`; the grant registers, but a seed names a workspace share (expected on a node that reads a share another node serves) ``.
- **The page** (`docs/AppFileFormat.md`).
  - `revoke` in the grammar, with its own section beside `seed`.
  - A seed's share rule, with the warning.
  - `service <name>`: a label kept as data, read by nothing.
  - A section "Principals and verbs", with the ruled vocabulary.
  - What deleting a `seed` line or a `revoke` line does.
- **Tests**, each seen red in a scratch copy with the part it guards switched
  off.
  - `a_revoke_line_parses_to_the_pair_it_withdraws` (`appdecl.rs`). With the
    `revoke` arm off, as at HEAD: "line 4: unknown declaration `revoke`".
  - `a_revoke_line_withdraws_a_seeded_grant`. With the arm off: "line 5:
    unknown declaration `revoke`". With `register` appending no revocation:
    `left: (0, 2)`, `right: (1, 2)`.
  - `a_seed_whose_share_no_loaded_workspace_declares_is_warned`. With the
    check warning of nothing, the warnings are the `v0` header's alone.
  - `both_roots_warn_of_a_seeds_undeclared_share_and_register_a_revoke_line`
    (`tests/assembled_path.rs`): both roots, two starts of one instance. With
    the check off, stderr has no warning line. With the registration off, the
    second start prints `app x registered (+0 record(s), 3 unchanged)`. With
    the arm off, the second start is refused:
    `` x.glade: line 6: unknown declaration `revoke` ``.
- **Default-path changes.** Two, both at start.
  - A file that carries `revoke` loads and registers the revocation. Before,
    the node refused it and did not start.
  - A seed whose share no loaded `workspace` line declares prints a warning on
    stderr. With today's files that is grazel-app.glade's two
    `seed owner grazel …` lines, 50 and 51, on the desk and in every suite that
    loads the file, and the fixtures' `seed owner gyld gyld.*` and
    `seed owner gwz gwz.*`. The suites discard or inherit the node's stderr,
    and none asserts on it.
- **The desk's next restart**, replayed in a temp home with the hand-written
  root and both files, as grazel starts the node:
  - on today's files it prints the two warnings and registers nothing new
    (`+0 record(s), 11 unchanged` and `+0 record(s), 12 unchanged`);
  - the node binary before this landing refuses landing (ii)'s file:
    `` apps/grazel-app.glade: line 56: unknown declaration `revoke` ``;
  - this build, on landing (ii)'s files, registers two records, the grant of
    `gwz.*` on `ws-razel` and the revocation
    (`app grazel registered (+2 record(s), 10 unchanged)`). The fold then gives
    `owner` nothing on `grazel`, and `gwz.*`, `gyld.*` and `read.*` on
    `ws-razel`. The next start registers nothing.
- **Landing (ii)**, once this build is the default binary (built: see "Part 1,
  landing (ii)").
  - Both grazel-app.glade copies: the seeds name `ws-razel`, and
    `revoke owner grazel` withdraws the old pair.
  - The fixtures' seeds name `ws-razel`.
  - The counts follow. grazel-app.glade registers 12 records: 7 bindings, a
    service, 2 seeds, the revocation and the workspace. Its `read.*` grant on
    `ws-razel` is byte-identical to gyld-app.glade's, so in one store it
    registers once.
  - The census test loads each shipped file as the node does, through
    `load_all`.
  - Dry-run in a scratch tree: the edited tests fail against today's files
    and pass, 270 on each root, against landing (ii)'s.
- **Measured** on 2026-09-25.
  - The gate passes all 8 components, with 270 node tests on each path (266
    before).
  - rustfmt: glade-node 316 hunks, one below its baseline, which is lowered to
    316; glade-wire 43.
  - clippy stays at 11 warnings for glade-node and 7 for glade-wire.
- **Size.** Production: 94 lines added and 15 removed in `appdecl.rs`; of the
  added, 47 are code and 46 comment. Tests: 183 added and 4 removed. The page:
  100 added and 14 removed.

### Part 1, landing (ii): the shipped files

Built on 2026-09-25 against glade `dea365e`, after the lane owner rebuilt the
default binary from landing (i), so the desk's node parses `revoke` before any
file carries it. No production code changed.

- **The files.**
  - grazel-app.glade, in both byte-identical homes (`grazel/apps` and
    `glade/apps`): the two seeds name `ws-razel`, and a new line,
    `revoke owner grazel`, withdraws the grants the old seeds made on
    `grazel`.
  - The fixtures seed `owner ws-razel gyld.*`
    (`glade-gyld/tests/fixtures/gyld-test-app.glade`) and
    `owner ws-razel gwz.*` (`glade-gwz/tests/fixtures/gwz-test-app.glade`).
    Their grants live only in the suites' temporary instances, so they carry
    no revocation.
  - `grazel/apps/gyld-app.glade`: a comment only. It no longer says
    grazel-app.glade "stays exactly as it is".
  - The page's grazel-app paragraph is in the past tense.
    `GladeGrazelAttachNotes.md` gains `revoke` in its scope, its grammar and
    its two notes on withdrawing a seed.
- **The tests that follow the files.**
  - `tests/binding_census.rs`: `records()` counts revocations.
    - grazel-app.glade registers 12 records: 7 bindings, a service, 2 seeds,
      the revocation and the workspace.
    - Row 9 is `{5, 7}` in both homes, and the census still appends 15.
    - In the owner's two-file store, grazel-app.glade registers `{12, 0}` and
      gyld-app.glade `{10, 2}`: its `seed owner ws-razel read.*` is now
      byte-identical to grazel-app.glade's, so it registers once.
    - Row 10's reload of grazel-app.glade is `{0, 12}`.
  - `tests/shipped_app_files.rs`, the census test, loads each shipped file as
    the node does, through `load_all`, so a seed on an undeclared share fails
    it.
  - `appdecl.rs`:
    - the trace shape checks that the seeds are on `ws-razel` and that the one
      revocation is `(owner, grazel)`;
    - registering twice counts 12;
    - the runtime-revocation regression moves to `ws-razel`;
    - the pre-amendment bytes put the revocation where `register` does.
  - `exchange.rs`, `grazel_attach_end_to_end`: 12 records, and the grants B
    shows A are on `ws-razel`.
- **Red first**, in a scratch copy that still held the files before this
  landing: ten tests fail with these test edits.
  - The four unit tests. The trace shape reads `("owner", "grazel")`, and
    registering twice reads `{11, 0}` for `{12, 0}`.
  - The five binding census tests: `{11, 0}` for `{12, 0}`, and row 9
    `{5, 6}` for `{5, 7}`.
  - The census test, on grazel-app.glade's lines 50 and 51, with the seed
    warning.
  - With this landing's files the copy passes 270.
- **The fixtures changed one commit after the warning**, not in its commit as
  precondition 3 said. The warning had to reach the desk's binary before any
  `revoke` line reached its file. In between, each fixture drew one warning on
  the suites' stderr, which no suite asserts on.
- **The desk's next restart** (replayed at landing (i)). Registration appends
  the `gwz.*` grant on `ws-razel` and the revocation:
  `app grazel registered (+2 record(s), 10 unchanged)`, because the gyld leg
  has run there. The two warnings stop, and the next start appends nothing.
- **Measured** on 2026-09-25.
  - The gate passes all 8 components, with 270 node tests on each path.
  - rustfmt: glade-node 315 hunks, one below the old baseline, which is lowered
    to 315; glade-wire 43. clippy stays at 11 and 7.
  - Against the default binary (inode 400427243, not rebuilt): client-rs
    25 + 10, client-ts 48, grazel 29 + 3, glade-gyld 233 (1 ignored) + 33,
    glade-gwz 9 + 7, all at baseline. None of their logs holds a warning line;
    grazel's held two per boot of grazel-app.glade before this landing.
- **Size.** Tests: 51 lines added and 32 removed, in four files. App files: 9
  added and 4 removed in each grazel-app.glade copy, one line changed in each
  fixture, and two comment lines in gyld-app.glade. The page: 4 added and 5
  removed; the attach notes: 12 added and 4 removed.

### Part 2, first half: the check on the peer paths

Built on 2026-09-25 against glade `1b62a69`, on the owner's rulings of
2026-09-24. Part 2 is split at the lane owner's seam: this half is the fold
adapter, the checks on the three peer paths, the admission table and the
re-check pass. The websocket path, behind its switch, and the Hello rule for
node ids are the second half. The split is because this half alone is past
the whole of part 2's estimate (below, "Size").

- **The contract** (`contracts/grant-api`, as ruled: a sentence and a pattern
  probe).
  - `GrantPort`'s documentation: matching is exact, but a granted `p.*`
    admits every verb that begins `p.`.
  - `admits(granted, asked)` states that rule once, for every adapter and fake.
    A lone `*` admits only itself, and so does `.*`.
  - GR-001 gains the pattern probe, and a probe that a principal named as the
    node's id is not the node ("a `Node` never matches a `Principal`", which
    the contract said and nothing tested).
  - `Holder::Node`'s documentation names the id as the public key (plan Step
    4.1a), not `sha256`.
- **The fold and the adapter** (`node/src/grants.rs`).
  - `Registry::policy` folds this registry's grants and revocations, as
    `grants_for` does. A load that quarantined a grant or a revocation leaves
    it unreadable: `Record::is_policy` (AZ-11) is called at last.
  - `PolicyView` is the `GrantPort` adapter the serve paths ask (LBT-009). It
    holds the last fold it was given, or none, and a generation. A node holds
    one in `Shared`; adoption fills it. Until then, and in the legacy form, it
    has none and refuses.
  - A grant names a node by its id in hex. A principal named with 64
    lower-case hex digits matches nothing.
  - The assembly binds `PolicyView` in place of `PendingGrantFold`, built
    with no fold; the module is dropped before the node serves.
  - A start whose load quarantined a grant or a revocation prints
    `grants unavailable: …`, on both roots.
- **The three peer paths**, enforced by default. The holder is the node id
  the link's HELLO proved (signed since 4.1a), threaded from `run_link`.
  `home` is exempt on each.
  - `serve_peer_subscribe` asks `read.subscribe`. Refused, the stream gets
    the refused subscribe's two frames (R6), `Heads{streams: []}` then
    `Error{Unauthorized}`, and is finished, so the forwarding node's forward
    lapses. Check, registration, ack and gap hold the cut. An admitted
    stream joins the admission table, and its writer finishes the stream
    when the re-check pass removes it.
  - `serve_peer_exchange` asks the exchange's glade id. Refused, it answers
    `ExchangeRes{ok: false}` with the reason, at once.
  - `serve_sync` asks `read.subscribe` per zone, and leaves a refused zone
    out whole. It still has no production caller.
- **The re-check pass** (`server::refresh_policy`). Whenever the view is
  replaced, each admitted peer stream is checked again under the cut, before
  the call returns. A refused one gets `Error{Unauthorized}` alone, and
  leaves the router, the session table and the admission table. In
  production the fold changes only at adoption, when no stream exists yet;
  tests change it through the directory authority (`claims::testing::accept`),
  as a runtime route would.
- **The refusal's reason**: `unauthorized: node <id> holds no grant of <verb>
  on <share>`, `… grants on <share> are revoked`, or `the grant fold is
  unavailable, so …`.
- **Tests**, each seen red in a scratch copy with the part it guards
  switched off.
  - The contract's `gr_001_exact_match_implies_nothing`, on a reference fold
    that matches exactly: "GR-001 a pattern admits a verb it begins",
    `left: Err(NoGrant)`, `right: Ok(())`.
  - `gr_001_to_003_the_nodes_grant_fold_keeps_the_grant_contract`
    (`tests/assembly`): without the pattern, the same; without the
    node-name rule, "GR-001 nothing is implied" on the principal named as
    the node, `left: Ok(())`.
  - `a_quarantined_grant_or_revocation_leaves_the_fold_unreadable`
    (`registry.rs`): without the quarantine flag, `left: (1, false)`,
    `right: (1, true)`.
  - `serve_sync_leaves_out_a_zone_the_claimed_holder_may_not_read`
    (`peer.rs`): unfiltered, `left: 6`, `right: 3`.
  - `a_peer_without_a_grant_is_refused_by_its_claimed_node_id` (`mesh.rs`):
    unchecked, B serves the stream and keeps it open, "B answered and
    finished the stream: Elapsed".
  - `a_peer_granted_by_its_claimed_node_id_is_served`: with no node holding
    anything, "timed out waiting for routed tree ops".
  - `a_revocation_ends_a_forwarded_stream_of_a_claimed_node_id` and
    `a_stale_fold_fails_closed`: with the pass off, "the pass ended the
    stream".
  - `grazel_attach_without_a_grant_is_refused_by_its_claimed_node_id`
    (`exchange.rs`): unchecked, the exchange waits on the provider, "timed
    out waiting for the refused exchange".
- **Existing tests changed**, as the design said: the node serving a peer now
  checks its own fold, so it is adopted and grants the reader.
  - `s_discovery_golden_path_end_to_end`: B grants A's id `read.*`.
  - `grazel_attach_end_to_end`: B registers `seed <A's id> ws-razel
    read.*,gwz.*`; its grants seen at A are three.
  - `workspace_create_routes_to_target_end_to_end`: B grants A's id `read.*`
    on `ws-new`.
- **Default-path changes.**
  - A peer that asks for a share, or an exchange on it, without a grant from
    the serving node is refused. No shipped flow uses a peer: grazel passes
    no `--peer`.
  - A node whose load quarantined a grant or a revocation refuses every
    peer check, and prints a line saying so.
  - The desk's next restart is unchanged. Replayed in a temp home with both
    app files, the default binary and this build print the same lines, and a
    desk tab (a random principal), the suppliers (`grazel`) and a session
    with no Hello are each accepted on `ws-razel`, and on `home`.
- **Named gaps.**
  - The websocket path is not checked: the second half.
  - A forwarding node's local subscribers are not told when the claim holder
    refuses: they have their ack from the local replica, and nothing more.
  - The pass runs in production only at adoption, since no runtime route
    changes the fold.
  - `serve_sync` has no production caller.
  - A provider attach is not gated (B1), `workspace.create` is exempt, and a
    peer can write `home` until 4.1b.
- **Measured** on 2026-09-25.
  - The gate passes all 8 components, with 278 node tests on each path.
  - rustfmt: glade-node 307, eight below the old baseline, which is lowered
    to 307; the moved two-node setup took rustfmt's layout. glade-wire 43.
  - clippy stays at 11 and 7. The contracts gate holds grant-api rustfmt- and
    clippy-clean.
  - Against the default binary, through the shims: client-rs 25 + 10,
    client-ts 48, grip-share 19, grazel 29 + 3, glade-gyld 233 (1 ignored) +
    33, glade-gwz 9 + 7.
- **Size.** Production: 448 lines added and 50 removed, 134 of the added
  comments; the new module `grants.rs` is 167. Tests: 708 added and 98
  removed.

### Part 2, second half: the websocket switch

Built on 2026-09-25 against glade `19fe640`, on the owner's rulings of
2026-09-24: the websocket path behind a switch that is off by default, and no
session may claim a node's id.

- **The switch.** `--enforce-client-grants`, off by default, parsed by both
  roots (the hand-written root's parser, and `Settings` for the assembled
  one). With it, `Server::enforce_client_grants` turns the check on before the
  node serves, and both roots print
  `client grants enforced: a client session reads a share other than home only with a grant`
  after the `app` lines. grazel passes no such flag.
- **The check**, when on (`server.rs`, the `Subscribe` arm). It comes after the
  provider-attach branch and the route's absence, under the cut, before
  anything is registered, served or forwarded.
  - A share other than `home` needs the session's principal, as its Hello
    claimed it, to hold `read.subscribe` there.
  - A session that names no principal holds nothing.
  - Refused, the subscribe gets the refused subscribe's two frames (R6), with
    `unauthorized: principal <p> holds no grant of read.subscribe on <share>`
    or `unauthorized: a session that names no principal holds no grant of …`.
  - A client's writes and exchanges are not checked.
- **The Hello rule**, always on: a Hello naming 64 lower-case hex digits binds
  no principal and mints no principal record. The adapter already matched
  such a principal to nothing.
- **The re-check pass** covers client zones when the switch is on. A refused
  zone gets a lone `Error{Unauthorized}` and leaves the router, and the
  session's other zones go on. `Router::entries` lists every subscription; a
  session that is not in the admission table is a client.
- **Tests**, each seen red in a scratch copy with the part it guards switched
  off.
  - `a_session_claiming_no_principal_is_refused` (`server.rs`). With the check
    off, the subscribe is accepted (`refused: 1`). With the Hello rule off, the
    reason names `principal 0101…` instead of no principal.
  - `a_session_claiming_a_granted_principal_is_served`. With the session's
    principal not read, bob is refused as naming no principal, not as
    `principal bob`.
  - `a_revocation_ends_a_client_zone_of_a_claimed_principal`. With client
    zones left out of the pass, "timed out waiting for the lone refusal".
  - `both_roots_check_client_grants_only_when_switched_on`
    (`tests/assembled_path.rs`). With the switch never turning on,
    `left: [Ok(1), Ok(1)]`, `right: [Err(…), Ok(1)]`.
- **Default-path change.** One, always on: a Hello naming a node's id binds no
  principal. No shipped client sends one. Everything else waits for the
  switch.
- **The desk's next restart**: unchanged. Replayed in a temp home with both
  app files, the default binary and this build print the same lines, and a
  desk tab (a random principal), the suppliers (`grazel`), a session with no
  Hello and one claiming `owner` are each accepted on `ws-razel`, as is `home`.
- **With the switch on today**, replayed on this build with the desk's app
  files and `--enforce-client-grants`:
  - A desk tab, with its random per-tab principal, is refused every subscribe
    on `ws-razel`, so the desk shows nothing. Its exchange, `gyld.ops`, is not
    checked and still answers.
  - The suppliers, as `grazel`, are refused their subscribes. glade-gyld's
    resume of its own chains is refused; it says so on stderr and writes
    anyway, from a stale value (`supplier.rs`, `resume`). glade-gwz reads
    nothing. Provider attaches are not checked.
  - A session with no Hello is refused.
  - A session claiming `owner`, as a tab opened with `?principal=owner` does,
    is served: grazel-app and gyld-app seed `owner`.
  - `home` is open to all.
  - The seeds grant `owner`, and no shipped client presents it. That is the
    gap the appearance plan's principal work (Steps 1.2 and 1.3, in gryth-ui)
    is meant to close.
- **Named gaps.**
  - The principal is the client's claim until session identity lands, and
    the `Origin` check admits any loopback page.
  - A client's writes and exchanges are not checked.
  - A provider attach is not gated (B1).
- **Measured** on 2026-09-25.
  - The gate passes all 8 components, with 282 node tests on each path.
  - rustfmt stays at 307, glade-wire 43. clippy stays at 11 and 7.
  - Against the default binary, through the shims: client-rs 25 + 10,
    client-ts 48, grip-share 19, grazel 29 + 3, glade-gyld 233 (1 ignored) +
    33, glade-gwz 9 + 7.
- **Size.** Production: 122 lines added and 40 removed, 41 of the added
  comments. Tests: 292 added and 4 removed. Part 2 as a whole came to about
  570 production lines against the estimate of 300.

### The grant fold: the node's own registry

Two folds hold `dir.grants` and `dir.revocations`.

| Fold | Holds | Who can add a grant | Exists |
| --- | --- | --- | --- |
| the registry: records.json's fold, held by the adopted `DirState` (`claims.rs:54-77`) | this node's own appends only: its app files' seeds (`appdecl.rs:588-616`) and its mints. `Records::ingest` (`assembly.rs:590-600`), the one path that ingests a carried op, is called only by the journeys | this node, at registration and through `DirAuthority::accept` | after `adopt_boot`; never on the legacy form |
| the served store's `home` share | the registry's records, seeded at adoption (`claims.rs:110-111`); every peer's pulled and pushed home records (`mesh.rs:258-265`, `:466-490`); until part 1 landed, any client's op on `home` too (`server.rs:262-282`) | any peer; until part 1 landed, any websocket client too | always |

The check reads the registry. Until 4.1b signs directory records, a grant read
from the served store could be written by any peer, and before part 1 landed,
by any client. Three things follow.

- A node's grants are its own operator's. A grant made on another node admits
  nothing here (node trust, SP-T1).
- The legacy form, `glade-node <port> [store]` with no `--profile` or `--name`
  (`bin/glade-node.rs:139-172`), boots no registry. It has no fold, so every
  check answers `Unavailable`.
- `GrantPort::check` is synchronous and must not block
  (`contracts/grant-api/src/lib.rs:37-41`), but the registry sits behind
  `DirState`'s async lock. So the adapter reads a policy view: the grants and
  revocations, folded as `grants_for` folds them (`registry.rs:495-516`).
  - The view is rebuilt after every accepted change that touches `dir.grants`
    or `dir.revocations`, and it carries a generation number.
  - One adapter serves both roots. The hand-written root reaches it through
    `Shared`. The assembled root binds it in place of `PendingGrantFold`
    (`assembly.rs:736-756`).

### The four paths

| Path | Where the check goes | Holder, as claimed | Verb (proposed; see "What is undecided") | On refusal |
| --- | --- | --- | --- | --- |
| `peer.rs` `serve_sync` (`:168-203`) | the zone loop (`:193`), the drop-in point its comment names (`:172-174`). The function gains the holder and a `&dyn GrantPort` | the dialer's node id from `hello_accept` (`:119-131`), accepted unverified (`verify_peer`, `:99-101`) | `read.subscribe` | the zone is left out: an absence, not a hole (`:172-174`) |
| `mesh.rs` `serve_peer_subscribe` (`:299-352`) | before the subscriber is registered (`:309`) and before the heads and the gap (`:327-344`) | the link's node id. `run_link` has it (`:203-204`) but does not pass it to `handle_peer_stream` (`:217-227`, `:234-240`), so it is threaded through | `read.subscribe` | an `Error{Unauthorized}` frame, then the stream is finished. The forwarding node's `run_forward` (`:395-426`) reads to the end and its forward lapses (`:372-375`). Its local subscribers are fed nothing |
| `exchange.rs` `serve_peer_exchange` (`:234-265`) | before `handle_request` (`:247`), for every share but `home`, so `workspace.create` (`:100-103`) stays exempt | the link's node id, threaded as above | the exchange's glade id, such as `gwz.ops` | `ExchangeRes{ok: false}` naming the refusal, corr intact: the path's existing failure form (`:53-60`) |
| `server.rs` websocket subscribe (`:201-261`) | after the provider-attach branch (`:207-216`) and `route_subscribe` (`:217`); before `router.subscribe` (`:235`), the heads, the gap and a forward (`:256-258`) | the session's principal as its Hello claimed it (`:195-198`). A session that named none holds nothing | `read.subscribe` | see "The refusal on the wire" |

Notes on the table:

- `serve_sync` has no production caller. The mesh serves a peer's pull with
  `serve_home`, which offers `home` only (`mesh.rs:432-458`). `serve_sync`
  runs only in `peer.rs`'s and `iroh_carrier.rs`'s tests.
- A forwarded subscribe is checked twice: at this node against this node's
  fold, and at the claim holder against its own.
- `home` is exempt on every path, because it is how grants arrive. Its pull at
  connect, its pushes and its reads run as today.

Content also leaves the node on three paths the plan does not list. This step
would not gate them.

- A provider attach. A session that subscribes to a declared exchange receives
  every later request for it, and replaces the provider already attached
  (`exchange.rs:87-93`). Who may answer an exchange is B1's provider
  authentication.
- The local exchange (`handle_request`'s local arm, `:116-131`). AZM §1 puts
  effects under the authority's own check, when it executes them.
- A client's append (`server.rs:262-282`). The ruling covers reads. `home` is
  the exception; see "Client writes to `home`".

### The refusal on the wire

Both clients resolve `subscribe()` on the next `Heads` frame, first in first
out, and ignore `Error` frames:

- client-rs: `client.rs:86-90`, `:108`, `:228-239`;
- client-ts: `client.ts:113-114`, `:155-163`.

`Route::Absent` answers with an `Error` alone today (`server.rs:218-229`). A
refusal sent that way leaves the `subscribe()` waiting for good, and the next
`Heads` then resolves the wrong call. Two clients would hang:

- glade-gyld's `resume` awaits each subscribe, with no deadline
  (`supplier.rs:1261-1270`);
- gryth-ui's `startGlade` awaits each subscribe in turn (`runtime.ts:163-166`).

The options:

- (a) `Error{Unauthorized}` alone. No heads leave the node, but both clients
  hang.
- (b) An empty `Heads` ack for the zone, with no origins, then
  `Error{Unauthorized}`. Both clients resolve, and nothing leaks. A client that
  does not read the error sees an empty zone.
- (c) As (b), with both clients made to read `Error` frames. That changes two
  repositories.

Recommend (b). It needs no wire change: both frames exist, and
`ErrorCode::Unauthorized` is already in the wire IR
(`wire-rs/src/generated.rs:86`).

### What is undecided: holders, principals, verbs

- **Holders.**
  - `CapabilityGrant.principal` is one string (`ir/sysdata.taut.py:60-63`).
  - The ruling of 2026-09-23 makes a node's grant a record "whose principal is
    the node id", and the directory writes a node id as 64 lower-case hex
    digits (`mesh.rs:45-48`).
  - `GrantPort` requires that "a `Node` never matches a `Principal`"
    (`grant-api/src/lib.rs:33-35`).
  - One string keeps the two apart only if the principal vocabulary reserves
    the node form. One such rule: a 64-hex principal is a node, and a session
    may not claim one. Nothing states a rule.
- **Principals.** Every seed grants `owner`: grazel-app `:50-51`, gyld-app
  `:63` and `:66`, and both fixtures. No client presents `owner` (see the flow
  table). The page defines a principal only as "an identity that can be granted
  access, such as `owner`" (`docs/AppFileFormat.md:20`).
- **Verbs.**
  - The seeds store patterns: `read.*`, `gwz.*` and `gyld.*`.
  - The page says "a verb may be a pattern such as `read.*`" (`:175-176`), and
    defines no verb.
  - `GrantPort` matches exactly: "No verb implies another" (`lib.rs:33-34`).
  - AZM §5 is a working draft. It has three wire verbs (`read.subscribe`,
    `read.window`, `write.append`) and exchange verbs named by provider
    (`gwz.status` and so on). The authority checks those exchange verbs. The
    node cannot, because they ride inside an opaque payload.
  - gyld-app's comment reads `gyld.*` as covering "every surface above"
    (`:64-65`), which makes the verb a glade-id namespace.
  - The exposure table records the conflict: "The verb taxonomies do not
    agree, and the code implements neither"
    (`dev-docs/glade/GladeMetadataExposureTable.md` §9, item 4).

  So three things are open: the verb a read path asks for, the verb the
  exchange path asks for, and whether a stored `read.*` admits either.

### How a revocation reaches a live subscription

This needs a revocation route (precondition 1). Given one:

1. Every change to the registry goes through `DirAuthority::accept`
   (`claims.rs:73-76`), or through registration at start. The route appends
   through the same call.
2. A save that touched `dir.grants` or `dir.revocations` rebuilds the policy
   view and bumps its generation, before the change is published.
3. For every admitted subscription the node keeps the holder and the zone:
   - a client session: the router's entry, with the session's principal from
     `Shared.principals`;
   - a peer stream: the session id that `serve_peer_subscribe` registers
     (`:306-309`), with the peer's node id.
4. At each new generation, one pass under the router's lock checks every
   admitted `(holder, share)` again.
   - A refused client zone is unsubscribed (`router.rs:33-37`) and sent
     `Error{Unauthorized}`. The session's other zones go on.
   - A refused peer stream has its writer stopped and the stream finished, so
     the forwarding node's forward lapses.
5. The pass ends before the accepting call returns. So no op fanned out after
   a revocation has been accepted reaches a revoked session.

Ops delivered before stay delivered: revocation is forward-only (GDL-009, AZM
§4), and the forwarding node keeps what it replicated. `GrantPort` already
forbids answering from a decision cached across fold changes (`lib.rs:37-39`).
Checking live streams at each generation applies that rule to a decision the
node has already acted on (AZM §6).

A route that acts only at start (options (a) and (b) under precondition 1)
never meets a live stream in production: the restart has already ended every
stream. The pass then runs only in tests, which append through `accept`. A
runtime route would change that: E-share-1's `share.revoke`, or option (e).

### A stale or unreadable fold

- **Unreadable.** The check answers `Unavailable` and the node refuses, as for
  no grant. There are two cases.
  - No directory authority: the legacy form, or a booted node before
    `adopt_boot`.
  - A policy record set aside at load. `Registry::from_snapshot` drops a
    rejected record and the rest of its chain, and counts it
    (`registry.rs:304-325`). `Record::is_policy` (`:90-94`) was meant to make
    policy fail closed at load (AZ-11), but nothing calls it. So today a
    revocation set aside at load lets its grant stand. Proposed: after a boot
    that set aside any `dir.grants` or `dir.revocations` record, every check
    answers `Unavailable`, and the start says so.
- **Stale.**
  - The view is rebuilt under the directory's lock before a change is
    published, so no check reads a view older than the last accepted change.
  - What can go stale is a decision: taken before a change, and still serving
    after it. The re-check pass closes that.
  - A revocation made on another node never reaches this fold, because
    registries take no peer's records. That is node trust, not staleness.
- **The stale-fold test.** A stream is admitted, and then the fold becomes
  unreadable: the view is marked unavailable, or the node restarts over a store
  with a policy record set aside. The test asserts that the live stream ends and
  that a new subscribe is refused. That is the fail direction.

### Client writes to `home` (H-R3)

H-R3 (`plan-docs/plans/GLP-0006-grazel-gryth-suppliers/RulingWorksheet.md:497`),
in full: "A client submits intent. The authority validates, performs the
effect, then appends the canonical result while preserving B3 context. Direct
client appends are allowed only for record kinds with no privileged effect."
Every `home` kind has a privileged effect:

- grants and revocations admit;
- workspace entries and claims route (`mesh.rs:121-142`, `:512-545`);
- bindings and services declare exchanges (`exchange.rs:66-81`);
- principal and node records are identity.

Until part 1 landed, every client op was appended, `home` included
(`server.rs:262-282`). With the fold read from the registry, a forged grant
never reaches the check. A forged home record still reaches three things:

- routing: a `ServeClaim` with a higher epoch redirects a share, because
  `who_serves` reads the served store;
- declared exchanges: a `ServiceDefinition` makes any glade id an exchange,
  which a session can then attach to;
- every folder of that record kind: a malformed payload panics the decoders
  (`GladeNodeSigning.md` F2).

**No legitimate client writes `home`.**

- The search covered glial, glade/client-ts, glade/client-rs, glade/demo,
  glade-chat, glade/grip-share, grip-core, grip-react, grazel, glade-gyld,
  glade-gwz and gryth-ui (`@grythjs/glade` and every plugin).
- None of them appends on `home`, and none subscribes to it. So the re-ship of
  `session.dump()` at connect cannot carry a home op (`demo/src/glade.ts:56-57`,
  gryth-ui `runtime.ts:167-168`).
- `workspace.create` is the one exchange on `home` (`exchange.rs:40`,
  `:157-193`), and no client sends it.
- The node's own tests send no op on `home` over a websocket. They only
  subscribe to it.
- The 4.1 note found the same (D4, D5).

**The design.**

- In the `Frame::Ops` arm, an op on `home` is not appended.
- The session gets `Error{Unauthorized}` naming `home` and the glade id. The
  frame's other ops are handled as before.
- The node's own records still reach the served store through
  `claims::publish` (`claims.rs:288-297`), which the change does not touch.
- Size: estimated at about 15 lines and one test, written to fail first. It
  landed as 23 lines and two tests.

It depends on nothing undecided, and it has landed alone, first: see "Part 1
(landed)".

**Named gap:** a peer can still write `home`, through the push
(`mesh.rs:258-265`) and the pull (`:466-490`), until 4.1b verifies directory
records.

### The `Origin` header

`ws::accept` (`ws.rs:94-118`) reads `Sec-WebSocket-Key` and nothing else. The
node binds 127.0.0.1 (`bin/glade-node.rs:214`). But a browser lets any page open
a websocket to any address, and only the server can refuse it, by its
`Origin`. So any page open in the owner's browser can reach
`ws://127.0.0.1:9099` and send a Hello naming any principal. With the check
keyed on the claimed principal, that page holds the principal's grants.
Against a web page the check is worth nothing.

The browser clients today, and the `Origin` each presents:

| Client | `Origin` |
| --- | --- |
| gryth-ui's Gyld target, dev (`gyld-ui.py start`, `pnpm dev:gyld`) | `http://localhost:5173` (`gyld-ui.py:159-162`). A second instance moves the port by an offset (`:191`) |
| the same target, built, with grazel serving `dist-gyld` | `http://127.0.0.1:8080` (`grazel/src/main.rs:321-323`) |
| gryth-ui's full desktop (`pnpm dev`) | `http://localhost:5173` |
| glade/demo (`run_demo.py`) | `http://localhost:5175` (`run_demo.py:45`; `GLADE_VITE_PORT` moves it) |

No non-browser client sends an `Origin`:

- client-rs (`client-rs/src/ws.rs:48-55`);
- the node's own test client (`node/src/ws.rs:121-128`);
- Node 22's `WebSocket`, which client-ts uses in the grip-share and client-ts
  suites. Measured on 2026-09-24: its upgrade request carries no `Origin`
  header.

The options:

- (a) Accept a request with no `Origin`, or with a loopback one: `localhost`,
  `127.0.0.1` or `[::1]`, any port, http or https. Refuse every other request
  with HTTP 403, before the upgrade. This keeps every client above. It does not
  stop a page served from another loopback port, or a local process.
- (b) An allowlist from configuration (`--allow-origin`), which grazel and
  `run_demo.py` pass. Tighter, but every launcher changes.
- (c) A per-start token that grazel hands out through its same-origin
  `/bootstrap.json`. The client presents it in the Hello's existing
  `capability` field, which client-ts already sends as `null`
  (`client.ts:143-150`). A foreign page cannot read the token. There is no wire
  change, but grazel and both clients change.
- (d) Nothing until session identity lands, recorded as a named gap.

Recommend (a) now, and (c) with session identity. It is not built: the owner's
word is pending, because it changes the path the live desk uses. Ruled (a) on
2026-09-24 and built: see "Hardening (after Step 4.4 and 4.3 part 1)", fix 1.

### The preconditions

**1. The revocation route: none exists.**

- No production code appends a `CapabilityRevocation`. Only tests write the
  kind (`appdecl.rs:782-797`, `registry.rs:699`, `tests/assembly/main.rs:403`).
  The journeys' fake fold loads the contract's own `Revoke` record instead.
- The app-file grammar has no revoke line (`appdecl.rs:364-376`). `glade-node`
  has no revoke command (`bin/glade-node.rs:116-133`). No exchange mints a
  revocation.
- The ruled route is E-share-1's `share.revoke`, owned by the glade-share
  family (`RulingWorksheet.md:492`; `dev-docs/glade/suppliers/glade-share.md:32`).
  The family is not built.
- A client op on `home/dir.revocations` used to land only in the served
  store, which the check does not read. Since part 1 landed, the `home`
  refusal refuses it.

The options. None is chosen here.

- **(a) An app-file line, `revoke <principal> <share>`.** At registration it
  compiles to a `CapabilityRevocation` under the registrant's chain, and it is
  diffed like a seed.
  - Grants and revocations stay hand-made, in one reviewed place
    (`identity_adapters = none_in_v1`).
  - The corrected grazel-app.glade can carry `revoke owner grazel`. Every
    instance that loads the file then withdraws the old grants at its next
    start.
  - Costs: it is a new directive in `glade-app v1`, and an older node refuses
    it as an unknown declaration. A revocation covers a (principal, share)
    pair and wins for good, so that pair can never be granted again
    (`registry.rs:495-505`). It acts only at start.
- **(b) A `glade-node revoke --principal P --share S` command.** It takes the
  instance lock and appends to records.json. There is no format change, but
  the node must be stopped, and no file records the revocation. It too acts
  only at start.
- **(c) An exchange on `home` that the node answers itself,** as it answers
  `workspace.create`, gated by an admin verb. It needs the verb vocabulary and
  an admin grant. E-share-1's glade-share owns this later.
- **(d) Revocations read from the served store as well, but not grants.** A
  forged revocation can only deny. But then any peer could deny anyone, and
  without the `home` refusal so could any client.
- **(e) (a), plus re-registration on a signal (`SIGHUP`).** The operator edits
  the file and signals the node, and the revocation cuts live streams. It is a
  new signal path on both roots.

Recommend (a) for this step. Add (e) only if the slice must show a live cut in
production.

**2. The operands and the vocabulary.**

- A seed's `<share>` was ruled on 2026-09-23 to be the workspace share. The
  page states it (`AppFileFormat.md:180-186`).
- `service <name>`: nothing reads `ServiceDefinition.name`
  (`sysdata.rs:171-175`). Routing reads only the glade id
  (`exchange.rs:66-73`).
  - The shipped names are `grazel` (grazel-app `:43`) and `glade-gyld`
    (gyld-app `:57`). The fixtures use `gwz` and `gyld`.
  - Proposed page text: the name of the provider that answers the exchange,
    kept as data. It is not a principal and not a share, and nothing routes or
    checks by it. This needs the owner's word.
- The verbs and the principals are undecided (see "What is undecided").

**3. The shipped seed lines.**

- grazel-app.glade `:50-51`, in both copies, become `seed owner ws-razel read.*`
  and `seed owner ws-razel gwz.*`.
  - They land with the route's withdrawal of the old pair: under (a), a
    `revoke owner grazel` line.
  - Not done: they wait for the route. Done at landing (ii): see "Part 1,
    landing (ii)".
- gyld-app.glade `:63` and `:66` already name `ws-razel`.
- The fixtures name the app: `glade-gyld/tests/fixtures/gyld-test-app.glade:27`
  (`seed owner gyld gyld.*`) and `glade-gwz/tests/fixtures/gwz-test-app.glade:18`
  (`seed owner gwz gwz.*`). Each declares `workspace ws-razel` (`:30` and
  `:21`).
  - **They must change, to `ws-razel`, in the commit that adds precondition
    4's warning.** They changed one commit later, at landing (ii), with
    grazel-app's lines: the warning had to reach the desk's binary before any
    `revoke` line reached its file. In between, each fixture drew one warning
    on the suites' stderr, which no suite asserts on.
  - The node's census test loads each of its five files alone, the two
    fixtures included (`node/tests/shipped_app_files.rs:22-30`). It asserts
    that each file loads with no warning, and with exactly one warning when
    headed `glade-app v0` (`:61-90`).
  - Their grants live only in the suites' temporary instances, so nothing
    needs withdrawing.

**4. The warning.**

- At load, a seed whose share no loaded `workspace` line declares prints
  `<file>: warning: line N: …` on the existing warning channel (R10(a)).
- Step 2.7's criterion still holds only if grazel-app's corrected lines land in
  the same commit. Its two current seeds would warn.
- A reading node that seeds a grant for a share another node serves warns by
  design: 4.6's node A, for example. The warning's text should say that this is
  expected there.

### Every flow the shipped apps and clients rely on

"Refused" means refused once a fail-closed check is on for the path named.
"Unaffected" means the flow uses no checked path, or only `home`.

| Flow | What it presents | What it reads, and exchanges | Path | Grant after 4.3 | Outcome |
| --- | --- | --- | --- | --- | --- |
| **The Gyld desk**: gryth-ui's Gyld target (`pnpm dev:gyld` on 5173, or the build grazel serves on 8080), on grazel's node at 9099 | Hello principal `?principal=` or `?user=`. Otherwise a random six-character id per tab (`gryth-ui/packages/glade/src/runtime.ts:43-55`, `bootstrap-util.ts:27-30`). `gyld-ui.py` opens `http://localhost:5173/` with neither (`:159-162`) | subscribes on `ws-razel` to `gyld.streams`, `gyld.stream`, `gyld.decisions`, `gyld.lens` and `gyld.file` (keyed), `gyld.output` (by run) and `gyld.ask` (by conversation) (`plugins/gyld/src/ops/surfaces.ts:31-58`, `ops.ts:305`); exchanges `gyld.ops` (`ops.ts:284`) | websocket subscribe. The exchange is local and not gated | none. The seeds grant `owner` (gyld-app `:63`, `:66`), and no seed can name a random per-tab id. Only a page opened with `?principal=owner` would hold them | **refused**: every subscribe. The exchange still answers |
| gryth-ui's full desktop (`pnpm dev`) | as the desk | adds `chat` (`chat.msgs` per group and `chat.groups`, `plugins/chat/src/groups.ts:22`) and `ws-razel/gwz.output` by run; exchanges `gwz.ops` | websocket subscribe | none | **refused** |
| glade-gyld's supplier, spawned by grazel | Hello principal `grazel` (`grazel/src/lib.rs:296-300`) | attaches to `ws-razel/gyld.ops`, which is not a read. Before it publishes a build, or first writes a run's log, it subscribes to its own `ws-razel` chains to resume them (`glade-gyld/src/supplier.rs:1261-1270`, `:1312-1318`, `:1352`) | websocket subscribe (the resume) | none for `grazel` | **refused**: the resume. With an `Error`-only refusal, `resume` never returns and nothing is published |
| glade-gwz's supplier, spawned by grazel | Hello principal `grazel` (`lib.rs:330-341`) | attaches to `ws-razel/gwz.ops`, writes `gwz.output`, reads nothing | attach, not gated | not needed | unaffected |
| grazel's suite (26 + 3) | probe sessions with no Hello (`grazel/tests/integration.rs:316-320`, `:472-476`) | local exchanges on `gwz.ops` and `gyld.ops` | not gated | not needed | unaffected. A resume the gyld supplier starts in the background would be refused, but no assertion waits on one |
| glade-gwz's suite (9 + 5) | `requester` and `subscriber` sessions with no principal (`glade-gwz/tests/integration.rs:327-341`) | `ws-razel/gwz.output`, by run | websocket subscribe | none. The fixture seeds `owner gwz`, and the sessions claim nothing | **refused**: `streaming_output_visible_to_subscriber` |
| glade-gyld's suite (233 + 31) | `subscriber` sessions with no principal (11 subscribes, `glade-gyld/tests/integration.rs:973-3288`), and the supplier under test | `ws-razel`: `gyld.ask`, `gyld.output`, `gyld.streams` and others | websocket subscribe | none | **refused** |
| **glade/demo** (`run_demo.py`) | runs the legacy form, `glade-node <port> <store>` (`run_demo.py:84-90`): no registry, so no fold. Hello principal `?user=`, otherwise the tab's id (`demo/src/glial.ts:77-81`, `glade.ts:44`) | `doc:<doc>`, both commons and `self:<user>`; `account:<user>` (`manifest.ts:69-77`); `chat` (`chat.ts:35`); `ws-razel/gwz.output` after a gwz run (`gwz.ts:169`) | websocket subscribe | none can exist: every check is `Unavailable` | **refused**: everything but `home` |
| glade/grip-share's and glade/client-ts's suites; client-rs's suite | the legacy form (`grip-share/test/helpers.ts:54`, `client-ts/test/integration.test.ts:24`). client-rs's suite uses both forms (`client-rs/tests/integration.rs:101`, `:127`) | app shares | websocket subscribe | none | **refused** |
| glial | a library. It runs no process and has no principal of its own. Its supplier says `Hello(principal)` when configured (`glial/src/supplier/index.ts:408-412`). Its tests use in-memory fakes and start no node (`glial/test/session.test.ts:6`, `supplier.test.ts:8`) | the embedding app's | — | — | as for the desk and the demo |
| the peer mesh, `home`: the pull each way at connect, and the record push (`mesh.rs:432-458`, `:466-490`, `:277-293`) | the peer's node id | `home` only | exempt | not needed | unaffected |
| the peer mesh, a forwarded subscribe or exchange between two booted nodes. Used by the node's own tests (`mesh.rs:690-807`, `exchange.rs:506-622`, `:638-783`) and by 4.5's and 4.6's crossing. No shipped app runs two nodes: grazel passes no `--peer` (`lib.rs:240-255`) | the dialer's node id, in hex | any routed share, such as `ws-razel` | peer subscribe, peer exchange | none exists. A seed naming the peer's id would allow it, if the principal vocabulary admits ids. Ids are per instance, so no shipped file can carry one | **refused**, until the serving node's operator grants the peer |
| the peer mesh, a forwarded `workspace.create` (`exchange.rs:157-193`) | the dialer | `home` | exempt | not needed | unaffected |

**The plan's leak tests.**

- At the plan's revision (glade `559cb2c`), `mesh.rs:520-584` was
  `two_booted_nodes_converge_home_share`. That test replicates only `home`, so
  it is exempt and stays green.
- The leak the plan means is phase (b) of `s_discovery_golden_path_end_to_end`:
  a session on A subscribes to `ws-razel`, and B serves A without consulting a
  grant. That phase was then at `:629-` and is now at `:690-807`.
- The exchange leak is `grazel_attach_end_to_end`, phases (b) and (c), now at
  `exchange.rs:638-783`.

### The owner's existing instances

This is read from the code and the history. The instance directories were not
opened: `~/.gyld-ui` and `~/.glade` are out of bounds.

Their stores hold these records on `home/dir.grants`, under the node's own
chain, in records.json and in the served store:

- `(owner, grazel, [read.*])` and `(owner, grazel, [gwz.*])`, from grazel-app,
  unchanged since grazel `56d9a32` (2026-07-12);
- `(owner, ws-razel, [read.*])` and `(owner, ws-razel, [gyld.*])`, from gyld-app
  since grazel `832591f` (2026-09-13), if the gyld leg has run.

They also hold one `dir.principals` record for every tab the desk has opened,
since each tab's Hello names a new random principal (`claims.rs:213-236`).

What an instance sees after it upgrades:

- **To this step's tree:** nothing changes. Only this note was written.
- **To part 1 as recommended** (route (a), the corrected lines, and `revoke
  owner grazel`). At its first start, registration:
  - appends `(owner, ws-razel, [gwz.*])`;
  - finds `(owner, ws-razel, [read.*])` already held if the gyld leg ever ran,
    and appends it if not;
  - appends one `CapabilityRevocation{owner, grazel}`. That withdraws both old
    grants at once. They stay in the store as history, and the fold answers
    `Revoked` for them.

  Nothing is served differently, because nothing reads the share `grazel`.
- **To part 2, with the websocket path enforced.** The desk presents a random
  principal and holds nothing, so every `gyld.*` subscribe is refused. The
  supplier presents `grazel` and holds nothing, so its resume is refused. The
  desk would show nothing. That is why question 4 recommends keeping this path
  off by default until the desk presents a granted principal.

### The node's own grant

The ruling of 2026-09-23 makes it an ordinary `CapabilityGrant` whose principal
is the node id. The fold is per node, so the serving node's own registry must
hold it: B serves A only if B's registry holds `{principal: <A's id in hex>,
share, verbs}`.

- **Who creates it in tests.** The test appends the grant to B's registry
  before adoption, as the mesh tests append claims today (`mesh.rs:696-723`).
  Or it registers a test app file on B that holds `seed <A's id> ws-razel
  read.*`. A mid-stream revocation goes through B's `DirAuthority::accept`,
  the path a runtime route would take.
- **Who creates it in 4.5's crossing.** B's operator writes the seed line in an
  app file B loads. A prints its id at start (`node <id>`,
  `bin/glade-node.rs:148`).
  - This works only if the principal vocabulary admits a node id in a seed.
  - After 4.1a the id is the node key's public key (`GladeNodeSigning.md` D2),
    so the line is written after 4.1a, and written again if A's key is lost.
- **A file for a reading node.** A seed and a `workspace` line can share one
  file. A reading node must not load the serving node's `workspace` line, or it
  claims the share itself, with a higher epoch (`claims.rs:157-162`). So node A
  needs a file of its own, with seeds and no workspace line. Precondition 4's
  warning fires there, as expected.

### The order against 4.1b

The 4.1 note recommends putting 4.3 after 4.1b (`GladeNodeSigning.md` D11:
"4.3 should follow 4.1b, or its tests should name F2's bypass"), because
clients and peers could forge `home` records.

- Refusing client writes to `home` closes the local half without signatures.
  That is what lets 4.3 go before 4.1b, and it has landed (part 1).
- The peer half stays a named gap until 4.1b.
- With the fold read from the registry, a forged grant cannot reach the check,
  even from a peer. A forged claim can still steer routing.

### What 4.3 builds once ruled

The whole step comes to about 450 production lines and 750 test lines. That is
over the ~400-line production cap, so it splits in two.

- **Part 1: the preconditions and the local bypass.** About 150 production
  lines and 250 test lines:
  - the `home` refusal (landed);
  - the revocation route;
  - grazel-app's corrected lines, in both copies, with `revoke owner grazel`;
  - the two fixtures;
  - precondition 4's warning;
  - the format page: the route beside `seed`, and the definitions of `service
    <name>`, the verbs and the principals.
- **Part 2: the check.** About 300 production lines and 500 test lines (built
  in two halves, and larger: see "Part 2, first half: the check on the peer
  paths" and "Part 2, second half: the websocket switch"):
  - the policy view and its generation;
  - the `GrantPort` adapter;
  - the checks, on the paths the answer to question 4 enforces;
  - the admission table and the re-check pass;
  - the refusal forms.

The tests. Each is written to fail first, and each name says the identity is
claimed.

- `a_peer_without_a_grant_is_refused_by_its_claimed_node_id`: the golden path's
  phase (b), turned round. Its twin is
  `a_peer_granted_by_its_claimed_node_id_is_served`.
- `grazel_attach_end_to_end`, turned round: without A's grant, the forwarded
  `ws.tree` subscribe and the forwarded `gwz.ops` exchange are refused. A twin
  with the grant is served.
- `a_revocation_ends_a_forwarded_stream_of_a_claimed_node_id`.
- `a_session_claiming_no_principal_is_refused`, and
  `a_session_claiming_a_granted_principal_is_served`.
- `a_stale_fold_fails_closed`.
- `a_client_op_on_home_is_refused`.
- `a_revoke_line_withdraws_a_seeded_grant`.
- The census test, extended for the warning.

### Named gaps, whatever the rulings

- Identities are claimed. A peer's node id comes from an unverified HELLO
  until 4.1a and 4.2. A session's principal comes from its Hello, on the
  client's word, until session identity lands.
- A provider attach is not gated (B1), and a later attach replaces the earlier
  provider.
- `workspace.create` is exempt, as an exchange on `home`. Any session or peer
  can create a workspace at any linked node.
- A peer can write `home` until 4.1b.
- Revocation is forward-only.
- A grant is per node: one made on another node admits nothing here.
- The directory still shows every share id, principal and grant to every
  accepted peer and every client (the exposure table's §6.2). The ruling
  exempts it.

### Questions for the owner

1. **The revocation route.** The options are (a) to (e) under precondition 1.
   Recommend (a), the `revoke <principal> <share>` app-file line. Add (e) only
   if the slice must show a live cut in production.
2. **Verbs.**
   - (a) The read paths ask `read.subscribe` (AZM §5's wire verb). The exchange
     path asks the exchange's glade id. A stored verb `p.*` admits every verb
     that begins `p.`, and any other stored verb admits only itself. So
     `read.*` admits `read.subscribe`, `gwz.*` admits `gwz.ops`, and `gyld.*`
     admits `gyld.ops`. `GrantPort`'s documentation gains one sentence and
     GR-001 a pattern probe: a change under `glade/contracts`.
   - (b) Exact verbs only, with the seeds rewritten to `read.subscribe`,
     `gwz.ops` and `gyld.ops`. The contract stays as it is. The page's pattern
     text goes, and every stored pattern grant goes inert.

   Recommend (a). It keeps what the seeds, the page and AZM §5 already say.
3. **Principals.** Recommend:
   - a principal is a token;
   - a token of 64 lower-case hex digits names a node, and its grants are node
     grants (the ruling of 2026-09-23);
   - a Hello that names such a token binds no principal;
   - a session that names no principal holds nothing;
   - `owner` is the owner.
4. **The flows that would be refused.** The options:
   - (a) Enforce behind a switch that is off by default, such as
     `--enforce-grants`. Nothing shipped changes. 4.3's tests and 4.6's route
     turn it on. It is not added without your word.
   - (b) Give every flow a granted principal.
     - `gyld-ui.py` opens the desk with `?principal=owner`, or
       `/bootstrap.json` names the principal.
     - The suppliers present `owner`, or grazel-app seeds `grazel`.
     - The suites' clients send `hello(owner)`, and their fixtures seed `owner
       ws-razel`.
     - The legacy form gets a fold, or an exemption.

     This touches gryth-ui, grazel, glade-gyld, glade-gwz, glade/demo,
     grip-share, client-ts and client-rs.
   - (c) Enforce the peer paths by default now, keyed on the claimed node id;
     no shipped flow uses them. Enforce the websocket path under (a), until the
     `Origin` check lands and the desk presents a granted principal.

   Recommend (c). It turns both leak tests round and meets the ruling's
   concern first (the exposure table's §6.1). A check keyed on a principal that
   any web page can claim protects nothing yet, and turned on by default it
   would cut the live desk.
5. **The refusal on the wire.** Recommend (b): an empty `Heads`, then
   `Error{Unauthorized}`.
6. **Client writes to `home`.** Recommend landing the refusal now, alone, as
   4.3's first commit. It is independent of questions 1 to 4, and no legitimate
   client writes `home`. Landed: see "Part 1 (landed)".
7. **`Origin`.** Recommend (a): accept no `Origin`, or a loopback one, and
   refuse the rest.
8. **Order.** Recommend 4.3 before 4.1b once question 6 has landed, with the
   peer half as a named gap. The alternative is the 4.1 note's order: 4.4,
   then 4.1b, then 4.3.

**Ruled, owner, 2026-09-24 ("all recommended"):** 1 (a), the `revoke` line;
2 (a), with the contract's sentence and pattern probe; 3 as recommended; 4 (c),
the peer paths enforced by default and the websocket path behind a switch that
is off by default; 5 (b); 6 landed; 7 (a); 8, 4.3 before 4.1b.

## Hardening (after Step 4.4 and 4.3 part 1)

Design addition, 2026-09-24, written before the code against glade `39b8b25`.
The red runs' messages and the measured figures were filled in afterwards.
Three fixes, each approved by the owner on 2026-09-24, and each begun with a
failing test:

1. the `Origin` check: 4.3's question 7, option (a);
2. an OS lock for the instance: 4.4's question 3;
3. publishing in chain order: 4.4's question 5.

Nothing changes in the wire, in any durable format, in the contracts, in the
node policy or in the dependencies. `glade/node/Cargo.toml` gains a
`rust-version` (fix 2), and the gate's rustfmt ratchet for glade-node comes
down to 333 (see "Measured").

### 1. The `Origin` check

**Cause.** `ws::accept` (`node/src/ws.rs:93-118`) reads one header of the
upgrade request, `Sec-WebSocket-Key`, and answers `101` to any request that
has one. The node binds 127.0.0.1, but a browser lets any page open a websocket
to any address, and only the server can refuse it, by its `Origin` (4.3, "The
`Origin` header"). So a page from any site, open in the owner's browser, can
reach `ws://127.0.0.1:9099` and send a Hello naming any principal.

**Fix.** In `accept`, before the key is read:

- Every `Origin` header is collected. Its name is matched without regard to
  case, as the key's is.
- A request with no `Origin` is upgraded. That is every client that is not a
  browser.
- A request with one `Origin` that is a loopback origin is upgraded. A
  loopback origin is `http://` or `https://`, then a host that is exactly
  `localhost`, `127.0.0.1` or `[::1]`, then either nothing or `:` and a port of
  one to five digits, no greater than 65535. A browser serializes an origin in
  lower case and with no path (RFC 6454 §6.2), so the match is byte for byte.
- Every other request is refused. That includes `null`, a lookalike
  (`http://localhost.evil.example`, `http://127.0.0.1.evil.example`,
  `http://localhost@evil.example`), another scheme, a path, an empty or
  malformed port, an empty value, and two `Origin` headers.
- A refusal is answered `HTTP/1.1 403 Forbidden`, with an empty body, and the
  connection is closed. No `101` is sent. `accept` returns an error
  (`PermissionDenied`), so `server::handle` makes no session.

Both roots accept clients through `server::accept_clients`
(`server.rs:114-125`), so the one check covers both.

**The clients it keeps**, re-read on 2026-09-24 against 4.3's table:

| Client | `Origin` | Kept because |
| --- | --- | --- |
| the Gyld desk in dev (`gyld-ui.py start`, `pnpm dev:gyld`) and gryth-ui's full desktop (`pnpm dev`) | `http://localhost:5173`, or a port moved by an offset for a second instance. Vite binds `localhost`: neither config sets `server.host` | a loopback host, any port |
| the Gyld desk built, served by grazel | `http://127.0.0.1:8080` (grazel binds 127.0.0.1, `grazel/src/main.rs:321`) | a loopback host |
| glade/demo (`run_demo.py`, `start-demo.sh`) | `http://localhost:5175` | a loopback host |
| client-rs: grazel's probes, the glade-gwz and glade-gyld suppliers and their suites; and the node's own test client | none (`client-rs/src/ws.rs:48-55`, `node/src/ws.rs:121-128`) | no `Origin` |
| Node 22's `WebSocket`: client-ts, grip-share and their suites | none. Measured again on 2026-09-24 with Node 22.19: its upgrade request carries no `Origin` | no `Origin` |

No other client of the node was found. grip-react-demo's websockets go to
exchange feeds, and glade-decl-ts's taut client to a taut service. There is no
Electron or Tauri shell, and no page is loaded from a file, which would send
`Origin: null`.

**Tests** (`ws.rs`), over a loopback socket, with the upgrade request written
by hand:

- `a_non_loopback_origin_is_refused_before_the_upgrade`: 18 requests, one for
  each refused form above and for the header name in upper and in lower case.
  Each must be answered `403`, with no session made. Red on the old code: all
  18 were answered `HTTP/1.1 101 Switching Protocols`, each with a session.
- `no_origin_or_a_loopback_origin_is_upgraded`: 8 requests: no `Origin`, the
  shipped clients' origins, a second desk's moved port, each loopback host,
  `https`, and the header name in lower case. Each is upgraded. It passed
  before the fix and after: it guards the check's scope.

**What it does not prove.**

- What a browser sends. The origins in the table are read from the launchers
  and configs, not captured from a browser.
- Protection from anything but a page from another site. It does not stop a
  page served from another loopback port, a local process, or any client that
  leaves the header out. 4.3's option (c), a per-start token, is the later
  step.
- A DNS-rebinding page is refused only because its origin names its own site,
  not a loopback host.

**Default-path change.** An upgrade request whose `Origin` is not a loopback
origin is refused with `403`. Before, it was upgraded. No shipped client sends
one.

### 2. An OS lock for the instance

**Cause.** `InstanceLock::acquire` (`sysdir.rs:79-106`) creates
`instance.lock` O_EXCL, and its drop removes the file. A crash skips the drop.
The file stays, and every later boot on the instance is refused (`AddrInUse`,
"instance already locked") until someone deletes it. gryth-ui's `gyld-ui.py`
does that: it reads the pid in the file, and removes the file when that
process is gone (`gyld-ui.py:343-398`, `:1450-1463`).

**Fix.**

- The boot opens `instance.lock`, creating it if absent and truncating
  nothing, and takes `File::try_lock` on the handle. That is an exclusive
  advisory lock: `flock` on Unix, `LockFileEx` on Windows. `Boot` keeps the
  handle for the instance's life, as it kept the file before. The kernel
  releases the lock when the process ends, however it ends.
- If another handle holds the lock, the boot is refused with the same error as
  before: `AddrInUse`, "instance already locked: <path>". That holds within
  one process too: a second boot opens a second handle, and a `flock` lock
  belongs to the open file, not to the process.
- A leftover file that no one holds is locked. The boot empties it and writes
  its own pid.
- What the file records is unchanged: the holder's pid, with no newline, which
  is what `gyld-ui.py` reads.
- A clean release still removes the file. It removes it first, while the lock
  is held, and the handle closes after. So `gyld-ui.py` and the node's own
  tests (`tests/lifecycle.rs:186-217`, `tests/stop_signal.rs:199-213`, `:256`)
  see what they saw before.
- Removing the file brings a race that O_EXCL did not have. A boot that opens
  the file just before its holder removes it and exits would lock the removed
  file. A third boot could then create a new file at the path and lock that:
  two holders. So once it holds the lock, a boot checks that the path still
  names the file it locked (the same device and inode). If not, it starts
  again, up to three times, and is then refused as locked.
- Off Unix, std has no stable file identity, so no check is made there. That
  is a named gap: the window is two system calls wide.
- The check lives in a braced platform module, `sysdir::platform`.
  `check_key_perms` and `write_secret`, which carried bare `#[cfg(unix)]` and
  `#[cfg(not(unix))]` attributes (`sysdir.rs:225-251`), move into the same
  module, laid out as rustfmt lays them out; their logic is unchanged. So does
  the key-permissions test in `sysdir`'s tests, which carried a bare
  `#[cfg(unix)]`, into a braced `unix` module of its own.
- `File::try_lock` is stable since Rust 1.89. glade-node declared no
  `rust-version`. It now declares 1.91, the floor its dependency graph already
  set: iroh 1.2.0 and seven crates of its family (iroh-base, iroh-dns,
  iroh-relay, n0-dns-resolver, n0-watcher, netwatch, portmapper) declare 1.91.
  So the lock raises no floor. Clippy reads the field as its MSRV; the
  warning count is 11 with the field and without it.

**Tests.**

- `sysdir::tests::a_lock_file_left_by_a_crash_blocks_no_boot`: an
  `instance.lock` that holds another process's pid and that no one has locked,
  as a crash leaves it; the test writes it. The boot succeeds, and the file
  then holds this process's pid. A second boot while the first lives is
  refused (`AddrInUse`). A clean release removes the file. Red on the old
  code: the first boot failed, `Custom { kind: AddrInUse, error: "instance
  already locked: …/glade-sysdir-crash/instance.lock" }`.
- `stop_signal.rs`, `a_node_killed_outright_restarts_and_a_second_node_is_still_refused`
  (Unix), on both roots: a node killed with SIGKILL leaves its
  `instance.lock`. A node started on the same instance then starts, and a
  second node started while that one runs exits 1 with "instance already
  locked". Red on the old code, on the hand-written root (the loop's first):
  ``glade-node did not print `listening `: [], stderr: instance already
  locked: …/glade-home/sys/a/instance.lock``. For that message, the file's
  `start_until` now shows a failed start's stderr, and a `spawn` it shares
  with a new `refused` helper starts a node without waiting on a line.
- `sysdir::tests::unix::the_lock_path_names_only_the_file_it_opened`: the
  identity check. A file removed after it was opened is not what its path
  names, nor is a new file made at the path since. It is written with the
  check, so it has no red form.
- `sysdir::tests::instance_lock_is_single_writer` is unchanged. It still
  passes: a second handle in one process is refused.

**What it does not prove.**

- The race above. It lies between two system calls in `acquire`, and no test
  forces it. The identity test proves the check, not that a boot meets the
  race.
- Any platform but this one. The suite runs on macOS. The branch for other
  platforms is only type-checked (see "Measured").
- File systems whose locks do not reach across hosts, such as some network
  mounts. On a platform where std offers no lock, the boot now fails
  (`Unsupported`), where O_EXCL worked.

**Default-path change.** A boot on an instance whose last process crashed now
starts. Before, it was refused until the file was removed. Nothing else
changes: a live holder refuses a second boot with the same error, the file
holds the holder's pid, and a clean stop removes it.

**Upgrading a running instance.** A node started from an older binary holds
its instance by the O_EXCL file alone, with no OS lock. So a node started from
this binary on the same instance while the older one runs is not refused: it
locks the file, and when it exits it removes the file from under the older
node. The other way round is safe: an older binary is refused by the file.
`gyld-ui.py start` still refuses while the older node runs, by its pid and its
ports. So stop a running node before starting this binary on its instance:
`gyld-ui.py stop`, then `start`.

### 3. Publishing in chain order

**Cause.** `claims.rs`'s three mints (serve, renewal, principal) accept their
records under the directory lock (`DirState.inner`), release it, and then
`publish` (`claims.rs:288-297`). The publish lands the records in the served
store (`mesh::ingest_and_fanout`), fans them out, and pushes them to peers. So
two mints on one chain can publish in either order: two Hellos naming new
principals (`dir.principals`), or a renewal racing a serve (`dir.claims`). The
served store takes a record only at its chain's next seq (`Store::append`). It
refuses the later one as a gap, and then every later record of that chain the
same way, until the next boot's seed fills the hole.

**Fix.**

- `publish` takes the directory lock's guard from its caller. It lands the
  records in the served store, and fans them out to local subscribers, before
  it drops the guard. It pushes them to peers after.
- So the served store takes each of this node's chains in the order the
  registry minted it, since the registry orders each chain under the same
  lock. The three mints hand `publish` their guard.
- What is now done under the lock: the served store's appends (a write each,
  no fsync), and taking the router's and the session table's locks. Local
  fan-out only queues each session's frames on its unbounded channel, for its
  writer task to send. So no slow client or peer holds the lock up.
- The push to peers stays outside the lock. `push_home` spawns one task per
  link and returns.
- The lock order is the directory lock, then the served store's, the router's
  or the session table's, never the reverse. The mints' callers hold none of
  those: the renewal loop, the Hello arm (`server.rs:209-212`), the
  `workspace.create` exchange (`exchange.rs:171-172`) and
  `Server::serve_workspace`. `note_principal` lets the store's lock go before
  it takes the directory's.

**Peers can still see a chain out of order.** Each push is its own QUIC stream,
written by its own task and read on the peer by its own task
(`mesh.rs:277-293`, `:253-266`), so two pushes can be ingested in either order.
The peer refuses the later as a gap, and then every later push on that chain.
What repairs it is the peer's next pull, from its heads, which the mesh runs
only when a link comes up (`run_link`, `mesh.rs:202-244`). That is 4.4's
ruling, "a lost push waits for the next pull", and this step does not change
it.

**Test** (`claims.rs`),
`a_renewal_racing_a_serve_reaches_the_served_store_in_chain_order`:

1. A node serves `ws-a`.
2. The test holds the router's lock and polls a serve of `ws-b` by hand. The
   serve lands its workspace entry and stops at the router, before its claim.
   The test checks that it stopped there.
3. It polls a renewal once. The renewal mints the next two claims of the same
   chain.
4. It lets the router go, and drives both mints to their end.
5. The served store must hold the node's `dir.claims` chain exactly as
   records.json holds it, and must still after one more renewal.

Each mint is polled by hand, outside tokio's cooperative budget
(`tokio::task::unconstrained`), so the held lock alone decides where each mint
stops, not timing. Red on the old code: "the served store holds records.json's
chain", left `[0, 1, 2]`, right `[0, 1, 2, 3, 4]`. The renewal had completed
while the serve was stopped, and its two claims were refused as a gap.

**What it does not prove.**

- The Hello race. A principal mint lands one record, with no await between the
  release and the append that a test can hold. The same change covers it, but
  no test forces it.
- Peers, which can still see pushes in either order (above).
- A mint cancelled after its save, while it lands. Its later records stay out
  of the served store until the next boot's seed, as 4.4 recorded. That is
  unchanged.

**Default-path change.** A mint holds the directory lock until its records are
in the served store and queued to local subscribers. A second mint waits that
long, a store append per record. A chain of the node's own records no longer
stalls in the served store.

### Named gaps

- The `Origin` check stops only a browser page from another site (fix 1).
- Off Unix, the instance lock's identity check is not made (fix 2).
- A peer can see one of this node's chains out of order, and then stalls on
  that chain until its next pull (fix 3).
- A mint cancelled while it lands leaves its later records out of the served
  store until the next boot (fix 3; 4.4's named gap).

### Questions for the owner

1. **`rust-version`.** glade-node now declares 1.91, the floor its dependencies
   already set; the lock alone needs 1.89. Recommend keeping it: a toolchain
   below 1.91 is then refused by name, not deep in iroh's build, which matters
   for 4.5's Pi and dabeest. It leaves the clippy count unchanged. The other
   choice is to declare none, as before.
2. **A reordered push stalls a peer's copy of a chain.** A peer that ingests one
   of this node's renewals ahead of the one before it refuses it as a gap, and
   then every later renewal on that chain until the link comes up again. The
   claim's lease then lapses at the peer, which routes the workspace as absent.
   The options:
   - (a) keep 4.4's ruling: the next pull repairs it;
   - (b) a node that refuses a pushed op as a gap pulls the pusher's home share
     from its heads. That is receiver-side only and needs no wire change;
   - (c) one ordered push stream per link, a change to the mesh's protocol.

   Recommend (b), as its own small step before 4.5's crossing, which is the
   first to keep two nodes linked for longer than a test.
3. **Off Unix, the identity check.** Recommend accepting the gap for the slice:
   the race needs three boots of one instance within two system calls. Revisit
   if a supervisor ever restarts dabeest's node in a tight loop. The options
   then are to leave the file in place on Windows (the lifecycle test's check
   that the file is gone becomes a check that the lock can be taken), or
   Windows' file index, which std has not stabilized.

### Measured

2026-09-24, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components, in 27 s warm.
  There are 213 node tests on each path, across 15 test binaries (207 before).
  The six new ones are the two `ws` tests, the two `sysdir` tests, the
  `claims` test and the `stop_signal` test.
- **rustfmt**: glade-node has 333 hunks, 4 below the old baseline of 337. The
  four went from `sysdir.rs` code this change rewrote or moved: the old
  `acquire`'s error arm, the two moved functions and the moved key test. No new
  line is a deviation: `ws.rs` holds 8 hunks and `claims.rs` 23, as before, and
  `stop_signal.rs` none. The baseline is lowered to 333 in `check.sh`, as Step
  4.4 lowered it. glade-wire stays at 43.
- **clippy**: glade-node 11 warnings and glade-wire 7, both at baseline, with
  `rust-version` declared and without it.
- **Red runs**: each new test against the old production code, with the
  messages above. The `Origin` guard test passed there too.
- **The branches not compiled here**: `InstanceLock` and both `platform`
  modules, extracted verbatim into a scratch crate, type-check with no warning
  (`rustc --emit=metadata`) against the standard library of
  x86_64-pc-windows-msvc, x86_64-pc-windows-gnu, x86_64-unknown-linux-gnu and
  aarch64-apple-darwin.
- **Time**: the five new lib tests take 0.04-0.05 s together, and the
  cross-process test 1.1-1.6 s, over three warm runs each. It starts six nodes.
- **Downstream**, against the rebuilt default binary
  (`glade/node/target/debug/glade-node`), each suite with a scratch target:
  grazel 26 + 3, glade-gwz 9 + 5, glade-gyld 233 (1 ignored) + 31. All are at
  baseline.

**Size**, in lines added and removed:

| Fix | Production | Tests |
| --- | --- | --- |
| 1, the `Origin` check | `ws.rs` +73/−1 | `ws.rs` +112 |
| 2, the instance lock | `sysdir.rs` +96/−35 (+88/−27 ignoring whitespace); `Cargo.toml` +3 | `sysdir.rs` +73/−23, the key test moved into a braced module among them; `stop_signal.rs` +74/−21, the start helper's split among them |
| 3, chain order | `claims.rs` +83/−80, or +24/−21 ignoring whitespace: the three mints lose a block and its indent | `claims.rs` +60 |
| the gate | `check.sh`: the rustfmt ratchet, one line | |

Production code grows by 136 lines net in `.rs` files, doc comments included,
and by 3 in `Cargo.toml`.

## Signing: the key, the id and HELLO (plan Step 4.1a)

Design addition, 2026-09-24, written before the code against glade `1501a67`.
The red runs and the measured figures were filled in afterwards. The spec is
plan Step 4.1a and the owner's rulings on `glade/dev-docs/GladeNodeSigning.md`,
every one as recommended: D1, D2, D3, D6, D7's HELLO tag, D11's 4.1a row, and
D8 (a) where this step changes the id (its findings F5 and F6 apply). Not in
this step: signed directory records, refusing unsigned ones and requiring
`prev` (4.1b), the recovery key (4.1c), a stable endpoint key and the binding
record (4.2).

Nothing changes in the wire IR. The contracts change in one doc line, the
`NodeId` doc of `signer-api`, which the ruling allows.

### 1. The crate and the randomness (D1)

- `ed25519-dalek = "=3.0.0"`, pinned exactly, default features (`fast`,
  `zeroize`). The lockfile moves `ed25519-dalek` from `3.0.0-rc.0` to `3.0.0`
  and `curve25519-dalek` from `5.0.0-rc.0` to `5.0.0`. iroh 1.2 accepts both,
  no crate is added, and the move resolves offline. Every check is
  `verify_strict`.
- `getrandom = "=0.4.3"`, pinned exactly, no features, for the seed of a new
  `node.key`. The lockfile holds 0.2.17 and 0.4.3. 0.4.3 is the one iroh's own
  randomness comes from: `rand` 0.10's OS source, from which iroh draws a new
  endpoint key at every start, so it already runs on every machine the node
  runs on, dabeest included. 0.2.17 is in the lock only for `ring`. The pin
  fixes iroh's copy too, as the dalek pin does.
- `random_key` read `/dev/urandom`, which Windows has not got, so on dabeest
  no node could make its key. It now calls `getrandom::fill`: `getrandom(2)`
  on Linux, `getentropy` on macOS, `ProcessPrng` on Windows.
- Both crates join the glade-node row of `glade/node/architecture-policy.json`,
  which the checker holds to an exact match, and the gate's confinement
  allowlist: `node ed25519-dalek glade-node` and `contracts ed25519-dalek -`,
  and the same two rows for `getrandom`. The policy change is for the owner's
  review, as shaku's and sdax's were.

### 2. The key and the id (D2, D3)

- `node.key` is kept. Its 32 bytes are the Ed25519 seed. A new key is 32 bytes
  from `getrandom`, written 0600, as before.
- The node id is the hex of the seed's Ed25519 public key.
  `NodeIdentity::from_key` gives its raw 32 bytes, which HELLO carries. So a
  verifier needs no lookup: the id is the key.
- A `node.key` that is not 32 bytes refuses the boot (`InvalidData`) before
  anything is written. Before, the boot wrote presence under a hash of it, and
  the start failed later, at `Boot::identity`.
- `NodeIdentity` keeps the seed private and prints only its id.
- `PeerEndpoint::bind()`, which only tests call, takes a fresh random seed. It
  used the iroh key's public bytes as the key, which anyone could now sign
  with. A booted node passes its own identity to `bind_with`, as before.
- `node.key` signs for the node (D3). Certification under an account root
  stays a slice gap for 5.1.

### 3. The signing domains (D7) and the adapter

Every signature is pure Ed25519 over a purpose's tag followed by the message.
The tags are ASCII and end in a zero byte, so no tag is a prefix of another.

| Purpose | Tag | Used by |
| --- | --- | --- |
| `PeerHello` | `glade/v1/peer-hello\0` | HELLO (this step) |
| `OriginOp` | `glade/v1/origin-op\0` | directory records (4.1b) |
| `LocalOverlay` | `glade/v1/local-overlay\0` | the local overlay (4.1c) |

A new module, `src/signing.rs`, holds the tags, `sign` and `verify`. `verify`
takes the signer's id as its key, and answers `Valid` only when the id is a
point, not of small order, and `verify_strict` accepts the signature over tag
and message. Anything else is `Invalid`.

`NodeSigner` is the `SignerPort` adapter, in the same module:

- `node_id` is the key's id. `sign` signs for the purpose.
- `verify` is `Err(Unavailable)` when the adapter holds no key, and when the
  signer is neither this node nor a node recorded as authenticated. That is
  SI-002's rule: an unknown key is not proof of invalidity. Otherwise it is the
  strict check.
- It replaces `PendingNodeSigner` in `NodeAssembly`, `#[lazy]` as before. The
  booted identity is its component parameter. With none, as in the legacy form
  or `NodeAssembly::builder().build()`, it holds no key and refuses both ways,
  as the pending signer did.
- No consumer resolves it before 4.1b. 4.1b also records the nodes that HELLO
  authenticates.

HELLO does not go through the port. The carrier signs with `NodeIdentity` and
checks with `signing::verify`, taking the claimed id as the key: first contact
needs no lookup (D2).

### 4. HELLO (D6)

- **The channel.** Where `dial` and `accept` hold the connection, the carrier
  reads what both ends know without sending it: the dialer's and the
  acceptor's iroh endpoint ids, and 32 bytes exported from the connection's
  TLS session (`Connection::export_keying_material`, label
  `glade/v1/peer-hello`, no context). Both ends compute the same bytes, and
  another connection gets other bytes. The in-memory tests pass a fixed
  channel.
- **The transcript** is canonical CBOR: `{1: protocol, 2: role, 3: node id,
  4: dialer endpoint id, 5: acceptor endpoint id, 6: exported bytes}`, the role
  being `"dialer"` or `"acceptor"`. It is signed under the `PeerHello` tag.
- `NodeHello` and `NodeWelcome` keep their three fields, and `sig` carries the
  64-byte signature. No wire-IR change.
- **The checks.** The acceptor checks the dialer's HELLO before it answers,
  and the dialer checks the WELCOME. A HELLO is refused when its protocol is
  not 2, when it has no signature, or when the signature does not verify for
  the transcript this end computes for the other role. A refused HELLO gets no
  answer, and the link is dropped. The accept loop goes on to the next
  connection. A `--peer` dial that fails prints `peer <target>: …`, with
  `HELLO refused: …` when the dialer refused the WELCOME, and a closed stream
  when the acceptor refused the HELLO.
- **The ALPN** becomes `glade/node/2` and `PROTOCOL` 2, so a node of
  protocol 1 and one of protocol 2 fail at connect, not mid-sync.

What a completed HELLO shows: the peer holds the key of the id it named, and
signed for this TLS session, between these two endpoints, in its role. A
recorded HELLO does not verify on another connection, whose exported bytes
differ. A HELLO reflected back with the roles swapped does not verify, because
the role differs. A relay through a third party does not verify: that is two
sessions, with two sets of bytes and endpoint ids. What it does not show: that
the id is bound to the iroh endpoint key by a record, or that the peer is one
the operator configured. Any key that proves itself is admitted, until 4.2.

### 5. Existing instances (D8 (a), for the id change)

Today's records name `hex(sha256(node.key))`. After this step the same key's
id is its public key.

**records.json.** At boot, once the key is loaded, every record in records.json
whose origin is the old id is written, byte for byte, to a new
`records.legacy-<date>.json` beside it: the date is UTC, and the file is a
`SystemSnapshot` of those records, with no heads. It is created new and synced,
and never overwritten: if the name is taken, `-2`, `-3` and so on are added.
Then records.json is saved without them. They are never folded. Records of
other origins stay, as before. The boot prints one line after `node`:
`set aside N record(s) of old node id <old> in records.legacy-<date>.json`.
A crash between the two writes repeats the set-aside at the next boot, into a
second file, so nothing is lost.

The node then re-mints as on a first boot: its presence (the `NodeRecord` and
the home claim), its registrations (the fold is empty, so every declaration
appends again), its claims as it serves, and principals as sessions say Hello.

**The served store.** Its `home` share holds the same records under the old id,
in one journal per origin: `cache/store/<hex(home)>/<hex(old id)>.log`. D8
gives this half to 4.1b. Left in place, those copies would:

- not break subscribe routing. `serve_workspace` fences over the old claims
  (epoch = old maximum + 1), and no client is admitted before the node's own
  claims are minted;
- put `declared_exchange` on two clocks for the binding family: the old id's,
  frozen, and the new one's, starting again at 0. The fold orders by lamport,
  so an old declaration or retraction outranks a newer record for the same
  binding under the new id, and a changed exchange binding would route by the
  old line. That breaks routing for a single node. It would not bite the
  desk's files today: they declare their exchanges as `service` lines, which
  any origin satisfies;
- stop the principal re-mint. `note_principal` finds the old copies and mints
  nothing, so records.json never holds the principals under the new id.

So this step handles the store half minimally. When the server adopts the
instance, before it seeds the registry, the old id's `home` journal is renamed
to `<name>.log.legacy-<date>`, which `Store::open` never replays, and its
chains leave the in-memory store. Every other share, the app data, is
untouched. The check runs at every adoption, so a crash between boot and
adoption is repaired at the next start.

**What the desk sees at its first restart on this binary.** grazel forwards
the node's lines. `node` names a new id. A `set aside` line names the old id
and the legacy file. Each app registers again: grazel `+11 record(s), 0
unchanged` and gyld `+10 record(s), 1 unchanged` (the `ws-razel` entry both
files declare), where a restart printed `+0`. `ws-razel` is served under the
new id with claim epoch 1, where the old id's had reached one per earlier
restart. When a tab says Hello again, each principal it presents is minted
under the new id. The app data (chat,
terminal logs, gyld output) is untouched, and no shipped client reads `home`,
so the UI works as before. The old records stay on disk, in the legacy file
and the renamed journal.

### 6. Tests, each begun red

Each was run first against the code without its change; the message is what
that run printed.

| Test | Proves | Red first |
| --- | --- | --- |
| `peer`: `node_id_is_the_ed25519_public_key_of_the_seed` | the id is RFC 8032 §7.1 TEST 1's public key for its seed | on the old derivation: `left` was `sha256(seed)`, `right` the RFC key |
| `peer`: `hello_handshake_exchanges_identities` | a genuine HELLO is accepted both ways (the existing test, with a channel) | none: it passed before and after |
| `peer`: `a_tampered_hello_is_refused` | a flipped signature byte, another node's id under the signature, protocol 1 and no signature are each refused, with no WELCOME sent; the untouched HELLO is answered | with the old accept-anything check: "a flipped signature byte: accepted" |
| `peer`: `a_hello_replayed_from_another_connection_is_refused` | a HELLO recorded on one channel is refused on another session, and on another endpoint | "replayed onto Channel { … exported: [4, …] }: accepted" |
| `peer`: `a_reflected_hello_is_refused` | a dialer's HELLO mirrored back as the WELCOME, and an acceptor's WELCOME presented to it as a HELLO, are refused | "the dialer took its own HELLO back" |
| `signing`: `a_signature_is_pure_ed25519_over_the_tag_then_the_message` | a plain `verify_strict` over tag then message accepts the signature, and over the bare message does not | against a signer that signed the bare message: the tagged check failed |
| `signing`: `a_small_order_id_signs_nothing` | the identity-point id with R the identity and S zero, which the lax check accepts, is `Invalid` | against a lax `verify`: `left: Valid`, `right: Invalid` |
| `iroh_carrier`: `dial_and_hello_over_iroh`, extended | over real QUIC both ends export the same bytes, and a second connection exports other bytes | none: it checks iroh, the premise of the replay refusal |
| `iroh_carrier`: `a_protocol_1_node_fails_at_connect` | an endpoint offering only `glade/node/1` fails at connect, dialing or dialed | with the ALPN at 1: "a protocol-1 dialer connected" |
| `tests/assembly`: SI-001, SI-002, SI-003 on `NodeSigner` | the adapter passes the port's suite; SI-002 on real keys, `other` recorded as authenticated | SI-001 and SI-002 on the provider the module bound before, `PendingNodeSigner`: "SI-001 signing: Unavailable", "SI-002 signing: Unavailable" |
| `sysdir`: `a_node_key_that_is_not_32_bytes_refuses_the_boot` | a 31-byte key refuses the boot (`InvalidData`), and nothing is written | on the old boot: "called `Result::unwrap_err()` on an `Ok` value" |
| `sysdir`: `a_first_boot_on_the_new_id_sets_the_old_records_aside_once` | the old id's records go, byte for byte, to a new file beside an older one it does not overwrite; another origin's record stays; one node for the operator; a second boot sets nothing aside | with the set-aside stubbed: "the old records are set aside" |
| `sysdir`: `dates_are_utc_calendar_dates` | the file's date across a leap day, a day boundary and 1969 | against a stub: `left: ""` |
| `claims`: `adoption_sets_the_old_ids_home_records_aside` | the dropped exchange binding stops routing, `alice` is minted again under the new id, `ws-x` is claimed at epoch 1, `who_serves` answers the new id from both stores, app data stays | without the adoption's set-aside: "the dropped binding routes no exchange". With its checks turned into prints, that run showed `x.gone` still routable, `alice` not minted, the new claim at epoch 4, fenced over the old 3, both ids' claim chains, and `who_serves` already answering the new id |
| `tests/assembled_path`: `both_roots_set_an_old_instance_aside_and_serve_under_the_new_id` | on each root: the new id, one `set aside` line, `+2 record(s), 0 unchanged`, records.json naming only the new id, `who_serves` the new id from records.json and the served store, `ws-x`'s claim at epoch 1 | with both halves of the set-aside stubbed out, the id change alone: no `set` line, and `app x registered (+1 record(s), 1 unchanged)`, the binding left under the old id |

What they do not prove: that another language's Ed25519 agrees with the tags
(the `proof_family` corpus has no vectors yet); a crash between the legacy
file and records.json; the Windows or Linux runs, which the lane owner makes
on dabeest and the Pi; the old binary itself, which the rehearsal below runs.

### Named gaps

- A peer that holds this node's old records, or its own, serves them at the
  next pull, and the served store takes them unchecked until 4.1b refuses
  unsigned records. No two nodes have linked yet: 4.5's machines ran the
  suites only.
- HELLO admits any key that proves itself (4.2's binding record and refusal at
  accept).
- The iroh endpoint key is new at every start (F1; 4.2).
- No consumer resolves the `SignerPort` adapter yet (4.1b).
- The legacy files are never read and never pruned.
- Off Unix, `node.key` gets default permissions and no check (F5, unchanged).

### Default-path changes

1. The node id changes once, for every instance: `node` prints the hex of the
   key's public key.
2. The first boot on this binary sets the node's old-id records aside, prints
   one `set aside` line after `node`, and rewrites records.json without them.
   The legacy file is about the size of today's records.json, which on the
   desk holds every renewal (F4); records.json starts small again.
3. Adoption renames the served store's old-id `home` journal aside.
4. That boot registers every app again (`+N record(s)`), and the first claim of
   each served workspace is epoch 1.
5. HELLO is signed and checked, on ALPN `glade/node/2`: a node of this build
   and one of an older build cannot link.
6. A new `node.key` comes from `getrandom`, not `/dev/urandom`; a `node.key`
   that is not 32 bytes refuses the boot before anything is written.

### Questions for the owner

1. **The served store's half of D8, done here.** D8 gave it to 4.1b; this step
   did it minimally, one journal renamed, because leaving it breaks exchange
   routing for a single node once a binding changes, and stops the principal
   re-mint. Recommend keeping it. The other choice is to leave it for 4.1b and
   accept both effects until then.
2. **The policy entry.** `ed25519-dalek =3.0.0` and `getrandom =0.4.3` join
   glade-node's row. Recommend accepting both, as shaku and sdax were.
3. **Which `getrandom`.** Recommend 0.4.3, as built: iroh's own randomness
   already runs on it on every machine. 0.2.17 is in the lock only for `ring`.
4. **A refused dialer learns nothing.** An acceptor that refuses a HELLO
   closes the stream without a reason. Recommend keeping it so: an
   unauthenticated peer gets no hint, and 4.2's refusal at accept is the
   place to report one locally.
5. **The legacy files are never read or pruned.** Recommend keeping them,
   under B5's "kept as history but never govern", and pruning by hand once
   4.1b has run; 4.1b's own set-aside writes a second file beside them.

### Measured

2026-09-24, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components. There are 226
  node tests on each path across 15 test binaries, 213 before: 10 in the
  library, 2 net in `tests/assembly` (three SI tests replace the pending
  signer's one) and 1 in `tests/assembled_path`. Confinement sees
  `ed25519-dalek` and `getrandom` from glade-node only, and neither from the
  contracts. `glade/contracts/check.sh` also passes on its own.
- **rustfmt**: glade-node has 325 hunks, 8 below the old baseline of 333, and
  none in a line this step wrote. The 8 went from code it rewrote: HELLO and
  its test in `peer.rs` (5), `Boot::identity` and the boot's return in
  `sysdir.rs` (2), and `PeerEndpoint::bind` (1). The baseline is lowered to
  325 in `check.sh`, as Steps 4.4 and the hardening lowered it. glade-wire
  stays at 43.
- **clippy**: glade-node 11 warnings and glade-wire 7, at the same sites as
  before; one moved four lines down in `iroh_carrier.rs`.
- **The lockfile**: `cargo update --offline --workspace` in `glade/node`, after
  a `--dry-run`. `ed25519-dalek` 3.0.0-rc.0 → 3.0.0 and `curve25519-dalek`
  5.0.0-rc.0 → 5.0.0; glade-node lists its two new dependencies; and
  `crypto-common` 0.2.2 and `signature` 3.0.0 each gain an edge to the
  `rand_core` 0.10.1 already in the lock, which the stable releases' features
  forward to. 379 packages before and after: none added or removed.
- **The branches not compiled here**: the whole node cannot be checked for
  MSVC on this machine, because `ring`'s build script needs the MSVC
  toolchain. So `signing.rs` and `registry.rs`'s two `entry_sync` modules,
  verbatim, with the set-aside's std calls, were checked in a scratch crate on
  the pinned crates: `cargo check` passes with no warning for
  x86_64-pc-windows-msvc, x86_64-pc-windows-gnu, x86_64-unknown-linux-gnu and
  aarch64-apple-darwin.
- **Time**: the 14 HELLO, signing, set-aside, date, adoption and carrier lib
  tests take 0.07-0.10 s together, SI-001..003 under 0.02 s, and the two-root
  transition test 1.3-2.1 s (it starts two nodes), over three warm runs each.
- **The rehearsal.** HEAD's binary (`1501a67`, built to a scratch path) ran
  twice on a scratch instance with the desk's two app files, `--name grazel`,
  as grazel starts it. It printed `node 76b2fa…` and, the second time,
  `+0 record(s), 11 unchanged` for each app. Then this build, on that
  instance:

  ```text
  node 5d8c6089…
  set aside 27 record(s) of old node id 76b2fa… in records.legacy-2026-09-24.json
  registry ready (home served: true)
  app grazel registered (+11 record(s), 0 unchanged)
  app gyld registered (+10 record(s), 1 unchanged)
  workspace ws-razel serving
  ```

  The old `home` journal became `<hex(old id)>.log.legacy-2026-09-24` beside
  the new id's, and `node.key` was untouched. A second start set nothing
  aside and printed `+0 record(s), 11 unchanged` for each app.
- **Downstream**, against the rebuilt default binary
  (`glade/node/target/debug/glade-node`), each suite with a scratch target:
  grazel 26 + 3, glade-gwz 9 + 5, glade-gyld 233 (1 ignored) + 31. All at
  baseline.

**Size**, in lines added and removed in `.rs` files, doc comments included:
production +568/−158 (net +410): `signing.rs` +138, `peer.rs` +160/−50,
`sysdir.rs` +152/−51, `iroh_carrier.rs` +36/−13, `store.rs` +37,
`assembly.rs` +17/−37, `claims.rs` +9/−1, the roots +14/−2, `registry.rs`
+4/−4, `lib.rs` +1. Tests +586/−24. Beside them `Cargo.toml` +8, the policy
+3/−1, and `check.sh`'s rows, text and ratchet.

## Client answers (client-writes plan Phase 2)

Design addition, 2026-09-24, written before the code against glade `54e5999`.
The red runs and the measured figures are filled in afterwards. The spec is
Phase 2 of `glade/dev-docs/GladeClientWritesPlan.md` as ruled: its §5 answers
("all recommended"), and the later ruling that an op below the first seq the
node holds on its chain is answered `Retention`, not `Ok`. The contract is
`GladeSubstrateV1.md` §6, "Session answers (client path)", R1 to R8. Step 2.1
builds R1 to R3 with R2's `Retention` point. Step 2.2 builds R4 to R6, and gets
its own subsection here when it starts.

Nothing changes in the wire IR. `Error.corr`, `ErrorCode::Ok` and
`ErrorCode::Retention` are in it (`taut/ir/glade.taut.py:55-57`, `:187-192`),
and in both copies a client decodes with: `taut/corpus/glade.ir.json`, which
client-ts's suites, grip-share and the demo load, and gryth-ui's vendored copy.

### Step 2.1: a status for every client op

**What changes.**

- The `Frame::Ops` arm (`server.rs`) answers each op with one `Error` frame, in
  the order of the frame's ops. Its `corr` is the op's hash (`chain::op_hash`)
  in lower-case hex, and its `share` and `glade_id` are the op's. One helper,
  `session::op_status`, builds every status, so no status goes out without its
  `corr`.
- The code is:
  - `Ok` when the node holds the op: `Append::Appended`, or `Append::Duplicate`,
    a byte-identical op already held at its seq;
  - `Retention` when the op's seq is below the first op its chain holds;
  - otherwise the refusal's code, as today, from `error_frame` and
    `home_refused` (`Unauthorized`), both of which now take the op.
- An appended op's status is queued on the sender's outbound channel after the
  op has been queued for every other session subscribed to its zone, a peer's
  forwarded interest included (R2). Any other status goes out at once, since
  nothing fans out.
- The store tells the two kinds of repeat apart. `classify` answered
  `Duplicate` both for an op it holds and for one below the chain's first held
  seq ("below retained range — treat as seen", `store.rs:308`). The second
  becomes its own outcome, `Append::BelowRetained`. The other callers act only
  on `Appended` (`ingest_and_fanout`, `seed_registry`) or count any `Ok`
  (`peer::pull_sync`), so they behave as before.
- R3: the session's heads (`client_heads`) take an op's seq only in the two
  `Ok` arms, and never fall, so a repeat of a lower seq leaves the head where it
  is. Before, each op's seq was recorded before its append, whatever the
  outcome, and replaced the one held. A `Retention` op is not held and adds
  nothing. Every op of its chain is above it, so no gap changes.

**Not changed.** The peer paths answer nothing. The `Hello` arm still replaces
the heads a session announces, and the subscribe arm is untouched: both belong
to R4, in Step 2.2. The store lock is still released between an append and its
fan-out (2.2 holds a lock of its own, `cut`, across both; see Step 2.2).

**Tests**, in `server.rs` unless named. Each, new or updated, was run first
against the code without the change, and the last column gives what that run
printed. Two were also run against a halfway form, with the statuses built but
the store not telling the two repeats apart and the heads still replaced, to
show that each pins its own clause.

| Test | Proves | Red first |
| --- | --- | --- |
| `every_client_op_gets_one_status_named_by_its_hash` | one frame of five ops (a new op, its repeat, another op at the held seq, an op past a gap, an op on `home`) gets `Ok`, `Ok`, `Equivocation`, `Protocol` and `Unauthorized`, in order, each naming its op's hash, share and stream | three statuses, not five: `Equivocation`, `Protocol` and `Unauthorized`, each with `corr: None` |
| `a_refused_op_is_not_held_by_its_sender` | a second session's refused `(w, 0)` leaves that session's heads alone, so its subscribe ships the first session's `(w, 0)`. Then R3's second clause: a session that repeats a lower seq keeps its head, so its own later op does not come back to it | "session 2's gap carries the op its refused op contested", `left: []`. Halfway: "session 1's own op came back after it repeated a lower seq", with its `(w, 1)` |
| `an_op_below_the_first_seq_held_is_answered_retention` | on a chain that starts at seq 5, an op at seq 3 gets `Retention` and is not stored, and a repeat of seq 5 still gets `Ok` | no status at all, `left: []`. Halfway: `Ok` where `Retention` was wanted |
| `a_client_op_on_home_is_refused_and_never_stored`, updated | the refusal's `corr` is the op's hash | "the refusal names the op by its hash", `left: None` |
| `a_client_op_on_any_other_share_still_lands`, updated | two `Ok` statuses, in order | `left: []` |
| `end_to_end_over_websocket`, updated | the writer reads its `Ok` before its exchange's answer. Its reads are now bounded (5 s) | "timed out waiting for client 1's status" |
| `grazel_attach_end_to_end` (`exchange.rs`), updated | the provider reads its two `Ok` statuses, by hash, before the forwarded request | "timed out waiting for the provider's op status" |
| `forked_op_surfaces_error_frame_not_silent` (`session.rs`), updated | `error_frame` names the refused op's hash | with the old signature, `error_frame(&err, "sh", "g")`: `left: None` |

**Default-path changes** (the plan's §9, items 1 and 2).

1. Every op a client sends gets one frame back: `Ok`, `Retention` or the
   refusal. Before, a held op got nothing. The shipped clients drop `Error`
   frames: client-rs without decoding them (`client-rs/src/client.rs:108`),
   client-ts after decoding them (`client.ts:97-136`).
2. A refusal carries the op's hash as `corr`, where it carried none.
3. A refused op no longer counts as held by its sender, so a later subscribe on
   that session ships the node's op at that seq. A session that repeats a lower
   seq no longer has its own later ops shipped back to it.
4. An op below its chain's first held seq was answered nothing, and now gets
   `Retention`.
5. The extra frame is queued once per client op. The `Ok` status of an
   appended op is 85 bytes plus the lengths of the share's and the stream's
   names (counted from the encoding, not captured): 104 for
   `ws-razel/gyld.output`. Most of it is the 64-digit `corr`. Its message is
   `appended` or `already held`, since the `corr` names the op. `Retention`
   and the refusals keep a message that names the origin and the seq.

**Named gaps.**

- The peer paths (push, pull, forward) get no status, as the plan says.
- A frame the node cannot decode is still dropped unanswered, so a client may
  not count on a status arriving (R1's failure mode).
- `Ok` promises only what R2 says: the op was written without fsync.
- The extra traffic is not measured.

**Questions for the owner.**

1. **The store's new outcome.** The plan named `server.rs` and `session.rs`
   for this step. The `Retention` ruling came later, and the store is the one
   place that knows which kind of repeat it saw, so `Append` gained
   `BelowRetained`. It is a public enum, but nothing outside the node matches
   it. Recommend keeping it. The other choice is a second look at the chain
   from the server after a `Duplicate`.
2. **The `Hello` arm.** It still replaces a zone's heads with what a `Hello`
   announces. So a `Hello` sent after held ops can lower a head, and the gap
   would then ship the session's own ops back to it, which R4 rules out. No
   client announces heads today. Recommend that Step 2.2 makes the `Hello` arm
   keep the highest too, with a test, since R4 is its rule.
3. **The substrate document.** `GladeSubstrateV1.md` §6 still says the rules
   are "not built" and describes the old answers. This step may not edit it.
   Recommend one edit when 2.2 lands: mark R1 to R6 as built, and reconcile the
   contradictions Step 1.1 listed "to reconcile when the plan's Phase 2 lands".
4. **The rustfmt ratchet.** It drops from 325 to 322 in `check.sh`, as 4.4,
   the hardening and 4.1a lowered it. Recommend keeping it at 322.

**Owner, 2026-09-24, on these questions** (2.1 landed as glade `bc606f4`): 1
keep `Append::BelowRetained`; 2 yes, Step 2.2 makes the `Hello` arm keep the
highest seq, with a test red first (done: see Step 2.2); 3 the owner updates
`GladeSubstrateV1.md` when 2.2 lands, marking R1 to R6 built and reconciling
Step 1.1's list; 4 keep 322, lowered again only if a rewrite removes more
hunks.

**Measured**, on 2026-09-24, on an Apple M3 Pro with Rust 1.96.0, on the final
tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components, in 39-43 s warm.
  There are 229 node tests on each path, across 15 test binaries, where there
  were 226. The 3 new ones are the three new `server.rs` tests.
- **rustfmt**: glade-node has 322 hunks, 3 below the baseline of 325. The rewrite
  removed three old ones: two in the `Ops` arm (the zone tuple and the fan-out)
  and one in `end_to_end_over_websocket` (the op's send). No new line is a
  deviation. The baseline is lowered to 322 in `check.sh`. glade-wire stays at
  43.
- **clippy**: glade-node 11 warnings and glade-wire 7, both at baseline. The
  first gate run counted 12. The extra one was a `clone` in a test, now
  `std::slice::from_ref`.
- **Time**: `server::` and `session::` run 10 tests in 0.01 s, warm.
- **Downstream**, against the rebuilt default binary
  (`glade/node/target/debug/glade-node`), each Rust suite with a scratch target:
  - client-rs: 9 + 3;
  - client-ts: 19, three of them through the node;
  - grip-share: 19;
  - grazel: 26 + 3;
  - glade-gwz: 9 + 6;
  - glade-gyld: 233 (1 ignored) + 31.

  All passed, unchanged. grazel, glade-gwz and glade-gyld are at their
  baselines.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +83/−37, net +46;
  - `server.rs` +57/−26;
  - `session.rs` +17/−9;
  - `store.rs` +9/−2;
- tests: +257/−19;
  - `server.rs` +228/−15;
  - `session.rs` +12/−3;
  - `exchange.rs` +17/−1;
- `check.sh`: the ratchet, one line.

### Step 2.2: the ack is a cut, with hashes, and one refusal form

Written before the code against glade `bc606f4`, where 2.1 landed. The red runs
and the measured figures are filled in afterwards. The spec is plan Step 2.2
(R4 to R6), and the owner's answer to 2.1's second question: the `Hello` arm
keeps the highest seq too.

**What changes.**

- **One lock for the cut.** `Shared` gains `cut`, a `Mutex<()>`. Every fan-out
  path holds it from its append until its ops are queued: the `Ops` arm, and
  `mesh::ingest_and_fanout`, which carries the peer push and pull, the forward
  and `claims::publish`. The subscribe arm holds it while it registers the
  session, reads the zone's heads and gap, and queues the ack and the gap. So
  an op of the zone either lands before the cut, and is in the gap, or after
  it, and is fanned out to the registered session. Either way it arrives once,
  after the ack (R4). Before, the arm registered the session, then read the
  gap, and every fan-out path appended, let the store's lock go, and only then
  routed. So an op could reach a subscriber before its ack, or arrive twice.
- **Why not the store's lock, as the plan says.** The hardening's
  `a_renewal_racing_a_serve_reaches_the_served_store_in_chain_order`
  (`claims.rs`) holds the router's lock, and then takes the store's to read
  what the serve it stopped has landed. If `ingest_and_fanout` held the store's
  lock until its fan-out was queued, that serve would wait for the router's
  lock while holding the store's, and the test would hang. The plan's lock
  argument covers the production sites, not that test. A lock of its own
  keeps the argument and the test. The order is `cut`, then the store's, the
  router's or the session table's, never the reverse, and the directory's
  before `cut` (`publish`). The store's lock is held no longer than before, so
  a route decision or an exchange lookup does not wait behind a fan-out.
- **The ack (R5).** `Store::zone_heads` is the per-zone half of `all_heads`,
  which now calls it. It gives each origin's last seq, and in `Head.hash` the
  32 bytes of that op's hash, the hash R1's `corr` spells in hex.
  `session::ack` puts it in the `Heads` frame. An empty zone's ack still names
  the zone and no origin.
- **The refusal (R6).** `session::refused_subscribe(code, reason, share,
  glade_id)` builds the two frames: `Heads{streams: []}`, then an `Error` with
  the subscribe's share and stream, the code and the reason, and no `corr`. The
  absent route sends them with `UnknownShare`. The session is not subscribed,
  as before. Step 4.3's grant refusal can call the same helper with
  `Unauthorized`.
- **The `Hello` arm** raises a zone's heads to what the `Hello` announces and
  never lowers them, as a held op does under R3. Before, it replaced them, so a
  later `Hello` naming a lower seq made the gap ship the session's own ops back
  to it.

**Not changed.** The peer subscribe (`serve_peer_subscribe`) still registers
before it reads its gap and does not take the cut: the plan leaves that race
to Step 4.3's peer check. Its ack still carries no hashes, and nothing reads it
(`run_forward` takes only `Ops`). A declared exchange's attach ack is
unchanged.

**Tests**, in `server.rs` unless named. Each was run first against glade
`bc606f4`, and the last column gives what that run printed.

| Test | Proves | Red first |
| --- | --- | --- |
| `no_op_of_a_zone_reaches_a_subscriber_before_its_ack` | a writer's op, sent while a subscribe is between its registration and its gap, reaches the subscriber once, after its ack | "an op reached the subscriber before its ack", with the op: 10 runs of 10 |
| `the_ack_names_each_origin_head_with_its_hash` | the ack of `sh/g` names `a` at seq 1 and `b` at seq 0, each with its op's 32-byte hash, which is the hex of R1's `corr`; a keyed zone's ack names its key; an empty zone's names the zone and no origin | `hash: None` for both heads |
| `a_later_hello_never_lowers_the_sessions_heads` | after the session's `(w, 0)` and `(w, 1)` are held, a `Hello` announcing `w` at 0 leaves the head at 1, so a subscribe ships nothing back | "the session's own op came back after a Hello announced a lower seq", with its `(w, 1)` |
| `s_discovery_golden_path_end_to_end` (`mesh.rs`), phase E turned round | a subscribe to `ws-attic` gets `Heads{streams: []}`, then `Error{UnknownShare}` naming the share and the stream with no `corr`; the next subscribe's ack names its zone | "expected an ack that names no zone, got Error(… UnknownShare …)" |

The plan's form of the first test holds only the store's lock. It is not red
on `bc606f4`: the subscribe arm takes the store's lock for its
declared-exchange check before it registers, so a subscriber waiting on that
lock has not registered, and the writer's fan-out misses it. Run 10 times as a
throwaway test, it passed 10 times. So the test holds the router's lock too,
and takes the store's with `try_lock`: it never waits for a lock while holding
one, and cannot deadlock the node, whatever the node's lock order. On the new
code no interleaving of the two sessions changes its answer. With its three
50 ms pauses cut to 0 ms, so that the frames reach their locks in whatever
order they will, it passed 50 runs of 50.

The lock claim above was also run. With the store's lock held through
`ingest_and_fanout`'s fan-out, as the plan has it,
`a_renewal_racing_a_serve_reaches_the_served_store_in_chain_order` hung until
a 30 s alarm killed it. With `cut` it passes, unchanged.

**Default-path changes** (the plan's §9, items 3 and 4).

1. A subscribe to an absent share gets `Heads{streams: []}`, 4 bytes, before
   its `Error{UnknownShare}`. A shipped client's `subscribe()` now resolves
   there, as an empty zone, where it waited for the next `Heads` and then
   resolved the wrong call (F3). So glade-gyld's `resume` and gryth-ui's
   `startGlade`, which 4.3's note found awaiting each subscribe with no
   deadline ("The refusal on the wire", above), go on where they would have
   hung. A share is absent only when the directory knows it and no live claim
   or reachable holder serves it.
2. An ack carries each origin's head hash: 33 more bytes per origin (a 34-byte
   byte string where a 1-byte null was).
3. No op of a zone reaches a session before its ack, and none reaches it
   twice. Before, an op could come live before the ack, or both live and in
   the gap.
4. Each fan-out holds `cut` from its append until its ops are queued: one
   route and one queue push per subscriber. A subscribe holds it while it
   registers, reads its heads and gap, and queues both, the gap's encoding
   included. Every fan-out on the node waits for a subscribe in progress, and
   a subscribe for a fan-out in progress.
5. A `Hello` no longer lowers a head the session holds.

**Named gaps.**

- The peer subscribe keeps F2's race, as the plan says: a forwarded interest
  can get a live op before its ack, or twice. With `cut` in place, closing it
  is a few lines in `serve_peer_subscribe`, for Step 4.3's peer check.
- The peer ack carries no hashes, and no reader wants them yet.
- What `cut` costs is not measured. A subscribe with a large gap holds it
  while the gap is read and encoded, and appends on every zone wait meanwhile.
- R4 holds only while a session's frames leave in the order they are queued,
  as the substrate says: today's websocket and first-in, first-out outbound.
- A `Hello`'s heads are still taken on the client's word (R4's failure mode).

**Questions for the owner.**

1. **`cut`, not the store's lock.** Recommend keeping it. It has the plan's
   shape, one lock that serializes fan-outs and subscribes, keeps the
   hardening test unchanged, and holds the store's lock no longer than before.
   The other choice is the plan's store lock. The hardening test would then
   have to check where the serve stopped with `try_lock` rather than by
   reading the store, and that test is outside this step's list.
2. **The cut test's form.** Recommend keeping it, with the router's lock held,
   since the plan's form passes on the old code. It needs its pauses only to
   show the red.

**Measured**, on 2026-09-24, on an Apple M3 Pro with Rust 1.96.0, on the final
tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components, in 56 s. There
  are 232 node tests on each path, across 15 test binaries, where there were
  229. The 3 new ones are the three new `server.rs` tests.
- **rustfmt**: glade-node has 318 hunks, 4 below the baseline of 322. The
  rewrite removed four old ones: three in `server.rs` (the `Hello` arm's zone
  line, and the subscribe arm's registration and heads lines) and one in
  `store.rs` (`all_heads`). No new line is a deviation. The baseline is
  lowered to 318 in `check.sh`, as the owner allowed. glade-wire stays at 43.
- **clippy**: glade-node 11 warnings and glade-wire 7, both at baseline, at
  the same sites.
- **Time**: `server::` and `session::` run 13 tests in 0.17 s, warm, where 10
  took 0.01 s. The cut test's three 50 ms pauses are the difference. The lib
  suite takes 0.47 s.
- **Downstream**, against the rebuilt default binary, each Rust suite with a
  scratch target:
  - client-rs: 9 + 3;
  - client-ts: 19;
  - grip-share: 19;
  - grazel: 26 + 3;
  - glade-gwz: 9 + 6;
  - glade-gyld: 233 (1 ignored) + 31.

  All passed unchanged, each at its baseline.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +110/−49, net +61;
  - `server.rs` +52/−37;
  - `session.rs` +30/−1;
  - `store.rs` +24/−11 (`all_heads` calls `zone_heads`);
  - `mesh.rs` +4;
- tests: +209/−13;
  - `server.rs` +187/−9;
  - `mesh.rs` +22/−4;
- `check.sh`: the ratchet, one line.

## Transport binding and the door (plan Step 4.2)

Design addition, 2026-09-24, written before the code against glade `e42824e`.
The red runs and the measured figures are filled in afterwards. The spec is
plan Step 4.2 with its line from the rulings of 2026-09-24 (a stable endpoint
key first, the signing note's F1); the rulings `transport_key_binding =
binding_record`, `scope_model = node_trust` and `relay_posture =
community_dev_only`; `GladeNodeSigning.md` D2, D6, D7 and D8, all ruled as
recommended; `dev-docs/IrohGladeMapping.md` §5.2 and §7.2 (`:245-282`,
`:387-395`); and the slice profile's SP-R2 and §8 item 3, which leave the
record's layout, stream, signed bytes and revocation to this step.

Nothing changes in the wire IR, in `NodeHello` or in the ALPN. The node's own
IR, `node/ir/sysdata.taut.py`, gains two record kinds.

**The step is split in three.**

- **4.2a, built with this note** (glade `40647d2`): the stable endpoint key,
  the two record kinds, their fold, minting at boot, and the clock rule
  (sections 1 to 6).
- **4.2b, built after the owner's ruling of 2026-09-25** (section 7): the
  door. That is the refusal at accept, the HELLO check, the refusal lines,
  the configuration of known peers and the `CarrierPort` accessor (sections
  8 and 10).
- **4.2c, its own step** (ruled the same day): an iroh `CarrierPort`
  adapter that tracks its links (sections 11 and 12).

Two things forced the first split.

- **First contact needs the owner's word** (section 7). The plan and the
  rulings point to configured peers. They do not say how the accepting side
  is configured, or whether the door is closed by default. Today `--peer` is
  dial-only and one-sided: Step 3.3's lifecycle and stop-signal tests link B
  to A with `--peer` on A alone. Once the door is closed, B must know A
  beforehand.
- **Size.** The whole step comes to about 800 production lines and 1,800 in
  all, above the brief's limits of about 450 and 1,100. 4.2a measured 483
  production lines, 49 of them generated, and 519 of tests ("Measured",
  below). 4.2b is about 300 production lines and 500 of tests. An iroh
  `CarrierPort` adapter would add about 300 more of each (section 8).

### 1. The endpoint key (F1)

**Cause.** `bind_endpoint` (`iroh_carrier.rs:53-61`) gives iroh no secret key,
so iroh draws a new one at every bind (iroh 1.2 `src/endpoint.rs:228`). So
every start of a node has a new endpoint id, and nothing can name it: not a
binding record, not 4.5's `--peer <endpoint-id>@…`, not a relay.

**The key.**

- `endpoint.key` is a second class-1 secret in the instance, beside
  `node.key`. It holds exactly 32 bytes, an Ed25519 seed.
- It is created mode 0600 from `signing::random_seed()`. The boot refuses it
  if it is group- or world-accessible, as it refuses such a `node.key`. Off
  Unix it gets default permissions and no check, as `node.key` does (F5).
- It is minted once: at a new instance's first boot, and at an existing
  instance's next boot on this build. Every later boot loads it. A length
  other than 32 refuses the boot, before records.json is read.
- Its Ed25519 public key is the endpoint id: iroh's
  `SecretKey::from_bytes(seed).public()`, the key `signing::public_key`
  computes from the same seed.
- It is not derived from `node.key`. The ruling says "identity must survive a
  transport key being replaced". A derived key could be replaced only with
  the node key, and so with the identity.
- It needs no recovery material. A lost `endpoint.key` means a new key and a
  new binding under the same node id (section 5).
- Both booted roots bind with it: `PeerEndpoint::bind_as(identity, key)`.
  `bind` and `bind_with` still draw a fresh key. Only tests and the async
  witness call them.
- `transport::EndpointKey` keeps the seed private, and its `Debug` prints
  only the endpoint id.

### 2. The record kinds

Both kinds ride `home`, in the node's own chain, one stream each.

| Kind | Stream | Fields (taut number, type) | Signed by `node`, over |
| --- | --- | --- | --- |
| `NodeTransportBinding` | `dir.transport-bindings` | 1 `node` text, 2 `endpoint_id` text, 3 `valid_from` int, 4 `sig` bytes | `glade/v1/transport-binding\0`, then the canonical CBOR of fields 1 to 3 |
| `NodeTransportRevocation` | `dir.transport-revocations` | 1 `node` text, 2 `endpoint_id` text, 3 `sig` bytes | `glade/v1/transport-revocation\0`, then the canonical CBOR of fields 1 and 2 |

- A binding says "endpoint key E is transport for node N, from
  `valid_from`". A revocation says "N no longer uses E".
- `node` and `endpoint_id` are 64 lower-case hex digits. That is how the
  directory writes every node id, and how iroh prints an endpoint id: the
  `peer` line shows the same text.
- `valid_from` is the minting node's wall clock in epoch milliseconds.
  Section 4 says how a reader judges it.
- `sig` is 64 bytes of pure Ed25519 by the node key, checked with
  `verify_strict`.
- `dir.bindings` is taken by app declarations (R9), hence the longer stream
  names.

**Why the record carries its own signature.** Until 4.1b, `home` records are
unsigned, and any peer can write `home` (4.3's named gap). So a binding must
prove itself. The node id is the node's public key (D2), so any reader checks
a binding with no lookup, on first contact too. A binding carried out of band
(section 7, option (c)) would be the same bytes.

**Why a domain of its own, not `origin-op`.**

- D7's `origin-op` signs an op's fields 1 to 10, with the record as the
  payload. This signature sits inside the payload, so it cannot cover the op
  that carries it.
- A tag of its own means a binding's signature can never be read as an
  op's, a HELLO's, an overlay's or a revocation's. The tags are ASCII and
  end in a zero byte, and none of the five is a prefix of another.
- `SignerPort`'s three purposes stay as they are, so no contract changes.
  Like HELLO, bindings are signed and checked with the node's own functions,
  not through the port.
- When 4.1b lands, the op that carries a binding gets the `origin-op`
  envelope, like every `home` record. The inner signature stays, so a
  binding can still be checked apart from its chain.

**Why a revocation is a kind of its own.** It is "the usual revocation-wins
rule" of `IrohGladeMapping.md` §7.2, the one grants follow (`dir.grants` and
`dir.revocations`). Each record keeps one meaning, and no later binding
revives a revoked pair (section 3).

### 3. The fold

`transport::TransportFold` is folded out of any op-set: the registry's records,
for the boot, or the served store's `home` share, which holds every peer's
records too and which 4.2b's door reads.

1. **A record counts, or it is ignored.** It counts only if all of these hold:
   - its payload is the canonical encoding of its kind: exactly the fields
     above, of those types;
   - its two ids are 64 lower-case hex digits;
   - `sig` verifies strictly under the named node for the kind's tag;
   - it rides `home`, in that node's own chain (`op.origin == node`, the
     ruling's "in the node's own chain").

   Anything else is ignored and counted: a forgery, a malformed payload, a
   record in another origin's chain. Payloads are read with a checked
   decoder, because the wire codec's `decode` panics on bytes it cannot read
   (F2). An ignored revocation revokes nothing: only the node itself can
   withdraw its binding, or any peer could cut any node off.
2. **Set union.** A pair (node, endpoint key) is bound when a counted binding
   names it. Its date is the earliest `valid_from` among those bindings.
3. **Revocation wins.** A counted revocation of a pair clears every binding
   of that pair, earlier or later, for good, as a `CapabilityRevocation`
   clears its (principal, share) (`registry.rs:495-516`). A node cannot take
   a revoked key back; it mints a new one.
4. **Arrival order never matters.** The fold is a pure function of the
   op-set.
5. **The answer.** For a pair, at a reader's clock (section 4), the fold
   answers `Live`, `NotYet { valid_from }`, `Revoked`, `Unbound` or
   `ClockUncertain`. It also lists the keys a node has bound and not
   revoked, which the boot uses (section 5).

### 4. The clock rule

- `valid_from` is judged at each reader's clock when it reads. It never
  enters the fold, as lease expiry does not (WD §2). A binding is live when
  `valid_from <= now` and no revocation names its pair. A revocation does
  not depend on time.
- There is no skew margin. A binding dated after a reader's clock is
  `NotYet` there, until the clock gets there. That is the closed direction:
  a reader whose clock is behind refuses more, never less. So a clock that
  has gone back is not treated as uncertain. It only makes the fold
  stricter, and treating it as uncertain would shut a node out for as long
  as its clock once ran ahead.
- **Uncertain.** The node keeps no clock watermark: SP-C2's belongs to the
  discovery kernel. So the only uncertainty the node can see is a clock it
  cannot read, one earlier than 1970, where `now_ms` answers 0. Then the
  fold answers `ClockUncertain` for every pair but a revoked one, and fails
  closed:
  - the boot mints no binding (a revocation needs no clock, and is still
    minted);
  - 4.2b's door admits nobody, configured peers included, and no HELLO
    completes; each refusal names the reason.
- **Not closed: a clock that runs ahead.** It makes a binding live before its
  `valid_from`. In the slice every binding is dated when it is minted, so
  this admits nothing its node did not sign. A binding dated ahead on
  purpose, a planned rotation, would need SP-C2's watermark or another
  trusted time. That is a named gap.

### 5. Minting at boot

The boot binds after presence, in its class-1 to class-2 step, and in the one
save of records.json it already makes.

1. If the fold holds a counted revocation of this node's current endpoint
   key, the boot is refused (`InvalidData`) before records.json is written.
   The message says to move `endpoint.key` aside to mint a new key. Only a
   restored old key meets this.
2. If no counted binding by this node names its current key, the boot
   appends one, dated at its clock. If its clock cannot be read, it appends
   none (section 4).
3. For every other key this node has bound and not revoked, the boot appends
   a revocation. So replacing a key is: stop the node, move `endpoint.key`
   aside, start it. Both roots then print `revoked N binding(s) of replaced
   endpoint key(s)` after `node`, and only then.
4. At adoption the served store takes the new records with the rest
   (`seed_registry`, `server.rs:103-113`), and peers pull them from there. No
   node signs or revokes another node's binding.

### 6. Compatibility

- **The wire** does not change: no frame, no field, no ALPN. 4.2a changes
  nothing a peer sees but two more streams in `home`.
- **An older node** (4.1a's build, on `glade/node/2`) linked to this one takes
  the new records into its served store like any `home` stream. It checks
  their chains, serves them on to its own peers, and decodes neither kind,
  because nothing it runs reads those streams: routing reads claims and
  workspaces, exchanges read bindings and services, the Hello arm reads
  principals. Its registry never takes a peer's records. A node older than
  4.1a fails at connect, as it has since 4.1a.
- **An older binary on an instance this build wrote**, a downgrade, keeps both
  kinds in records.json. Its registry keeps every record whose chain checks,
  and decodes neither kind. It never reads `endpoint.key`, and draws a fresh
  endpoint key at each start, as before. Upgrading again finds the binding
  and mints nothing. Nothing is lost either way.
- **The owner's desk at its first restart on this build:**
  - `endpoint.key` appears, 0600, beside `node.key` in `<data>/sys/sys/grazel/`;
  - records.json gains one record, the binding, under the node's id, as seq
    0 of `dir.transport-bindings`, beside the claim every start already
    mints;
  - the served store takes it at adoption;
  - the `peer <endpoint-id> <addr>` line names the same endpoint id at every
    restart from then on;
  - no other line changes: nothing is set aside, and each app registers `+0`;
  - nothing connects to the desk's endpoint, since grazel passes no `--peer`
    (`grazel/src/lib.rs:240-255`), so nothing else changes. Nothing is lost.
- **4.2b's door** would change nothing on the desk either, for the same
  reason.

### 7. First contact: a question for the owner

**Ruled, owner, 2026-09-25 ("all recommended"):** (a2). An accepting node
lists each peer's endpoint id with no address, known and not dialed; the door
is closed by default on booted nodes; bindings learned through a configured
peer's directory count as known. Built in 4.2b (section 8).

**The fact.** A binding reaches a peer through the `home` pull that a
completed HELLO opens (`mesh.rs:202-244`). So on first contact neither side
holds the other's record. The plan's HELLO check ("the presented `node_id`
bound to `remote_id()`") and its refusal at accept both need something else to
stand in for the record, once.

**What 4.1a's HELLO already proves** on every connection: the named node holds
its key, and signed for this TLS session, between these two endpoint ids, in
its role. So once a connection is admitted, the HELLO binds the node to the
endpoint for that connection. What first contact needs is a rule for
admitting an endpoint key that no record names yet.

**The options.**

- **(a) Configured peers.** The operator names the peer's endpoint key on
  each side.
  - On first contact a configured key stands in for the record: the door
    admits it, and the HELLO binds the node id to it.
  - The record then arrives by the pull, and it governs every later
    connection. A revocation refuses the key even though it is configured.
  - The accepting side must be configured too, in one of two forms:
    - **(a1)** `--peer <id>@<addr>` on both sides, so each dials the other.
      A `--peer` whose node is down holds the start until iroh gives up:
      measured at 30.2 s on the hand-written root, which then prints `peer
      …: timed out`. The roots dial before they serve;
    - **(a2)** an admit-only entry, `--peer <endpoint-id>` with no address:
      known, not dialed.
  - Either way, a node's endpoint id must be known before its peer starts.
    Since section 1, one first start prints it, and it never changes.
- **(b) Trust on first use.** The accepting side admits any key that proves a
  node key at HELLO, and pins what it learns. The door refuses only revoked
  keys, and keys a record binds to another node. That is today's behaviour
  plus revocation. Anyone who learns an endpoint id can connect and pull the
  directory, which is the relay ruling's concern: on a public relay, endpoint
  ids are "the only lock on the door".
- **(c) The binding carried out of band.** The operator copies the peer's
  signed binding, the record's own bytes, into the other node's
  configuration, and the node takes it in before any connection. Then the
  door and HELLO check a record even on first contact, and the configuration
  names the node as well as the key. It needs an export command, an import
  flag, a place in 4.5's configuration, and a relaxation of section 3's chain
  rule, since a carried binding has no chain.

**Introductions.** A binding that arrives in a configured peer's `home` share,
a third node's record, names a key no configuration named. Under node trust it
counts as known: the peer is trusted, and it can carry a binding but cannot
forge one.

**What the rulings point to.**

- `scope_model`'s source, `IrohGladeMapping.md` §5.2, gives model N as
  "accept by key against the known-node set", and says for the first slice
  that "the fixed-peer configuration already is a node-trust set".
- `relay_posture` says peers are "named directly", and asks for a door that
  locks.
- §7.2 says that until the record exists, "the only honest binding is the CLI
  `--peer` flag".

They point to (a). They do not say how the accepting side is configured, or
whether the door is closed by default. Closing it breaks today's one-sided
`--peer`.

**Recommendation:** (a2); the door closed by default on every booted node;
introductions count. No shipped flow uses peers (grazel passes no `--peer`),
so closing by default changes only tests. The lifecycle and stop-signal tests
would give the accepting node the dialer's key as an admit-only entry, having
minted the dialer's key first.

### 8. The door (plan Step 4.2b, as built)

Built on 2026-09-25, as section 7's ruling sets it, against glade `40647d2`.
The tests and their red runs are in section 10.

- **Where.** The door is `transport::Door`. iroh's
  `EndpointHooks::after_handshake`, on the accepting side, checks the key; it
  is the one hook iroh offers after TLS, and it sees `remote_id()`. Then HELLO
  checks the node on both sides, in `hello_accept` and `hello_dial`
  (`peer.rs`), before a WELCOME is sent or taken. Each side checks the far
  end's key, the channel's `dialer` or `acceptor` id. An outbound connection
  passes the hook and is checked at HELLO.
- **The configuration.** A `--peer` entry names an endpoint id, and the door
  admits it on first contact (`iroh_carrier::PeerEntry`).
  - `--peer <endpoint-id>` configures the key and dials nothing.
  - `--peer <endpoint-id>@<ip:port>` configures the key and dials it, as
    before.
  - Anything else prints `peer <entry>: expected <endpoint-id> or
    <endpoint-id>@<ip:port>`, where it printed the form with `@` alone.
  - Both roots read the entries before they bind, and bind behind the door
    (`PeerEndpoint::bind_door`).
- **The view.** The door keeps the configured keys, fixed when it is made,
  and its own copy of the fold.
  - The copy is loaded from the served store's `home` share before the
    accept loop starts (`enable_mesh`).
  - It is fed each record that lands there after, in `ingest_and_fanout`,
    which the pull, the push, a forward and `publish` all use.
  - `seed_registry` does not feed it. Both roots adopt the instance before
    they enable the mesh, so the load covers what adoption seeds.
  - The hook holds the door, never the endpoint, which iroh warns would be a
    reference cycle.
- **The policy for an endpoint key E**, at the reader's clock:

  | What the fold and the configuration hold for E | At accept | At HELLO, node N |
  | --- | --- | --- |
  | the clock is uncertain | refused | refused |
  | a revocation, and no live binding | refused | refused |
  | a live binding (M, E) | admitted | admitted if N is M; else refused |
  | only bindings dated after the clock | refused | refused |
  | no record, E configured | admitted: first contact | admitted; the pull brings the record |
  | no record, E not configured | refused | refused |

  A configured key whose record arrives is judged by the record from then
  on: a revoked one is refused though configured.
- **A refusal.**
  - At accept, the connection is closed with code 0 and an empty reason.
    The dialer learns nothing: its root prints `peer <target>: connection
    lost`, and in-process the error is `ConnectionLost(ApplicationClosed(
    ApplicationClose { error_code: 0, reason: b"" }))`.
  - At HELLO, the acceptor sends no WELCOME and drops the connection.
  - The refusing node prints one stderr line, `peer refused: endpoint <E>:
    <reason>`. The reason is `clock uncertain`, `unknown endpoint key`,
    `revoked by node <M>`, `bound from <valid_from>` or `bound to node <M>,
    not <N>`; a refusal at HELLO reads `HELLO refused: <reason>`, or 4.1a's
    signature refusal.
  - The hook reports its refusals, and `PeerEndpoint::accept` those of
    HELLO. The accept loop is unchanged.
  - A dialer that refuses a WELCOME reports it with the line it printed
    before, `peer <target>: HELLO refused: <reason>`.
  - On the assembled root, the lines go to the console's stderr.
- **A live link whose key is revoked** is closed when the revocation lands.
  Only the revoking node's link on that key is closed: another node's live
  binding of the same key is not revoked.
- **Which endpoints get a door:** booted nodes, on both roots. `bind`,
  `bind_with` and `bind_as` keep none: they serve tests and the async
  witness, which boot no instance.
- **The accessor.** `CarrierLink` gains `fn remote_id(&self) ->
  Option<TransportId>`, with `TransportId(pub Vec<u8>)`. It is the identity
  the transport authenticated for the far end: 32 bytes for iroh, `None`
  from a carrier with none, as the client role's WebSocket has none. It names
  a key, never a node, and outlives the link's close.
  - CA-005 checks it over three endpoints: `a` dials `b`, then `fresh` bound
    where `b` was. Two endpoints cannot tell a link that names its own end
    from one that names the far end; a third can. (Since 4.2c, `fresh` is
    dialled where its bind answers, which on iroh is not `b`'s address:
    section 11.)
  - The contract's fixture gains a deliberately wrong link that names its
    own end, which CA-005 refuses. The node's fake network and its faulty
    link implement the method.
  - `glade/contracts/architecture-policy.json` lists `remote_id` among
    `CarrierLink`'s methods, so the checker requires it. The lane owner
    added it in 4.2b's commit, for the owner's review, as the policy's
    earlier changes were.
- **The iroh adapter's links:** 4.2c, ruled a step of its own (section 11).
- **A gap for later.** A HELLO over a `CarrierLink` would also need the TLS
  exporter bytes (D6), which the port does not expose. That belongs to the
  step that moves the mesh onto the port.

### 9. Tests (4.2a), each begun red

Each was run first against the code without its change, and the message is
what that run printed. For the red runs of the library tests, the key was a
fresh one at each boot, as iroh drew it before, and the fold and the boot's
binding were stubs that folded and appended nothing.

| Test | Proves | Red first |
| --- | --- | --- |
| `tests/assembled_path`: `both_roots_keep_one_endpoint_id_across_restarts` | on each root, a second start of one instance prints the same endpoint id on its `peer` line, and it is not the node id | on the old code: `HandWritten`, left `b48e1b17…`, right `38c056d6…` |
| `tests/assembled_path`: `both_roots_revoke_a_replaced_endpoint_keys_binding` | on each root, a start after `endpoint.key` was moved aside prints a new endpoint id and, after `node`, `revoked 1 binding(s) of replaced endpoint key(s)`; records.json then revokes the old key and binds the new one | with the fresh key per boot: there was no `endpoint.key` to move, "No such file or directory" |
| `sysdir`: `the_endpoint_key_is_a_second_secret_kept_across_boots` | `endpoint.key` holds 32 bytes, its key is not the node's, a second boot has the same key, and a 31-byte file refuses the boot, naming the file | the same: "No such file or directory" |
| `sysdir::unix`: `endpoint_key_is_0600_and_group_readable_is_refused` | the file is created 0600, and at 0640 the boot is refused (`PermissionDenied`), naming the file | the same |
| `sysdir`: `a_boot_binds_its_endpoint_key_and_revokes_a_replaced_one` | the first boot binds the key; the next writes nothing; a boot with a new key binds it and revokes the old one; the old key restored refuses the boot and records.json is not written | with the boot's binding stubbed: left `Rebound { minted: false, revoked: 0 }`, right `minted: true` |
| `sysdir`: `an_instance_from_before_the_step_binds_a_new_key_at_its_next_boot` | an instance with no `endpoint.key` and no binding mints both at its next boot, one binding, no revocation, no set-aside, one presence | with the fresh key per boot: "No such file or directory" |
| `transport`: `the_fold_is_a_set_union_and_a_revocation_wins_for_good` | the earliest date of a pair counts; a revocation clears its pair's bindings, earlier and later, and no other pair; the ops reversed give the same answers | with the fold stubbed: left `Unbound`, right `Live` |
| `transport`: `a_binding_is_live_from_its_valid_from_at_the_readers_clock` | `NotYet` before `valid_from`, `Live` from it, `ClockUncertain` at 0 and below, `Revoked` whatever the clock | the same stub: left `Unbound`, right `NotYet { valid_from: 1000 }` |
| `transport`: `a_record_that_does_not_prove_itself_binds_nothing` | nine records that prove nothing (a flipped signature byte, another key's signature, another origin's chain, a non-canonical payload, an upper-case id, a torn payload, bytes that are not CBOR, a revocation on the bindings stream, a revocation signed by another node) are each ignored and counted, none panics, and the genuine binding stays live beside them | the same stub: `ignored`, left `0`, right `9` |
| `transport`: `a_boot_binds_its_key_once_and_revokes_a_replaced_one` | the boot rule over a registry, and the refusal of a revoked key with nothing appended | with the boot's binding stubbed: left `minted: false`, right `minted: true` |
| `transport`: `a_boot_on_an_unreadable_clock_binds_nothing_and_still_revokes` | at clock 0 no binding is minted and the replaced key is still revoked | the same stub: left `revoked: 0`, right `revoked: 1` |
| `mesh`: `a_peers_binding_arrives_by_the_pull_and_folds_live` | over real iroh, each node's binding reaches the other's served store by the pull, and folds live there for the key the connection came from | with the boot's binding stubbed: "timed out waiting for B's binding at A" |
| `tests/assembly`: `the_directory_profile_hosts_every_directory_record_kind_and_nothing_else`, extended | the directory profile hosts both new streams | before `DirectoryRules` named them: "dir.transport-bindings" |
| `transport`: `each_record_is_signed_in_its_own_domain_over_its_own_fields` | both signatures, checked against bytes built by hand: pure Ed25519 over the tag, then the canonical CBOR of the fields, and in no other domain | none: it pins the layout it was written with |
| `signing`: `no_tag_is_a_prefix_of_another` | the five tags are ASCII, end in a zero byte, and none is a prefix of another | none: it guards the table |

What they do not prove: that another language's Ed25519 agrees with the two
tags (the `proof_family` corpus has no vectors yet); a crash between the key
file and the save of records.json (the next boot binds the key it finds);
Windows and Linux, which the lane owner runs on dabeest and the Pi; anything
about refusal, since 4.2a has no door.

### Named gaps (4.2a)

- No door yet. HELLO still admits any key that proves a node key, so the relay
  ruling's rule stands: endpoint ids stay on our own machines until 4.2b.
- A clock that runs ahead makes a binding live before its `valid_from`
  (section 4). The node keeps no clock watermark.
- Off Unix, `endpoint.key` gets default permissions and no check, as
  `node.key` does (F5).
- The two tags have no vectors in the `proof_family` corpus.
- A peer can still write `home` until 4.1b. The fold ignores a forged binding,
  but a malformed record on another `home` stream still panics its readers
  (F2), unchanged.
- Nothing reports the fold's `ignored` count yet. 4.2b's door is its first
  reader.
- The `peer` line prints the endpoint id, now the same at every start, to
  stdout, which grazel forwards. 4.5 takes endpoint ids out of logs.

### Default-path changes (4.2a)

1. A booted node creates `endpoint.key`, 0600, at its first start on this
   build, and its iroh endpoint binds with it. The endpoint id on the `peer`
   line is the same at every start. Before, it was new at every start.
2. The boot appends a signed `NodeTransportBinding` when its current key has
   none, once per key, and a `NodeTransportRevocation` for each key it
   replaced. Only a replacement prints a line: `revoked N binding(s) of
   replaced endpoint key(s)`.
3. A boot whose `endpoint.key` this node has revoked is refused (`InvalidData`),
   and so is an `endpoint.key` that is not 32 bytes, or that is group- or
   world-accessible on Unix, as for `node.key`.
4. The served store's `home` holds two more streams, which peers pull.

Nothing on the wire changes, and an ordinary start prints the lines it printed
before.

### Questions for the owner (4.2a)

1. **First contact** (section 7). Recommend (a2): an admit-only `--peer
   <endpoint-id>`, the door closed by default on booted nodes, and
   introductions counting. The alternatives are (a1), symmetric dialing, which
   costs a 30 s start when the peer is down; (b), trust on first use, which
   leaves the door open; and (c), carried bindings, which need an export
   command and an import flag. 4.2b waits on this answer.
2. **The two signing domains**, `glade/v1/transport-binding\0` and
   `glade/v1/transport-revocation\0`, outside `SignerPort`'s purposes.
   Recommend keeping them. The other choice is `origin-op` over the same
   bytes, with a binding's signature kept apart from an op's only by the
   shape of what it signs.
3. **The clock rule** (section 4): uncertain means unreadable, and no skew
   margin. Recommend keeping it. The other choice is a watermark (the node's
   newest own `valid_from`), which adds no safety, since a clock that went
   back only refuses more, and can shut a node out for as long as its clock
   once ran ahead.
4. **A revoked key refuses the boot** rather than being replaced
   automatically. Recommend keeping it: a restored old key is an operator's
   mistake to see, not to paper over.
5. **The iroh `CarrierPort` adapter** with its link tracking (section 8).
   Recommend a step of its own, 4.2c, or with 4.5. 4.2b adds only the
   accessor and its conformance probe.

**Ruled, owner, 2026-09-25 ("all recommended"):** 1 (a2), the door closed by
default, introductions counting; 2 the two domains kept; 3 the clock rule
stands; 4 a revoked key refuses the boot; 5 the adapter is 4.2c, a step of
its own.

### Measured (4.2a)

2026-09-24, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components, in 68 s from
  an empty target. There are 246 node tests on each path, across 15 test
  binaries, where there were 232. The 14 new ones are those of section 9 but
  the extended profile test.
- **rustfmt**: glade-node has 318 hunks, its baseline, and none in a line this
  step wrote. `transport.rs` is new and formatted whole; the new tests in
  `sysdir.rs`, `mesh.rs` and `tests/assembled_path.rs` were formatted as
  rustfmt lays them out, and no older line was touched. glade-wire stays at
  43.
- **clippy**: glade-node 11 warnings and glade-wire 7, at baseline. The fold's
  pair type first drew a `type_complexity` warning, removed by naming it.
- **The contracts** do not change in 4.2a. `glade/contracts/check.sh` passes
  on its own (87 tests).
- **The regeneration.** The generator at taut `7a5f616` reproduces HEAD's
  `sysdata.rs` byte for byte (`cmp`). After the IR change, from `taut/`:

  ```text
  PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=src /opt/homebrew/bin/python3 -m taut.cli gen \
      ../glade/node/ir/sysdata.taut.py -o $S/gen-after -l rust --api-only --legacy-codec
  cp $S/gen-after/rust/api.rs ../glade/node/src/sysdata.rs
  ```

  `cmp` shows `sysdata.rs` is exactly what the generator wrote. The diff is
  +49 lines, the two structs and their codecs, and nothing else moves.
- **Time**: the eleven new library tests of `transport`, `signing` and
  `sysdir` take 0.04-0.05 s together; the two-node test 0.04 s; the two
  process tests 1.19-1.22 s, for eight starts of the node. Three warm runs
  each.
- **Option (a1)'s cost**: HEAD's binary, with one `--peer` whose node is down,
  printed `listening` after 30.2 s, and `peer …: timed out`.
- **The rehearsal.** HEAD's binary (`e42824e`, built to a scratch path) ran
  twice on a scratch instance with the desk's two app files, as grazel starts
  it (`--profile local --name grazel`). Its endpoint id differed between the
  two starts, and there was no `endpoint.key`. Then this build ran twice on
  that instance:

  ```text
  node e8d8b8ec…
  registry ready (home served: true)
  app grazel registered (+0 record(s), 11 unchanged)
  app gyld registered (+0 record(s), 11 unchanged)
  peer 5c153822… 127.0.0.1:56131
  workspace ws-razel serving
  workspace ws-razel serving
  listening 59041
  ```

  The lines were HEAD's, with the same node id. `endpoint.key` appeared: 32
  bytes, mode 0600. records.json gained the one binding, beside the claim
  that every start mints. The second start printed the same endpoint id and
  added no binding.
- **Downstream**, against the rebuilt default binary
  (`glade/node/target/debug/glade-node`), each Rust suite with a scratch
  target:
  - client-rs: 9 + 3;
  - client-ts: 19;
  - grip-share: 19;
  - grazel: 26 + 3;
  - glade-gwz: 9 + 6;
  - glade-gyld: 233 (1 ignored) + 31.

  All are at baseline.
- **The async witness**, which calls `PeerEndpoint::bind_with`, type-checks
  against this tree (`cargo check --workspace --all-targets`). That ran on a
  scratch copy, because the witness's lockfile predates 4.1a's two crates and
  `--locked` refuses it at HEAD too.
- **Platform branches**: no new one. The only change inside a platform module
  is the Unix permission refusal naming its file.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +523/−40, net +483;
  - 49 of them are the generated `sysdata.rs`;
  - `transport.rs` +341, new;
  - `sysdir.rs` +36/−10;
  - `iroh_carrier.rs` +26/−11;
  - `signing.rs` +24/−5;
  - `registry.rs` +19/−2;
  - the roots: `bin/glade-node.rs` +13/−7, `lifecycle.rs` +9/−2;
  - `assembly.rs` +5/−3, `lib.rs` +1;
- tests: +524/−5, net +519;
- beside them, the IR +23 (`sysdata.taut.py`).

### 10. Tests (4.2b), each begun red

Each was run against the code without the part it guards, switched off by one
edit and restored after; the message is what that run printed.

| Test | Proves | Red first |
| --- | --- | --- |
| `mesh`: `an_unknown_endpoint_key_is_refused_at_accept_and_reported` | over real iroh: a door that knows nothing refuses the dialer's key at accept, before HELLO; its node reports `peer refused: endpoint <E>: unknown endpoint key`; the dialer's error carries no reason; no link | without the accept hook, the key was refused a step later, at HELLO: left `…: HELLO refused: unknown endpoint key`, right `…: unknown endpoint key` |
| `mesh`: `a_bound_key_links_and_is_refused_once_its_revocation_lands` | a key bound by a record in the served store links with no configuration; the node's revocation, landing as a push lands, closes the live link; the next dial is refused, `revoked by node <A>` | without the door's load: "a bound key links: … `ApplicationClose { error_code: 0, reason: b"" }`"; without the feed, and again without the close: "the live link was closed", left 1, right 0 |
| `mesh`: `a_key_bound_to_another_node_cannot_complete_hello` | a key the fold binds to another node passes the hook, and the HELLO of the node that holds it is refused, unanswered: `HELLO refused: bound to node <M>, not <N>` | without the HELLO check: "a node linked through another's key" |
| `peer`: `a_hello_completes_only_for_a_node_bound_to_its_endpoint_key` | in memory: an unknown key is refused and unanswered; a configured one admitted, first contact; one bound to another node refused; one bound to the dialer admitted; and the dialer refuses a WELCOME from a node not bound to the acceptor's key | with the check stubbed: "unknown: Ok(PeerHello { … })" |
| `transport`: `the_door_admits_a_live_or_first_configured_key_and_refuses_the_rest` | section 8's table, row by row, and a configured key its record revokes | with the rule stubbed to admit: left `Ok(())`, right `Err(BoundElsewhere { … })` |
| `transport`: `a_door_takes_records_as_they_land_and_reports_its_refusals` | the door's feed, its clock, and its line | the same stub: left `Ok(())`, right `Err(Unknown)` |
| `iroh_carrier`: `a_peer_entry_names_a_key_and_perhaps_where_to_dial_it` | the two entry forms, and what is no entry | without the admit-only form: a bare endpoint id parsed to nothing |
| `tests/assembled_path`: `both_roots_refuse_an_unknown_dialer_and_admit_a_configured_one` | on each root, as processes: B refuses A, naming A's key on stderr, and A's line carries no reason; B started again with `--peer <A's endpoint id>` admits A, which links | with the hand-written root bound without its door: A linked |
| `tests/lifecycle`: `a_dialer_its_peer_does_not_know_is_refused_and_reported` | on the assembled root, in process: the refusal line reaches B's console, A's line carries no reason, and both stop clean | with the assembled root bound without its door: B reported no refusal |
| `tests/lifecycle`: `a_node_links_to_a_peer_and_stops_clean_with_its_ports_free`; `tests/stop_signal`: `a_stop_signal_stops_the_assembled_node_cleanly`, both changed | Step 3.3's done-when, with B admitting A's key, minted by one boot of A's instance first | before the change, on this tree: "no `peer-connected` line", A's stderr `peer …: connection lost` |
| contracts: `ca_005_each_link_names_the_far_ends_transport_identity`; node: `ca_005_the_fake_network_names_each_far_end` | CA-005 on the contract's fixture and on the node's fake network | on a fixture whose links named their own end: "two endpoints name the endpoint that reached both alike", left `[3, 0, …]`, right `[1, 0, …]` |
| contracts: `rejects_a_link_that_names_its_own_end` | CA-005 refuses that fixture | none: it guards the probe |

What they do not prove: Windows and Linux, which the lane owner runs on
dabeest and the Pi; a relay (4.5); two nodes started at once, each dialing the
other.

### Named gaps (4.2b)

- The door covers the iroh endpoint only. The websocket listener is as it
  was: loopback, the `Origin` check, and 4.3's switch.
- A key admitted on first contact is bound to its node by that connection's
  HELLO. If a record later binds the key to another node, the live link stays
  open until it closes; only a revocation closes one.
- Introductions: under node trust, a configured peer can introduce any number
  of nodes through its directory.
- Until 4.1b, peers can push `home` records, transport records among them.
  The fold ignores and counts any that do not prove themselves, and never
  decodes them unsafely.
- The accept loop takes one connection through HELLO at a time, as before: a
  slow dialer delays the next.
- The refusal line names endpoint ids, as ruled. 4.5 takes endpoint ids out of
  logs.

### Default-path changes (4.2b)

1. A booted node's endpoint refuses, at accept, every endpoint key that no
   record it holds binds and no `--peer` entry names, and prints `peer
   refused: endpoint <id>: <reason>` on stderr. Before, any key that proved a
   node key at HELLO linked.
2. HELLO, both ways, refuses a node that is not bound to its connection's key,
   save a configured key on first contact.
3. `--peer <endpoint-id>`, with no address, is accepted: it configures a key
   and dials nothing. A bad entry's message now names both forms.
4. A revocation that lands closes the revoking node's live link on that key.
5. One-sided `--peer` no longer links. The accepting node must name the
   dialer's key, or hold its binding.
6. `CarrierLink` has `remote_id`, which every implementer must provide. No
   production implementer exists; the contract's fixture and the node's two
   fakes provide it.

**What the owner's desk sees at its next restart:** nothing new. The
rehearsal below printed 4.2a's lines exactly, with the same node id and the
same endpoint id and an empty stderr, and records.json gained only the claim
every start mints. Nothing connects to the desk, since grazel passes no
`--peer`, so its door refuses nothing. A desk that has not restarted since
before 4.2a gets 4.2a's changes as well: `endpoint.key` and one binding.

### Questions for the owner (4.2b)

1. **The contracts' policy.** Add `remote_id` to `CarrierLink`'s methods in
   `glade/contracts/architecture-policy.json`, a file outside this step.
   Recommend yes: one line, in the lane owner's commit, so the checker
   requires the method. Done so in 4.2b's commit, for the owner's review.
2. **A first-contact link that a record later contradicts** (named gaps).
   Recommend leaving it for the slice: it needs a record by another node
   naming a key that node cannot hold, and the next connection is refused
   anyway.
3. **Learning an endpoint id before a peer starts.** Today one first start
   prints it, and a node must be started once before its peer can name it.
   Recommend that 4.5's configuration add a way to print it without
   serving.

**Ruled, owner, 2026-09-25 ("all recommended"):** 1 `remote_id` in the
contracts' policy accepted; 2 a first-contact link that a later record
contradicts is left for the slice; 3 4.5's configuration prints a node's
endpoint id without serving (added to Step 4.5 in the root plan, so not
part of 4.2c).

### Measured (4.2b)

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components, in 42 s warm
  and 89 s from an empty target. There are 256 node tests on each path,
  across 15 test binaries, where there were 246. The ten new ones are
  section 10's rows, less the two changed tests and the contract's own two.
- **rustfmt**: glade-node 318 hunks, at its baseline; no touched file gained
  a deviation. glade-wire 43.
- **clippy**: glade-node 11 warnings and glade-wire 7, at baseline.
- **The contracts**: `glade/contracts/check.sh` passes, with 89 tests where
  there were 87 (CA-005 and its wrong fixture), and the checker passes.
- **Time**: the door's library tests, eleven with the transport module's,
  take 0.04-0.07 s together. The process test starts eight nodes; the
  `assembled_path` binary's seven tests take about 1 s.
- **The rehearsal.** HEAD's binary (`40647d2`, 4.2a, built to a scratch path,
  deleted after) ran twice on a scratch instance with the desk's two app
  files, as grazel starts it. This build then ran twice on that instance:

  ```text
  node 09ec287a…
  registry ready (home served: true)
  app grazel registered (+0 record(s), 11 unchanged)
  app gyld registered (+0 record(s), 11 unchanged)
  peer 452aa848… 127.0.0.1:56410
  workspace ws-razel serving
  workspace ws-razel serving
  listening 55492
  ```

  These are 4.2a's lines, with the same node and endpoint ids and nothing on
  stderr.
- **Downstream**, against the rebuilt default binary,
  `glade/node/target/debug/glade-node` (inode 399665534; the verified 4.2a
  build was inode 399433812), each Rust suite with a scratch target deleted
  after it:
  - client-rs: 9 + 3;
  - client-ts: 19;
  - grip-share: 19;
  - grazel, at `5fc2598`, clean before and after: 29 + 3, its new baseline,
    with no skip, on that binary;
  - glade-gwz: 9 + 6;
  - glade-gyld: 233 (1 ignored) + 31.
- **The async witness** type-checks against this tree, on a scratch copy.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +403/−55, net +348;
  - the node +387/−53, net +334: `transport.rs` +172/−15, `iroh_carrier.rs`
    +113/−11, `peer.rs` +32/−9, `mesh.rs` +26, `lifecycle.rs` +26/−11,
    `bin/glade-node.rs` +18/−7;
  - the carrier contract +16/−2: `TransportId`, `remote_id` and the trait's
    note;
- tests: +625/−69, net +556;
  - the node +556/−64;
  - the contract's CA-005 probe +37/−1, and its fixture +32/−4.

### 11. The iroh `CarrierPort` adapter (plan Step 4.2c)

Built on 2026-09-25, against glade `63a5799`, as the owner ruled it a step
of its own (question 5 of 4.2a). Plan Step 4.2 asks for it: "closing an
endpoint ends its links, so the iroh adapter tracks its links (the witness
measured that a surviving connection keeps the socket bound)". The tests and
their red runs are in section 12.

- **What it is.** `iroh_carrier::IrohCarrier` implements `CarrierPort`, and
  each link it makes implements `CarrierLink`. A port binds one iroh
  endpoint, with the endpoint key it was lent, on its own ALPN,
  `glade/carrier/1`. The node's endpoint keeps `glade/node/2`, so a node's
  endpoint and an adapter never connect. The address is
  `<endpoint-id>@<ip:port>`, the syntax of a `--peer` entry. A link's
  `remote_id` is the 32-byte endpoint id its TLS session proved,
  `Connection::remote_id()`.
- **A link** is one QUIC connection with one bidirectional stream. The dialer
  opens the stream and writes four bytes, `gcl1`, because QUIC shows a
  stream to its peer only once bytes cross it: without them the acceptor's
  `accept` would wait for the first frame. An acceptor that reads another
  word answers `Transport("not a carrier link")`, which refuses that attempt
  only. One connection per link makes a link's close the connection's, and
  its `remote_id` the connection's.
- **Frames** are a `u32` little-endian length and the bytes.
  - `send` refuses a frame over the limit with `FrameTooLarge` and sends
    nothing; the link stays usable.
  - `recv` refuses a length over the limit before it reads the body, and ends
    the link.
  - What has arrived of the next frame is kept in the link, not in the
    receive, so a receive dropped part-way loses none of it. noq's `read` is
    cancel-safe, and the buffer never holds more than the limit and one read.
- **A torn frame.** A send marks its half torn until its write completes, so
  a send dropped part-way leaves it marked. The next send ends the link and
  answers `Closed` rather than follow a frame that may be torn, and a close
  does not finish such a stream.
- **A link's close** ends the link at its first poll: `send` answers
  `Closed`, `recv` `Ok(None)`, and its handles come out of it by value. It
  then finishes the stream and waits, three seconds at most, until the peer
  has acknowledged everything (`SendStream::stopped`), and closes the
  connection with code 0 and no reason. The peer reads what was sent and then
  the end of the stream, though the connection is closed by then: noq keeps
  received stream data readable after the peer's close (CA-001).
- **The port's close** takes the endpoint and its links out of the port at
  the first poll, and the port answers as closed from then on. It ends every
  live link as the link's own close would, under one three-second bound for
  all their drains, closes their connections, and then awaits iroh's
  `Endpoint::close`. A pending `accept` answers `Ok(None)` once the endpoint
  closes, and so does one whose handshake the close cut short.
- **The tracking.** The port holds its links weakly, so a link dropped by its
  owner is not kept by the port. A link holds the endpoint as well as its
  connection, so a port dropped without `close` does not take the link's
  transport with it. iroh aborts an endpoint whose last handle drops
  unclosed: a link's receive then failed with `connection lost`, and in one
  run waited for ever. Both handles come out when the link ends, so once the
  port's close has run, no link keeps the port bound.
- **Binding.** The adapter binds loopback, on a port the OS picks, as the
  node's endpoint binds. `CarrierConfig::local` is 4.5's bind address, and is
  not read. So CA-004's re-bind, "close frees the address by value", is
  vacuous on iroh until 4.5: `fresh` binds elsewhere. The adapter's own test
  shows the port freed instead.
- **CA-005, changed.** The probe reached `fresh` by dialling `b`'s address.
  An iroh address names its endpoint's key, so that dial reached nobody, and
  the probe waited until its bound ran out. `fresh` is now asked to bind where
  `b` was, and dialled where its bind answers. On the two fake networks that
  is `b`'s address again, as before.
- **Where it is wired.** `NodeAssembly` binds `IrohCarrier` for the peer role
  (`peer_carrier_binding`), in place of `PendingIrohAdapter`, with the
  endpoint key as its component parameters (`Option<EndpointKey>`). Neither
  root lends one, so it refuses to bind: `Transport("the iroh adapter was
  lent no endpoint key")`, where the pending adapter answered `the iroh
  CarrierPort adapter is not built yet (plan Phase 4)`. `dial` answers
  `Closed` and `accept` `Ok(None)`, as before. The record transport's view
  rides it and is never called. The mesh stays on `PeerEndpoint`.
- **No door.** The adapter's endpoint has no accept hook. A door would be lent
  with a key, when the mesh moves onto the port.
- **What moving the mesh would take** (question 1): HELLO's exporter bytes
  (D6), which the port does not expose; the door's hook on the adapter's
  endpoint; the sync driver's reads and writes moved from QUIC streams to a
  link's frames; and 4.5's bind address and relay mode. Until then no root
  lends the adapter a key, so no second endpoint shares the node's endpoint
  key.

### 12. Tests (4.2c), each begun red

Each was run against the code without the part it guards, switched off by an
edit and restored after; the message is what that run printed. CA-001..005
run on real iroh over loopback in `iroh_carrier`'s tests, each bounded at
20 s, since `tests/assembly` opens no socket.

| Test | Proves | Red first |
| --- | --- | --- |
| `iroh_carrier`: `ca_001_iroh_carries_frames_whole_once_and_in_order` | CA-001 on real iroh: frames cross whole, once and in order, both ways; a link's close delivers what was sent, then the end of the stream | with a link's close that does not drain: "CA-001 close delivers what was sent", left `Ok(None)`, right `Ok(Some([108, 97, 115, 116]))` |
| `iroh_carrier`: `ca_002_iroh_holds_the_frame_limit_both_ways` | CA-002 | with no limit on an arriving frame: "CA-002 an oversized arrival", left `Ok(Some([49, 50, 51, 52, 53, 54]))`, right `Err(FrameTooLarge)` |
| `iroh_carrier`: `ca_003_irohs_futures_are_lazy_and_recv_is_cancel_safe` | CA-003 | with a half given up whenever an operation lets it go: "CA-003 a dropped recv consumes nothing", left `Ok(None)`, right `Ok(Some([107, 101, 112, 116]))` |
| `iroh_carrier`: `ca_004_an_iroh_port_gives_its_endpoint_up_by_value` | CA-004, its re-bind vacuous (section 11) | with a port close that ends no link: "CA-004 close ends the endpoint's links", left `Err(Transport("connection lost"))`, right `Err(Closed)` |
| `iroh_carrier`: `ca_005_iroh_names_each_far_end_by_its_endpoint_id` | CA-005, as changed | with the probe as it was: "the probe finished within 20 s: Elapsed(())"; with a link that names no endpoint: "CA-005 three endpoints are named apart" |
| `iroh_carrier`: `a_closed_carrier_frees_its_port_though_its_links_survive` | a port's close frees its UDP port though both handles of its link survive; `remote_id` is the id of the key the far port was lent; the closed port's link answers `Closed`, and the far end's stream ends | with a port close that ends no link: "a link the port made keeps it bound", after the two-second wait |
| `iroh_carrier`: `a_frame_read_in_two_parts_arrives_whole` | from a raw dialer: another first word is refused with `Transport("not a carrier link")`, and the port accepts the next; a receive dropped with half a frame keeps it, and that frame and the next arrive whole | with no check of the first word: "another word is refused", left `None`; with a receive that keeps no part: "the dropped receive kept its part", left `Err(FrameTooLarge)`, right `Ok(Some([97, 98, 99, 100, 101, 102]))` |
| `iroh_carrier`: `a_torn_frame_is_never_followed_by_another` | a 4 MiB send past the peer's window, dropped part-way; the next send answers `Closed`, and nothing arrives after the torn frame | with the torn mark ignored: "the link ended instead", left `Err(Elapsed(()))`: the next send waited behind the torn frame |
| `iroh_carrier`: `a_link_outlives_a_port_dropped_without_close` | with both ports dropped unclosed, a frame still crosses their link | with a link that holds no endpoint: "the transport went with its port", left `Ok(Err(Transport("connection lost")))` |
| `tests/assembly_registration`: `an_assembly_with_nothing_overridden_builds_its_real_providers_and_they_refuse`, changed | the assembled path builds `IrohCarrier` for the peer role, which the test's construction observer records (a process-wide counter counted it then), and lent no key it refuses to bind | on HEAD's `assembly.rs`: left `Err(Transport("the iroh CarrierPort adapter is not built yet (plan Phase 4)"))`, right `Err(Transport("the iroh adapter was lent no endpoint key"))` |
| the module's `compile_fail` doctests, three changed | a cycle, two peers and a carrier asked for by port type still fail to compile, with `IrohCarrier` where `PendingIrohAdapter` was | none: they guard the module. Each body, built as an example on a scratch copy, failed with its recorded code and no other: E0277 (the missing binding, unchanged), E0275, E0119, E0277 |
| contracts: `ca_005_each_link_names_the_far_ends_transport_identity` and `rejects_a_link_that_names_its_own_end`; node: `ca_005_the_fake_network_names_each_far_end` | the changed probe on both fakes, whose `bind` answers the address it was asked for | none: they pass unchanged |

What they do not prove: Windows and Linux, which the lane owner runs on
dabeest and the Pi; a relay, and two machines (4.5); the re-bind at the
address a closed port held (4.5).

### Named gaps (4.2c)

- A close while a send on the same link is under way does not drain: that
  send ends, and frames the peer has not yet acknowledged may be lost.
- A `dial` or `accept` still pending when the port closes holds an endpoint
  handle until it is next polled or dropped, as iroh frees the socket only
  once every handle is gone. The close wakes it, and it answers `Closed` or
  `Ok(None)`.
- The acceptor waits for a link's first four bytes without a bound: a dialer
  that connects and sends nothing holds that `accept`.
- CA-004's re-bind is vacuous on iroh until 4.5's bind address.
- Nothing binds the adapter on either root, so its behaviour under load, on a
  relay or across machines is not measured.

### Default-path changes (4.2c)

1. `NodeAssembly`'s peer role is `IrohCarrier`, lent no key, where it was
   `PendingIrohAdapter`. Its refusal to bind reads `the iroh adapter was lent
   no endpoint key`. Nothing on either root binds, dials or accepts through
   it.
2. CA-005 dials the address `fresh`'s bind answers.
3. Nothing else. The node's endpoint, its ALPN, the mesh, HELLO, the door and
   every line a node prints are as they were. `bind_endpoint` takes the ALPN
   as an argument.

**What the owner's desk sees at its next restart:** nothing new. The desk's
node starts from the hand-written root, which builds no `NodeAssembly`; the
assembled root builds the adapter and never binds it. The rehearsal below
printed HEAD's lines exactly on both roots, with the same node and endpoint
ids, and records.json gained only the claim every start mints.

### Questions for the owner (4.2c)

1. **When the mesh moves onto the port.** Recommend a step of its own after
   4.5, not 4.2c. It needs HELLO's exporter bytes (D6) through the port, or a
   session-level replacement for them; the door's hook on the adapter's
   endpoint; the sync driver on a link's frames; and 4.5's bind address and
   relay mode. Until then the roots lend the adapter no key. The other choice
   is to fold the move into 4.5, so that its crossing runs on the port; that
   makes 4.5 larger by all of the above.
2. **The adapter's own ALPN, `glade/carrier/1`, and its first word, `gcl1`.**
   Recommend keeping both: a node's endpoint and an adapter cannot connect by
   mistake, and a later framing can take a new word.
3. **The unbounded wait for the first word** (named gaps). Recommend a bound
   when the adapter first faces other machines, with the door in front of it:
   the mesh's move, or 4.5.

**Ruled, owner, 2026-09-25 ("all recommended"):** 1 the mesh moves onto the port
in a step of its own after 4.5, the plan's Step 4.5b, placed before 4.6; 2 the
adapter keeps `glade/carrier/1` and `gcl1`; 3 the first word's wait gets a bound
in 4.5b, where the adapter first faces other machines.

### Measured (4.2c)

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components, in 94-97 s
  from an empty target. There are 265 node tests on each path, across 15
  test binaries, where there were 256: the nine new `iroh_carrier` rows of
  section 12.
- **rustfmt**: glade-node 317 hunks, one fewer than before: the body of
  `PeerEndpoint::addr`, which rustfmt would lay out otherwise, moved into
  `loopback_addr` in rustfmt's layout. The gate's table now records 317, as
  the gate asks when a count falls. glade-wire 43.
- **clippy**: glade-node 11 warnings and glade-wire 7, at baseline.
- **The contracts**: `glade/contracts/check.sh` passes, with 89 tests, as
  before.
- **Time**: the fifteen `iroh_carrier` tests, the nine new ones among them,
  finish in 0.29 s together.
- **The rehearsal.** HEAD's binary (`63a5799`, built from a copy of the tree
  with this change reversed, deleted after) ran twice on a scratch instance
  with the desk's two app files, as grazel starts it. This build then ran
  twice on that instance:

  ```text
  node df5b4298…
  registry ready (home served: true)
  app grazel registered (+0 record(s), 11 unchanged)
  app gyld registered (+0 record(s), 12 unchanged)
  peer 6507b0a7… 127.0.0.1:51960
  workspace ws-razel serving
  workspace ws-razel serving
  listening 62022
  ```

  These are HEAD's lines, with the same node and endpoint ids and nothing on
  stderr; the instance holds one binding. On a second instance, started with
  `GLADE_NODE_ASSEMBLED=1`, the same held, with the assembled root's one
  stderr line, as before.
- **Downstream**, against the rebuilt default binary,
  `glade/node/target/debug/glade-node` (inode 399895034; 4.2b's was
  399732389), each Rust suite with a scratch target deleted after it:
  - client-rs: 9 + 3;
  - client-ts: 19;
  - grip-share: 19;
  - grazel, at `c9f9c7f`, clean before and after: 29 + 3, on that binary;
  - glade-gwz: 9 + 6;
  - glade-gyld: 233 (1 ignored) + 31.
- **The async witness** type-checks against this tree, on a scratch copy.
  Its `Cargo.lock` lacks glade-node's `ed25519-dalek` and `getrandom` edges,
  added at 4.1a, so `--locked` refuses it; the check ran `--offline` alone.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: the node +442/−35, net +407: `iroh_carrier.rs` +419/−15,
  `assembly.rs` +23/−20;
- tests: +214/−22, net +192;
  - the node +202/−11: `iroh_carrier.rs` +190, the registration test +9/−9,
    `tests/assembly/conformance.rs`'s header +3/−2;
  - the contract's CA-005 probe +12/−11.

## Home records signed (plan Step 4.1b)

Design addition, 2026-09-25, written before the code against glade `4586e1b`.
The red runs and the measured figures are filled in afterwards. The spec is
plan Step 4.1b, split from 4.1 by the owner's ruling of 2026-09-24, and
`glade/dev-docs/GladeNodeSigning.md`, every decision ruled as recommended:
chiefly D4 (the envelope), D5 (the node signs every `home` record), D7 (the
`origin-op` tag), D8 (existing stores) and D9 (*deferred*), with D11's 4.1b
row and the findings F2, F3 and F4. The earlier steps it builds on: 4.1a (the
key, the id, `NodeSigner`), 4.2a (the transport records), 4.3 part 1 (client
writes to `home` refused) and 4.4 (durable acceptance).

Nothing changes in the wire IR. The node's own IR, `node/ir/sysdata.taut.py`,
gains one message. The ALPN moves to `glade/node/3` (section 7).

**The step is split in two.**

- **Part 1, built with this note:** the envelope, sealing at append, the check
  at every `home` ingest, D8's set-aside and re-mint, the folds and the ALPN
  (sections 1 to 7).
- **Part 2, D9's *deferred* path:** designed in section 8 and built after
  part 1 landed (glade `02e2a9e`), on the lane owner's word of 2026-09-25:
  option (a), as ruled. It defers every record of a node this node has not
  met; the door's introductions, ruled a day later for 4.2b, assumed such
  records arrive, and question 1 puts (b) and (c) to the owner. With it, a
  hardening the lane owner asked for (section 10).

### 1. The envelope

- One new message in the node's IR: `SignedRecord{1 record: bytes, 2 sig:
  bytes}`, regenerated with `--legacy-codec` (`GladeProgramStatus.md:29`).
- Every `home` op's payload is the canonical CBOR of one. `record` is the
  canonical CBOR of the record the op's stream holds, which before this step
  was the payload itself.
- `sig` is the origin's Ed25519 signature, 64 bytes, pure, over
  `glade/v1/origin-op\0` then the canonical CBOR of the op's fields 1 to 10
  with `record` as the payload (D7). So it covers the chain position
  (`origin`, `seq`, `prev`), the stream and the rest of the op.
- It is checked with `verify_strict`, the key being the origin. The id is the
  key (D2), so the check needs no lookup.
- The op hash covers the envelope, so a chain commits to its signatures, and
  `prev` names the previous op, envelope and all.
- Field 1 is bytes, where every record kind's field 1 is text. So an unsigned
  record is told from an envelope before anything decodes it (D4). The wire
  codec's decoders panic on a type they do not expect, so an envelope is read,
  and the record inside it checked against its stream's kind, with a checked
  decoder (`envelope::parse`, which the transport fold now shares).
- **Which records:** every op on `home`, from every origin, on each of the
  directory's eleven streams. That is the nine kinds D5 names and 4.2a's two
  transport kinds. No other share: app ops stay unsigned (D5, a 5.1 gap).
- Sealing and checking use the node's own functions (`signing::sign` and
  `signing::verify`, through `NodeIdentity`), as HELLO and the transport
  records do. `SignerPort` gets its consumer in part 2.

### 2. Sealing: the node signs its own records (D5)

- A booted node's `Registry` holds the node's identity
  (`Registry::from_snapshot_as`). Each append is sealed as it is built: the
  record is encoded, the op is built with it as the payload, and the payload
  is replaced by the envelope before the op is hashed into its chain.
- A sealed registry appends only under its own node id. Any other origin is
  refused (`RegistryError::NotOurs`).
- Every record the node writes goes through it: presence, the home claim, the
  transport binding and its revocations at boot; each app file's registration,
  on both roots; claims, renewals and principals as the node runs.
- An unsealed registry (`Registry::new`, `Registry::from_snapshot`) appends
  bare records, as before. Only tests and the journeys' in-memory record host
  use one.
- Idempotent minting diffs records, not envelopes: `Registry::contains`,
  `appdecl::register` and the binding fold's `declared_by` compare the record
  inside. An envelope differs at every append, since its signature covers
  `seq` and `prev`.

### 3. What a `home` record needs before it is taken

One function, `envelope::verify`, checks a `home` op wherever one is taken in.
Its rules, in order:

1. **An envelope:** canonical, with a 64-byte signature. Anything else is
   *unsigned*.
2. **The directory's form:** no zone key, shape `log`, no refs, as the
   registry writes every record.
3. **Its predecessor** (B5, F3): seq 0 names none, and every later seq names
   one.
4. **A directory stream:** one of the eleven the directory profile hosts
   (`assembly::DirectoryRules`).
5. **Its stream's kind:** the record is exactly that kind's canonical CBOR,
   fields 1 to n of the declared types. So no fold meets a record it cannot
   decode (F2's panic), whoever signed it.
6. **A node id as origin:** 64 lower-case hex digits.
7. **The signature**, strictly valid under the origin's key.

The chain rules hold as before: contiguous seqs, `prev` equal to the
predecessor's hash, a fork refused with its proof. The served store also
requires a `home` chain to begin at seq 0, as the registry does: an op whose
predecessor it has not seen cannot have that predecessor checked (B5).

Where the check runs (D11's list):

| Where | Before this step | After |
| --- | --- | --- |
| records.json at boot (`sysdir::boot_at`) | chain checks | an unsigned record is set aside (D8, section 4). Every other record must verify, or it is quarantined with its chain's suffix, as a chain break is. A quarantined grant or revocation leaves the grant fold unreadable (AZ-11) |
| the registry's `ingest` (the record host's; the journeys) | chain checks | a sealed registry verifies each op first |
| the served store's `append` | chain checks, `prev` optional | a `home` op must verify before anything new of it is kept. A byte-identical repeat is taken as held without a check: it was checked when it landed |
| the served store's `open` | journals replayed unchecked | each `home` journal is checked as if its ops were appended one by one. One that does not verify is set aside whole (section 4) |
| the pull at connect, a peer's push, the seeding at adoption, the node's own publish | `Store::append` | the same, so each is checked |

A refused op is not stored, fanned out or fed to the door. On the peer paths it
is dropped, as a chain break is today. Part 2 counts it and reports it.

### 4. Existing stores (D8)

Every store written before this step holds unsigned `home` records. The owner's
desk holds nothing else: 4.1a re-minted everything under the new id, unsigned.
At the first start on this build, the node sets them aside and mints again.

**records.json.** At boot, every record whose payload is not an envelope is
written, byte for byte, to a new `records.legacy-<date>.json`, by 4.1a's
mechanism:

- the file is a `SystemSnapshot` of those records, with no heads, dated UTC;
- it is created new and synced before records.json is saved without them;
- it never replaces a file: `-2`, `-3` and so on are added to a name in use;
- a crash between the two writes repeats the set-aside at the next boot, into
  a second file, so nothing is lost.

The records are never folded (B5: kept as history, never governing). The boot
prints `set aside N unsigned record(s) in records.legacy-<date>.json` after
`node`, where 4.1a printed its own line.

This takes in 4.1a's pass. Records under the key's old id are unsigned too, so
they go the same way, and the old id's special case is removed: the boot's
`legacy_id`, and the served store's `set_aside` of one origin at adoption.

**The served store.** At open, each `home` journal,
`cache/store/<hex(home)>/<hex(origin)>.log`, is checked as section 3 says: this
node's own, and any copy of a peer's.

- A journal holding any op that does not verify is renamed, whole, to
  `<name>.log.legacy-<date>`, which `open` never replays and which never
  replaces a file.
- The node then prints `set aside K journal(s) of the served store's home
  share (N record(s)) that do not verify, renamed *.legacy-<date>`, after the
  `app` lines.
- App data is untouched: only the `home` share's directory is checked.

**Then the node mints again**, as at 4.1a's first start:

- its presence and the home claim;
- the binding of the endpoint key it already has. The old binding is set
  aside, so nothing is revoked;
- every app's registration, `+N record(s)`;
- at adoption, the seed of the served store with all of these, signed;
- each served workspace's claim, at epoch 1, since the old claims are aside;
- principals, as sessions say Hello again.

`node.key` and `endpoint.key` stay, so the node id and the endpoint id do not
change. A second start sets nothing aside and registers `+0`.

### 5. The transport records (4.2a)

- A binding or a revocation rides an envelope like every `home` record: its
  op is sealed by its node, in that node's chain.
- It keeps its own signature, in its own domain, so a binding can still be
  checked apart from its chain (4.2a's section 2).
- The transport fold reads the record inside the envelope and judges it as
  before: canonical, its ids in hex, its own signature, in its node's own
  chain.
- The fold reads verified op-sets, the registry's and the served store's, so
  the envelope's check is already made. The door is fed only what lands in
  the served store.

### 6. What the folds read afterwards

- Every fold reads the record inside the envelope, through one helper,
  `envelope::record`. A bare payload, which only an unsealed registry holds,
  reads as itself.
- **The grant fold** (`Registry::policy`, plan Step 4.3) reads the registry:
  this node's own grants and revocations, from its app files' `seed` and
  `revoke` lines, now signed. Nothing changes in what it answers, and no
  peer's record reaches it.
- **The served store's folds** are routing (`who_serves`, `directory_knows`),
  exchange declarations (`declared_exchange` and the binding fold), principals
  (`knows_principal`), epoch fencing (`max_claim_epoch`) and the door. The
  store now holds only records that verified: each signed by its origin, in
  that origin's chain, and of its stream's kind.
- So F2 is closed for peers. No peer can forge another node's claim, and no
  record in the store panics a fold. The named gap "a peer can write `home`
  until 4.1b" closes as far as forgery goes. Which nodes may write is part 2.

### 7. Compatibility

- **The wire:** no frame or field changes. The ALPN moves to `glade/node/3`,
  and `peer::PROTOCOL` to 3. D4 requires that old and new nodes do not sync: a
  4.1a to 4.3 node decodes a `home` payload as a record and panics on an
  envelope, and this build refuses its unsigned records. So, as at 4.1a (D6),
  they fail at connect, not mid-sync.
- **Clients:** none reads or writes `home` (D4's search; 4.3 part 1 refuses
  client writes). A session that subscribes to a `home` stream, as the node's
  own tests do, receives envelopes.
- **A downgrade:** an older binary cannot start on an instance this build has
  written. At boot it decodes a record (`has_node`), meets an envelope and
  panics, before it writes anything. Going back needs records.json and the
  served store's `home` journals moved aside.
- **The owner's desk** at its next restart: section 4's two lines, `+N` for
  each app, `ws-razel` at epoch 1. "Measured" has the replay.

### 8. D9: *deferred* (part 2)

D9 as ruled: a record whose verifier is unavailable is never persisted or
folded; it is retried next round and reported as *deferred*. Under D2(A) the
verifier is unavailable when "the origin is not a node this node knows", and a
node knows "itself and the nodes it has authenticated at HELLO".

- **Who is known:** the node, and each node whose HELLO it has verified since
  it started. The node's `SignerPort` adapter keeps the set. The mesh builds a
  `NodeSigner` from its endpoint's identity, and `run_link` records each peer
  its HELLO proved (`NodeSigner::authenticated`, which 4.1a built for this).
  Its `verify` answers `Unavailable` for any other node (SI-002).
- **Where:** the two peer paths that carry `home` records, the pull at connect
  and a peer's push.
  - Not at boot or at open. What the node holds was admitted when it arrived,
    and section 4 checks it again at open.
  - Admitting it again would drop every peer's records at each restart until
    that peer links again. Epoch fencing would then bump over nothing
    (`max_claim_epoch`), and a share whose holder is offline would route
    `Local` instead of `Absent`.
- **What:** before the store sees a `home` op from a peer, its origin must be
  known.
  - If it is not, the op is not stored, fanned out or fed to the door. Nor is
    any later op of its chain (stream, origin) on that stream, since each
    chains on it. The chain counts as *deferred*.
  - A known origin's op that the store refuses counts as *refused*, and its
    chain's later ops are skipped the same way.
- **The round** is one stream: a pull, or one push. The next pull, at the next
  connect, asks again, since the heads the node announces lack what it
  deferred.
- **The report**, at each stream's end, one line per chain, on stderr through
  the door's reporter, so the assembled root puts it on its console:
  - `deferred N home record(s) of node <origin> on <stream> from peer <peer>:
    not a node this node knows`;
  - `refused N home record(s) of node <origin> on <stream> from peer <peer>:
    <reason>`.
- **Boot** defers nothing, so prints no `deferred` line: records.json holds
  the node's own records, and an unreadable key refuses the start (D9).
- `SyncOutcome` gains `deferred`, beside `applied` and `rejected`, and the
  library's sync driver, `pull_sync`, takes the same rule. A mesh round
  returns one too.
- **Size:** estimated at about 100 production lines and 200 of tests.
- **What it does to introductions** (question 1). A configured peer's
  directory can no longer carry a third node's records here, its binding
  included. The door's rule that such bindings count (4.2b's ruling) keeps its
  code, and never fires for a node this node has not met.

### 9. Tests (part 1), each begun red

Each was run against the code with the part it guards switched off, by one
edit in a scratch copy of the tree; the message is what that run printed.

| Test | Proves | Red first |
| --- | --- | --- |
| `envelope`: `a_sealed_record_verifies_and_each_flaw_is_refused` | a sealed op verifies, and each of section 3's rules, broken once, refuses for its own reason: a bare record and a non-canonical envelope (`Unsigned`), a zone key (`Form`), seq 1 with no `prev` (`Prev`), another stream (`Stream`), a claim on `dir.nodes` (`Kind`), an upper-case origin (`Origin`), a flipped signature byte, another key's signature and another seq (`Signature`) | with `verify` answering `Ok`: "a bare record", left `Ok(())`, right `Err(Unsigned)` |
| `envelope`: `the_signature_is_pure_ed25519_over_the_tag_then_the_op_with_its_record` | D7's encoding, built by hand: pure Ed25519 by the origin's key over `glade/v1/origin-op\0` then the op's ten fields with the record as payload | with `seal` signing the record alone: `assertion failed: key.verify_strict(&signed, &sig).is_ok()` |
| `envelope`: `each_directory_kind_passes_its_own_check_and_a_misshapen_one_fails` | each of the eleven streams takes its kind as the generated codec encodes it, and not with a byte left over; a claim is no node record; verbs are text | with the kind check accepting anything: "dir.nodes: a byte left over" |
| `envelope`: `the_checked_decoder_reads_the_codecs_bytes_and_refuses_the_rest` | the checked decoder reads what the codec writes and answers `None`, never a panic, for a torn item, a byte left over, a float, a text map key, nesting past two and a count past the bytes | with bytes left over accepted: left `Some(Map(…))`, right `None` |
| `registry`: `a_sealed_registry_signs_its_appends_and_takes_only_what_verifies` | a sealed registry's append verifies and diffs as the record; an append under another origin is `NotOurs`; a bare and a forged op are refused `Unverified`, where an unsealed registry takes a bare one; a sealed reload quarantines nothing | with the seal skipped at append: "the appended op verifies", left `Err(Unsigned)`, right `Ok(())` |
| `store`: `a_home_op_lands_only_signed_and_from_seq_0` | the served store refuses a bare and a forged `home` op and keeps nothing; a signed one lands and its repeat is a duplicate; a chain begun above seq 0 is a gap; an app op lands bare (D5) | with no check at append: "expected Unsigned, got Ok(Appended)" |
| `store`: `open_sets_aside_a_home_journal_that_does_not_verify` | `open` renames an unsigned `home` journal to `<name>.legacy-<date>` and replays a signed one and an app journal; it reports what it set aside; a second `open` sets nothing aside | with no check at open: "the unsigned journal set aside" (none reported) |
| `sysdir`: `a_first_boot_sets_the_unsigned_records_aside_once_and_mints_them_signed` | an unsigned instance (presence, home claim, binding and a grant under the node's id, and a record under the pre-4.1a id) is set aside byte for byte beside an older file it does not overwrite; the node mints presence and the same key's binding again; every record verifies and names the node; a second boot writes nothing | with the set-aside stubbed out: "the unsigned records set aside" |
| `sysdir`: `a_record_whose_signature_fails_at_load_is_quarantined_and_closes_the_grant_fold` | a signed revocation with a byte of its record changed is quarantined, not set aside as unsigned, and leaves the grant fold unreadable | with the sealed load not verifying: `rejected`, left `0`, right `1` |
| `claims`: `adoption_after_the_unsigned_home_journal_is_set_aside_serves_signed` (replaces 4.1a's adoption test) | an unsigned instance's served store: its journal set aside at open, so adoption seeds signed records alone: the dropped binding routes nothing, `alice` is minted again, `ws-x` is claimed at epoch 1, every `home` record verifies, app data stays, the old journal is kept | with no check at open: "the unsigned journal set aside"; and with that line not asked: "the dropped binding routes no exchange" |
| `mesh`: `a_claim_a_peer_did_not_sign_is_refused_where_it_is_pushed` | over real iroh: a peer's push of a higher-epoch claim it did not sign, bare or sealed by another key, is refused, so B still routes its share to itself; the same claim sealed by the peer then lands in that slot | with no check at append: left `Some(<A's id>)`, right `Some(<B's id>)` |
| `tests/assembled_path`: `both_roots_set_an_unsigned_instance_aside_and_serve_signed` (replaces 4.1a's transition test) | on each root, as processes: the same id, both `set aside` lines, `+2 record(s)`, every record in records.json and the served store verifies, `ws-x` at epoch 1; a second start sets nothing aside and registers `+0` | with both set-asides off: the kinds `["instance", "node", "quarantined", …]` where `["instance", "node", "set", …]` were expected: every unsigned record quarantined, none set aside |
| `iroh_carrier`: `a_protocol_2_node_fails_at_connect` (was protocol 1) | an endpoint offering only `glade/node/2`, as every build from 4.1a to 4.3 does, fails at connect either way | with the ALPN back at 2: "a protocol-2 dialer connected" |

Changed to carry signed records, and passing: `sysdir`'s
`boot_takes_a_record_held_twice_as_one_and_keeps_the_rest_of_its_chain` (a
peer's signed chain), `peer`'s
`serve_sync_leaves_out_a_zone_the_claimed_holder_may_not_read` (a signed
`home` record), `mesh`'s door tests (sealed transport records), and the
exchange tests' registries (sealed). Every other test runs unchanged, the
two-node ones now over signed records end to end.

What they do not prove: Windows and Linux, which the lane owner runs on
dabeest and the Pi; that another language's Ed25519 agrees with the tag (no
`proof_family` vectors yet); a crash between the legacy file and records.json
(4.1a's mechanism, unchanged); a relay; three nodes.

### Named gaps (4.1b part 1)

- **D9 is not built** (part 2). Until it is, a record from any node whose
  signature verifies is taken from a linked peer, a third node's included.
- **F4, measured.** Every start checks every `home` record twice, records.json
  and the journal: about 0.6 ms a record in a debug build, which the desk
  runs. See question 3.
- **A push that reaches a peer before the pull at connect.** A `home` chain now
  starts at seq 0, so such a push is refused as a gap, and so is each later
  push on that chain, until the next pull. Before, it was taken, and the
  pull's older ops were then taken as seen and not held. The ruled pull-on-gap
  step heals it.
- **Completeness.** A signature proves a record, not that none is missing: a
  trailing record deleted from records.json or a journal goes unseen, as
  before. An envelope stripped from a record in records.json reads as unsigned
  history, not as tampering.
- **A downgrade** cannot start (section 7).
- App ops stay unsigned (D5; a 5.1 gap). No `proof_family` vectors for the
  tag. `NodeSigner` still has no consumer. The legacy files are never read or
  pruned. Equivocation proofs of `home` ops recorded before the step are
  unsigned evidence, kept and never folded.
- Each `home` record is about 70 bytes larger (the envelope and its
  signature): a renewal's op grows from about 224 bytes to about 294.

### Default-path changes (4.1b part 1)

1. The node signs every `home` record it writes, and every `home` op's
   payload is a `SignedRecord`.
2. A `home` op is taken only if it verifies: at boot's load of records.json,
   at the served store's append (the pull, a push, the seed, the node's own
   publish) and at its open. A `home` chain starts at seq 0, on one of the
   directory's eleven streams, each record of its stream's kind.
3. The first start on this build sets aside records.json's unsigned records
   and prints `set aside N unsigned record(s) in records.legacy-<date>.json`
   after `node`; renames every `home` journal that does not verify to
   `*.legacy-<date>` and prints `set aside K journal(s) of the served store's
   home share (N record(s)) that do not verify, renamed *.legacy-<date>` after
   the `app` lines; and mints again: presence, the home claim, the endpoint
   key's binding, the registrations (`+N`), claims at epoch 1, principals as
   sessions say Hello.
4. The ALPN is `glade/node/3`, and `peer::PROTOCOL` 3.
5. The legacy form prints the served store's line too, should its store hold
   a `home` journal that does not verify.
6. An older binary panics at start on an instance this build has written.
7. Each start costs about 0.6 ms more for each `home` record it holds, in a
   debug build (question 3).

**What the owner's desk sees at its next restart**, from the replay below: the
same node id and endpoint id; the two `set aside` lines; `app grazel
registered (+12 record(s), 0 unchanged)` and `app gyld registered (+10
record(s), 2 unchanged)`; `ws-razel` served at epoch 1; each tab's principal
minted again at its next Hello. App data is untouched, and no shipped client
reads `home`, so the UI works as before. The next restart prints the old
lines.

### Questions for the owner (4.1b)

1. **Who counts as known** (part 2; section 8). As ruled, a node takes a peer's
   `home` records only from itself and the nodes whose HELLO it has verified
   since it started; any other node's are deferred at every pull. Then a
   configured peer's directory cannot carry a third node's records, bindings
   included, and the door's introductions (4.2b's ruling) never fire for a
   node not met. The options:
   - (a) as ruled: nodes met. Only nodes that passed this node's door write
     its directory. Nothing the slice runs needs a third node.
   - (b) node trust: any origin whose signature verifies, carried by a met
     peer. Introductions work; but any key holder whose records reach a
     trusted peer can write this node's directory, such as a claim at a
     higher epoch that routes a share away (F2, now needing only a free key).
     This is what part 1 does until part 2 lands.
   - (c) met or introduced: a node counts as known once a met peer carries its
     self-proving binding (4.2a's inner signature); its other records land at
     the next pull.

   Recommend (a) for the slice, and revisit with account-root certification
   (D3's gap), where a certificate, not a peer, would introduce a node.
   **Built as (a)**, as ruled, on the lane owner's word of 2026-09-25; (b)
   and (c) are with the owner.
2. **What a node holds is not admitted again** (part 2; section 8). At boot and
   at open, records are checked (section 4), not held to D9's known set: D9
   governs arrivals. Recommend keeping this. The other reading would drop
   every peer's records at each restart until the peer links again, and epoch
   fencing would bump over nothing. **Kept as built** (the lane owner,
   2026-09-25).
3. **The cost on the desk's debug build** (F4, measured). A check costs about
   0.29 ms in a debug build, and each start checks each `home` record twice,
   so a desk restart costs about 5 s more for each day of uptime since the
   upgrade, and about 36 s after a week. With `opt-level = 3` for
   `curve25519-dalek`, `ed25519-dalek` and `sha2` in dev builds
   (`[profile.dev.package.<crate>]` in `node/Cargo.toml`, six lines), a check
   measured 46 µs: about 6 s after a week. The options: (a) accept, as F4 was
   ruled; (b) the profile override, now; (c) AZ-12's checkpoints, F4's ruled
   remedy. Recommend (b) with part 1's landing, and (c) later. **The lane
   owner added (b) with part 1** (glade `02e2a9e`), for the owner's review.
4. **Built in part 1, for review; each recommended as built:**
   - the ALPN and `PROTOCOL` at 3, which D4's "old and new nodes must not
     sync" requires;
   - 4.1a's old-id set-aside taken into this one (those records are unsigned),
     with `Boot::legacy_id` and `Store::set_aside(origin)` removed;
   - the served store takes only the directory's eleven streams, each as its
     kind, and a `home` chain from seq 0;
   - at load, an unsigned record is history (set aside, B5) and a signed one
     that fails is quarantined (AZ-11 for grants and revocations);
   - the second pair of legacy files is kept, never read, and pruned by hand,
     as 4.1a's ruling has it.

### Measured (4.1b part 1)

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components. There are 291
  node tests on each path, across 15 test binaries, where there were 282: the
  nine new tests of section 9, whose other four rows replace or rename a test.
  The contracts gate passes, unchanged.
- **rustfmt**: glade-node 300 hunks, 7 below the old baseline of 307, which
  is lowered to 300 in `check.sh`. The 7 went from lines this step rewrote,
  the decode sites in `registry.rs`, `exchange.rs`, `mesh.rs` and `appdecl.rs`.
  `envelope.rs` is new and formatted whole; no line this step wrote is a
  deviation. glade-wire 43.
- **clippy**: glade-node 11 warnings, glade-wire 7, at baseline.
- **The regeneration.** The generator at taut `7a5f616` reproduces HEAD's
  `sysdata.rs` byte for byte (`cmp`). After the IR change, regenerated with
  `--legacy-codec` as 4.2a ran it, `sysdata.rs` is exactly what the generator
  wrote: +20 lines, the one struct and its codec.
- **Verification cost**, in a debug build: sealing 0.23 ms a record, a check
  0.28–0.29 ms; a boot over 5,000 signed renewals 1.51 s, and the served
  store's open over the same 1.47 s. With the three crates at `opt-level = 3`:
  34 µs, 46 µs, 0.27 s and 0.26 s.
- **The first start at the desk's scale.** On a stand-in holding 60,001
  unsigned records (about a week of renewals; records.json 13.4 MB), this
  build's first start reached `listening` in 0.41 s: the set-aside reads no
  signature, and the journal fails at its first op. records.json fell to 7.7
  KB beside a 13.4 MB legacy file. The next start took 0.055 s.
- **The replay** (the brief's D8 rehearsal). A stand-in instance in a scratch
  home, started from `glade-wz/grazel` as grazel starts the node (`--profile
  local --name grazel --app apps/grazel-app.glade --app apps/gyld-app.glade
  0`). Today's default binary (glade `4586e1b`, inode 400818912) ran twice: a
  tab's Hello (`tab-a`) and 22 s the first time, `tab-b` and 3 s the second.
  The instance then held 31 unsigned records in records.json and the same 31
  in its `home` journal: 15 bindings, 5 claims, 3 grants, the node, 2
  principals, the revocation, 2 services, the binding and the workspace. This
  build's first start printed:

  ```text
  instance $R/home/sys/grazel
  node ee048e608c9c949ebcdd1a0dc7aaf34a65657ba647c4b251b205dc02b3e60234
  set aside 31 unsigned record(s) in records.legacy-2026-09-24.json
  registry ready (home served: true)
  app grazel registered (+12 record(s), 0 unchanged)
  app gyld registered (+10 record(s), 2 unchanged)
  set aside 1 journal(s) of the served store's home share (31 record(s)) that do not verify, renamed *.legacy-2026-09-24
  peer c9d8ab137022e62175180efeff619aaa3edef19188878aee53f1e9160e391d25 127.0.0.1:63584
  workspace ws-razel serving
  workspace ws-razel serving
  listening 60070
  ```

  The node id and the endpoint id are today's; nothing was printed on
  stderr. records.json then held 28 signed records, and the `home` journal
  the same 28 beside `<journal>.log.legacy-2026-09-24` with the 31 old ones.
  `tab-a`'s Hello minted its principal again, signed. A second start printed
  today's lines (`+0 record(s), 12 unchanged` for each app) and set nothing
  aside. Today's binary, started on the upgraded instance, exited 101 at boot
  (`panicked at … wire-rs/src/cbor.rs:50:13: not text`) and wrote nothing but
  `instance.lock`; this build then started cleanly again. The date is UTC's,
  a day behind the desk's clock in the morning. The first of this build's
  starts took 2.1 s, the first run of a new binary file on macOS; on a fresh
  copy of the same state it took 62–64 ms.
- **Downstream**, against today's default binary (inode 400818912, not
  rebuilt), through the shims, each Rust suite with a scratch target deleted
  after it: client-rs 25 + 10, client-ts 48, grip-share 19, grazel 29 + 3,
  glade-gyld 233 (1 ignored) + 33, glade-gwz 9 + 7. All at baseline.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +603/−220, net +383;
  - `envelope.rs` +262, new (the checked decoder moved in from
    `transport.rs`, which is +11/−58);
  - `store.rs` +124/−24, `registry.rs` +83/−22, `sysdir.rs` +34/−48,
    `sysdata.rs` +20 (generated), `peer.rs` +13/−20, `assembly.rs` +9/−23,
    `bin/glade-node.rs` +9/−4, `claims.rs` +8/−11, `server.rs` +7,
    `appdecl.rs` +4/−1, `iroh_carrier.rs` +4/−4, `mesh.rs` +4/−3,
    `lifecycle.rs` +3, `session.rs` +3, `exchange.rs` +2/−1, `signing.rs`
    +2/−1, `lib.rs` +1;
- tests: +878/−173, net +705;
- beside them, the IR +13 (`sysdata.taut.py`) and `check.sh`'s baseline.

### 10. A format this build does not know (part 2's hardening)

Asked by the lane owner with part 2, after the replay showed today's older
binary panicking (`not text`) at boot on an upgraded store. That cannot be
fixed after the fact, but the next format change, such as 4.1c's recovery-key
record on a stream of its own, should not repeat it.

Part 1 already reads a `home` payload without panicking, but it guesses: a
payload that is not this build's envelope is set aside as unsigned, and a
signed record on a stream or of a kind this build does not know is quarantined
and dropped at the next save. On a downgrade either would rewrite a newer
store. So each `home` payload read from disk is classed first
(`envelope::format`):

| Class | What it is | At boot (records.json) | At the served store's open |
| --- | --- | --- | --- |
| sealed | this build's envelope, on a directory stream, its record of that stream's kind | checked (section 3) | checked with its journal |
| unsigned | a map whose field 1 is text: the shape of every record before this step, whose kinds were only ever added, never changed (the IR's history) | set aside (D8) | its journal set aside (D8) |
| unknown | anything else: another envelope, a stream or a kind this build does not know, a map whose field 1 is bytes, bytes that are not CBOR | the start is refused, before anything is written | the start is refused; the journal is not renamed |

The refusal names the file, the stream, the origin and the seq, and says what
to do: `records.json holds a home record this build cannot read (<stream> of
node <origin> at seq <n>): its format is newer than this build's, or it is
damaged; start the build that wrote it, or move records.json aside`, and the
same for a journal. It is printed as every refused start is, and the node
exits 1. A record that arrives from a peer is not classed this way: it is
checked (section 3) and, whatever its format, refused alone.

Not covered: the containers. records.json's `SystemSnapshot` and each
record's wire `Op` are still read by the wire codec's decoders, which panic on
a type they do not expect. The wire IR is frozen (plan §3), so a change there
comes with a wire amendment and a new protocol.

### 11. Tests (part 2), each begun red

Built on 2026-09-25 against glade `02e2a9e`. Each test was run against the code
with the part it guards switched off, by one edit in a copy of the sources;
the message is what that run printed.

| Test | Proves | Red first |
| --- | --- | --- |
| `mesh`: `a_third_nodes_records_are_deferred_until_its_hello_and_reported` | over real iroh, three nodes: A's pull from B takes B's own record and defers C's two, keeping none, with the line `deferred 2 home record(s) of node <C> on dir.principals from peer <B>: not a node this node knows`; once A has met C by a HELLO, B's push of C's records lands them; a record under B's id that B did not sign, in that push, is refused with `refused 1 home record(s) of node <B> on dir.principals from peer <B>: (<B>,1) does not verify: its signature does not verify` | with a round that takes every origin: "C's is kept nowhere", left `[Op { … }]`, right `[]`; with HELLO recording no one: "B's own record lands", left `[]` |
| `peer`: `pull_sync_defers_a_home_chain_whose_origin_is_not_known` | the library's sync driver defers a `home` chain whose origin the puller does not know, whole, and keeps none of it; the known node's chain and an app zone land | with the rule off: `deferred`, left `[]`, right `[("home", "dir.principals", [], "<origin>")]` |
| `envelope`: `format_tells_this_builds_envelope_from_an_older_record_and_from_what_it_cannot_read` | section 10's classes: this build's envelope is sealed; a record from before the step is unsigned, whatever its later fields; its envelope on an unknown stream or holding another shape, another envelope, a map whose field 1 is bytes, and bytes that are not CBOR are unknown | with unknown read as unsigned: "a stream this build does not know", left `Unsigned`, right `Unknown` |
| `sysdir`: `a_boot_refuses_a_record_in_a_format_it_does_not_know_and_writes_nothing` | a newer build's record in records.json refuses the boot (`InvalidData`) with the message naming it; records.json is as it was, and no legacy file appears | with the record kept, as part 1 kept it (then quarantined at load): `called Result::unwrap_err() on an Ok value` |
| `store`: `open_refuses_a_home_journal_in_a_format_it_does_not_know_and_leaves_it` | a `home` journal holding such a record refuses the open, naming the journal, and is not renamed | with no format check at open: "expected a refusal, got Ok(())" |
| `tests/assembled_path`: `both_roots_refuse_a_store_in_a_newer_format_with_a_clear_message` | on each root, as processes: exit 1, the message on stderr, no panic, records.json as it was | with neither refusal: "glade-node (HandWritten) still ran after 20s: the start was not refused" |

Changed, and passing: the `pull_sync` calls in `peer`'s and `iroh_carrier`'s
tests pass a puller that knows every node; `refused`, in
`tests/assembled_path`, keeps its checks over a new `ended`. Every two-node
test now runs with D9 on: each node's records reach the other, whose HELLO it
verified.

What they do not prove: Windows and Linux; four nodes; a relay; a round that a
link's close cuts short midway (its report is made, as for a round's end).

### Named gaps (4.1b part 2)

- **Third nodes, as ruled (question 1).** A configured peer cannot introduce a
  node: its records, bindings included, are deferred at every pull until this
  node meets it, so the door's introductions (4.2b) never fire. Nothing the
  slice runs has a third node.
- **The known set lives for the run.** A restart knows no peer until each
  links again. What the node already holds stays, checked again at open
  (question 2).
- **A deferred push waits for the next pull**, at the next connect: nothing
  asks sooner. A long-lived link learns nothing more of a node it met after
  the link's own pull.
- **The report is one line per chain per round.** A peer that keeps a third
  node's chain prints its line at every pull.
- **The containers**: records.json's `SystemSnapshot` and each record's wire
  `Op` are still read by the wire codec's decoders, which panic on a type they
  do not expect (section 10).
- A record in an unknown format that arrives from a peer is refused like any
  other that does not verify; only the stores on disk refuse the start.

### Default-path changes (4.1b part 2)

1. A peer's `home` record is taken only from this node or a node whose HELLO
   it has verified this run; any other node's chain is deferred for the round,
   kept nowhere, and asked for again at the next pull.
2. Each round, a pull or a push, reports each chain it deferred or refused,
   one line each, on stderr (the door's reporter, so the assembled root's
   console): `deferred N home record(s) of node <origin> on <stream> from peer
   <peer>: not a node this node knows`, or `refused N … : <reason>`.
3. A start whose records.json, or a `home` journal, holds a record in a format
   this build does not know is refused: exit 1, with `<file> holds a home
   record this build cannot read (<stream> of node <origin> at seq <n>): its
   format is newer than this build's, or it is damaged; start the build that
   wrote it, or move <file> aside`. Before, part 1 set such a record aside as
   unsigned, or quarantined it and dropped it at the next save.
4. The legacy form and every flow with no peer see nothing new: the desk's
   next restart prints part 1's lines.

### Measured (4.1b part 2)

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** passes all 8 components, with 297 node tests on each path,
  where there were 291: the six new tests of section 11. rustfmt: glade-node
  299 hunks, one below part 1's 300, from a line this part rewrote (a
  `pull_sync` call in `iroh_carrier.rs`); the baseline is lowered to 299, and
  no line this part wrote is a deviation. glade-wire 43. clippy 11 and 7. The
  contracts gate passes, unchanged.
- **The replay**, from `glade-wz/grazel` as grazel starts the node, each
  instance in a scratch home:
  - **Part 1's store, then this build** (as the lane owner asked). Part 1's
    default binary (inode 401046031) ran twice (`tab-a`'s Hello and 22 s,
    `tab-b` and 3 s), leaving 31 signed records. This build's first start
    printed part 1's lines exactly, `+0 record(s), 12 unchanged` for each app,
    with nothing set aside and nothing on stderr; it added the claim and its
    renewal (33 records), and `tab-a` minted nothing, being known. A second
    start was the same. (Its first start took 1.12 s, the first run of a new
    binary file on macOS; the second 0.14 s.)
  - **The 4.3 store, then this build**: the desk's case, should it not restart
    before landing. The 4.3-era instance was rebuilt byte for byte from part
    1's replay (its legacy file and journal moved back). This build printed
    part 1's first-start lines exactly: `set aside 31 unsigned record(s) in
    records.legacy-2026-09-24.json`, `+12` and `+10`, `set aside 1 journal(s)
    of the served store's home share (31 record(s)) …`, `ws-razel` at epoch 1,
    and the next start `+0`. So every record a 4.3 build wrote reads as
    unsigned, not unknown.
  - **A newer format.** Into a copy of the first instance, one record of this
    build's envelope on a stream it does not know (`dir.recovery-keys`). Both
    roots exited 1 with the line, the assembled root after its own line:

    ```text
    $R/home/sys/grazel/records.json holds a home record this build cannot read (dir.recovery-keys of node 9c3d3d5304fa8aa10cd2689c8b82a6e9cadc135da6180a30c9555e723409d90e at seq 0): its format is newer than this build's, or it is damaged; start the build that wrote it, or move $R/home/sys/grazel/records.json aside
    ```

    records.json was unchanged. On a copy, part 1's binary started instead,
    printed `quarantined 1 record(s) at load`, and dropped the record from
    records.json at its next save.
- **Downstream**, against the default binary (part 1, inode 401046031, not
  rebuilt), through the shims: client-rs 25 + 10, client-ts 48, grip-share
  19, grazel 29 + 3, glade-gyld 233 (1 ignored) + 33, glade-gwz 9 + 7. All at
  baseline.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +292/−53, net +239;
  - D9: `mesh.rs` +152/−28, `peer.rs` +17/−3, `signing.rs` +15/−7,
    `transport.rs` +7, `claims.rs` +1/−1;
  - the hardening: `envelope.rs` +45/−4, `sysdir.rs` +19/−8, and `store.rs`
    +36/−2, of which 25 are `StoreError`'s text, which D9's report uses;
- tests: +350/−12, net +338;
- beside them, `check.sh`'s baseline.

## Pull on a gap (the hardening's question 2)

Design addition, 2026-09-25, against glade `7cd2311`. The red runs and the
measured figures were filled in afterwards. The owner ruled the hardening's
question 2 on 2026-09-24, "all recommended": option (b), "a node that refuses
a pushed record as a gap pulls from the pusher at once, a small step of its own
before 4.5" (`dev-docs/GladeFirstSlicePlan.md` at the glade-wz root). It closes
two named gaps: the hardening's, where a peer that sees one of this node's
chains out of order stalls on it until its next pull (fix 3), and 4.1b part
1's, a push that reaches a peer before the pull at connect.

Nothing changes in the wire, in any durable format, in the contracts or in the
dependencies. The code is in `node/src/mesh.rs`, with one task site in
`node/src/tasks.rs`.

### 1. A gap, and what is not one

- **A gap** is a pushed `home` op that the served store refuses with
  `StoreError::Gap`. Either its seq is not one past the last its chain holds
  here, or its chain holds nothing here and its seq is not 0 (4.1b's
  `classify_home`).
  - Its predecessor is missing here, and the pusher holds it: a node pushes a
    record only once its own served store has taken it (`claims::publish`).
- **Not a gap**, and so starting no pull:
  - **A deferred chain** (D9): its origin is a node this node has not met. The
    round keeps none of it, and the store never sees it. It waits for the next
    pull at connect, as ruled.
  - **A chain break**: the seq follows but the `prev` does not. A pull from
    this node's heads would bring the same op, since the pusher answers by seq,
    not by hash (`missing_for`), and it would break the same way.
  - A record that does not verify, a fork, or a shape or writer conflict. A
    pull changes none of them.
- **Only a push starts a pull.** A pull asks from this node's heads, so what
  it brings follows what is held. Its round notes no gap.

### 2. The pull

- A push's round (`handle_peer_stream`'s `Ops` arm, one `Round`) notes each
  chain it cut short as a gap (`Gaps`): the chain's (stream, origin), the
  highest seq of it that the push carried, and one refusal.
- If no pull from that pusher runs, one starts at once: `pull_on_gap`.
  - It is a task at a new site, `GapPull`, owned by `Sessions`, like the
    link's other streams.
  - It opens a new stream on the pusher's live link, found by its node id. It
    sends this node's `home` heads and takes what comes back: `pull_home`, one
    D9 round, as at connect.
  - The pusher answers with `serve_home`, as it answers the pull at connect,
    whatever its build. Receiver-side only.
- A pull covers every gap noted before it began. The op of each such push was
  in the pusher's store before it was pushed, so before the pusher answered.

### 3. One pull at a time per pusher

- **The table.** The mesh keeps, per pusher, a pull that runs and the gaps
  noted since it began (`Mesh::gap_pulls`). A push refused as a gap while a
  pull from its pusher runs is noted there, and starts nothing.
- **A pull's end.** Each gap noted while it ran is judged against the store:
  - one it healed is done;
  - one still short is pulled for again, since its push may have come after
    the pusher answered;
  - a gap noted before the pull began, which the pull did not heal, is not
    pulled for again. Another pull from the same heads would not heal it
    either, and the pull's round has said why.
- **The entry goes** when a pull ends with no gap noted, under the lock that
  notes them. So a gap noted at that moment starts the next pull.
- **The bound.** At most one pull runs from a pusher, and each pull after the
  first follows a push refused while the one before ran. A burst of gaps costs
  one pull, or two if a push in it came after the pusher's answer. The bound
  is per pusher: n linked peers can run n pulls at once.
- **Order.** A push's gaps are noted before its round's lines are reported,
  and a new pull starts after them. Once a refusal is on the console, some
  pull answers for it, and that pull's line comes after it.

### 4. The report

One line per pull, through the door's reporter, beside the rounds' lines:

- `pulled N home record(s) from peer <peer> after G gap(s): <stream> of node
  <origin> healed`.
  - N is what the pull took, a duplicate included, as `SyncOutcome::applied`
    counts. G is how many refusals it answers.
  - Then each chain it was for: `healed` once the store holds it up to the
    highest seq pushed, else `not healed`. Several are joined by `; `.
- `a pull from peer <peer> after G gap(s) failed: <error>: <chains>`: no live
  link, or the stream failed.
- The pull's own round reports what it deferred or refused, before that line,
  as the pull at connect does.

### 5. App shares: not built

The ruling names the `home` share. App shares get no pull on a gap, as decided
here (question 1):

- A push carries `home` alone. `push_home`'s one caller is `claims::publish`,
  and the receiver's `Ops` arm takes `home` ops only.
- App-share content moves by interest, one QUIC stream per zone. The claim
  holder queues the ack, the resume gap and every live op of the zone on one
  channel (`serve_peer_subscribe`), so they cannot arrive out of order.
- `serve_home` serves `home` alone. An app share's pull is a subscribe, under
  4.3's grant check.

### 6. Tests, each begun red

Built on 2026-09-25 against glade `7cd2311`. The first test was run against
that commit's production code; the other two with the part each guards
switched off, by one edit in a copy of the sources. The message is what each
red run printed. `<B>` and `<C>` stand for the nodes' ids.

| Test | Proves | Red first |
| --- | --- | --- |
| `mesh`: `a_renewal_pushed_ahead_of_the_one_before_it_heals_by_a_pull` | over real iroh, A behind a door, linked to B, which holds B's claim. B's renewal at seq 2, pushed ahead of the one at seq 1, is refused as a gap: `refused 1 home record(s) of node <B> on dir.claims from peer <B>: a gap: expected seq 1, got 2`. A pulls from B at once, and holds both: `pulled 2 home record(s) from peer <B> after 1 gap(s): dir.claims of node <B> healed`. The late push of seq 1 changes nothing, and the next renewal lands in order | against `7cd2311`'s production code: "timed out waiting for the chain to heal at A", after 6.3 s |
| `mesh`: `a_burst_of_gaps_from_one_pusher_is_answered_by_one_pull` | B's store is held, so A's pull waits for B's answer. B pushes seq 4, 3 and 2, each refused as a gap while that one pull runs. Released, it heals all three, with `pulled 4 home record(s) from peer <B> after 3 gap(s): dir.claims of node <B> healed`, and ends; no other pull runs | with every gap starting its own pull: three lines `pulled 4 home record(s) from peer <B> after 1 gap(s): dir.claims of node <B> healed`, where one `after 3 gap(s)` was expected |
| `mesh`: `a_deferred_chain_starts_no_pull` | B pushes two records of C, a node A has not met. A defers them, with one line, and no pull runs. B's renewal pushed ahead of the one before it then starts one, whose line names B's chain alone | with a deferred chain noted as a gap: the lines `deferred 2 home record(s) of node <C> on dir.principals from peer <B>: not a node this node knows` and `pulled 0 home record(s) from peer <B> after 1 gap(s): dir.principals of node <C> not healed`, where the first alone was expected |

What they do not prove:

- The second pull, for a gap refused while a pull ran whose push came after
  the pusher's answer. No test can place a mint between the answer and the
  pull's end without a hook (question 3).
- A pull that fails, and the line's second form.
- The race of 4.1b part 1's named gap. It is the same refusal, on a chain that
  holds nothing yet (`expected seq 0`).
- Windows and Linux; more than two nodes.

### Named gaps

- **A lost push** with no later push on its chain still waits for the next
  pull at connect (4.4's ruling, "a lost push waits for the next pull"). A
  later push on the chain now heals it: for a served share, the next renewal,
  10 s on.
- **A chain that cannot heal**, because this node refuses a record the pull
  brings (one that does not verify, say), gets a pull for each push refused on
  it, one at a time, with its lines each time.
- **The bound is per pusher**, not per node: n linked peers can run n pulls at
  once.
- **A pull cancelled midway** leaves its pusher's entry in the table, and later
  gaps from that pusher wait on a pull that is gone. Only the owner's stop
  cancels one, and the node is then ending.
- **A third node's chain** that the pusher holds is reported deferred at each
  such pull, as at each connect (4.1b part 2's named gap).
- **A chain break** on a push is not pulled for, nor is a record that does not
  verify (section 1).

### Default-path changes

1. A node whose served store refuses a peer's pushed `home` record as a gap
   pulls that peer's `home` share at once, from its heads, over the same link,
   one pull at a time per peer. It reports a line per pull: `pulled N home
   record(s) from peer <peer> after G gap(s): …`.
2. Nothing a node sends changes. A node of an older build answers the pull as
   it answers the one at connect.
3. The desk sees nothing: it has no peer. Its restart prints the same lines
   (Measured).

### Questions for the owner

1. **App shares** (section 5). Recommend no pull on a gap for them, as built.
   They are never pushed, and their one path, a forwarded interest, is ordered
   by construction. The other choice is a gap check on the forwarded stream
   (`run_forward`, which drops a refused op today), ending the forward so that
   the next subscribe resumes from its heads. That would be its own step, if a
   gap is ever seen there.
2. **A chain break on a push** (section 1). Recommend no pull, as built: a pull
   from the same heads brings the same op. A break means the pusher's chain
   forked or the op was altered on the way. The round's line reports it.
3. **The second pull** (section 3). It is built, and no test covers it.
   Without it, a gap whose push came after the pusher's answer waits for the
   next push on its chain: 10 s for a renewal, one missed renewal inside a
   30 s lease. Recommend keeping it. A test would need a hook in `serve_home`
   that holds the answer between computing and sending it.

### Measured

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** passes all 8 components, in 103 s from an empty scratch target.
  There are 300 node tests on each path, across 15 test binaries, where there
  were 297: the three new `mesh` tests.
  - rustfmt: glade-node 299 hunks, at its baseline, and no line this step wrote
    is a deviation. `mesh.rs` holds its 36 hunks as before; one of them now
    shows an edited doc line as its context. glade-wire 43.
  - clippy: glade-node 11 warnings and glade-wire 7, at their baselines.
  - The contracts gate passes, unchanged.
- **Time**: alone, over three warm runs each, the reorder and deferred tests
  take 0.05 s, the burst test 0.07 s. Each binds two endpoints on loopback.
- **Repeat runs**, on one build: the `mesh` module's 15 tests 40 times, and the
  library's 202 tests 8 times. No failure.
- **The replay**, from `glade-wz/grazel` as grazel starts the node, on one
  scratch instance with the desk's two app files: the default binary (4.1b,
  inode 401201248) twice, then this build twice.
  - The first start registered `+12 record(s)` and `+10 record(s), 2
    unchanged`.
  - Every later start, this build's two included, printed the same nine lines:
    `+0 record(s), 12 unchanged` for each app, `ws-razel` serving, then
    `listening`. They differed only in the client port the OS chose, and
    printed nothing on stderr.
- **Downstream**, against the default binary (inode 401201248, not rebuilt),
  through the shims: client-rs 25 + 10, client-ts 48, grip-share 19, grazel
  29 + 3, glade-gyld 233 (1 ignored) + 33, glade-gwz 9 + 7. All at baseline.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +198/−4, net +194: `mesh.rs` +194/−3, of which 143 lines are
  code and the rest comments and blank lines; `tasks.rs` +4/−1;
- tests: +179, in `mesh.rs`.

## The persistence suite on records.json (the owner's ruling of 2026-09-24)

Design addition, 2026-09-25, written before the code against glade `d69fdce`.
The red runs and the measured figures were filled in afterwards. The owner
ruled on 2026-09-24, "all recommended": "PS-001..008 run on records.json with
an optional revision field in `SystemSnapshot` (today's files read as revision
1), as a step of its own" (`dev-docs/GladeFirstSlicePlan.md` at the glade-wz
root). That is option (ii) of 4.4's "Blocked, for the owner", above.

**The step is split in two.**

- **Part 1, built with this note:** the revision in records.json, the store
  that commits it with the bytes, and a checked read that answers a damaged
  records.json with an error where the wire codec panicked (sections 1 to 5).
  No dependency, contract or policy changes.
- **Part 2, the persistence port itself:** the dependency on
  `glade-persistence-api`, the policy change, `SnapshotStore` over records.json
  and PS-001..008. It waits on one question for the owner (section 6): the
  contract's probes commit bytes that are not a snapshot, and records.json, as
  ruled, holds a snapshot.

### 1. The format

- records.json stays the canonical CBOR of a `SystemSnapshot`. It gains key 3,
  `revision`: the store's revision, a CBOR unsigned integer, 1 at the first
  save and one more at each save after. In the IR, `F("revision", 3, INT,
  optional=True)`, regenerated with `--legacy-codec`
  (`GladeProgramStatus.md:29`).
- **Today's files.** A records.json without key 3, as every build so far wrote
  it, reads as revision 1, its bytes unchanged. So does a key 3 that is null,
  which is what the generated codec writes for no revision.
- **An older build** (today's default binary, glade `d69fdce`) reads a file
  this build wrote. Its decoder takes keys 1 and 2 and ignores key 3. Its next
  save writes keys 1 and 2 alone, so the revision is dropped, and this build
  then reads the file as revision 1 again.
- **The generated decoder is not used on records.json.** It panics on a
  missing key 3, as on any key it does not find. The store reads the file with
  a checked reader instead (section 3).
- **In memory**, a snapshot stays a fold and its heads. `Registry::snapshot`
  leaves the field `None`, `BlobStore::load` hands back the snapshot without
  it, and a save ignores it. The revision belongs to the store: "storage-local,
  not a source epoch", as the contract has it.
- **The range** is the contract's, 1 to `u64::MAX`. The IR's INT holds up to
  `i64::MAX`, which one save every 10 s would pass in about 3×10^12 years. A
  revision above it is written as the CBOR unsigned integer it is, which the
  generated INT codec cannot hold. Only a test writes one.

### 2. The store: `RecordsFile` (`node/src/records_file.rs`)

It keeps records.json and its revision, whatever the snapshot inside holds.

- **`load()`** answers `None` when records.json is absent: established absence,
  as today. Otherwise it answers the revision and the snapshot's bytes, which
  are the file's map without key 3, byte for byte. A file without key 3 comes
  back exactly as it is on disk.
- **`compare_exchange(expected, snapshot)`**, under the store's lock:
  1. It reads the revision records.json holds, and answers `Conflict` unless it
     is `expected` (`None` meaning absent).
  2. It writes the snapshot with the next revision at key 3, in its canonical
     place, by 4.4's order: to `records.json.tmp`, synced, renamed over
     records.json, and the directory synced.
  3. It answers the new revision.
  The snapshot must be a CBOR map with its keys in order and no key 3. The
  node's always is.
- **The outcomes** are the persistence contract's, each with what went wrong:
  - `Unavailable`: a read, the lock, or a write before the rename failed.
    Nothing changed.
  - `Corrupt`: records.json is not a CBOR map this build can read, or its key 3
    is not a revision (section 3). A load answers it, and so does a
    compare-and-swap, which then writes nothing. Corruption never reads as
    absence or as an empty snapshot.
  - `Conflict`: records.json holds another revision.
  - `Exhausted`: the revision is `u64::MAX`. Nothing is written.
  - `OutcomeUnknown`: the rename happened and the directory's sync failed. The
    new snapshot is in place, and may not survive a crash.
  - `NotASnapshot`: the bytes handed to it are not such a map. Nothing is
    written. What the port makes of this is section 6's question.
  - No capacity is enforced. A full disk fails the temp write, as
    `Unavailable`, and records.json is unchanged.
- **The lock** is `records.json.lock` in the instance directory: an exclusive
  OS lock (`File::lock`, in std since 1.89) held for each compare-and-swap.
  - Two handles, in one process or two, take turns: the second reads the
    revision the first wrote, and conflicts.
  - The file stays, empty.
  - A load takes no lock. The rename is atomic, so a load sees the old file or
    the new one, whole.
  - It closes 4.4's finding under "Concurrent handles": two saves in flight
    could tear the file through their one temp name.
- **Cost**: each compare-and-swap reads records.json to find its revision, as
  well as writing it (Measured).

### 3. The checked reader

- **The container.** The store walks records.json without decoding it: the
  map's head, then each entry's key, an unsigned integer, and its value, one
  well-formed CBOR item of the kinds glade-wire writes, skipped by its length.
  Key 3 may appear once, and its value must be an unsigned integer from 1, or
  null.
- **`Corrupt` names what is wrong**: a torn or unreadable item (an indefinite
  length among them, which glade-wire never writes), bytes left over, not a
  map, a key that is not an unsigned integer, a tag, key 3 twice, or a key 3
  that is not a revision.
- **The snapshot.** The node's engine finds keys 1 and 2 by the same walk and
  reads each with 4.1b's checked decoder (`envelope::parse`): a list of byte
  strings. Other keys are ignored, as the wire codec ignored them: a newer
  build's, which this build then drops at its next save, as an older build
  drops the revision. A key 1 or 2 that is missing, repeated or of another
  type fails too.
- **So** a records.json that is damaged, or not a records.json, refuses the
  start with a message, and nothing is written, where the wire codec panicked.
  This closes 4.1b's named gap ("The containers") for the snapshot. Each record
  inside, a wire `Op`, is still read by the wire codec (named gaps).

### 4. The node's engine: `BlobStore` over `RecordsFile`

- **`load()`** answers the snapshot, decoded as section 3 says, without its
  revision. The handle keeps the revision it read. A damaged file fails as
  `InvalidData`: `<path> cannot be read as a snapshot (<why>): it is damaged,
  or not a records.json; move it aside to start without it`.
- **`save()`** writes the snapshot's keys 1 and 2, compared and swapped against
  the revision this handle last read or wrote, and keeps the new one.
  - A handle that has read nothing saves over whatever records.json holds, at
    its revision plus one. That is what `BlobStore::new(dir).save(..)` did
    before, and tests use it to write an instance. The node reads first at
    every boot, and saves through that handle.
  - A conflict fails the save: `<path>: it holds revision N, where revision
    M was expected: another handle saved it`. It fails as a refused save
    does, so `Registry::accept` commits nothing and publishes nothing. Under
    the instance lock a node has no other writer, so it does not happen there.
  - After `OutcomeUnknown`, the handle takes records.json's revision afresh at
    its next save. The node is its instance's one writer, so what it finds is
    its own.
- `MemStore` and the journeys' engines are unchanged: they keep what they are
  given.

### 5. Compatibility, and the desk

- **The desk's next restart on this build.** records.json reads as revision 1,
  its records and heads as they were. The first save writes revision 2, and
  each save one more: the hand-written root saves once per app file at every
  start, then once per mint (the claim, a renewal every 10 s, each new
  principal), so about 8,640 a day. `records.json.lock` appears beside
  records.json at the first save.
- **Back to today's binary** (a downgrade): it starts, reads keys 1 and 2, and
  drops key 3 at its first save. It ignores `records.json.lock`. This build
  then reads revision 1 again, so the revision restarts after a downgrade. A
  handle that read revision r before the downgrade could then compare equal to
  a different file at r; no handle outlives its process, so no node sees it.
- The served store, the wire and every record's bytes are unchanged.

### 6. The question for the owner: the probes' bytes

**The fact.** The contract stores opaque bytes: "Schema/authenticity
validation remains with the caller" (`contracts/persistence-api/src/lib.rs`,
the trait's doc), and its README speaks of "opaque caller-encoded snapshots".
The probes commit `[1, 2, 3]`, `[]`, `[4, 5]` and `[7, 8]`, and PS-008 preloads
`[9]`: none is a CBOR map. A store that keeps its revision inside the snapshot,
as ruled, has to read the snapshot's container to put it there, so it can keep
a map but not arbitrary bytes. So PS-001..008 cannot run on records.json's
format as the contract states them. Neither 4.4's note nor the ruling looked
at the probes' bytes.

**The options.**

- **(a) Snapshots only, the probes through a fixture.** The adapter keeps the
  node's snapshot, a map, and adds the revision. It refuses any other bytes
  before writing. The contract has no outcome for that; the nearest is
  `Capacity` ("records.json has room for a snapshot"). PS-001..008 run on it
  through a test fixture that carries each probe's bytes as the one record of
  a snapshot, as `tests/durable/adapter.rs` already carries its bytes. They
  then exercise records.json's one form, the form the node writes, end to
  end. The deviation: a precondition on the bytes, which the contract does not
  provide for.
- **(b) Any bytes, in two forms.** As (a) for a map. Any other bytes are kept
  whole at key 4 beside the revision, `{3: revision, 4: bytes}`, which is
  option (i)'s container. The probes run unmodified, but on key 4's form,
  which no node writes; the node's form is covered by its own tests. There is
  no deviation from the contract, and a second form that an older build cannot
  read (no node writes it).
- **(c) A second view.** The port's impl keeps a caller's bytes as the one
  record of a snapshot, through the same store and revision code. The probes
  run unmodified on it. The node's own bytes and the port's are then shaped
  differently in one file format, so a later engine behind the port could not
  replace records.json for the node without the node moving onto the port.
- **(d) Option (i) after all.** A container, `{revision, bytes}`, holds
  anything, and the probes run unmodified on the node's own form. An older
  build cannot read a new file. A build from before 4.1b cannot start on an
  instance a later build wrote anyway, so what (ii) keeps is a downgrade to a
  build since 4.1b, today's among them.
- **(e) A contract change**: the probes take their bytes from the fixture. It
  changes a contract for one adapter.

**Recommendation: (a).** It keeps the ruled format with one form, runs the
node's saves and the probes through the same store, and has the probes cover
exactly what the node writes. The precondition is what (ii) implies: a
revision inside the snapshot needs a snapshot. Part 1 stands under (a), (b),
(c) and (e); (d) would replace its format.

**Part 2, once ruled** (an estimate): the dependency, normal and dev with
`conformance`, and its lockfile entry; the policy's two entries and its
reason, for the owner's review; `impl SnapshotStore for RecordsFile` with the
contract's outcomes, 40 to 60 lines (the refusal under (a), key 4 under (b),
the second view under (c)); PS-001..008, about 120 lines of tests. PS-006's
lost reply is injected by a fixture that drops the answer of a committed swap,
and PS-008's file, at revision `u64::MAX`, is written by hand.

### 7. Tests (part 1), each begun red

Built on 2026-09-25 against glade `d69fdce`. Three tests were run against
that commit's production code, in a copy of the sources with the new test
added; the rest with the part each guards switched off, by one edit in a copy
of this tree. The message is what each red run printed.

| Test | Proves | Red first |
| --- | --- | --- |
| `records_file`: `a_file_from_before_the_revision_loads_byte_for_byte_as_revision_1` | the ruling's acceptance: a records.json as every build before wrote it (keys 1 and 2), and one whose key 3 is null, load byte for byte as revision 1; the node's engine reads the same snapshot, writing nothing; its next save is revision 2, with keys 1 and 2 as they were | with the generated decoder reading records.json, as `BlobStore::load` did, over the regenerated IR: `no map key 3` |
| `records_file`: `each_swap_commits_the_next_revision_with_its_bytes` | absence is `None`; a swap from none is revision 1 and the file is the snapshot's map with `3: 1` after keys 1 and 2; the next is 2; a second handle reads it | with the revision not written: the file `[162, …]` where `[163, …, 3, 1]` was expected |
| `records_file`: `a_stale_revision_conflicts_and_writes_nothing` | a swap expecting none, 0 or 2 over revision 1 conflicts, naming both, and writes nothing | with no comparison: `None: Ok(2)` |
| `records_file`: `a_damaged_records_json_is_corrupt_and_never_a_panic` | twelve damaged files (empty, torn, a byte left over, an array, a text key, an indefinite length, a tag, a count past the bytes, key 3 of 0, of text, negative, twice) are each `Corrupt` with its reason, for a load and a swap, which writes nothing; the node's engine answers `InvalidData` naming the file, and for a key 1 that is not a list | with the wire codec reading the container: `index out of bounds: the len is 0 but the index is 0` |
| `records_file`: `a_swap_waits_for_the_lock_another_handle_holds` | while the test holds `records.json.lock`, a swap in another thread waits and writes nothing; released, it lands at revision 1 | with no lock taken: `the swap waits for the lock` |
| `records_file`: `the_last_revision_is_exhausted_and_writes_nothing` | a file at revision `u64::MAX`, written as the 9-byte CBOR unsigned integer, loads; a swap from it is `Exhausted` and writes nothing | with the revision wrapping: `Ok(0)` |
| `records_file`: `an_older_build_reads_the_file_and_drops_the_revision_when_it_saves` | the decoder the build before generated for keys 1 and 2 reads a file at revision 2; its save, keys 1 and 2 alone, reads here as revision 1 | with the revision kept in option (i)'s container, `{3: revision, 4: snapshot}`: `no map key 1` |
| `sysdir`: `a_boot_refuses_a_damaged_records_json_and_writes_nothing` | a records.json cut in half refuses the boot (`InvalidData`) with the message; records.json is as it was, and nothing is set aside | against `d69fdce`: `range end index 913 out of range for slice of length 658` (the wire codec) |
| `sysdir`: `an_instance_from_before_the_revision_boots_unchanged_as_revision_1` | an instance whose records.json has keys 1 and 2 alone boots with the same fold, nothing quarantined or set aside, records.json read and not written; the boot's store then saves revision 2 | with the generated decoder, as above: `no map key 3` |
| `tests/durable`: `a_handle_that_did_not_read_the_last_save_conflicts_and_writes_nothing` (replaces `two_handles_on_one_directory_replace_each_other_without_a_conflict`) | of two engines on one directory, the one whose next save is over a revision it never read conflicts, with the message, and writes nothing; having read it, it saves | against `d69fdce`: ``called `Result::unwrap_err()` on an `Ok` value: ()`` |
| `tests/assembled_path`: `both_roots_refuse_a_damaged_records_json_with_a_clear_message` | on each root, as processes: a records.json cut in half ends the start with exit 1 and the message on stderr, no panic, records.json as it was | against `d69fdce`: exit `Some(101)` where `Some(1)` was expected; the node panicked with `range end index 912 out of range for slice of length 657` |

Changed and passing: the other two tests in `tests/durable/adapter.rs` and
the module's note (records.json now has a revision; PS-001..008 wait on part
2); `SystemSnapshot` literals gain `revision: None`. Every other test runs
unchanged, the durable journeys over the revised records.json.

What they do not prove: a crash between the rename and the directory's sync
(`OutcomeUnknown` is not produced); two processes, rather than two threads, on
the lock; Windows and Linux, which the lane owner runs on dabeest and the Pi.

### Named gaps (part 1)

- **The port.** PS-001..008 do not run yet (section 6).
- **Each record inside** records.json, a wire `Op`, is still read by the wire
  codec, which panics on a type it does not expect (4.1b's "The containers",
  now closed for the snapshot alone). A checked `Op` decoder would be about 40
  lines: the op nests three deep (its `refs`), past `envelope::parse`'s two.
- **Each save reads records.json whole** to find its revision, under the lock.
  At a week of renewals, about 60,000 records and 18 MB, a save measured 32
  ms where it was 19 ms, and a load 24 ms where it was 11 ms (Measured). A
  handle could skip the read while the file is the one it last wrote, but std
  has no portable file identity, as the hardening found for the instance
  lock.
- **The IR's INT** is `i64`, and the store's revision is the contract's `u64`.
  A revision past `i64::MAX` is written as the unsigned integer it is, which the
  generated codec cannot hold; only a test writes one.
- **A downgrade restarts the revision** at 1 (section 5).
- **`records.json.lock` stays**, empty, like `instance.lock` before the
  hardening removed it on release. Removing it would need the identity check
  `InstanceLock` makes, for the same race.
- **Seen in passing, not this step's:** from the fourth start of the replay,
  30 s after the first, every build prints `registry ready (home served:
  false)`. The `home` claim minted at a first boot carries a 30 s lease, and
  nothing renews it: `renew_leases` renews the shares `serve_workspace_on`
  entered. So any start more than 30 s after an instance's first boot prints
  `false`, today's binary's too; earlier replays stopped each start at
  `listening`, within the 30 s.

### Default-path changes (part 1)

1. records.json gains key 3, its revision: 2 at the desk's first save after
   the upgrade, one more at each save (about 8,640 a day).
2. `records.json.lock` appears in the instance directory at the first save.
3. Each save reads records.json, under that lock, before it writes.
4. A save over a revision its handle did not read fails. Under the instance
   lock the node has one handle, so this does not happen.
5. A damaged records.json refuses the start with `<path> cannot be read as a
   snapshot (<why>): it is damaged, or not a records.json; move it aside to
   start without it`, exit 1, where the wire codec panicked (exit 101).
6. A save over a damaged records.json fails (`Corrupt`), where it overwrote
   it. Boot refuses such a file before any save, so only a test could meet
   this.
7. A downgrade to today's binary works: it starts, and drops the revision at
   its first save.

### Measured (part 1)

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** passes all 8 components, in 102 s from an empty scratch target,
  with 310 node tests on each path, where there were 300: the ten new tests of
  section 7 (the eleventh replaces a test). rustfmt: glade-node 299 hunks, at
  its baseline. The regenerated `sysdata.rs` adds one, its long codec lines,
  and `BlobStore::new`'s old one went with its rewrite; no line this step
  wrote by hand is a deviation. glade-wire 43. clippy: 11 and 7, at baseline.
  The contracts gate passes, untouched.
- **The regeneration.** The generator at taut `7a5f616` reproduces
  `d69fdce`'s `sysdata.rs` byte for byte (`cmp`); after the IR change,
  regenerated with `--legacy-codec`, `sysdata.rs` is exactly what it wrote:
  +3 lines, the field and its codec.
- **Time.** `tests/durable`'s 15 tests take 0.22–0.26 s, as before; the fast
  loop does not touch records.json.
- **A save and a load**, debug build (the desk's), records of 294 bytes (a
  signed renewal), three interleaved rounds against `d69fdce`'s engine, each
  the mean of ten, load average 5–8:

  | records | records.json | save, before | save, after | load, before | load, after |
  | --- | --- | --- | --- | --- | --- |
  | 300 | 90 KB | 6.5–7.4 ms | 5.1–7.6 ms | 0.07–0.16 ms | 0.12–0.60 ms |
  | 8,640 (a day) | 2.6 MB | 8.3–8.8 ms | 9.0–10.8 ms | 1.5–1.8 ms | 3.2–3.3 ms |
  | 60,480 (a week) | 18 MB | 18.8–19.4 ms | 31.9–32.7 ms | 11.2–11.4 ms | 24.3–24.5 ms |

  A save is dominated by its two syncs until the file is large; the read and
  walk under the lock then add about 13 ms at a week's size. A load walks the
  file twice and copies it once more than before; it runs once a boot, beside
  about 3 s of signature checks at that size (4.1b's F4).

- **The replay**, from `glade-wz/grazel` as grazel starts the node
  (`--profile local --name grazel --app apps/grazel-app.glade --app
  apps/gyld-app.glade 0`), on one scratch instance. Each start lived 11 s past
  `listening`, one renewal tick, and was stopped. Today's default binary
  (inode 401419601) twice, this build twice, then today's binary on the store
  this build wrote, then this build again:

  | Start | Lines | records.json after it |
  | --- | --- | --- |
  | today's, 1 | `registry ready (home served: true)`, `+12 record(s), 0 unchanged`, `+10 record(s), 2 unchanged`, `ws-razel` serving (twice), `listening` | keys 1 and 2, 27 records, 8 heads, 8,043 bytes; no lock file |
  | today's, 2 | `+0 record(s), 12 unchanged` for each app, the rest as before | keys 1 and 2, 29 records |
  | this build, 1 | the same lines as today's second | keys 1, 2 and 3, **revision 5** (read as 1, then two registrations, the claim, a renewal), 31 records: the 29 from before first, byte for byte; `records.json.lock` present |
  | this build, 2 | the same, but `home served: false` (below) | revision 9, 33 records |
  | today's, on this build's store | the same lines, nothing on stderr | **keys 1 and 2**: the revision dropped at its first save; 35 records; the lock file left, unused |
  | this build, again | the same | revision 5 (read as 1 again), 37 records |

  Every start printed nothing on stderr. Node and endpoint ids were the same
  throughout. From the fourth start on, every build printed `home served:
  false`, the 30 s `home` lease (named gaps).
- **Downstream**, against the default binary (inode 401419601, not rebuilt),
  through the shims: client-rs 25 + 10, client-ts 48, grip-share 19, grazel
  29 + 3, glade-gyld 233 (1 ignored) + 33, glade-gwz 9 + 7. All at baseline.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +485/−35, net +450: `records_file.rs` +389, new;
  `registry.rs` +82/−33; `sysdir.rs` +6; `envelope.rs` +4/−2; `sysdata.rs`
  +3 (generated); `lib.rs` +1;
- tests: +395/−21: `records_file.rs` +269, `sysdir.rs` +61, `tests/durable/
  adapter.rs` +31/−20, `tests/assembled_path.rs` +33/−1, `registry.rs` +1;
- beside them, the IR +11/−1.

## The `home` claim renewed like any served share (the lane owner's ruling of 2026-09-25)

Design addition, 2026-09-25, written before the code against glade `83ba787`.
The red runs and the measured figures were filled in afterwards. The
persistence suite's part 1 found the defect (its named gaps, "Seen in
passing"). The lane owner ruled the fix for the lane: "`home` joins the renewal
loop at adoption, like any served share", which fits GDL-038's "the home share
stays an ORDINARY share".

Nothing changes in the wire, in any durable format, in the contracts or in the
dependencies. The code is in `node/src/claims.rs`, with the start line in both
composition roots.

### 1. The defect

- A first boot (`sysdir::boot_at`) mints the node's presence and a
  `ServeClaim` on `home`, at epoch 1, on a 30 s lease.
- Adoption (`adopt_boot_tuned`) starts the renewal set, `DirAuthority.served`,
  empty. `renew_leases` renews only the shares in it, which
  `serve_workspace_on` enters.
- So the `home` claim lapses 30 s after the first boot and is never renewed.
  Every later start prints `registry ready (home served: false)`, and a peer
  holds the node's `home` claim as lapsed.
- Routing never reads it: `mesh::route_subscribe` answers `Local` for `home`.
  The damage is a false status line and a stale claim in the directory.

### 2. `home` in the renewal set

- Adoption starts the renewal set with `home` in it, at the epoch of section
  3. Every tick renews it with the other served shares: one acceptance, one
  save of records.json, one push.
- **Adoption renews it at once**, before it returns, as a serve mints its
  first claim at once. A claim that lapsed while the node was stopped is live
  again before the peer endpoint binds or the listener opens, so no peer or
  client of this run sees it lapsed. A renewal whose save fails is neither
  folded nor published, as at any tick, and the next tick retries it; the
  start goes on.
- So a running node's `home` claim is live from adoption on, renewed every
  10 s. It lapses one lease, 30 s, after the last renewal, once the node has
  stopped.

### 3. The claim rules

`serve_workspace_on` mints a share's first claim one epoch above the highest
the served replica holds for that share, live or lapsed, from any node. A node
that restarts or takes over so fences out any stale claim, and its renewals
keep that epoch.

- **The epoch.** `home` joins at the highest epoch among this node's own
  claims on it in the served replica, live or lapsed. For every node so far
  that is 1, the epoch the first boot minted. A node with no claim of its own
  on `home` (its claims chain quarantined at load) joins at 1, as a first boot
  mints.
  - Not one above the highest. Every node serves `home` at once, from its own
    replica, so no node's claim on it is stale for another's to fence out.
    Under a serve's rule, each start would move `who_serves(home)` to
    whichever node started last, as though it had taken `home` over.
  - So a renewal and a later boot both keep the epoch: all of a node's claims
    on `home` carry one epoch.
- **A lapsed claim** is renewed at its epoch with a fresh expiry, an ordinary
  `ServeClaim` append like any renewal. Expiry is judged at the reader's
  clock, so the newest claim decides.
- **A later boot** finds the claim its last run renewed: live if the node
  stopped less than 30 s before, lapsed otherwise. Adoption takes it up at its
  epoch and renews it at once.
- **`serve_workspace_on` of `home`**, from a `workspace home ...` line or the
  create ceremony, finds `home` in the set and mints nothing (`created:
  false`). Before, it minted a `WorkspaceEntry` naming `home` and a claim one
  epoch above the highest.

### 4. The start line

- `registry ready (home served: ...)` reads `who_serves(home)` from the
  registry's fold. Both roots print it before adoption: the hand-written root
  right after boot, the assembled root in its `Assembly` step, through
  `Directory::serves`.
- Printed there, a start more than 30 s after the node stopped still reads the
  claim as lapsed, which it is at that moment, and prints `false`, even with
  section 2: adoption renews the claim only afterwards.
- **So the line moves.** Both roots print it right after adoption, from the
  adopted registry's fold (a new `Server::serves`), where it reads `true`. Its
  words stay. The hand-written root prints it after `adopt_boot`, the
  assembled root in `Storage`, after the same call. If the save at adoption
  fails, a lapsed claim stays lapsed, and the line says `false`, which is
  then true.
- The order of the lines becomes: `instance`, `node`, any of the boot's own
  lines (`set aside`, `revoked`, `quarantined`, the grant fold's), each file's
  `app` line, any of the served store's (`set aside`, `client grants
  enforced`), `registry ready`, `peer` and any `peer-connected`, each
  `workspace` line, `listening`. Before, `registry ready` came before the
  first `app` line. Nothing downstream reads the line: every consumer reads
  `listening` alone.
- `Directory::serves` keeps its tests; no root calls it any more.

### 5. The cost

**Since F1 (2026-09-27, the owner's ruling on question 32 (a)):** a claim lives five minutes and
is renewed every 100 s by default, the node's `Leases`, which each composition root takes from its
entry point. The figures below are at the old 30 s and 10 s. At the new defaults the desk mints
1,728 renewal records a day (about 0.5 MB), and after a week each save rewrites about 3.5 MB once
every 100 s. Growth is still linear; plan Step 4.5c's signed checkpoints end it.

- **One more renewal record every 10 s** on every adopted node: 8,640 a day,
  each about 290 bytes signed, so about 2.5 MB a day in records.json and as
  much again in the served store's `home` journal. The desk serves one
  workspace, `ws-razel`, so its renewals double, from 8,640 a day to 17,280.
- **Saves.** Where a workspace is served, a tick still makes one acceptance
  and one save. A node that serves none, which saved nothing on a tick
  before, now saves records.json every 10 s.
- **One more record at each start**: the renewal at adoption.
- **Each tick's push** carries two records instead of one.
- **Boot.** The signature checks at load grow with the records (4.1b's F4): a
  week of the desk's renewals becomes about 121,000 records instead of
  60,000. Nothing compacts them (named gaps).

### 6. Tests, each begun red

Built on 2026-09-25 against glade `83ba787`. Each test was run first against
that commit's production code, in a copy of its sources with the three tests
added; the message is what each red run printed. `<node>` stands for the
node's id.

| Test | Proves | Red first |
| --- | --- | --- |
| `claims`: `the_home_claim_is_renewed_while_the_node_runs` | a node whose `home` claim was leased for 300 ms, written as the node writes it, adopted on 300 ms leases renewed every 100 ms, holds a claim on `home` live three leases past the first one's end, in the served store and in records.json, every one at epoch 1 | "timed out waiting for a claim on home live three leases past the first", after 6.3 s |
| `claims`: `a_later_boot_renews_its_lapsed_home_claim_at_once_at_its_epoch` | a node whose `home` claim lapsed while it was stopped, adopted with the loop an hour off, holds it live at once, in the served store and in the adopted registry, and the renewal carries epoch 1 (`[1, 1]`), where a serve's rule would have minted 2 | "live at once": `left: None`, `right: Some("<node>")` |
| `assembled_path`: `both_roots_report_home_served_on_a_start_after_the_claim_lapsed` | on each root, a start on an instance whose `home` claim lapsed prints `instance`, `node`, `registry`, `peer`, `listening`, the third `registry ready (home served: true)`, and records.json then holds the claim live | on the hand-written root, the first: `left: "registry ready (home served: false)"`, `right: "registry ready (home served: true)"` |

Changed and passing: `both_roots_boot_register_and_serve_alike` and
`both_roots_set_an_unsigned_instance_aside_and_serve_signed` expect
`registry ready` after the `app` line (section 4), and
`adoption_after_the_unsigned_home_journal_is_set_aside_serves_signed` holds
three claims at epoch 1 on the node's chain, where it held two: the `home`
claim, its renewal at adoption, then `ws-x`'s.

What they do not prove:

- A save that fails at adoption. `a_renewal_whose_save_fails_is_not_published`
  covers a failed save in `renew_leases`, which adoption calls.
- The epoch for a node with no claim of its own on `home` (1).
- A peer taking the renewed claim. The two-node F1 test covers a served
  share's renewals reaching a peer, and `home`'s ride the same push.
- `serve_workspace_on` of `home` minting nothing.
- Windows and Linux.

### Named gaps

- **Nothing compacts renewals**, and the desk's now accumulate twice as fast
  (section 5). Boot's signature checks and each save's read of records.json
  grow with them (4.1b's F4; the persistence suite's part 1, "Each save reads
  records.json whole").
- **A first boot writes two claims on `home`**, the boot's and adoption's
  renewal, milliseconds apart.
- **A `workspace home ...` line** loads and now mints nothing (section 3).
  Refusing it at load would be `appdecl`'s; nothing ships one.
- **A start that fails after adoption**, say on a port already bound, leaves
  its `home` claim live for up to 30 s, as it leaves a served workspace's
  today.
- **`Directory::serves`** has no caller outside its tests.

### Default-path changes

1. Every adopted node renews its `home` claim every 10 s, like a served
   workspace, and once at adoption. The claim lapses 30 s after the node
   stops.
2. Both roots print `registry ready (home served: ...)` after adoption, after
   the `app` lines. On the desk it moves from the third line to the fifth, and
   every start prints `true`.
3. The desk's records.json and served store gain 17,280 renewal records a
   day, where they gained 8,640, and one more record at each start.
4. A node that serves no workspace saves records.json every 10 s; before, a
   tick saved nothing.
5. A peer receives one more record at each of a node's ticks: two from the desk's, where it received one.
6. `serve_workspace` or a create ceremony naming `home` mints nothing.
7. A downgrade to today's binary starts. It prints `home served: false`, as
   today, and no longer renews `home` (Measured).

### Questions for the owner

1. **The epoch** (section 3). Recommend the node's own epoch, as built: every
   node serves `home` at once, so a serve's rule, one above the highest at
   each start, would fence out nothing real and move `who_serves(home)` to
   whichever node started last.
2. **The line** (section 4). Recommend moving it after adoption, as built,
   with its words kept. The other choice keeps its place and has it answer
   whether the fold holds a claim of this node's on `home` at all, live or
   lapsed; that is true of every booted node from now on, so it could not say
   `false` when a renewal fails.
3. **The renewal at once** (section 2). Recommend keeping it: one record per
   start. Without it, a start more than 30 s after a stop keeps its `home`
   claim lapsed until the first tick, 10 s on, which peers see, and the moved
   line would print `false`.

### Measured

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** passes all 8 components, in 101 s from an empty scratch target,
  with 313 node tests on each path, across 15 test binaries, where there were
  310: the three new tests.
  - rustfmt: glade-node 298 hunks, below its baseline of 299. The literal
    this change rewrote, `DirAuthority { boot, served }`, was one of the 299;
    no line it wrote is a deviation. glade-wire 43.
  - clippy: glade-node 11 warnings and glade-wire 7, at their baselines.
  - The contracts gate passes, untouched.
- **Time.** The `claims` module's ten tests finish in 0.97 s: the renewal test
  waits, by design, until a renewal's lease ends three 300 ms leases past the
  first one's, about 0.9 s. `assembled_path`'s twelve take 1.2 to 1.4 s.
- **The replay**, from `glade-wz/grazel` as grazel starts the node
  (`--profile local --name grazel --app apps/grazel-app.glade --app
  apps/gyld-app.glade 0`), on one scratch instance. Each start lived 11 s past
  `listening`, one renewal tick, and was stopped, and 35 s passed before the
  next, so the `home` claim of the start before had lapsed. Today's default
  binary (inode 401590725), this build twice, then today's binary again:

  | Start | Lines | records.json after it |
  | --- | --- | --- |
  | today's, the first boot | `registry ready (home served: true)` third, `+12 record(s), 0 unchanged`, `+10 record(s), 2 unchanged`, `ws-razel` serving (twice), `listening` | revision 5, 27 records; one claim on `home`, epoch 1, left to lapse 30 s after the boot |
  | this build, 1 | `+0 record(s), 12 unchanged` for each app, then **`registry ready (home served: true)`**, `peer`, `ws-razel` serving (twice), `listening` | revision 10, 31 records; three claims on `home` (the boot's, adoption's, one tick's), all epoch 1, live 28.7 s past the stop |
  | this build, 2 | the same | revision 15, 35 records; five claims on `home`, all epoch 1 |
  | today's, again | `registry ready (home served: false)` third, then as before | revision 19, 37 records; the same five claims on `home`, lapsed |

  Every start printed nothing on stderr, and the node and endpoint ids were
  the same throughout. `ws-razel`'s claims took a new epoch at each start, 1
  to 4, as a serve does. A signed renewal on `home` is 291 bytes in
  records.json (the first claim, at seq 0 with no `prev`, is 258), one on
  `ws-razel` 295; the served store's `home` journal grew 1,188 bytes over this
  build's first start, four records.
- **Downstream**, against the default binary (inode 401590725, not rebuilt),
  through the shims: client-rs 25 + 10, client-ts 48, grip-share 19, grazel
  29 + 3, glade-gyld 233 (1 ignored) + 33, glade-gwz 9 + 7. All at baseline.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +72/−21, net +51, of which 24 lines are code:
  `claims.rs` +51/−6, `bin/glade-node.rs` +13/−10, `lifecycle.rs` +8/−5;
- tests: +159/−13: `claims.rs` +104/−4, `tests/assembled_path.rs` +55/−9.

## Custody and the local overlay's check (plan Step 4.1c)

Design addition, 2026-09-25, written before the code against glade `01514b4`,
parked, and revised on 2026-09-26 against glade `a1f97ee` for the owner's rule
of no process globals (glade's `AGENTS.md`; `dev-docs/ProcessGlobalsPlan.md` at
the glade-wz root), whose Steps 2.1 and 4.1 landed in between. The red runs
and the measured figures were filled in afterwards. The rulings:
`GladeNodeSigning.md` D10, ruled (a), a recovery key; D7's
`glade/v1/local-overlay\0` tag; D11's 4.1c row, about 250 lines. The plan's
Step 4.1 asks that recovery material be "minted at first setup and written
where the operator names, offline", and its section 3 that 4.1 "mints recovery
material and stops": rotation stays 5.1's gap.

Nothing changes in the wire, the contracts or the dependencies. The IR gains
one record kind, and `sysdata.rs` is regenerated from it. The code is in two
new modules, `node/src/recovery.rs` and `node/src/overlay.rs`, with the boot's
calls in `sysdir.rs` and each root's lines.

### 1. The recovery key

- A separate Ed25519 key, from the operating system's randomness
  (`signing::random_seed`), never derived from `node.key`.
- **The commitment** is a new record kind in the node's own chain, on a new
  stream, `dir.recovery-keys`: `NodeRecoveryKey`, fields 1 `node` and 2
  `recovery_key`, each 64 lower-case hex digits. Like every `home` record it
  is sealed by the `origin-op` envelope (4.1b), which is what proves it. It
  carries no signature of its own: the transport binding's has one because
  it was minted before `home` records were signed.
- **The secret half**, the key's 32-byte seed, the form `node.key` holds, is
  written to the file the operator names and nowhere else. The node keeps no
  copy.
- **Committed** means the node's own chain holds a `NodeRecoveryKey` naming
  it (`Registry::recovery_key`). One per node: the command refuses a second.
  Replacing a lost one is rotation's (5.1).
- Nothing reads the key until rotation exists. The commitment only has to be
  made while the node key is trusted.

### 2. The one-shot command

`glade-node recovery --name <name> --out <path>`, on the stopped instance
`<root>/sys/<name>`. `<root>` is the instance root, `GLADE_HOME`, else
`$HOME/.glade` (`sysdir::instance_root`), which the binary reads once at its
entry point and hands to the command, as it hands it to a start. The command
reads no environment.

1. Both flags are required; anything else is refused with the usage.
2. A name with no instance (no `node.key`) is refused: the command commits for
   an instance that has booted.
3. `--out` is checked (section 3) before anything is written.
4. It boots the instance as a start does (`sysdir::boot_at`): it takes the
   instance lock, which a running node holds, so it is refused then ("stop the
   node first"). It loads and verifies records.json, and makes any write a
   start would.
5. If the node has already committed a key, it refuses, naming the key, and
   writes nothing.
6. It mints the key and writes the secret (section 3). Then it appends the
   commitment and saves records.json, in one acceptance (SP-L1).
7. It prints `node <id>`, then `recovery key <hex> committed; wrote its secret
   to <path>; this node keeps no copy: move the file offline now`, and exits
   0. The line names no mode: off Unix the file has none of its own.

- **The file is written before the commitment.** A crash between the two
  leaves a key that nothing commits, and the command can be run again with
  another path. It never leaves a commitment with no key. If the save fails,
  the command exits 1 and says the file is not known to be committed. It
  does not delete the file: a save whose outcome is unknown may have landed.
- **It starts no node**, so it runs before the composition root is chosen,
  and `GLADE_NODE_ASSEMBLED` does not apply to it.
- **The entry point reads the arguments once.** `start` reads them and hands
  them to the command or to the composition root it runs, where each root
  read its own. So the allowlist's permanent `env::args` entry counts one
  read, where it counted two.
- The served store takes the commitment at the next start's adoption
  (`seed_registry`), and peers pull it from there.

### 3. The file

- **The path is named on the command line only, and is absolute.** Resolving
  a relative one would read the working directory, which only a program's
  entry point may do; a relative path is refused. Its directory must exist,
  and symbolic links in the path are resolved.
- **Refused inside GLADE_HOME**, the instance root, compared by path
  components once both paths are resolved.
- **Never written over.** A path where anything exists is refused. The file is
  then created exclusively (`create_new`), so one that appears in between is
  refused too.
- **Mode 0600** on Unix, as `node.key`. Off Unix it gets default permissions
  and no check, as `node.key` does (F5).
- **Synced**, with its directory entry, before the commitment is saved.

### 4. At a first boot: `--recovery-out <path>`

- Both roots take `--recovery-out <path>` in the booted form. The path is
  checked as in section 3 before anything is written.
- At a first boot, the boot that mints the node's presence, the node mints the
  key, writes the secret, and appends the commitment in the same save as its
  presence. After `node`, both roots print the `recovery key ... committed`
  line of section 2.
- At a later boot the start is refused before records.json is written:
  `--recovery-out is taken at a node's first boot only, and <dir> has booted:
  stop it and run glade-node recovery --name <name> --out <path>`.
- The legacy form ignores it, as it ignores `--app` and `--peer`.

### 5. Until the commitment exists: the warning

- Each start of a booted node whose chain holds no commitment prints one line
  on stderr after its boot lines, then starts as before, so grazel is
  untouched. Both roots print it at the same point.
- It says exactly what to run, from the instance directory
  (`<root>/sys/<name>`, under the root the entry point handed down) and the
  path of the running program:

  `no recovery key is committed for this node: stop it, then run
  GLADE_HOME=<root> <program> recovery --name <name> --out <an absolute path
  outside GLADE_HOME>`

- **The program's path** is read once at the binary's entry point
  (`std::env::current_exe`, then its links resolved) and handed down: to
  `run` as an argument, and to the assembled root in `Settings.program`, as
  the instance root travels in `Settings.instance_root`. A caller that hands
  none, as a test's settings do, gets `glade-node` in its place. Read below
  the entry point, it would name whatever program the library runs in.
- A path holding a character a shell would split or expand is single-quoted.
- It sets `GLADE_HOME`, which overrides `HOME` (`sysdir::instance_root`), so
  the command reaches the same instance whatever `HOME` is.
- The desk: grazel runs the node with `GLADE_HOME=<data>/sys`
  (`grazel/src/lib.rs:236-241`), from grazel's directory, as
  `../glade/node/target/debug/glade-node`.

### 6. The local overlay's check (D7)

- `local.json` (class 3, never shipped) holds the canonical CBOR of a
  `SignedRecord`: `record` is the overlay, and `sig` is 64 bytes.
- **The overlay** is the canonical CBOR of a map of assertions numbered from 1.
  This build knows none, so the only overlay it applies is the empty map.
- **`sig`** is the node key's Ed25519 signature, checked strictly, over
  `glade/v1/local-overlay\0` then `record` (`Purpose::LocalOverlay`). A
  signature by another key, or made for another purpose, fails.
- **No local.json**: the fail-closed defaults, and nothing is said. Nothing
  writes one yet.
- **A local.json that fails any part** (unreadable, not that envelope, the
  signature, an assertion this build does not know, bytes not canonical):
  every assertion is discarded to its fail-closed default, the start goes on,
  and both roots print one stderr line: `<path>: <why>; its assertions are
  discarded to their fail-closed defaults`.
- Nothing writes local.json: the first assertion that needs one brings the
  writer. The check moves out of `sysdir.rs`, which is past 1,000 lines, into
  `overlay.rs`.

### 7. Compatibility

- **The desk** warns at each start until the owner runs the command (section
  5). Nothing else it prints changes.
- **A downgrade** to a build before this one, today's default binary
  (`a1f97ee`) included, still starts the instance until the command has run.
  Once the commitment exists, it refuses to start: `.../records.json holds a
  home record this build cannot read (dir.recovery-keys of node <id> at seq
  0): ...`. That is 4.1b part 2's hardening against records a build does not
  know (Measured).
- **An older peer** refuses the commitment when it is pushed or pulled (`not a
  directory stream`), and keeps the rest of the node's `home`. The desk has no
  peer.
- Four tests used `dir.recovery-keys` as a stream no build knew, through
  `envelope::testing::newer` and two fixtures of their own. They now use
  `dir.key-rotations`.

### 8. Tests, each begun red

Built on 2026-09-26 and 2026-09-27 against glade `a1f97ee` (`2a2cb12` changes
only the allowlist file). The eight tests use this step's API, so they were
run in two sources-only copies of the final tree, each with the part a test
guards switched off by one edit: six in the first copy, and in the second the
GLADE_HOME check and the first boot's mint, which in the first would have
turned another test red before its own part. The message is what each red run
printed. `<key>` stands for a recovery key.

| Test | Proves | Red first |
| --- | --- | --- |
| `overlay`: `local_json_is_taken_only_as_this_nodes_empty_overlay_under_its_tag` | the empty overlay, sealed by this node under the local-overlay tag, is taken; each flaw is refused for its own reason: a byte of the signature changed, another purpose's tag, another node's key, the overlay bare, an assertion this build does not know, an empty map encoded otherwise than canonically | with the signature not checked: "its signature is not this node's", `left: Ok(LocalOverlay)`, `right: Err("its signature is not this node's")` |
| `recovery`: `the_command_commits_the_key_and_writes_its_secret_only_where_named` | the command prints `node <id>` and the committed line; the next boot verifies the commitment in the node's own chain and does not warn; the file holds 32 bytes whose public key is the committed one; no file in the instance holds the secret; a second run is refused, naming the key, and writes nothing | with the commitment not saved: `left: None`, `right: Some("<key>")` |
| `recovery`: `the_command_refuses_any_other_place_or_instance_and_writes_nothing` | refused, each for its reason: a path inside GLADE_HOME, one inside the instance, a relative one, an existing file, a missing directory, a name with no instance, a flag missing, an unknown argument, and a running node ("stop the node first"); records.json and the offline directory are as they were | with the GLADE_HOME check off: `called Result::unwrap_err() on an Ok value`, the command having committed and written the secret inside GLADE_HOME |
| `recovery`: `a_first_boot_takes_recovery_out_and_a_later_boot_is_refused_it` | a first boot given a path inside GLADE_HOME is refused before anything is written; given one outside, it commits the key with its presence and writes the secret; a later boot given one is refused before records.json is written, and writes no file | with a later boot not refused: `called Result::unwrap_err() on an Ok value: ()` |
| `recovery::tests::unix`: `the_secret_is_written_0600` | the secret's mode | with no mode given: `left: 420`, `right: 384` (0644 and 0600) |
| `assembled_path`: `both_roots_warn_until_a_recovery_key_is_committed` | on each root, the warning word for word, naming the instance root the entry point read and the program's resolved path; the command's two lines and a 32-byte file; the next start says nothing of it | with no warning: the assertion on the hand-written root's stderr, which was empty |
| `assembled_path`: `both_roots_take_recovery_out_at_a_first_boot_only` | on each root: `instance`, `node`, `recovery`, `registry`, `peer`, `listening`, the third the committed line with the key records.json holds; no warning; a later start given the flag exits 1 with the refusal, and writes no file | with a first boot that mints nothing: `left: ["instance", "node", "registry", "peer", "listening"]`, `right: ["instance", "node", "recovery", "registry", "peer", "listening"]` |
| `assembled_path`: `both_roots_discard_a_local_json_that_fails_its_check` | on each root, a local.json holding `{}` is discarded, with the line naming it, and the node starts | with local.json never checked: the assertion on the hand-written root's stderr, which was empty |

Changed and passing:

- `tests/stop_signal.rs`'s two clean-stop tests and `tests/lifecycle.rs`'s
  linked-peer test allow the warning on stderr, as they allow the assembled
  root's name.
- `tests/instance_root.rs` passes no `--recovery-out` to `sysdir::boot`.
- The kind censuses name the new kind: `envelope`'s, and `tests/assembly`'s
  list of the streams the directory hosts.
- The four fixtures that named `dir.recovery-keys` as an unknown stream now
  name `dir.key-rotations` (section 7).

What they do not prove:

- A save that fails after the secret is written: the command's message, and a
  first boot's file left unused.
- Off Unix: the file's permissions (F5), and the directory sync, a no-op
  there.
- Single-quoting: no test path holds a character a shell would split.
- A relative GLADE_HOME, which the check resolves as the file system does.
- The warning as grazel forwards it, prefixed `[node] `: the replay runs the
  node alone.
- An older peer taking the commitment.
- Windows and Linux.

### Named gaps (4.1c)

- **Nothing reads the recovery key.** Rotation is 5.1's gap, and until it
  exists a lost key cannot be replaced: the command commits one per node.
- **An unused secret file.** A save that fails after the secret is written
  leaves a file nothing commits, and the command says so. A first boot that
  fails at that point leaves one too, and the next first boot given the same
  path is refused, since the file exists.
- **The secret is not zeroed in memory**; the command's process ends at once.
- **The command boots as a start does**: it makes the writes a start would
  (a set-aside, a binding) and prints none of a start's lines.
- **`current_exe` is outside the checker's list.** It is read once, at the
  entry point, and the process-globals checker has no pattern for it (the
  plan's section 6 names such blind spots).
- **An older build cannot start an instance** that has committed a key.
- **Nothing writes local.json**, so the check's accepting path is reached only
  by tests. An assertion this build does not know discards the whole file:
  per-assertion defaults come with the first assertion.
- **The budget.** D11 estimated about 250 lines, tests included; this step is
  about 1,000 (Measured), about half of it tests. The process-globals rule
  added the entry-point plumbing and the absolute-path check.

### Default-path changes (4.1c)

1. A booted start whose node has committed no recovery key prints one line on
   stderr after its boot lines, and starts as before. The desk prints it at
   every start until the owner runs the command; grazel forwards it as
   `[node] no recovery key is committed for this node: ...`.
2. `glade-node recovery --name NAME --out PATH` runs the command. A first
   argument `recovery` no longer starts a node.
3. `--recovery-out PATH` is a flag of the booted form. It was two positionals.
4. A local.json that fails its check prints one line on stderr. The desk has
   none.
5. After the command, a start says nothing of it, records.json holds one more
   record (314 bytes, on `dir.recovery-keys`), and a build before this one
   refuses the instance.
6. The binary reads its arguments once, in `start`, where each root read its
   own, and its own path once, as its roots start. The allowlist's `env::args`
   entry counts one read; `env::var`'s reason names the command.

### Questions for the owner (4.1c)

1. **An absolute `--out`** (section 3). Recommend as built: the rule keeps the
   working directory at the entry point, and the operator names an offline
   place anyway. The other choice reads the working directory once at the
   entry point (`env::current_dir`, a new permanent entry) and resolves a
   relative path against it.
2. **One recovery key per node** (section 1). Recommend as built: replacing a
   lost key needs rotation's rules (5.1). The other choice lets a later
   commitment supersede the first, which needs a rule for which counts before
   anything reads them.
3. **The file holds the bare 32-byte seed**, as `node.key` does. Recommend as
   built; 5.1 can wrap it. The other choice is a self-describing file naming
   the node and the public key.
4. **`--recovery-out` at a later boot** (section 4) is refused, as the ruling
   says "at first boot". The other choice takes it whenever the node has no
   key, which the command already does.
5. **When the desk runs the command**: once the owner means to stay on this
   build, since an older one then refuses the instance (section 7).

**Ruled, owner, 2026-09-27 ("all recommended"):** 1 `--out` stays absolute; 2 one recovery key per
node; 3 the file holds the bare 32-byte seed; 4 `--recovery-out` is taken at a first boot only; 5 the
owner runs the command on the desk once he means to stay on this build.

### Measured (4.1c)

2026-09-27, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** passes all 9 components, in 94 s from an empty scratch target,
  with 326 node tests on each path, across 16 test binaries, where there were
  318: the eight new tests.
  - rustfmt: glade-node 295 hunks, below its baseline of 296. The `use` block
    this change rewrote in `registry.rs` was one of the 296; no line it wrote
    is a deviation. glade-wire 43.
  - clippy: glade-node 11 warnings and glade-wire 7, at their baselines.
  - process-globals: 51 files, 3 permanent entries, 0 debt, nothing new.
  - The contracts gate passes, untouched.
- **The regeneration.** The generator at taut `7a5f616` reproduces
  `a1f97ee`'s `sysdata.rs` byte for byte; after the IR change, `--legacy-codec`,
  it adds the new kind's 20 lines and nothing else.
- **Time.** The recovery and overlay unit tests take about 0.03 s. Each
  both-roots test takes about 1.1 s.
- **The replay**, on a stand-in laid out as grazel lays out the desk: data
  directory `$S/desk/instances/5173`, `GLADE_HOME=<data>/sys`, the instance
  `<GLADE_HOME>/sys/grazel`, started from grazel's directory with the desk's
  two app files (`--profile local --name grazel --app apps/grazel-app.glade
  --app apps/gyld-app.glade 0`). This build ran by a relative path, as grazel
  runs the desk's binary. Each start lived 11 s past `listening` and was
  stopped. `$S` is the scratch directory:

  | Start | Lines | stderr |
  | --- | --- | --- |
  | today's default binary (inode 404917081), the first boot | `registry ready (home served: true)` after the two `app` lines (`+12`, `+10 record(s), 2 unchanged`), `ws-razel` serving twice, `listening` | nothing |
  | this build, 1 and 2 | the same, `+0 record(s), 12 unchanged` for each app | one line, the warning: `no recovery key is committed for this node: stop it, then run GLADE_HOME=$S/desk/instances/5173/sys $S/bin/glade-node recovery --name grazel --out <an absolute path outside GLADE_HOME>`, the binary's path resolved from the relative one it ran by |
  | the command, as the warning says, `--out $S/offline/grazel-recovery` | exit 0: `node <id>`, `recovery key <key> committed; wrote its secret to $S/offline/grazel-recovery; this node keeps no copy: move the file offline now`; the file `-rw-------`, 32 bytes | nothing |
  | the command again | exit 1: `node <id> has committed recovery key <key> already, and commits one`; no second file | that line |
  | this build, after the command | the same lines | nothing |
  | today's binary, after the command | refused, exit 1 | `.../records.json holds a home record this build cannot read (dir.recovery-keys of node <id> at seq 0): its format is newer than this build's, or it is damaged; start the build that wrote it, or move .../records.json aside` |

  records.json then held 42 records at revision 22, one of them the
  commitment: 314 bytes, seq 0 of the node's own `dir.recovery-keys` chain.
- **Downstream**, against the default binary (inode 404917081, not rebuilt):
  client-rs 25 + 10 + 1, client-ts 48, grip-share 19, at baseline. grazel's,
  glade-gwz's and glade-gyld's suites were not run, as asked.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +551/−57, net +494, about 340 of the added lines code, the rest
  doc comments: `recovery.rs` +263, new; `overlay.rs` +90, new;
  `bin/glade-node.rs` +74/−15; `sysdir.rs` +46/−27; `sysdata.rs` +20
  (generated); `registry.rs` +20/−2; `lifecycle.rs` +16/−3; `envelope.rs`
  +10/−9; `assembly.rs` +10/−1; `lib.rs` +2;
- tests: +449/−19: `recovery.rs` +212, `tests/assembled_path.rs` +151/−2,
  `overlay.rs` +56, `envelope.rs` +11/−3, `tests/stop_signal.rs` +9/−8,
  `tests/lifecycle.rs` +4/−1, `tests/assembly/main.rs` +3/−2,
  `tests/instance_root.rs`, `sysdir.rs` and `store.rs` +1/−1 each;
- beside them, the IR +11 and the allowlist +3/−3.

## Relay configuration and the first crossing (plan Step 4.5)

Design addition, 2026-09-27, written before the code against glade `1ce4b46`
(glade-wz root `9aa40d3`). The red runs and the measured figures are filled in
afterwards. The spec is plan Step 4.5 with its run notes, and these rulings:

- `relay_posture = community_dev_only` (2026-09-22): n0's relays for the slice,
  `RelayMode::Default`, with `Custom(RelayMap)` the later switch; no
  address-lookup service; peers named directly; "the relay is a configuration
  value".
- The owner, 2026-09-24: no NAT or firewall checks; the two LAN machines, the Pi
  and dabeest, with n0's relays configured; the path iroh picks is noted, not
  measured.
- The owner, 2026-09-25, on 4.2b's question 3: 4.5's configuration prints a
  node's endpoint id without serving. Since the door, the accepting node must
  name the dialer's key, so both machines need each other's id before either
  starts.
- The portmapper note (glade `1192bf2`): off since the loopback fix; 4.5 decides
  it with the bind address and the relay mode.
- The owner, 2026-09-25, on 4.2c: the mesh moves onto the carrier port in 4.5b,
  not here. So the mesh stays on `PeerEndpoint`, and the configuration reaches
  both.

4.2 has landed (4.2a, 4.2b, 4.2c), so the plan's alternative, a crossing run
before the door on a private id, does not arise: the door is the lock the relay
ruling asked for. The plan's rule stands anyway: endpoint ids stay in
configuration files at 0600 and out of logs.

Nothing changes in the wire, the IR, the contracts or the dependencies: no
crate is added, and no `Cargo.lock` line moves.

**The step comes in two parts and a run.**

- **Part 1, the configuration** (sections 1 to 7): the file, the endpoint's
  recipe, the portmapper, the `endpoint-id` command, and endpoint ids out of
  logs.
- **Part 2, the notes** (section 8): the lines a node prints about its relay,
  its links' paths and its `home` rounds, which the crossing reads.
- **The crossing** (sections 9 and 10), on the Pi and dabeest, after part 2.

### 1. What does not change: every profile, and the owner's desk

The desk runs grazel, which starts the node as `--profile local --name grazel
--app apps/grazel-app.glade --app apps/gyld-app.glade 9099`, with
`GLADE_HOME=<data>/sys` (`grazel/src/lib.rs:256-271`, `src/main.rs:108-111`).
It passes no `--peer`, and it will pass no `--config`.

**No file means today's network, on every profile.** A profile picks the
default instance name and nothing else, as `sysdir.rs` says ("a deployment
label only"). The network comes from the file alone.

| Profile | Default instance | With no `--config` |
| --- | --- | --- |
| `local` | `glade-local` | the endpoint binds `127.0.0.1:0` alone; relays off; portmapper off; no address lookup; peers only from `--peer` |
| `peer` | `glade-peer` | the same |
| `server` | `glade-server` | the same |

- For that default, the endpoint builder's calls are today's, call for call:
  `presets::Minimal`, the key, the ALPN, the door's hook, the portmapper
  disabled, the IP transports cleared, `bind_addr(127.0.0.1:0)`. No relay call
  is made, so iroh has no relay transport, and its net report has no relay to
  probe.
- The websocket listener stays on `127.0.0.1:<port>`, whatever the file says.
  The file configures the iroh endpoint only.
- So the desk still binds loopback alone and contacts no relay. Its one visible
  change is a log line: the `peer` line names the endpoint by a tag, not the
  full id (section 7).
- Tests pin it (section 11): the default is loopback with relays off; the
  recipe maps `off` to `RelayMode::Disabled`; no production code names iroh's
  environment readers; `an_endpoint_listens_on_loopback_alone` stays.
- The replay before the desk's restart runs the rebuilt binary on a stand-in
  laid out as grazel lays out the desk, and lists its sockets with `lsof`: UDP
  and TCP on `127.0.0.1` alone, and no TCP connection leaving the machine.
- No test binds beyond loopback, and no test contacts a relay. So the Mac's
  firewall has nothing to ask, and n0 hears nothing from the Mac.

### 2. The configuration file

**Named on the command line.** The booted form takes `--config <path>`. The
path must be absolute: resolving a relative one would read the working
directory, which only a program's entry point may do (4.1c's rule for
`--out`). The legacy form ignores the flag, as it ignores `--peer`.

**Loaded before anything is written.** Both roots load the file after the
`--app` files and before the instance boots. A file that cannot be read, is
readable by others, or has one bad line refuses the start with exit 1, as a
bad app file does. The instance is not created and nothing is written.

**The 0600 rule.** On Unix the file is refused if group or others have any
access (`mode & 0o077`), with the message `<path> is group/world-accessible
(mode 644) — refusing`, as for `node.key`. This holds whatever the file
contains. Off Unix there is no check (F5), as for the key files. The check
sits in a braced platform module of its own, as `sysdir.rs`'s does.

**The format** is lines, like the app files. A `#` starts a comment that runs to
the end of its line, and blank lines are skipped. There are three keywords:

| Line | Means | At most | When absent |
| --- | --- | --- | --- |
| `relay off` | no relay: `RelayMode::Disabled` | one `relay` line | `off` |
| `relay n0` | n0's production relays: `RelayMode::Default` | | |
| `bind <ip:port>` | the endpoint binds this socket address; port 0 lets the OS choose | one IPv4 and one IPv6 | `127.0.0.1:0` alone |
| `peer <endpoint-id>` | admit this key on first contact; dial nothing | | no peers |
| `peer <endpoint-id>@<ip:port>` | admit it, and dial it at that address | | |
| `peer <endpoint-id>@<relay-url>` | admit it, and dial it through that relay | | |

- An endpoint id is 64 lower-case hex digits, and a key iroh accepts as an
  Ed25519 point. That is checked at load, with iroh's own type.
- A relay URL is written as the node prints it, for example
  `https://aps1-1.relay.n0.iroh.link./`. It must be one of the four relays
  `relay n0` names: n0's production map, as iroh defines it
  (`defaults::prod`). A peer's relay URL needs `relay n0`. Under `relay off`
  it is refused, since the endpoint would have no relay to send through.
- Lines naming one endpoint id merge into one entry: admitted once, and dialed
  once at every address they name. iroh then chooses among them.
- `--peer` flags take the same three forms and join the file's entries after
  them. A malformed flag now refuses the start at load, where it printed a
  line and was skipped.
- Anything else refuses the start: an unknown keyword; a second `relay` line;
  `relay` with any other word; a second IPv4 or IPv6 `bind`; a bad socket
  address, id or URL.
- `relay` takes no URL list. `Custom(RelayMap)` is the ruling's later switch,
  and waits for a relay of our own (question 6).

**Messages never echo a line.** An error names the file and the line number,
`<path>: line 3: expected <endpoint-id>, <endpoint-id>@<ip:port> or
<endpoint-id>@<relay-url>`. A bad `--peer` flag is named by its place,
`--peer entry 2: …`. So no message prints an id.

An example, the Pi's file for the crossing's run 2 (section 10):

```text
# plan Step 4.5, run 2: n0's relays, the Wi-Fi address, dabeest admitted
relay n0
bind 10.1.1.236:4545
peer <dabeest's endpoint id>
```

### 3. Where each value comes from

The node reads nothing from the environment below its entry point (glade's
`AGENTS.md`, "No process globals"; `scripts/checks/check_process_globals.py`).
The file path arrives with the arguments, which `start` reads once. So the
allowlist does not change: `env::args` stays one read, and `env::var` two.

| Value | Comes from | With no file | Read |
| --- | --- | --- | --- |
| relay mode | the file's `relay` line | off | by the root, before boot |
| bind addresses | the file's `bind` lines | `127.0.0.1:0` | by the root, before boot |
| peers | the file's `peer` lines, then `--peer` flags | none | by the root, before boot |
| the file's path | `--config`, among the arguments | none | once, in `start`, the entry point |
| the instance root | `GLADE_HOME`, else `$HOME/.glade` | as today | once, at the entry point |
| the endpoint key | `<instance>/endpoint.key` (4.2a) | minted at the first boot, or by `endpoint-id` (section 6) | by the boot, or the command |
| portmapper | nowhere: always off (section 5) | off | never |
| address lookup | nowhere: none | none | never |
| proxies for iroh's builder | nowhere | none | never |

**What the node never asks iroh to do**, because each reads the environment or
adds a lookup service:

- `presets::N0` adds n0's pkarr and DNS lookups, which the ruling excludes. It
  also sets its relay mode through `default_relay_mode()`.
- `default_relay_mode()` and `force_staging_infra()` read
  `IROH_FORCE_STAGING_RELAYS` (iroh 1.2 `src/endpoint.rs:2030-2047`). The node
  names `RelayMode::Default` itself.
- `Builder::proxy_from_env()` reads `HTTP_PROXY`, `http_proxy`, `HTTPS_PROXY`
  and `https_proxy` (`:1871-1901`).

A source test keeps these names out of `src/` (section 11). grazel hands the
node its whole start-up environment, so this matters: a variable in the
owner's shell must not steer the node's relays.

**What iroh's dependencies still read.** The process-globals checker scans
glade's code, not its dependencies (`dev-docs/ProcessGlobalsPlan.md` §6). Two
reads are therefore named gaps:

- With relays on, iroh's net report makes its HTTPS probes and captive-portal
  check with `reqwest` clients that iroh gives no proxy. `reqwest` then reads
  the system proxy as it builds each client (through `hyper-util`'s
  `Matcher::from_system`): `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`,
  `NO_PROXY`, their lower-case forms and `REQUEST_METHOD`, and on macOS and
  Windows the system's proxy settings.
- Every bind builds iroh's DNS resolver from the system's DNS configuration
  (`with_system_defaults`). On Windows, it finds the hosts file through
  `SystemRoot`. With relays off the resolver resolves nothing. With `relay
  n0` it resolves the relay hosts.

### 4. `ConfigPort` and the endpoint's recipe

**The shape.** The file is parsed into a carrier-free value in a new module,
`node/src/netconf.rs`. No iroh type crosses it (LBT-004), and `ConfigPort`
hands it out:

```rust
pub struct Network {
    pub relays: Relays,          // off unless the file says `relay n0`
    pub bind: Vec<SocketAddr>,   // at most one per family; [127.0.0.1:0] by default
    pub peers: Vec<PeerEntry>,   // the file's, then --peer's, one per key
}
pub enum Relays { Off, N0 }
pub struct PeerEntry { pub key: [u8; 32], pub via: Vec<Via> }  // no via: admit only
pub enum Via { Ip(SocketAddr), Relay(String) }                 // the URL as checked at load

pub trait ConfigPort: Send + Sync {
    fn settings(&self) -> &Settings;
    fn network(&self) -> &Network;   // new
}
```

- `Settings` gains `config`, the `--config` path, and `network`, which the root
  fills by loading the file before it builds `NodeStart`. `Settings.peers`
  stays the raw `--peer` flags, which the load merges in. `CommandLine` still
  reads nothing: it hands back what the root gave it. As built, `network()`
  is a provided method of the port, `&self.settings().network`, so no
  implementor changes.
- 4.2b's `PeerEntry` in `iroh_carrier.rs` gives way to this one. `PeerAddr`
  stays the endpoint's own dialable address, and converts into a dial target
  with one `Via::Ip`, so the tests that dial a `PeerAddr` are unchanged.
- `ConfigPort` stays node-local. The 3.2 note said it would move to
  `glade/contracts` at 4.5, when a second consumer needed it. Its consumers
  are the two roots and the iroh adapter, all in glade-node, so there is still
  no second crate (question 5).

**The recipe.** `bind_endpoint` (`iroh_carrier.rs:75-94`) takes the network
with the key, the door and the ALPN:

- `presets::Minimal`, the key, the ALPN and the door's hook, as today;
- `portmapper_config(PortmapperConfig::Disabled)`, always (section 5);
- `clear_ip_transports()`, then `bind_addr(addr)` for each `bind` address, so
  nothing binds that the file does not name, and no pre-bound `[::]` returns;
- `relay_mode(RelayMode::Default)` for `relay n0`, and no relay call for
  `off`;
- no `address_lookup`, no `proxy_url` and no `net_report_config`, so the net
  report keeps iroh's defaults (question 7).

**The dial.** A dial target becomes `EndpointAddr::from_parts(id, addrs)`, with
`TransportAddr::Ip` for an address and `TransportAddr::Relay` for a relay URL.
Before a path is chosen, iroh sends a connection's first packets to every
address it knows (`remote_state.rs`, "sending datagram to all known paths").
So a target naming only a relay URL makes the connection begin through that
relay. The door's hook and HELLO's exporter bytes work unchanged over a relay
path: it is the same QUIC connection.

**The `peer` line's address** is the address as bound, from `bound_sockets()`,
IPv4 first. `loopback_addr` (`:129-138`) rewrote it to `127.0.0.1`, which is
wrong for any other bind. For the default bind the line prints the same address
as before.

**The iroh adapter** (4.2c) binds the socket that `CarrierConfig::local` names,
as `<ip:port>` or as `<endpoint-id>@<ip:port>`. It ignores the id: the address
names where, and the lent key says who. That makes CA-004's re-bind real on
iroh, where it was vacuous (4.2c's named gap). The adapter gets no relay mode
here, and no root lends it a key. Both wait for 4.5b.

- **As built, its `close` also waits for the address** (not in the design
  above). iroh lets a closed endpoint's sockets go a few milliseconds after
  its last handle drops, and gives no signal when it has. With the re-bind
  real, CA-004's `fresh` port was refused, `Failed to bind sockets`, in 21 of
  25 runs of the module. So `close` resolves once each socket it bound can be
  bound again, within `LINGER` (3 s), binding each address itself to see:
  the only witness iroh leaves (`released` in `iroh_carrier.rs`).

### 5. The portmapper

**Decision: off in every configuration, with no switch in the file.**

- On one LAN it adds nothing. Both machines sit behind one router, and iroh
  reaches the other side's Wi-Fi address directly.
- It asks the router, over UPnP, PCP or NAT-PMP, to open an external port to
  the node. That exposes the node beyond the LAN, and the owner deferred NAT
  work.
- It opens a UDP socket on every interface and multicasts SSDP. That is what
  raised the Mac's firewall dialogs (iroh's own documentation says so,
  `src/portmapper.rs:26-31`).
- The cost is direct paths across NATs that iroh's hole punching alone cannot
  open. That belongs to the two-network check after Phase 4.

The alternatives are in question 3: a `portmapper on` line, or compiling it
out.

### 6. Printing a node's endpoint id without serving

`glade-node endpoint-id --name <name>` is a one-shot command beside
`recovery`. It is chosen by its first argument, runs before a composition root
is chosen, and starts no node. It takes the instance `<root>/sys/<name>`,
under the root the entry point read.

- **The key exists:** the command checks its mode, as the boot does, reads its
  32 bytes and prints the id. It takes no lock and writes nothing, so it works
  while the node runs. The key cannot change under a running node, since
  replacing one means stopping the node first (4.2a, section 5).
- **No key yet:** the command creates the instance directory, takes the
  instance lock, and mints `endpoint.key` at 0600 from the OS's randomness,
  with the boot's own helper. It then prints the id and releases the lock. The
  lock is refused only if a node holds the instance while its key is missing,
  and then the command says to stop the node.
- **It mints nothing else**: no `node.key`, no records.json. The first start
  then binds the key it finds, as a boot binds any key its node has not bound
  (4.2a, section 5).
- **Output:** exactly one line on stdout, the 64 hex digits, so a pipe can carry
  it. A refusal goes to stderr, with exit 1.
- **Where it lives:** a new module, `node/src/endpoint_id.rs`, over
  `sysdir.rs`'s key and lock helpers made `pub(crate)`. `sysdir.rs` is past
  1,000 lines.
- A first argument `endpoint-id` no longer starts a node. The legacy form's
  first positional is a port, so nothing that ran before is lost.

**How the Pi and dabeest exchange ids before their first start.** Each machine
mints its key with the command, then the lane owner pipes each id from one
machine into a file on the other, through the Mac. The names are section 10's,
written out for each machine, not expanded on the Mac:

```text
$PI  "GLADE_HOME=$S/home $B endpoint-id --name pi45"  | $DAB "umask 077; cat > $S/pi45.id"
$DAB "GLADE_HOME=$W/home $B endpoint-id --name dab45" | $PI  "umask 077; cat > $S/dab45.id"
```

An id is then on its own machine and in a file on the other, and in the Mac's
pipe for a moment. It never reaches a terminal, a log or the lane owner's
transcript. The two ends are compared by tag: the first 10 hex digits of the
file on one machine, against the command's output cut to 10 on the other. Each
machine then writes its configuration file with `umask 077`, taking the id from
the file with `$(cat …)`.

### 7. Endpoint ids out of logs

**The tag.** A line that must name an endpoint names it by the first 10 hex
digits of its id. That is iroh's own short form (`PublicKey::fmt_short`).
Ten digits are enough to match a refusal to a line of the file, and no use for
dialing. The full id comes only from `endpoint-id`, the files and the store.

| Line | Before | After |
| --- | --- | --- |
| the endpoint (both roots) | `peer <endpoint-id> 127.0.0.1:<port>` | `peer <tag> <address as bound>` |
| a refusal (the door) | `peer refused: endpoint <endpoint-id>: <reason>` | `peer refused: endpoint <tag>: <reason>` |
| a failed dial | `peer <the --peer text>: <error>` | `peer <tag>@<ip:port or relay-url>: <error>` |
| a bad entry | `peer <the --peer text>: expected …` | `--peer entry N: expected …`, or `<path>: line N: …` |
| `EndpointKey`'s `Debug` | the id | the tag |

- `peer-connected <node-id>` and every line naming a node id are unchanged.
  Node ids are not the relay's lock.
- The tests that dialed through the `peer` line (`tests/assembled_path.rs`,
  `tests/lifecycle.rs`, `tests/stop_signal.rs`) take the full id from
  `endpoint-id`, or from an in-process boot as they already do for the dialer,
  and the address from the `peer` line.
- **Still holding the id, by design:** the configuration files (0600), the
  `*.id` files of section 6, `endpoint.key`, and records.json with the served
  store's `home` journal. The last two carry the binding record (4.2a). They
  replicate only to linked peers, and they take the umask's mode (named gaps).

### 8. The notes the crossing reads (part 2)

A node that is linked, or has relays on, notes on stdout what iroh does. These
are ordinary status lines, like `peer-connected`: on the hand-written root they
go to stdout, and on the assembled one to the console's `out`. The mesh gets
this reporter beside the door's refusal reporter. A node with no peers and
relays off prints none, so the desk prints none.

| Line | When | Printed by |
| --- | --- | --- |
| `relay <url>` | a home relay connects, or the home relay changes | a node with `relay n0` |
| `relay <url> not connected: <error>` | its connection fails or drops | the same |
| `link <node-id> via relay <url>, rtt <n> ms` | at HELLO, and whenever iroh selects another path | both ends of a link |
| `link <node-id> via direct <ip:port>, rtt <n> ms` | the same | the same |
| `link <node-id> closed` | the link's connection ends | both ends |
| `home round with node <id>: <n> record(s) in <ms> ms` | this node's pull from that peer ends | both ends |

- **iroh's types stay in the adapter** (`IrohGladeMapping.md` §7.6).
  `iroh_carrier.rs` describes a path, a selected `TransportAddr` and its RTT
  estimate, and reads the home relay's status (`Endpoint::home_relay_status`,
  a `Watcher` iroh re-exports). The mesh sees only text and numbers.
- **The link's watch** rides the existing `Site::Unlink` task
  (`mesh.rs:289-294`). It already waits for the connection to close. It now
  also reads `Connection::paths()` every 250 ms and notes a change of the
  selected path. iroh's change stream, `path_events`, needs the `Stream` trait
  from a crate the node does not depend on directly. A poll needs none.
- **The relay's watch** is one task per endpoint, a new `Site::RelayWatch`
  owned by `Sessions`. It is spawned only with `relay n0`.
- **The home round** is timed around `pull_home` (`:603`) with `Instant`. The
  dialer's `peer-connected` still follows its own pull.
- A `link … closed` line soon after the peer's stop shows a close that reached
  the peer. One about 30 s later shows a peer that died, since that is iroh's
  idle limit on a relay path (15 s on a direct one).

**As built** (part 2):

- The reporter rides the door, beside its refusal reporter:
  `Door::with_status(sink)`, and `Door::status(line)`, which the mesh calls.
  The hand-written root points it at stdout, the assembled root at its
  console's `out`; a door built without it, as the tests' are, notes
  nowhere.
- A note stdout cannot take is dropped. The notes come after `listening`,
  which is where a parent may stop reading, and `println!` panics on a
  closed pipe: in the first full run, an acceptor whose stdout the test no
  longer read panicked in its link's task, and its dialer's round failed.
  The hand-written root's sink and the assembled root's `StdConsole::out`
  now write with `writeln!` and drop an error. grazel drains the node's
  stdout, so the desk loses nothing.
- The `link` note at HELLO needs a path iroh has selected by then; if none
  is yet, the link's watch notes it within 250 ms. On loopback both ends
  had one at once.
- `n` in `home round` is the records the round took (`SyncOutcome::applied`),
  noted when the pull ends well. A failed pull notes nothing: the dialer
  reports its failed dial, as before.
- The relay's watch holds no handle on the endpoint (iroh's watcher and its
  close future do not), so it keeps no socket bound; it ends when the
  endpoint closes, or with `Sessions` on the assembled root. It notes a
  state change, not every retry: the same error twice is one line.

### 9. What n0's relays can see

This comes from the protocols (iroh 1.2 and iroh-relay 1.2) and is not observed.
It is set out in the plan's terms, with what goes beyond them. "The relay" is
the node's home relay unless a line says otherwise.

**Endpoint ids.**

- Each node's own id, proved by a signature when it connects to its home relay
  (`iroh-relay` `protos/handshake.rs`).
- For every datagram it relays: the sender's id and the receiver's id. So n0
  knows which ids talk to which: that the Pi's endpoint and dabeest's talk.
- A dialer that names a relay other than its own home relay connects to that
  relay too, and shows it the same.
- `endpoint.key` is stable across starts (4.2a). So n0 can follow a node by its
  id across restarts and networks. The crossing's keys are made for the run and
  deleted after it.

**IP addresses.**

- The home relay sees the public address and TCP port of the node's relay
  connection, a WebSocket over TLS on 443.
- Every relay the net report probes sees the node's public address. Through
  QUIC address discovery (QAD, on its own ALPN and with no id), it also sees
  the node's public UDP address and port, the NAT mapping, and tells the node.
- On one LAN, both nodes show the same public address, so n0 can tell they sit
  together.

**Timing.**

- When each node connects to and leaves its relay, with a ping every 15 s
  (`relay/actor.rs:74`).
- The time of every relayed datagram.
- The net report runs every 20 to 26 s (`socket.rs:1999-2003`). At start, and
  in a full report every 5 minutes (`net_report.rs:132`), it probes all four
  relays, over QAD and HTTPS. In between it reads the node's address from the
  one QAD connection it keeps open, with a keep-alive every 25 s.

**Volume.**

- The size and count of every relayed datagram, each way.
- Once a direct path is selected, the link's data leaves the relay. But the
  relay path stays open beside it, and iroh pings a path idle for 5 s
  (`socket.rs:109`). So n0 still sees a small datagram between the two ids
  every few seconds for as long as the link lives.

**Beyond the four:**

- A connection's first packets, when they cross the relay, are protected only by
  keys any observer can derive (QUIC, RFC 9001 §5.2). The relay can therefore
  read the TLS ClientHello they carry, and in it the ALPN, `glade/node/3`: that
  these two ids speak Glade's node protocol, version 3. iroh stopped sending
  the SNI after 1.0.3, so nothing else in it names Glade.
- The captive-portal check, at the first report, is a plain `GET
  http://<relay-host>/generate_204` with a header `X-Iroh-Challenge:
  ts_<host>`. It carries no id, but anyone on the path sees it.
- The relay hosts are resolved through the machine's resolver. The authoritative
  servers for `iroh.link` see the resolver's lookups.

**Not visible to n0:** node ids, HELLO, the binding that ties an endpoint id to
a node id, every `home` record and share, and app data. All of these ride
TLS 1.3 inside QUIC, keyed to the two endpoint keys.

### 10. The crossing on the Pi and dabeest

**The machines**, as read on 2026-09-27 (read only):

| | the Pi | dabeest |
| --- | --- | --- |
| reached by | `ssh -o BatchMode=yes gianni@10.1.1.236`, with Gianni's agent socket as `SSH_AUTH_SOCK` | `ssh -o BatchMode=yes -o ClearAllForwardings=yes gianni@dabeest` |
| system | Raspberry Pi 5, Debian 13 aarch64, 4 cores, 7.9 GiB | Windows 11, MinGW bash (MSYS) |
| Rust | 1.96.0 | 1.98.1, with `CARGO_HOME=/c/Users/gianni/.cargo RUSTUP_HOME=/c/Users/gianni/.rustup` |
| work area | `~/git/glade-wz` (glade at `1192bf2`) | `/e/git/glade-wz` (glade at `63a5799`) |
| address | `wlan0` `10.1.1.236/16`, gateway `10.1.1.1`; IPv6 unique-local only | Wi-Fi `10.1.1.239`, gateway `10.1.1.1`; also WSL `172.19.80.1` and an idle Tailscale adapter |
| space | 9.3 GB free (84% used) | 904 GB free on `E:` |
| clock | NTP synchronised | within a second of the Pi and the Mac |
| UDP 4545 | free | free |
| helpers | `python3`, `timeout`, `ss` | `python` (Python 3.13 in `AppData/Local/Programs`), `timeout`, `taskkill`, `tasklist` |

Addresses come from DHCP and are read again at the run.

**Roles.** The Pi accepts and dabeest dials, in every run. The Pi admits
dabeest's key, and dabeest names the Pi's key and the Pi's relay. The Pi is
the one stopped cleanly, because the assembled root stops on SIGTERM there. On
Windows it stops on Ctrl-C alone, which an ssh session cannot send, so
dabeest's node is always ended by force. Both run the assembled root
(`GLADE_NODE_ASSEMBLED=1`) as profile `peer`, with instances `pi45` and
`dab45`.

**Two runs.**

- **Run 1, the relay alone.** Both nodes bind `127.0.0.1:0` with `relay n0`. A
  loopback socket cannot reach the other machine, so HELLO, the round and
  everything after cross n0's relay. That is the plan's "through the relay",
  with nothing listening on the LAN. QAD cannot leave a loopback socket, so
  each node should find its relay through the HTTPS probes. If one finds
  none, run 1 is recorded as not possible and run 2 stands alone.
- **Run 2, iroh's choice.** Both bind their Wi-Fi address on UDP 4545, with
  `relay n0`, and dabeest still dials through the Pi's relay alone. iroh may
  then find the LAN path by hole punching. The run notes whether it does and
  when, as the owner ruled.

**Names used below.** `$PI` and `$DAB` are the two ssh commands above. On the
Pi, `S=$HOME/git/glade-wz/scratch/4.5` and `B=$S/target/debug/glade-node`. On
dabeest, `S=/e/git/glade-wz/scratch/4.5`, `W=E:/git/glade-wz/scratch/4.5` and
`B=$S/target/debug/glade-node.exe`, and each command starts with the two Cargo
variables and `PATH=/c/Users/gianni/.cargo/bin:$PATH`. A path given to the
native Windows binary is written `E:/…`, never `/e/…`. Everything the run makes
is under `$S`, and nothing else on either machine is touched: not the rest of
`scratch/`, not the siblings' checkouts.

**0. Preconditions (read only).** Both machines answer. Their clocks agree with
the Mac's within a second (`date -u +%s.%N`). A binding is judged at the
reader's clock with no margin (4.2a, section 4), so skew matters from run 2
on. UDP 4545 is free on both (`ss -lunH 'sport = :4545'`; `netstat -ano -p udp
| grep ':4545 '`). No `glade-node` process runs on either (`pgrep -af
glade-node`; `tasklist //FI "IMAGENAME eq glade-node.exe"`).

**1. The code.** Once part 2 has landed, the lane owner pushes glade's `main`
to GitHub under the owner's standing rule and notes the commit `<sha>`. Each
machine runs `git -C <its glade> pull --ff-only`, and `git rev-parse HEAD` must
print `<sha>`. Only glade is pulled.

**2. Build, then the node suite.** Both use a target of their own under `$S`:

```text
Pi:      cd ~/git/glade-wz/glade && mkdir -p $S \
         && CARGO_TARGET_DIR=$S/target cargo build --locked --manifest-path node/Cargo.toml --bin glade-node \
         && CARGO_TARGET_DIR=$S/target cargo test --locked --manifest-path node/Cargo.toml
dabeest: the same from /e/git/glade-wz/glade, the build and then the suite, one heavy job at a time
```

Expected: every suite passes on the Pi, which holds the siblings. dabeest passes
every suite but `binding_census` and `shipped_app_files`, which need the
siblings, and `stop_signal` has no tests there. No test contacts a relay. If the
Pi's sibling checkouts are behind GitHub, those two suites may fail there for
that reason alone, and the record says so rather than pulling more than glade.

**3. The ids**, as section 6 shows: minted, piped across, and compared by tag. A
small line-stamper, `$S/stamp.py`, is written on each machine. It prefixes each
line with the machine's UTC time in seconds to the millisecond.

**4. Run 1.**

1. Write the Pi's file with `umask 077`: `relay n0`, `bind 127.0.0.1:0`, and
   `peer $(cat $S/dab45.id)`.
2. Start the Pi, from the Mac in the background, so its ssh session holds it:
   `$PI "cd $S && GLADE_HOME=$S/home GLADE_NODE_ASSEMBLED=1 timeout
   --preserve-status -s TERM 420 $B --profile peer --name pi45 --config
   $S/pi45-1.conf 0 2>&1 | python3 -u stamp.py > pi45-1.log; echo exit
   \${PIPESTATUS[0]} >> pi45-1.log"`. Every start carries such a `timeout`,
   so nothing outlives the run.
3. Wait up to 30 s for its `relay <url>` line, and read the URL. A URL is not
   an id, so it may appear on the screen.
4. Write dabeest's file with `umask 077`: `relay n0`, `bind 127.0.0.1:0`, and
   `peer $(cat $S/pi45.id)@<url>`.
5. Start dabeest the same way, `timeout 240`, `--config $W/dab45-1.conf`,
   through Python 3.13's full path, into `dab45-1.log`.
6. Watch for 120 s: dabeest's `peer-connected`, both `link` lines, and both
   `home round` lines. The path should stay `via relay`.
7. dabeest's `timeout` ends its node by force. The Pi should note `link …
   closed` about 30 s later: a peer that died, at iroh's idle limit for a relay
   path. Then the Pi's own `timeout` sends SIGTERM, and it exits 0.

**5. Run 2**, on the same instances. The Pi's fold now holds dabeest's binding,
so the door admits dabeest by record, not by first contact.

1. Write the Pi's file, with `umask 077` as before: `relay n0`, `bind
   10.1.1.236:4545`, and the same `peer` line admitting dabeest. Start it as in
   run 1, with `timeout 600`, into `pi45-2.log`, and read its `relay` line
   again.
2. Write dabeest's file: `relay n0`, `bind 10.1.1.239:4545`, and `peer $(cat
   $S/pi45.id)@<url>`. No LAN address is named. Start it with `timeout 600`,
   into `dab45-2.log`.
3. Watch for 120 s. Note each `link … via …` line, with its time and RTT, and
   the path selected at the end.
4. **Port release after close.** SIGTERM the Pi's node alone: `$PI "pkill
   -TERM -f '^$S/target/debug/glade-node '"`. The anchor keeps the signal off
   the `timeout` that holds it. Record the exit status and the time from the
   signal to the exit. `ss -lunH 'sport = :4545'` must print nothing at once.
   dabeest must note `link … closed` within about a second, not after 15 to
   30 s.
5. Start the Pi again on the same file. Its `peer … 10.1.1.236:4545` line
   shows the port bound again. Stop it with SIGTERM, then end dabeest (its
   `timeout`, or `taskkill //F //IM glade-node.exe`).

On dabeest, the first bind of a Wi-Fi address may raise a Windows Defender
Firewall prompt on the console, or, with no one to answer it, leave inbound
traffic to the new program blocked. The run changes no firewall setting on
either machine, and notes the path iroh picks either way. dabeest dials, so
its own outbound packets should open the return path for hole punching.

**6. Teardown.**

- No `glade-node` process is left on either machine: `pgrep -af "$S/target"` on
  the Pi and `tasklist` on dabeest. Anything found under `$S` is killed. UDP
  4545 is free on both.
- No log names an endpoint id. On each machine, `grep -c -F -f` with each id
  file, and with each node's own `endpoint-id` output, finds 0 lines in every
  `*.log`.
- The logs are copied to the Mac's scratch space for the record. They hold node
  ids, the relay URL and LAN addresses, and no endpoint id. The public address
  that QAD reflected is never printed by the node, so it cannot be in them.
- `rm -rf $S` on both machines: the instances, both keys, the files, the logs
  and the build. The ids n0 saw now belong to keys that no longer exist.

**What is recorded**, in this note as "Measured (4.5)", and in a line under
plan Step 4.5:

| Item | Run 1 | Run 2 |
| --- | --- | --- |
| commit, machines, Rust | | |
| each node's home relay (`relay` line) | | |
| HELLO: dabeest's `peer` line to its `link` line, and the Pi's `link` line | | |
| the path at HELLO, and its RTT | | |
| each `home round`: records and time | | |
| later path changes: when, to what, RTT | | |
| the path selected at 120 s | | |
| the Pi's `link … closed` after dabeest is ended by force | | |
| the Pi's stop: exit status, time to exit, UDP 4545 free | | |
| dabeest's `link … closed` after the Pi's stop | | |
| the Pi's restart binds 4545 | | |
| no log holds an endpoint id | | |
| what n0 could see (section 9), with what the run showed of it | | |

If n0's relays refuse iroh 1.2 (the community tier serves the latest stable
release only), the `relay … not connected: <error>` line is the record, and the
run stops there. The `version_pin` ruling says what follows.

### 11. Tests, each begun red

Each test is run first against the code with the part it guards switched off,
in a scratch copy, and the message it prints is recorded.

**Part 1**, built on 2026-09-27 against glade `4a976de`. The tests use this
step's API, so each was run red in one sources-only copy of the final tree
(glade's `node`, `wire-rs` and `contracts`), with the part it guards switched
off by one edit, or two where the part sits in two places, and put back
before the next. The message is what each red run printed. `<id>` stands for
an endpoint id, `<node>` for a node id, and `<s>` for the scratch directory.

| Test | Proves | Red first |
| --- | --- | --- |
| `netconf`: `no_file_is_loopback_with_relays_off_and_no_peers` | the default network: `[127.0.0.1:0]`, `Off`, no peers | with a default of `N0`: `left: Network { relays: N0, bind: [127.0.0.1:0], peers: [] }`, `right: Network { relays: Off, bind: [127.0.0.1:0], peers: [] }` |
| `netconf`: `the_file_takes_relay_bind_and_peer_lines` | each form parses, the relay URL as the node prints it included; lines for one key merge; comments and blank lines are skipped; the flags join after the file, a key the file names gaining the flag's address. Parsed only: nothing binds or dials | with no `@<relay-url>` form: `called Result::unwrap() on an Err value: Custom { kind: InvalidInput, error: "net.conf: line 8: expected <endpoint-id>, <endpoint-id>@<ip:port> or <endpoint-id>@<relay-url>" }` |
| `netconf`: `each_bad_line_is_refused_by_its_number_and_never_echoed` | refused, each for its reason, naming the line, the message exactly the file, the line and a fixed reason: an unknown keyword, a second `relay`, `relay` with a URL, a second IPv4 and a second IPv6 `bind`, a bad socket, an upper-case and a short id, a key iroh rejects (y = 2, on no point of the curve), a relay URL under `relay off`, one that is not n0's; a flag naming a relay with no file is refused by its place | with bad lines and flags skipped: `called Result::unwrap_err() on an Ok value: Network { relays: N0, bind: [127.0.0.1:0], peers: [] }` |
| `netconf`: `a_peer_entry_names_a_key_and_perhaps_where_to_dial_it` (moved from `iroh_carrier`) | an entry without an address configures the door, one with an address is dialed too, and junk is refused, `--peer entry 1: expected …`, where it was skipped | the same edit: `called Result::unwrap_err() on an Ok value: Network { relays: Off, bind: [127.0.0.1:0], peers: [] }` |
| `netconf`: `an_entry_is_named_by_its_tag` (added) | a failed dial's line names the peer `<tag>@<address>,<address>` | with `transport::tag` the whole id: `left: "<id>@127.0.0.1:4711,https://aps1-1.relay.n0.iroh.link./"`, `right: "6e7a1cdd29@127.0.0.1:4711,https://aps1-1.relay.n0.iroh.link./"` |
| `netconf::tests::unix`: `a_config_file_others_can_read_is_refused` | 0640 and 0644 are refused, naming the file and the mode; 0600 is taken | with no mode check: `called Result::unwrap_err() on an Ok value: Network { relays: Off, bind: [127.0.0.1:0], peers: [] }` |
| `netconf`: `a_relative_config_path_is_refused` | the path must be absolute | with the path not checked: `left: "glade/net.conf: No such file or directory (os error 2)"`, `right: "glade/net.conf: not an absolute path"` |
| `iroh_carrier`: `the_recipe_maps_off_to_disabled_and_n0_to_the_production_relays` | pure: `Off` gives `RelayMode::Disabled`, and `N0` gives `RelayMode::Default`, whose map is `defaults::prod`'s four relays; a relay URL is taken as the node prints it when it is one of the four, and never one of the staging relays | with `N0` mapped to `Staging`: `left: Staging`, `right: Default` |
| `iroh_carrier`: `an_endpoint_binds_where_its_network_says` | on loopback: a port found free is the one bound, and the `peer` line's address is the bound one | with a recipe that binds `127.0.0.1:0` whatever the network says: `left: [127.0.0.1:58770]`, `right: [127.0.0.1:54295]` |
| `iroh_carrier`: `an_endpoint_listens_on_loopback_alone` (kept) | now over `Network::default()` | not run red: its red form binds `0.0.0.0`, beyond loopback, which no run of this step may do. It was seen red when it was written (glade `1192bf2`) |
| `iroh_carrier`: `a_dial_target_names_each_address_it_was_given` | pure: `Via::Ip` and `Via::Relay` become one `EndpointAddr` with both; the entry is named by the tag iroh's own `fmt_short` prints | with relays dropped: `left: ([10.1.1.236:4545], [])`, `right: ([10.1.1.236:4545], ["https://aps1-1.relay.n0.iroh.link./"])` |
| `iroh_carrier`: `an_iroh_port_binds_where_its_config_says` | `IrohCarrier` binds `local`'s socket, so a fresh port asked to bind where a closed one was lands at its very address | with the adapter binding `127.0.0.1:0` whatever `local` says: `assertion left == right failed: where b was`, `left: 127.0.0.1:62362`, `right: 127.0.0.1:60715` |
| `iroh_carrier`: `no_production_code_names_irohs_environment_readers` | a source check over `src/`, each file's code before its tests with comments set aside: no `presets::N0`, `N0DisableRelay`, `default_relay_mode`, `force_staging_infra` or `proxy_from_env` | with `presets::N0` in `bind_endpoint`: `left: ["<s>/red/glade/node/src/iroh_carrier.rs:94: presets::N0"]`, `right: []` |
| `iroh_carrier`: the release wait, through `ca_004_an_iroh_port_gives_its_endpoint_up_by_value` and the test above (section 4, as built) | `close` resolves once its address can be bound again | with the wait off, the module run 25 times: 21 runs failed, on `CA-004 close frees the address by value: Err(Transport("Failed to bind sockets"))` or on the test above's `called Result::unwrap() on an Err value: Transport("Failed to bind sockets")`. With it on, 25 of 25 passed |
| `endpoint_id`: `the_command_takes_a_name_and_nothing_else` (added) | `--name <name>` and nothing else; anything else is the usage line | with a third argument taken: `called Result::unwrap_err() on an Ok value: "n"` |
| `endpoint_id`: `a_held_instance_with_no_key_is_refused` (added) | while a node holds an instance whose key is missing, the command mints nothing and says to stop the node | with the mint going on without the lock: `called Result::unwrap_err() on an Ok value: "<id>"` |
| `tests/endpoint_id`: `the_command_mints_a_key_that_the_first_start_binds` | on a new instance: one line of 64 lower-case hex digits, the same when asked again; `endpoint.key` alone, 0600 on Unix: no `node.key`, no records.json, no lock left; the first boot takes that key and binds it to its node, `Live` | with the command absent: `glade-node endpoint-id still ran after 20s: instance <s>/…/glade-home/sys/n`, `node <node>`, `registry ready (home served: true)`, `peer 26cd4a5ee8 127.0.0.1:51728`: a node started, the name read as a positional |
| `tests/endpoint_id`: `the_command_reads_a_running_nodes_key_and_writes_nothing` | with the instance held by a boot: the same id, no lock taken, every file of the instance as it was | with a command that takes the lock to read: `instance already locked: <s>/…/sys/n/instance.lock: …: stop the node first`, `left: Some(1)`, `right: Some(0)` |
| `tests/endpoint_id`: `platform::a_group_readable_key_is_refused` (split from the row above: modes are Unix's) | a key at 0640 is refused as a boot refuses it: exit 1, `endpoint.key is group/world-accessible (mode 640) — refusing`, nothing on stdout | with the key read without its mode checked: `left: Some(0)`, `right: Some(1)`, the id printed |
| `tests/assembled_path`: `both_roots_take_their_network_from_the_config_file` | B, with no file, refuses A, whose 0600 file dials B at `127.0.0.1`, and says so by A's tag; B's file (`relay off`, `bind 127.0.0.1:0`, a blank line, `peer <A>` and a comment) admits A, and they link | with `--config` ignored on both roots: `HandWritten: no recovery key is committed for this node: …`, B's whole stderr: no refusal, since A, its file ignored, dialed nothing |
| `tests/assembled_path`: `both_roots_refuse_a_bad_config_file_before_writing` | a file with a second `relay` line, and on Unix one at 0644: exit 1, the message naming the file, and nothing under `GLADE_HOME` | with the hand-written root loading after its boot: `written under GLADE_HOME: [Ok(DirEntry("<s>/…/glade-home/sys"))]`; then, that put back, with the assembled root loading in its `Instance` step after the boot: the same, on the assembled root |
| `tests/assembled_path`: `no_line_names_an_endpoint_id` | on each root, A links to B and C is refused: none of the three ids is in any line of the three, stdout or stderr; each `peer` line carries its tag, B's refusal names C's, and C's failed dial B's | with `transport::tag` the whole id, today's lines: `assertion left == right failed: HandWritten`, `left: ["peer <id> 127.0.0.1:64975"]`, `right: []` |

Changed and passing: the door tests' refusal lines carry the tag
(`transport`'s and three of `mesh`'s); the two-node tests take the full id
from `endpoint-id` (`tests/assembled_path.rs`'s three endpoint tests) or from
an in-process boot (`tests/lifecycle.rs`, `tests/stop_signal.rs`), and the
address from the `peer` line; `tests/lifecycle.rs`'s start loads the network
as a root does; `mesh`'s door helper binds `Network::default()`.

**Part 2**, built on 2026-09-27 against glade `b4e890f`, red in one
sources-only copy of the final tree as part 1 was, each part switched off by
one edit and put back before the next.

| Test | Proves | Red first |
| --- | --- | --- |
| `iroh_carrier`: `a_path_is_described_by_where_it_goes` | pure: a relay `TransportAddr` reads `relay <url>`, as the node prints the URL, and an IP one `direct <ip:port>` | with iroh's `Debug` form: `left: "Relay(https://aps1-1.relay.n0.iroh.link./)"`, `right: "relay https://aps1-1.relay.n0.iroh.link./"` |
| `mesh`: `each_end_notes_its_link_at_hello_and_its_close` | over real iroh on loopback: both ends note `link <node> via direct 127.0.0.1:<port>, rtt <n> ms`, the address the other end is bound at, and the dialed end notes `link <node> closed` once the dialer's endpoint has closed | with the notes sent nowhere: `not noted: []`; with the close's note off: `not noted: ["link <node> via direct 127.0.0.1:55703, rtt 1 ms", "home round with node <node>: 0 record(s) in 0 ms"]` |
| `mesh`: `each_end_notes_its_home_round` | both ends note `home round with node <id>: <n> record(s) in <ms> ms`, `n` the other's `home` records the round took: B holds two of its own, A one | with the notes sent nowhere: `not noted: []`; with the round's note off: `not noted: ["link <node> via direct 127.0.0.1:56810, rtt 1 ms"]` |
| `mesh`: `the_relay_lines_follow_the_home_relays_states` (added) | the `relay` lines, from home relay states as the adapter reads them off iroh, with no relay reached: `relay <url>` once one is connected, again after a drop and at a change of home relay; `relay <url> not connected: <error>` once for each error, where iroh reports a failure again at every retry; nothing while connecting | with no lines computed: at the third state, `left: []`, `right: ["relay https://aps1-1.relay.n0.iroh.link./"]` |
| `tests/lifecycle`: `a_node_links_to_a_peer_and_stops_clean_with_its_ports_free` (extended) | the notes reach the assembled console's `out`: A notes its link to B and its round before `peer-connected`, and B notes `link <A> closed` once A has stopped; the stop is still clean | with the assembled root's door noting nowhere: `link, round, peer-connected: [...]`, A's lines holding neither note |
| `tests/assembled_path`: `both_roots_refuse_an_unknown_dialer_and_admit_a_configured_one` (extended) | on each root, the linked A notes on stdout its link to B at B's address and its round, before `peer-connected` | with the hand-written root's door noting nowhere: `HandWritten: [...]`, its stdout holding neither note |
| `tests/assembled_path`: `no_line_names_an_endpoint_id` (extended) | no node panicked, though B noted its link and its round after its `listening`, when the test no longer read its stdout | with both roots printing notes by `println!`: B's stderr held `failed printing to stdout: Broken pipe (os error 32)`, a worker thread's panic |

The last row is a finding of the build (section 8, as built): the first full
run failed both of those tests on the hand-written root, A's `peer-connected`
missing, until a note stdout could not take was dropped rather than panic.

**What they do not prove:** a relay path, the relay's watch, and the step
from iroh's `RelayStatus` to the states the lines are computed from: iroh
gives `RelayStatus` no public constructor, and a node picks a home relay
only from relays whose probes answered, so the watch reports nothing
without a relay that answers. No test reaches a relay: n0's would be reached
from the Mac, and a local one needs iroh's `test-utils` server, whose crates
are not in the lock, and a TLS bypass the node must never carry. The
crossing is their evidence. Nor do they prove a change of selected path (on
loopback a link has one path), Windows' file modes (F5), hole punching, or
two networks.

**The gate** (`glade/node/check.sh`) must pass all 9 components:

- the node's tests on both paths, 326 plus the new ones;
- rustfmt at or below its baseline of 295, with no deviation in a line this
  step writes; clippy at 11 and 7;
- process-globals at 3 permanent entries, 0 debt, nothing new;
- confinement with no new crate; the contracts gate unchanged, 89 tests.

Beside the gate: the six downstream suites at baseline against the rebuilt
binary, and the desk's replay (section 1) with `lsof`.

### 12. Size and the split

Estimated, in `.rs` lines with doc comments:

| Part | Production | Tests |
| --- | --- | --- |
| 1: `netconf.rs` about 190 (new); `iroh_carrier.rs` about 110 (the recipe, the dial target, the load-time check with iroh's types, the bound address, the adapter's `local`); `endpoint_id.rs` about 50 (new); the roots about 60; `assembly.rs` about 20; `transport.rs` and `sysdir.rs` about 10 | about 440 | about 550 |
| 2: `iroh_carrier.rs` about 70 (the path and the relay status); `mesh.rs` about 70 (the reporter, the link's watch, the round's note, the relay's watch); `tasks.rs` and the roots about 20 | about 160 | about 200 |

About 600 production lines in all, over the ~450 brief. Hence the split. Each
part is one commit through gwz, gated, with the desk's replay. The crossing
follows part 2 and adds no code: its record goes into this note and the plan.
Part 1 also carries the plan's two one-line notes, in the glade-wz root's
commit, that the ruling of 2026-09-22 governs:
`dev-docs/IrohGladeMapping.md:420-426` (self-host for the first slice) and
`dev-docs/glade/GladeDiscoveryModel.md:198` (the public iroh relay).

### Named gaps (4.5)

- **The relay path has no automated test**, only the crossing (section 11).
- **iroh's dependencies read the environment** when relays are on: the proxy
  variables through `reqwest`, and, on Windows, `SystemRoot` through the DNS
  resolver (section 3). The checker does not scan dependencies.
- **n0 reads the ALPN and the pairing** of ids, beyond the plan's four (section
  9).
- **A stable endpoint key is a stable handle for n0.** It is replaced only by
  hand (4.2a). The crossing's keys are deleted after the run.
- **With relays on, anyone who learns an endpoint id can reach its door
  through n0.** The door refuses an unknown key after the TLS handshake, so
  each such attempt costs a handshake.
- **Off Unix**, the configuration file's mode is not checked (F5).
- **records.json and the served store's `home` journal** carry the endpoint id
  in the binding (4.2a), and they, with the instance directory, take the
  umask's mode.
- **`--peer` puts ids in argv**, which other local users can read with `ps`
  (question 4).
- **A dialer's relay URL goes stale** if the acceptor's home relay changes,
  after a network change for example. There is no lookup service, by ruling.
- **A lost link is not dialed again**, as today: `--peer` and the file's dial
  entries are dialed once, at the start.
- **Windows stops by force over ssh**, so dabeest's release is not observed.
- **The notes are lines, not `node.status`.** Metrics and path events reach
  `node.status` through bindings later (`IrohGladeMapping.md` §7.6).
- **The 4.2c gap stays for 4.5b:** the adapter still waits for its first word
  without a bound.
- **Part 2's relay lines are tested from computed states** (section 11): the
  step from iroh's `RelayStatus` to them, and the relay's watch, are the
  crossing's to show.
- **A change of selected path** is noted by a 250 ms poll, and no test sees
  one: on loopback a link has one path.
- **A note is dropped when stdout is not read** (section 8, as built): a
  parent that stops reading after `listening` loses the notes after it.
- **The adapter's release wait binds each address it had**, for an instant, to
  see that it is free (section 4, as built). Every test binds it on loopback;
  bound beyond loopback in 4.5b, its close would probe beyond loopback too. A
  port another process takes meanwhile ends the wait at 3 s, not the close.
- **`an_endpoint_listens_on_loopback_alone` was not run red** in part 1: its
  red form binds `0.0.0.0` (section 11).
- **The `endpoint-id` command checks no `--name`**, as the boot checks none: a
  name with `..` reaches outside `<root>/sys`.

### Default-path changes (4.5)

1. `--config <absolute path>` is a flag of the booted form. With none, a node's
   network is as before on every profile.
2. `glade-node endpoint-id --name NAME` is a command. A first argument
   `endpoint-id` no longer starts a node.
3. Lines name an endpoint by a 10-digit tag (section 7). The desk's `peer` line
   changes that way, and its address is the bound one, `127.0.0.1:<port>` as
   before.
4. A malformed `--peer` entry refuses the start, where it printed a line and
   was skipped.
5. `IrohCarrier` binds where `CarrierConfig::local` says, and its `close`
   resolves once that address can be bound again, within 3 s. No root binds
   it.
6. Part 2: a linked node prints `link` and `home round` lines, and a node with
   `relay n0` prints `relay` lines. The desk prints none.

**What the owner's desk sees at its next restart:** the `peer` line's endpoint
id becomes a 10-digit tag. Nothing else changes: it binds loopback alone,
contacts no relay, opens no portmapper socket, writes no new file and prints
nothing new on stderr.

### Questions for the owner (4.5)

1. **The file.** Recommend `--config <absolute path>` on the booted form, in a
   line format like the app files' (`relay`, `bind`, `peer`), at 0600, and
   refused whole, before anything is written, on any bad line. Alternatives: a
   fixed `network.conf` in the instance directory, found without a flag but
   putting a hand-edited file in glade's system tree; or TOML, which needs a
   new crate.
2. **What a profile does.** Recommend nothing new: every profile, given no
   file, binds `127.0.0.1:0` alone with relays off, and a profile still picks
   only the instance name. The alternative gives `peer` and `server` n0's
   relays and every interface by default. grazel's `--mode peer` would then
   reach n0 and listen beyond the Mac.
3. **The portmapper.** Recommend off in every configuration, with no switch
   (section 5). Alternatives: a `portmapper on` line, which asks the router
   to open an external port; or compiling it out (`default-features = false`
   on iroh). That drops the portmapper crates from the lock, and makes the
   async witness's `--locked` stale again.
4. **Endpoint ids outside the file.**
   - In lines, recommend iroh's short form, 10 hex digits, with the full id
     only from `endpoint-id`. Alternatives: no endpoint named at all, so a
     refusal cannot be matched to a line of the file; or the full id, against
     the plan.
   - On the command line, recommend keeping `--peer`, which tests and
     one-machine runs use, noting that argv is readable by other local users.
     The crossing uses files only. The alternative retires `--peer`, and every
     two-node test writes a file.
5. **`ConfigPort` stays in glade-node.** Recommend so: all its consumers are
   there (section 4). The alternative is a `config-api` contract crate with a
   conformance suite now, as the 3.2 note expected.
6. **n0 only.** Recommend that `relay` take `off` or `n0`, and that a peer's
   relay URL be one of n0's four. `Custom(RelayMap)` waits for a relay of our
   own. The alternative takes any relay URL and a `relay <url>…` map now, with
   no relay to test them against.
7. **The net report.** Recommend iroh's defaults: QAD, the HTTPS latency probes
   and the plain-HTTP captive-portal check. Run 1 needs the HTTPS probes. The
   alternative, `NetReportConfig::minimal()`, sends n0 less, but a node whose
   QUIC cannot reach n0 (run 1's included) then finds no relay.
8. **The notes** (section 8). Recommend that the node print them, so the node
   itself notes the path iroh picks, here and in 4.6. The alternative is an
   ignored test that embeds the node and reads iroh's paths, leaving the node
   silent. It would be a second composition that must track both roots.
9. **The runs.** Recommend run 1 (loopback binds, the relay alone), then run
   2 (Wi-Fi binds, iroh's choice), with the Pi accepting, dabeest dialing, and
   fresh scratch instances deleted afterwards. The alternative is run 2 alone,
   where HELLO and the round may already ride the LAN path, so "through the
   relay" goes unshown.
10. **The done-when's list.** Plan Step 4.5 says n0 sees endpoint ids, IP
    addresses, timing and volume, "nothing else". It can also read the ALPN,
    `glade/node/3`, in each connection's first packets, and it sees which ids
    talk to which. Recommend naming both in the record and changing nothing:
    our own relay, the ruling's later switch, is the remedy. The alternative
    is an ALPN that names nothing, which hides the protocol's name but not the
    pairing.
11. **The split.** Recommend part 1, then part 2, then the crossing, each gated
    and replayed. The alternative is one commit of about 600 production lines.

**Ruled, owner, 2026-09-27 ("all recommended"):** 1 `--config <absolute path>` on the booted form,
the line format, 0600, refused whole on any bad line; 2 no profile changes: given no file, every
profile binds `127.0.0.1:0` alone with relays off; 3 the portmapper off in every configuration, with
no switch; 4 lines name endpoints by the 10-hex short form, the full id comes only from `endpoint-id`,
and `--peer` stays; 5 `ConfigPort` stays in glade-node; 6 `relay` takes `off` or `n0`, and a peer's
relay URL is one of n0's four; 7 iroh's net-report defaults; 8 the node prints the notes; 9 run 1,
then run 2, the Pi accepting and dabeest dialing, the scratch instances deleted afterwards; 10 the
record names the ALPN and the pairing too, our own relay being the remedy; 11 part 1, part 2, then
the crossing.

### Measured (4.5, part 1)

2026-09-27, Apple M3 Pro, Rust 1.96.0, on the final tree (glade `4a976de`
plus part 1):

- **The gate** passes all 9 components, in 100 s from an empty scratch
  target, with 348 node tests on each path, across 17 test binaries, where
  there were 329 across 16: `netconf` 7 (one moved in from `iroh_carrier`),
  `iroh_carrier` 5 new, `endpoint_id` 2, `tests/endpoint_id` 3 (a new binary)
  and `tests/assembled_path` 3.
  - rustfmt: glade-node 294 hunks, at its baseline; no line this step wrote
    is in one, and the two new modules and the new test file are
    rustfmt-clean. glade-wire 43.
  - clippy: glade-node 11 warnings and glade-wire 7, at their baselines.
  - process-globals: 53 files (the two new modules), 3 permanent entries, 0
    debt, nothing new. No production code reads the environment:
    `glade-node endpoint-id` takes the instance root `start` read.
  - confinement: no new crate; `Cargo.toml` and `Cargo.lock` are untouched.
    The contracts gate passes, untouched.
- **Nothing reached beyond loopback.** No test or run binds anything but
  loopback, and none contacts a relay: `relay n0` and the non-loopback `bind`
  lines are parsed and mapped, never bound or dialed. The only relay URLs
  the tests name are parsed, and n0's map is iroh's constant.
- **The release wait**: the `iroh_carrier` module, run 25 times, passed 25
  times with it and 4 times without it (section 11).
- **Time.** The `netconf` and `endpoint_id` unit tests take under 0.01 s;
  `tests/endpoint_id`'s three 0.1 to 0.9 s; `tests/assembled_path`'s 21 about
  1.6 to 1.9 s; the `iroh_carrier` module about 0.3 s.
- **The replay**, from `glade-wz/grazel` as grazel starts the desk's node
  (`--profile local --name grazel --app apps/grazel-app.glade --app
  apps/gyld-app.glade 0`), with only `PATH`, `HOME` and `GLADE_HOME` set, on
  one scratch instance. Each start lived 11 s past `listening`, had its
  sockets listed with `lsof`, and was stopped with SIGTERM. Today's binary
  (inode 405329410), this build (inode 405439944, from the gate's scratch
  target), then today's again:

  | Start | Lines | Sockets |
  | --- | --- | --- |
  | today's, the first boot | `instance`, `node`, `app grazel registered (+12 record(s), 0 unchanged)`, `app gyld registered (+10 record(s), 2 unchanged)`, `registry ready (home served: true)`, `peer <id> 127.0.0.1:<port>`, `workspace ws-razel serving` twice, `listening <port>` | UDP `127.0.0.1:<port>` and TCP `127.0.0.1:<port>` (LISTEN), nothing else |
  | this build | the same, `+0 record(s), 12 unchanged` for each app, and `peer cbf33c61e0 127.0.0.1:<port>`: the tag of the same id | the same |
  | today's, again | the same as this build's, with the `peer` line's full id | the same |

  Every start printed one stderr line, the recovery warning, naming its own
  binary. records.json gained the same records from each start: a renewal of
  `home` and `ws-razel`'s first claim at its next epoch. `glade-node
  endpoint-id --name grazel` on the stopped instance then printed one line of
  64 characters whose first 10 are `cbf33c61e0`, and wrote nothing.
- **Downstream**: grazel, glade-gyld and glade-gwz pass no `--peer`, no
  `--config` and no first argument `endpoint-id` (their sources, read only),
  and read `listening` alone. Their suites, client-rs's, client-ts's and
  grip-share's were not run, as asked.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +675/−145, net +530, over section 12's estimate of about 440:
  `netconf.rs` +322, new (240 of them code); `iroh_carrier.rs` +157/−66;
  `endpoint_id.rs` +69, new; `bin/glade-node.rs` +55/−36; `lifecycle.rs`
  +22/−26; `assembly.rs` +21/−4; `transport.rs` +13/−5; `mesh.rs` +7/−5;
  `sysdir.rs` +7/−3; `lib.rs` +2. The adapter's release wait is not in the
  estimate;
- tests: +866/−67: `tests/assembled_path.rs` +260/−29; `netconf.rs` +195;
  `tests/endpoint_id.rs` +192, new; `iroh_carrier.rs` +139/−16;
  `endpoint_id.rs` +35; `tests/lifecycle.rs` +26/−12; `tests/stop_signal.rs`
  +12/−5; `mesh.rs` +6/−4; `transport.rs` +1/−1.
- Beside them, the two one-line notes at the glade-wz root:
  `dev-docs/IrohGladeMapping.md` §7.6 and `dev-docs/glade/GladeDiscoveryModel.md`
  (the v1 relay).

### Measured (4.5, part 2)

2026-09-27, Apple M3 Pro, Rust 1.96.0, on the final tree (glade `b4e890f`
plus part 2):

- **The gate** passes all 9 components, in 104 s from an empty scratch
  target, with 352 node tests on each path, across 17 test binaries, where
  there were 348: `iroh_carrier` 1 and `mesh` 3 new, and three tests
  extended.
  - rustfmt: glade-node 294 hunks and glade-wire 43, at their baselines; no
    line this part wrote is in one.
  - clippy: glade-node 11 warnings and glade-wire 7, at their baselines.
  - process-globals: 53 files, 3 permanent entries, 0 debt, nothing new.
  - confinement: no new crate; `Cargo.toml` and `Cargo.lock` untouched. The
    contracts gate passes, untouched.
- **Nothing reached beyond loopback.** No test binds anything but loopback,
  and none reaches a relay: the relay lines are computed from states a test
  writes.
- **The notes on loopback**, from the tests: `link <node> via direct
  127.0.0.1:<port>, rtt 1 ms` at each end at HELLO; `home round with node
  <node>: 2 record(s) in 0 ms` and `…: 1 record(s) in 0 ms`; `link <node>
  closed` at the dialed end within milliseconds of the dialer's close. The
  two mesh tests passed 20 of 20 runs.
- **The replay**, as in part 1: today's binary (inode 405499642), this build
  (inode 405580880, from the gate's scratch target), then today's again. The
  three starts printed the same lines apart from ports, line for line
  (`instance`, `node`, the two `app` lines, `registry ready (home served:
  true)`, `peer 706eed245d 127.0.0.1:<port>`, `workspace ws-razel serving`
  twice, `listening <port>`), nothing after `listening`, and on stderr the
  recovery warning alone. `lsof`: UDP and TCP on `127.0.0.1` alone. A node
  with no link and no relay notes nothing.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +249/−21, net +228, of which +160/−13 are code, against
  section 12's estimate of about 160: `mesh.rs` +105/−10; `iroh_carrier.rs`
  +93/−1; `transport.rs` +18/−2; `bin/glade-node.rs` +18/−1;
  `lifecycle.rs` +11/−6; `tasks.rs` +4/−1;
- tests: +176/−9: `mesh.rs` +124/−4; `tests/assembled_path.rs` +20/−3;
  `tests/lifecycle.rs` +19/−2; `iroh_carrier.rs` +13.

### The crossing, 2026-09-27

Run by an agent for the lane owner, on glade `4a34168`, as section 10 lays out
and under the rulings: run 1, then run 2; the Pi accepts and dabeest dials;
fresh scratch instances, deleted afterwards; `relay n0`; no NAT or firewall
checks, and the path iroh picks noted, not measured. Times come from
`stamp.py`. `<Pi node>` and `<dab node>` stand for the two node ids, which
the logs carry in full.

**The machines**

| | the Pi | dabeest |
| --- | --- | --- |
| system | Raspberry Pi 5, Debian 13 aarch64, Rust 1.96.0 | Windows 11, MSYS bash, Rust 1.98.1 (MSVC) |
| glade | `1192bf2` to `4a34168` by `pull --ff-only`; clean | `63a5799` to `4a34168`; clean |
| Wi-Fi | `wlan0` `10.1.1.236/16` | `10.1.1.239/16` (`ipconfig`, read before each run) |
| before the runs | no `glade-node`; UDP 4545 free | the same; no build or other heavy job (2% load) |
| build, `--bin glade-node`, empty target | 183.2 s | 54.0 s |
| suite, its compile included | 301.2 s: 346 passed | 58.7 s: 335 passed |
| failed, the siblings' (not counted) | `binding_census` 5, `shipped_app_files` 1 | the same 6 |
| failed, counted | none | 1: `assembled_path`'s `both_roots_warn_until_a_recovery_key_is_committed` |

- Per suite, the Pi's then dabeest's where they differ: the library 237 and
  232, `assembled_path` 21 and 20 of 21, `assembly` 29,
  `assembly_registration` 1, `durable` 15, `endpoint_id` 3 and 2,
  `instance_root` 1, `journeys` 12, `lifecycle` 6, `one_file_per_app` 3,
  `release_order` 7, `start_refusals` 2, `stop_signal` 4 and 0, doc-tests 5.
  dabeest's 10 fewer are Unix-only. The Pi's 346 and 6 make the gate's 352.
- **The Pi's siblings are behind:** grazel `c9f9c7f` (2026-09-25),
  glade-gyld `c9ef7a6` (09-23) and glade-gwz `a079921` (09-24), where the Mac
  has `1df2782`, `2c0a9b3` and `35b38ba`. Its `grazel-app.glade` registers
  `unchanged: 6` where the census expects 7, and warns at lines 50 and 51.
  Only glade was pulled, as ruled.
- **dabeest's counted failure is 4.1c's**, which no Windows run had met (its
  last suite ran at `63a5799`). `recovery::warning` single-quotes any word
  with a character outside `[A-Za-z0-9/._-+=:,@%]`, so on Windows it prints
  `GLADE_HOME='C:\Users\…\glade-home'
  '\\?\E:\git\glade-wz\scratch\4.5\target\debug\glade-node.exe' recovery
  --name h …`, where the test expects both paths bare. dabeest's run logs
  carry the same warning, its program path quoted, `\\?\` and all.
- **The clocks**, each read over one held ssh session: the Pi 87.6 ms ahead of
  dabeest before the runs and 89.7 ms after, each to within 5 ms. The Pi's
  times below are moved onto dabeest's clock by 89 ms.

**The ids.** `endpoint-id` minted each key in a new instance (`pi45`,
`dab45`), and each id crossed through the Mac's pipe into a 0600 file on the
other machine, as section 6 shows. The tags agree: the Pi's `e8cd666d9c` and
dabeest's `2d22ccf41f`, each file's first 10 digits against the other
machine's `endpoint-id`. Each file is 65 bytes, one line of 64 lower-case hex
digits. Each instance then held `endpoint.key` alone, 32 bytes at 0600, with
no lock left. The Pi's instance directory took the ssh session's umask, 0775
(section 7's named gap).

**The relay.** Every start, the Pi's three and dabeest's two, took the same
home relay, `https://usw1-1.relay.n0.iroh.link./`, 3.1 to 3.6 s after it
began (the Pi 3.61, 3.60 and 3.62 s; dabeest 3.13 and 3.11 s). That is the URL
dabeest dialed in both runs. No `relay … not connected` line was printed, and
no refusal or error. At each look (`ss -tunap` and `netstat -ano`, at about
24 s in each run, and the Pi's again at 122 s in run 1), each node held one
TCP connection beyond loopback, from its Wi-Fi address to `5.78.69.43:443`.

**Run 1: loopback binds, everything through the relay.** dabeest started
32.0 s after the Pi. Seconds count from dabeest's start (a stamp taken just
before its `timeout`), on its clock.

| s | the Pi (accepts) | dabeest (dials) |
| --- | --- | --- |
| −31.9 | `peer e8cd666d9c 127.0.0.1:41438`, `listening 34101` | |
| −28.4 | `relay https://usw1-1.relay.n0.iroh.link./` | |
| 0.12 | | `peer 2d22ccf41f 127.0.0.1:64815` |
| 1.56 | `link <dab node> via relay https://usw1-1.relay.n0.iroh.link./, rtt 434 ms` | |
| 1.73 | | `link <Pi node> via relay https://usw1-1.relay.n0.iroh.link./, rtt 991 ms` |
| 1.99 | `home round with node <dab node>: 4 record(s) in 435 ms` | |
| 2.07 | | `home round with node <Pi node>: 4 record(s) in 348 ms`, `peer-connected <Pi node>`, `listening 57670` |
| 3.13 | | `relay https://usw1-1.relay.n0.iroh.link./` |
| to 122.7 | nothing: the path stayed `via relay` | nothing |
| 240 | | ended by force by its `timeout`, `exit 124`; UDP 64815 free at the next look |
| 275.3 | `link <dab node> closed` | |
| the Pi's 420 | SIGTERM from its `timeout`: `exit 0`, its session ended within 15 ms of the signal; UDP 41438 free at the next look | |

- **HELLO:** from dabeest's `peer` line (its endpoint bound, the dial begun)
  to its `link` line, 1.60 s, and to the Pi's, 1.44 s; 1.73 s from dabeest's
  start.
- **The round trip through usw1:** 434 ms by the Pi's estimate at HELLO, and
  the rounds took 435 and 348 ms. dabeest's 991 ms is iroh's estimate at that
  moment, early in the connection: the node notes the RTT only at HELLO and
  at a change of path.
- Both UDP sockets were on `127.0.0.1` alone, so every packet of the link
  crossed usw1. QAD could not leave loopback, and both nodes still found a
  relay, through the HTTPS probes, as section 10 expected.
- **The close came 35.3 s after dabeest's end**, not the 30 s section 8 gives
  for a peer that died on a relay path. The likely reason, not shown: QUIC
  restarts its idle timer when the Pi sends its first probe after dabeest's
  last packet, so the 30 s begin a few seconds after the end.

**Run 2: Wi-Fi binds, iroh's choice**, on the same instances. dabeest started
17.5 s after the Pi.

| s | the Pi | dabeest |
| --- | --- | --- |
| −17.4 | `peer e8cd666d9c 10.1.1.236:4545`, `listening 39823` | |
| −13.9 | `relay https://usw1-1.relay.n0.iroh.link./` | |
| 0.11 | | `peer 2d22ccf41f 10.1.1.239:4545` |
| 1.52 | `link <dab node> via relay https://usw1-1.relay.n0.iroh.link./, rtt 426 ms` | |
| 1.68 | | `link <Pi node> via relay https://usw1-1.relay.n0.iroh.link./, rtt 967 ms` |
| 1.77 | `link <dab node> via direct 10.1.1.239:4545, rtt 3 ms` | |
| 1.78 | `home round with node <dab node>: 1 record(s) in 266 ms` | `home round with node <Pi node>: 3 record(s) in 101 ms`, `peer-connected <Pi node>`, `listening 57913` |
| 1.94 | | `link <Pi node> via direct 10.1.1.236:4545, rtt 6 ms` |
| 3.11 | | `relay https://usw1-1.relay.n0.iroh.link./` |
| to 122.1 | nothing: direct | nothing: direct |
| 143.50 | SIGTERM to its PID alone, shown first to be the scratch binary under its `timeout` | |
| 143.67 | | `link <Pi node> closed` |
| 146.34 | `exit 0`; UDP 4545 free the same moment (`ss` empty) | |
| 165.3 | restarted on `pi45-2.conf`: `peer e8cd666d9c 10.1.1.236:4545`, 18.9 s after the release | nothing: dabeest dials once, and the Pi only admits |
| 181.0 | SIGTERM to its PID: `exit 0` 0.18 s later, UDP 4545 free the same moment | |
| 197.1 | | `taskkill //PID 4940 //F`, the PID's path first shown to be the scratch binary: `exit 1`; UDP 4545 free within 0.11 s |

- **HELLO:** dabeest's `peer` line to its `link` line, 1.58 s, and to the
  Pi's, 1.41 s; 1.68 s from its start. At HELLO the path was `via relay` on
  both ends, as in run 1: dabeest named the relay alone.
- **Direct, and when: yes.** Each end noted `via direct` at its link watch's
  first poll, 251 ms (the Pi) and 255 ms (dabeest) after its HELLO line, 1.8 to
  1.9 s after dabeest's start, and it stayed direct to 122 s. dabeest's round,
  101 ms, is under a third of run 1's relayed rounds, so it most likely ran
  direct before the poll saw the change. That is an inference: the lines show
  when the poll saw the path, not when iroh switched.
- Each node's relay connection stayed open beside the direct path (at 24 s).
- **The Pi's stop took 2.84 s** from SIGTERM to exit, within `STOP_WITHIN`
  (10 s), where the restart, with no link, took 0.18 s. dabeest noted the close
  0.17 s after the signal, so the close reached it at once, and the rest was
  the Pi's own drain. Section 4's `close` allows "about three seconds on a bad
  link"; this link was direct at 3 ms, with its relay path at about 430 ms
  beside it, on which QUIC's closing period is plausibly reckoned (not shown).
- **The port's release:** UDP 4545 was free the moment each Pi node exited
  (`ss` polled every 5 ms), and the restart bound it again. dabeest's forced
  end freed it within 0.11 s.

**Firewall.** No dialog or block was seen, and nothing was changed on either
machine. dabeest's Private profile has `NotifyOnListen` on, but no user was
logged on at its console (`query user`, after run 2), so no dialog could be
shown or answered. A read-only query found no firewall rule naming the
program, before run 2 or after it. The Pi reached `10.1.1.239:4545` directly:
dabeest dials, so its own packets to the Pi likely opened the return path, as
section 10 expected.

**What n0 could see** (section 9), with what the run showed:

- **Endpoint ids.** Both keys, at usw1. Each proved its key at each of its
  starts, the Pi's three and dabeest's two, and usw1 relayed between them in
  both runs, so it saw which ids talk to which. dabeest's dial reached usw1
  before dabeest had a home relay of its own (its `relay` line came 1.4 s after
  its `link`), and usw1 then became its home relay too: one relay saw both
  ends, and nothing passed from one relay to another. The two keys stayed the
  same across both runs and the restart, so n0 could tie the five connections
  together. They are deleted now.
- **IP addresses.** Each relay connection was TCP to `5.78.69.43:443`, from
  `10.1.1.236` and `10.1.1.239` behind the one gateway, `10.1.1.1`, so usw1 saw
  one public address for both machines. The node printed no public address,
  and none was looked up. In run 2 the UDP socket on the Wi-Fi address could
  also reach n0's QAD, which reflects the NAT's mapping of UDP 4545 (not
  observed).
- **Timing.** Each arrival at usw1, 3.1 to 3.6 s after a start, and each
  departure: dabeest's by force, the Pi's by SIGTERM. In run 1 every datagram
  of the link crossed usw1: 238 s of link from HELLO to dabeest's end, and the
  Pi's 35 s after. In run 2, only the link's first quarter of a second, then
  the relay path's pings beside the direct one (section 9; not observed).
- **Volume.** Not measured: no capture, as ruled. In run 1 it was all of the
  link: HELLO, both rounds (4 and 4 records) and about four and a half minutes
  of keep-alives. In run 2, HELLO and at most the rounds' first packets.
- **Beyond the four**, as section 9 sets out: both runs' connections began
  through usw1 (each end's first `link` line reads `via relay`), so their first
  packets carried the ALPN, `glade/node/3`, past n0; and the pairing, above.
  Not observed directly: there was no capture.
- **Added by the run:**
  - One relay, n0's US West, at every start. So the relayed round trip was
    about 430 ms by the Pi's estimate, and a relayed round 348 to 435 ms.
    `aps1-1`, section 2's example, was not chosen.
  - A dialer needs no home relay of its own to reach its peer through the URL
    it names.
  - The URL did not go stale: the Pi's home relay was usw1 at all three
    starts.
- **Not visible to n0**, as section 9 says: node ids, HELLO, the binding, the
  `home` records and app data.

**Adaptations of the commands**

1. The Pi's build ran with `PATH=$HOME/.cargo/bin:$PATH`: a non-interactive
   ssh shell there has no `cargo`, since only `.profile` and `.bashrc` source
   `~/.cargo/env`.
2. `--offline` on both machines' `cargo build` and `cargo test`, to hold the
   run to its permitted network (GitHub, and n0's relays in the runs).
   `node/Cargo.lock` is unchanged since `63a5799`, which both machines had
   built and tested, so every crate was cached, and nothing was fetched.
3. `--no-fail-fast` on `cargo test`: without it cargo stops at the first
   failing test binary, `binding_census`, which fails for its siblings, and
   the suites after it never run.
4. The build and the suite wrote to `$S/build.log` and `$S/suite.log`, timed
   with `date`.
5. Each start was preceded by `date +%s.%N > <name>.t0`, the stamp the times
   above count from.
6. The Pi's restart logged to `pi45-2b.log`, since the commands name no log for
   it, so that `pi45-2.log` keeps its `exit 0`.
7. On the Pi, process checks used an anchored or bracketed pattern (`pgrep -f
   "^$S/target/debug/glade-node "`, `pgrep -af
   "[/]home/gianni/git/glade-wz/scratch/4.5/target"`): a bare one matches the
   checking shell's own command line, as it did at the preconditions.
8. During the runs, the logs were read through a filter on each machine that
   replaced any id the machine held with a marker. None appeared.
9. After the Pi's SIGTERM, `/proc/<pid>` and `ss` were polled every 5 ms, to
   time the exit and the port's release.
10. From the Mac, dabeest's ssh ran with `-o LogLevel=ERROR`, which drops the
    client's post-quantum warning, and both with `-o ConnectTimeout=10`.
11. Checked, not changed: dabeest's `timeout` is MSYS's `/usr/bin/timeout`,
    ahead of Windows' `timeout.exe` on its `PATH`; `$PY` is
    `/c/Users/gianni/AppData/Local/Programs/Python/Python313/python.exe`
    (3.13.5), its `python3` being the Store's stub; and
    `CARGO_TARGET_DIR=$S/target` reaches cargo as
    `E:/git/glade-wz/scratch/4.5/target`, since MSYS converts it.
12. Read only, beside the commands: each node's sockets in each run, dabeest's
    firewall rules naming the program and its console sessions, and the clock
    offsets.

**Also unexpected**

- dabeest's `instance` line mixes separators,
  `E:/git/glade-wz/scratch/4.5/home\sys\dab45`, and its recovery warning names
  the program as `'\\?\E:\…\glade-node.exe'` (the suite's failure above).
- The suite keeps its scratch in each machine's temp directory, outside `$S`:
  about 150 directories on each carry the run's time (5.2 MB on the Pi), under
  fixed names that earlier runs made too, and on dabeest one is named by PID
  and time, left by the failing test. They were left in place, being outside
  the named directories.

**Teardown**

- No process under either scratch target, and no `glade-node.exe` on dabeest.
- UDP 4545 free on both machines.
- No log holds an endpoint id. On each machine `grep -c -F -f` found 0 lines
  in every `*.log`, the build and suite logs included, with the other
  machine's id file, the machine's own id file, and its own id taken again by
  `endpoint-id` at the teardown (identical to the first, by `cmp`). No 64-digit
  hex string in a run log is anything but the two node ids.
- The logs and start stamps were copied to the Mac, identical by `md5`. Then
  `rm -rf $S` on both machines, 6.3 GB on the Pi and 6.6 GB on dabeest: the
  instances, both keys, the configuration and id files, the logs and the
  builds. The rest of `scratch/`, and both glade checkouts, clean at
  `4a34168`, are as they were.

## The mesh on the carrier port (plan Step 4.5b)

Design addition, 2026-09-27, written before any code against glade `71b9f0b` (F5; glade-wz
root `efafed9`). A document only: nothing was changed, built or run. The red runs and the
measured figures are filled in as each part is built.

The spec is plan Step 4.5b (`dev-docs/GladeFirstSlicePlan.md:895-909` at the glade-wz root) and
these rulings:

- The owner, 2026-09-25, on 4.2c (plan `:782`; this note's 4.2c questions, `:3647-3665`): the mesh
  moves onto the carrier port in a step of its own, after 4.5 and before 4.6, so that 4.6's journeys
  run over the carrier the node keeps; the adapter keeps its ALPN, `glade/carrier/1`, and its first
  word, `gcl1`; the wait for the first word gets a bound here, where the adapter first faces other
  machines.
- 4.2c's question 1 (`:3649-3656`) names what the move needs: HELLO's exporter bytes (D6) through
  the port, or a session-level replacement for them; the door's hook on the adapter's endpoint; the
  sync driver on a link's frames; 4.5's bind address and relay mode; both roots lending the adapter
  the node's endpoint key.
- 4.5c's ruled design (`GladeDirectoryCheckpoints.md:700-705`): once the mesh rides
  `glade/carrier/1`, HELLO's `protocol` check (`peer.rs:174-176`) is the only version gate, and 4.5c's
  move to `PROTOCOL` 4 rides it. This step must keep that gate working.
- `GladeNodeSigning.md` D6 and D7 (`:212-264`), ruled as recommended: HELLO signs a transcript bound
  to the TLS session, under `glade/v1/peer-hello\0`.
- The standing rules: no process globals (glade's `AGENTS.md`); nothing listens beyond loopback by
  default, or reaches a network the owner has not configured (4.5, section 1).

Paths: `node/src/…` is `glade/node/src/…`, with line numbers at `71b9f0b`, and `dev-docs/…` alone
is the glade-wz root's. Every node file cited but `mesh.rs` is as it was at `e27cb71`; F5 moved
`mesh.rs`, and the node lane's next steps may move it again before this one is built.

Nothing changes in the wire IR, in the node's own IR or in the dependencies: no crate is added and
no `Cargo.lock` line moves. The carrier contract gains one method.

### Summary

**The recommendation.**

- **One carrier link per peer**, the link 4.2c's adapter makes: one QUIC connection with one stream,
  opened by `gcl1`. HELLO is its first frame each way. After HELLO the mesh carries on it every
  exchange it now gives a QUIC stream of its own (the two pulls, each push, each forwarded interest
  and exchange, each pull on a gap), each as a conversation: frames under a 5-byte header, ended as
  a QUIC stream is. One writer task is the link's only sender and one reader task its only receiver,
  so a cancelled conversation never tears a frame (sections 2 and 3).
- **HELLO through the port.** `CarrierLink` gains `channel_binding(label)`: 32 bytes both ends of the
  link's transport session export under `label`, and no other session does, or `None`. HELLO asks for
  `glade/v1/peer-hello`, the label it exports under today, so D6's transcript is unchanged, byte for
  byte. CA-006 pins the method (section 4).
- **The version gate.** `glade/node/3` retires: the endpoint offers `glade/carrier/1` alone. `PROTOCOL`
  stays 3, and from here HELLO's number is the node protocol's only gate, which 4.5c moves to 4 as
  ruled. A node of 4.5 or older fails at the TLS handshake, whichever end dials, before the door or
  HELLO (section 5).
- **The door** is the adapter's endpoint hook, lent by the root with the key; HELLO's half runs on
  the link, in the mesh. A refused dialer learns no reason (section 6).
- **The first word** is awaited 10 s at most, from the connection's arrival to its fourth byte. At
  expiry the attempt is closed with code 0 and no reason, the door reports it, and the port accepts
  the next. HELLO gets a 10 s bound of its own and leaves the accept loop (section 7).
- **The sync driver** runs the same exchanges with the same meaning: pulls, pushes, pull on a gap,
  forwards, exchanges, D9, the grant check, the rounds and 4.5's `link` and `home round` notes.
  Frames are at most 16 MiB, and every serve path chunks (section 8).
- **4.5's configuration** reaches the adapter as values the root lends it (the key, the door, the
  relays) and the sockets `bind` is given; a dial names every address its entry names. The release
  wait keeps probing the port's own sockets, beyond loopback too (section 9).
- **Both roots** build one adapter from the booted instance. The assembled root lends that adapter
  to the module's peer role, binds it in `PeerCarrier` and releases it with the port's `close`;
  `EndpointSlot` retires. No value comes from a process global (section 10).
- **What retires:** `PeerEndpoint`, `PeerLink`, `EndpointSlot`, the ALPN `glade/node/3`, the stream
  HELLO and the hand-written `u32` writers. **Nothing stays for compatibility** on the wire. The async
  witness, which builds on `PeerEndpoint`, is frozen at the last revision that has it (section 11).
- **The desk** sees nothing but ports (section 12).

**The split** (section 14), each part gated, replayed on a stand-in of the desk and landed through
gwz, then 4.5's crossing again:

| Part | What | Production | Tests |
| --- | --- | --- | --- |
| 1 | The port's half: `channel_binding` and CA-006; the adapter lent a door and relays, bound on the sockets it is given, dialing every address; the first word's bound; the notes port | ~270 | ~300 |
| 2 | The link's conversations and HELLO on a link, as a library | ~280 | ~300 |
| 3 | The move: the mesh and the exchanges on conversations; both roots lend and bind the adapter | ~+350/−330 | ~+350/−150 |
| 4 | The retirement: `PeerEndpoint`, the stream HELLO, their tests | ~+20/−330 | ~−250 |

Parts 1, 2 and 4 change nothing a running node does; part 3 does.

**Questions for the owner**, at the end, each with a recommendation:

1. One link per peer, the node's conversations on it.
2. `channel_binding(label)` in the carrier contract, with CA-006.
3. `glade/node/3` retires, `PROTOCOL` stays 3, and no compatibility window.
4. The first word's bound: 10 s over the whole attempt, reported; HELLO's own, off the accept loop.
5. The path and relay notes through a node-local port, not the contract.
6. Frames of at most 16 MiB; chunks of 64 ops or 1 MiB.
7. The release wait keeps probing the port's own sockets beyond loopback.
8. One adapter per node, lent to the module and bound by `PeerCarrier`.
9. What retires; the library's sync driver stays; the async witness is frozen.
10. Four parts, then the crossing.

### 1. What the mesh does on QUIC today

The mesh keeps each peer's iroh `Connection` (`mesh.rs:80`) and gives each exchange a QUIC stream
of its own:

| Exchange | Opened by | Today | Ended by |
| --- | --- | --- | --- |
| HELLO | the dialer | stream 0's first two frames (`iroh_carrier.rs:376-397`, `peer.rs:200-268`) | the WELCOME, or the stream dropped |
| the dialer's pull | the dialer | stream 0 after HELLO; the acceptor serves it (`mesh.rs:358-367`) | the acceptor finishing the stream, "close = gap complete" (`mesh.rs:734`) |
| the acceptor's pull | the acceptor | a stream it opens (`mesh.rs:368`) | the same |
| a push | the minting node | a stream per push per link, one `Ops` frame (`mesh.rs:482-498`) | a finish |
| a forwarded interest | the forwarding node | `Subscribe`, then the ack, the gap and live ops (`mesh.rs:658-703`, `:513-587`); since F5 a refusal too | the forwarder's close, or the holder's finish |
| a forwarded exchange or `workspace.create` | the requesting node | `ExchangeReq`, then `ExchangeRes` (`exchange.rs:219-235`, `:243-268`) | a finish |
| a pull on a gap | the receiver of a refused push | `Heads`, then the gap (`mesh.rs:913-923`) | a finish |

Every inbound stream is dispatched by its first frame (`mesh.rs:344-354`, `:438-472`). The framing
is the node's own `u32` length prefix (`peer.rs:45-63`), written by hand twice more
(`mesh.rs:546-547`, `exchange.rs:264-266`), with no limit on a frame's length. QUIC gives each stream
its own flow control, at most 100 open bidirectional streams a connection (noq-proto 1.3.0, the
version in the node's lock, `src/config/transport.rs:557`), and a reset when a stream's handle drops
unfinished.

The port offers less: one ordered duplex of frames per link, each at most `max_frame_bytes`, ended as
a whole (`contracts/carrier-api/src/lib.rs:130-171`; below, `carrier-api/src/lib.rs`). "An adapter
MAY carry many links over one transport connection; this port promises nothing about reuse"
(`:115-116`). 4.2c's adapter makes each link one QUIC connection with one stream (this note,
`:3524-3530`; `iroh_carrier.rs:550-590`).

### 2. The unit of a link (question 1)

- **(a) One link per peer, conversations on it. Recommended.** The link the dialer dials and HELLO
  proves carries every exchange between the two nodes, each a conversation (section 3). The adapter
  stays as 4.2c built it and the owner ruled it: one connection, one stream, `gcl1`, and no task of
  its own. HELLO runs once per link, as it runs once per connection today, and every frame on the
  link is from the node it proved. The cost: the conversations share one stream's flow control, so
  a conversation that reads slowly slows its link; and the node carries a small multiplexer, about
  240 lines.
- **(b) A link per conversation, the adapter carrying a peer's links on one connection.** Each QUIC
  stream is a link, as `dev-docs/IrohGladeMapping.md` leans ("one stream per `(share, glade_id, key)`
  interest or sync round", `:382-384`; per-stream flow control, `:125`), and as the contract allows.
  The exchanges keep QUIC's per-stream flow control and need no multiplexer. But:
  - the adapter becomes a connection manager: live connections by endpoint id, a `dial` of a bare id
    onto a live connection, an `accept` of new streams on every connection as well as of new
    connections (which needs tasks, or a hand-polled set of futures), and a link's close that leaves
    its connection up;
  - an acceptor reaches an admit-only peer only through such a live connection, so the mesh would
    rest on reuse the contract does not promise, and the contract would have to promise it, with a
    probe;
  - HELLO would run once per connection, and every later link be taken as that node's by an equal
    channel binding: a new trust rule, sound over TLS, but new;
  - it changes the link 4.2c built and the owner ruled.
- **(c) A link per conversation, each its own connection.** Every push and pull would pay a TLS
  handshake and a HELLO, about 1.5 s through a relay (the crossing, `:6530-6532`), and an acceptor
  could not reach an admit-only peer at all. Rejected.
- **(d) Sub-streams in the contract** (a `CarrierLink` that opens and accepts streams). (b)'s session,
  moved into every carrier's contract, the WebSocket one's included. Larger than the slice needs.

What decides it for the slice: the substrate already runs the client path as one lane of size-capped
frames, and calls a second lane "a transport change with no protocol change"
(`GladeSubstrateV1.md:236-251`); two nodes of the slice exchange a few hundred records and a few
forwarded zones; and (a) leaves the ruled adapter as it is and touches the contract only for HELLO's
binding. The mesh speaks only through conversations (section 3), so (b) stays open as a later change
of the layer beneath them, under a new HELLO number, with no change to the sync driver.

### 3. Conversations on a link

**The header.** After HELLO, every frame on the link begins:

| Bytes | Field |
| --- | --- |
| 0-3 | the conversation, a `u32` little-endian: odd from the link's dialer, even from its acceptor, never 0 |
| 4 | what follows: `0` a frame, `1` END, `2` RESET |
| 5 on | with `0`, one `Frame` as `frame.rs` encodes it: its type byte, then its CBOR |

- HELLO is the link's first frame each way, a bare `Frame` with no header (section 4).
- A conversation is opened by its first frame. The peer's handler is chosen by that frame, as a
  stream's is today: `Heads` a pull to serve, `Subscribe` a forwarded interest, `ExchangeReq` a
  forwarded exchange, `Ops` a push (`mesh.rs:438-472`).
- END says its sender sends nothing more on the conversation: QUIC's finish, on which every "close =
  gap complete" rests (`mesh.rs:734`). Its receiver reads the end as `UnexpectedEof`, as now.
- RESET is sent for a conversation dropped before its END, as QUIC resets a stream dropped
  unfinished. Its receiver reads an error, so a pull cut short is not a completed round.
- The link's end ends every open conversation with an error, never with a clean end.
- A conversation's number is never used twice on one link. A frame for a conversation this end has
  ended, or for a number of this end's parity it never opened, is dropped.

**Two tasks per link.**

- **The writer** is the link's only caller of `CarrierLink::send`. Conversations queue their frames
  to it, first in first out, and never wait for the network. So a conversation cancelled at any
  await point never leaves a frame torn, which would end the link for every conversation on it
  (`iroh_carrier.rs:825-829`). Today the served subscription's writer is aborted mid-write when its
  peer leaves (`mesh.rs:585`); writing to the link itself, that abort would end the peer's whole link.
- **The reader** is the link's only caller of `recv`. It reads the header, puts the rest in its
  conversation's queue, and starts a handler task for each conversation the peer opens. It decodes no
  `Frame`: a frame that panics the decoder (F12's shape value) ends that conversation's task, as it
  ends one stream's task today, not the link. A frame too short for its header ends the link.
- The reader also keeps the link's watch (section 8): the path every 250 ms, and at the link's end
  the unlink and the `link … closed` note.

**Bounds.**

- Each conversation's queue holds 16 frames. When one is full the reader waits, and the carrier's
  flow control holds the peer back: a slow conversation slows its own link and no other. The frames
  of a conversation whose handler has let go of it (dropped its receiving half) are dropped.
- At most 100 conversations the peer opened are open at once, QUIC's default above; a further one is
  reset at once.
- The writer's queue is unbounded, as each session's outbound queue already is (`mesh.rs:535`).

**Why the reader cannot wait for ever.** It waits only on a full queue whose handler is running. No
handler waits for the writer, since it only queues, and none holds a node lock (`cut`, `store`,
`links`) across a receive: `pull_home` and `run_forward` take them per op, in `ingest_and_fanout`
(`mesh.rs:1003-1026`), and `serve_home` collects its gap under the store lock and sends after
(`mesh.rs:717-730`). So every full queue drains. Every later handler must keep that rule, and the
module's documentation states it.

**The shape**, in a new, carrier-free module, `node/src/conversation.rs` (it names `CarrierLink` and
`Frame`, and no iroh type):

```rust
/// One HELLO'd carrier link and its conversations (plan Step 4.5b).
pub(crate) struct Linked { /* the link, its node, its writer's queue, its open conversations */ }

impl Linked {
    /// Start the link's writer and reader; each conversation the peer opens
    /// goes to a handler the caller supplies.
    pub(crate) fn start(
        link: Arc<dyn CarrierLink>,
        node: [u8; 32],
        dialed: bool, /* the handler, the tasks, the limits */
    ) -> Arc<Linked>;
    /// A new conversation of this end's.
    pub(crate) fn open(self: &Arc<Self>) -> Conversation;
    /// End the link from anywhere, never waiting: its writer closes it.
    pub(crate) fn end(&self);
}

impl Conversation {
    /// Queue `frame` for the writer; a frame over the link's limit is refused here.
    pub(crate) fn send(&self, frame: &Frame) -> io::Result<()>;
    /// The next frame; `UnexpectedEof` at its END.
    pub(crate) async fn recv(&mut self) -> io::Result<Frame>;
    /// END. Dropped before it, the conversation is reset.
    pub(crate) fn end(self);
}
```

### 4. HELLO through the port (question 2)

D6 signs `{1: protocol, 2: role, 3: node id, 4: dialer endpoint id, 5: acceptor endpoint id, 6:
exported bytes}` (`peer.rs:111-138`), the bytes being 32 exported from the connection's TLS session
under `glade/v1/peer-hello` (`iroh_carrier.rs:63-78`). Through the port the mesh has the far end's
endpoint id, `remote_id()`, and its own, from the key the root lends the adapter. It lacks the
exported bytes: no port method gives them (4.2b's gap for later, `:3178-3180`).

- **(a) An accessor. Recommended.** `CarrierLink::channel_binding(&self, label: &[u8]) ->
  Option<ChannelBinding>`, with `ChannelBinding(pub [u8; 32])`: the bytes both ends of the link's
  transport session derive under `label` (TLS's exporter, RFC 8446 §7.5, as RFC 9266 uses it for a
  channel binding), which no other session derives; `None` from a transport with no session secret,
  as the WebSocket client carrier will be. Links an adapter carries over one session share them, and
  that is what binds a HELLO to its session. HELLO asks for `glade/v1/peer-hello`, so field 6, and
  D6, stay as they are. The iroh adapter answers with the connection's `export_keying_material`
  (iroh 1.2.0 `src/endpoint/connection.rs:1085`), while the link lives.
- (b) The same accessor with no label, RFC 9266's fixed one. Simpler to state; field 6 then changes
  its label, a D6 detail, and every later use of an exporter shares one value.
- (c) A nonce each way before HELLO, as bare frames below the IR. No contract change, but one more
  round trip before HELLO, about 430 ms through a relay (`:6533-6536`); D6 weighed a challenge as its
  option (c) and set it aside.
- (d) No binding. The transcript keeps both endpoint ids, so a relay through a third party still
  fails; but a HELLO recorded on one connection would verify on the next between the same two
  endpoints, and whoever holds a node's `endpoint.key` could speak for the node without its
  `node.key`, which 4.2's two keys exist to prevent. Rejected.
- (e) HELLO in the adapter. The contract says a carrier authenticates no node
  (`carrier-api/src/lib.rs:132-135`). Rejected.

**The contract's change**, for (a): one required method, as `remote_id` was (4.2b), and its name in
`CarrierLink`'s row of `glade/contracts/architecture-policy.json:43`, for the owner's review as that
file's earlier changes were. CA-006 (section 13) runs on the contract's fixture, the node's fake
network, the journeys' faulty link (a keyed checksum over a per-link token and the label, never
cryptography) and iroh. No type crosses the port but bytes (LBT-004).

**HELLO on a link**, both roles, as `peer.rs` does it on a stream today (`peer.rs:200-268`):

- the dialer's first frame is `NodeHello` and the acceptor's `NodeWelcome`, bare;
- the channel is the dialer's endpoint id, the acceptor's, and the link's
  `channel_binding(b"glade/v1/peer-hello")`; a link whose `remote_id` is not 32 bytes, or which has
  no binding, cannot complete HELLO;
- the checks are today's: the protocol (`peer.rs:174-176`), the signature for this transcript, the
  door's `binds` (`peer.rs:187-198`); then D9's `authenticated` (`mesh.rs:326`), the refusal lines
  and the refused HELLO left unanswered.

### 5. The version gate (question 3)

Today the ALPN names the node protocol, `glade/node/3` (`iroh_carrier.rs:54-57`), and HELLO checks
the number as well (`peer.rs:174-176`), so a node of an older protocol fails at connect (4.1a, 4.1b).

After this step the endpoint offers `glade/carrier/1` alone. That names the carrier's framing and its
first word, as ruled, and nothing of the node protocol, which is now: HELLO (D6) as the link's first
frame each way, then section 3's conversations, each carrying the frames its stream carries today.
HELLO's `protocol` is that protocol's only gate.

- **`PROTOCOL` stays 3. Recommended.** No node has spoken `glade/carrier/1` before this step, since
  no root lends the adapter a key before part 3 (`assembly.rs:761-777`), so no carrier peer can
  mistake the number, and 4.5c's ruled move to 4 needs no amendment. A HELLO of the stream era cannot
  be replayed on a carrier link either: its exported bytes belong to another TLS session. The
  alternative, 4 now for the new framing and 4.5c at 5, changes a ruled answer for no peer.
- **How an older peer fails.**
  - A node of 4.5 or older, dialing or dialed: the TLS handshake finds no common ALPN and fails,
    before the door's hook and before HELLO. The dialer prints `peer <tag>@<address>: <error>`. The
    acceptor prints nothing, as for any failed handshake today (`mesh.rs:296`).
  - A 4.5b node and a 4.5c node: HELLO is refused, `protocol 3, not 4`. The acceptor prints `peer
    refused: endpoint <tag>: HELLO refused: protocol 3, not 4`; the dialer's link ends with no reason.
    This step pins that gate over the port (section 13), for 4.5c to turn.
- **No window of both ALPNs.** Nothing deployed links: the desk has no peer, and the crossing's
  instances are deleted (`:6683-6696`). A window would keep `PeerEndpoint`'s stream protocol, about
  400 lines, beside the new one.
- n0 now reads `glade/carrier/1` in a relayed connection's first packets, where it read
  `glade/node/3` (`:5928`, `:6621`): the carrier's name, no longer the node protocol's version.

### 6. The door on the adapter's endpoint

- **Where.** The accept-time half stays iroh's `after_handshake` hook on the accepting side
  (`iroh_carrier.rs:156-186`), now on the adapter's endpoint: the root lends the adapter the door with
  the key (section 9), and `bind` hands it to the recipe, where 4.2c hands it none
  (`iroh_carrier.rs:618`). The HELLO half runs on the link, in the mesh (section 4). The mesh still
  loads the one door before its first accept and feeds it each record that lands
  (`mesh.rs:256`, `:1003-1026`).
- **Alternatives weighed.** The mesh could check `remote_id()` after `accept`, with no hook; every
  key would then complete TLS, open a stream and hold the first word's wait before its refusal,
  where the relay ruling asked for a lock on the door. Or the adapter could hold an admission policy
  of its own; the door is one shared object now, and stays one.
- **What a refused dialer sees.**
  - Refused at accept: the connection is closed with code 0 and no reason (`:3139-3143`). The
    adapter's `dial` may already have answered, since the dialer's handshake ends before the
    acceptor's hook runs; HELLO then reads the link's end. Either way the error names no reason
    (`connection lost`, or `the link ended before a WELCOME`), and the root prints `peer
    <tag>@<address>: <error>`, as 4.2b and 4.5 laid it out.
  - Refused at HELLO, or by either bound: the acceptor sends no WELCOME and ends the link; the same.
  - The refusing node prints `peer refused: endpoint <tag>: <reason>` on stderr, as now.
- **A revocation that lands** still ends the revoking node's live link on that key
  (`mesh.rs:1031-1037`). The mesh finds the link by node, compares `remote_id()` and calls
  `Linked::end`, which returns at once; the writer closes the link. The port's `close` drains for up to
  3 s, so it must not be awaited under the cut lock `ingest_and_fanout` holds, where today's
  `conn.close` returns at once.

### 7. The first word's bound, and HELLO's (question 4)

**The wait today.** The adapter's `accept_on` waits for the handshake, the stream and the four bytes
with no bound (`iroh_carrier.rs:577-590`; 4.2c's gap, `:3624-3625`). A dialer the door admits that
connects and sends nothing holds that `accept`, and the port takes one attempt at a time. HELLO has
no bound either, and runs inside the accept loop (`iroh_carrier.rs:388-397`, `mesh.rs:284-299`;
4.2b's gap, `:3400-3401`).

- **The bound: 10 s**, from the moment the endpoint hands the port an incoming connection to the
  word's fourth byte: the handshake, the door's hook, the stream and the word under one deadline.
  The crossing's whole HELLO, from the dial to the `link` line, took 1.4 to 1.6 s through n0's
  relay at about 430 ms a round trip (`:6530-6536`, `:6567-6569`), and the first word arrives in
  less than half of that. So 10 s is more than ten times what the relay took: room for a far slower
  path and a lost first flight. It is also the length of the node's other waits, `STOP_WITHIN`
  (`lifecycle.rs:88`) and the provider's timeout (`exchange.rs:46`).
- **At expiry** the adapter closes the connection with code 0 and no reason, and answers that
  `accept` with `Err(Transport("no first word within 10 s"))`: the contract makes that one refused
  attempt, the endpoint staying usable (`carrier-api/src/lib.rs:122-123`), and the mesh's accept loop
  goes on (`mesh.rs:296`). Once the handshake has proved a key, the door reports `peer refused:
  endpoint <tag>: no first word within 10 s`; before that nothing is reported, as for any failed
  handshake.
- **HELLO's bound: 10 s**, from the moment the port hands over the link, at either end, to a
  verified `NodeHello` (the acceptor) or `NodeWelcome` (the dialer). HELLO leaves the accept loop
  for the accepted link's own task (`Site::AcceptedLink`), so a slow HELLO no longer holds the next
  dialer. Refused: `peer refused: endpoint <tag>: HELLO refused: no HELLO within 10 s`; the
  dialer's error reads `no WELCOME within 10 s`.
- Both bounds are constants, passed as arguments to the adapter's constructor and to HELLO, so tests
  pass 200 ms: no flag, no global.
- The adapter still takes one attempt at a time, so a dialer the door admits can hold the others up
  to 10 s an attempt (named gap). Handshakes in parallel would need tasks the adapter does not have.
- **Alternatives weighed.** 5 s, which a slow relay's first contact could miss; 30 s, QUIC's idle
  timeout (noq-proto 1.3.0 `src/config/transport.rs:560`), which adds little over none; a bound on the
  four bytes alone, which leaves the handshake and the stream unbounded.

### 8. The sync driver over the port

| Exchange | Today | On the port |
| --- | --- | --- |
| HELLO | stream 0's first two frames | the link's first frame each way, bare (section 4) |
| the dialer's pull | stream 0 after HELLO | the dialer's first conversation, `Heads`, served and ended by the acceptor |
| the acceptor's pull | a stream it opens | the acceptor's first conversation |
| a push | a stream per push per link | a conversation per push per link: one `Ops`, END |
| a forwarded interest | a stream | a conversation: `Subscribe`, then the ack, the gap in chunks, live ops, or F5's refusal |
| a forwarded exchange, `workspace.create` | a stream | a conversation: `ExchangeReq`, `ExchangeRes`, END |
| a pull on a gap | a stream on the pusher's link | a conversation on the pusher's link |

- **The rounds and the notes.** `home round with node <id>: <n> record(s) in <ms> ms` is timed around
  the pull's conversation, as `mesh.rs:372-379` times the stream. `link <node> via relay <url>|direct
  <ip:port>, rtt <n> ms` is noted at HELLO and at each change the reader's 250 ms poll sees
  (`mesh.rs:383-406` today), the path coming from the adapter through the node-local notes port
  (section 9). `link <node> closed` is noted when the reader sees the link end, and `peer-connected`
  follows the dialer's pull, as now.
- **Pull on a gap** is unchanged: its table, one pull per pusher, its lines (`mesh.rs:885-923`);
  `pull_from` opens a conversation where it opened a stream.
- **D9 and the grant check** are unchanged: every conversation on a link belongs to the node its HELLO
  proved (`Holder::Node`), as every stream of a connection does today.
- **The forwards**, F5's refusal included (`mesh.rs:597-637`, `:658-703`), read the conversation as
  they read the stream. The claim holder's subscription writer ends its conversation where it
  finished its stream.
- **The frame limit: 16 MiB** (question 6), the roots' `CarrierConfig::max_frame_bytes`, less the
  header for a frame. Today no frame has a limit (`peer.rs:54-63`), and the forwarded gap is one
  frame, however long (`mesh.rs:573-575`). So every serve path chunks, at `OPS_PER_CHUNK` (64) ops or
  once a chunk holds 1 MiB: the pull's serve (`mesh.rs:730`) and the forwarded gap. A single op over
  the limit cannot cross: its conversation's send refuses it, with a line naming the zone (named gap;
  the wire's `Chunk` frame is the later remedy).
- **The release.** `release_links` still takes the table by value (`mesh.rs:644-654`) and closes each
  link, all together, each within the adapter's 3 s drain; a link's close carries no reason, where it
  said `glade node stopping` (`mesh.rs:650`). The port's `close`, in `PeerCarrier`'s release, then ends
  anything left (CA-004).
- **The table's race.** The reader removes its node's entry only while it still holds that link;
  today's unlink removes by node id (`mesh.rs:334-341`), and so could remove a newer link's entry.

**The tasks** (`tasks.rs:37-73`), all `Sessions`' unless said:

| Site | Before | After |
| --- | --- | --- |
| `AcceptLoop` | accept, HELLO inline | accept only |
| `AcceptedLink` | the accepted link's driver | its HELLO, bounded, then its driver |
| `Unlink`, `StreamDispatch` | watch the connection; accept its streams | one `LinkReader`: route frames, watch the path, unlink at the end |
| (new) `LinkWriter` | | the link's only sender |
| `PeerStream`, `StreamZero` | one inbound stream; the dialer's stream 0 | one `InboundConversation`, chosen by its first frame |
| `RecordPush` (`Records`) | a push on a stream of its own | retires: a push only queues its frame, so `Records` keeps the renewal loop alone |
| the rest | | unchanged |

### 9. 4.5's configuration through the adapter

- **What the root lends the adapter**, as `IrohCarrier::new`'s argument and the module's component
  parameters: `Lent { key: EndpointKey, door: Option<Arc<Door>>, relays: Relays, first_word:
  Duration }`, where 4.2c lends `Option<EndpointKey>` (`assembly.rs:761-777`). Lent none, it refuses
  to bind, as now. The CA probes lend no door.
- **`bind`.** `CarrierConfig::local` names the sockets, `<ip:port>[,<ip:port>]`, at most one per
  family as the file allows, an `<endpoint-id>@` prefix ignored as 4.5 made it
  (`iroh_carrier.rs:593-602`). The recipe is `bind_endpoint` with the lent key, door and relays
  (`:92-114`), where the adapter now passes one socket, relays off and no door (`:614-618`). It
  answers `<endpoint-id>@<socket>[,<socket>]` as bound, IPv4 first, and the root prints `peer <tag>
  <first socket>` from that, as now.
- **`dial`.** A `CarrierAddr` takes a peer entry's form with the full id, `<endpoint-id>@<via>[,<via>]`,
  each via an `ip:port` or one of n0's relay URLs, and becomes one `EndpointAddr` with every address
  (`endpoint_addr`, `:141-154`), where the adapter now dials one socket (`:629-641`). A relay URL needs
  a port lent `relay n0`, as the file's load already demands. Two helpers in `iroh_carrier.rs` turn a
  `PeerEntry` into such an address and a bound address into the `peer` line's tag and socket, so the
  syntax stays the adapter's.
- **Peers** are not the adapter's: the door holds the keys to admit, and the mesh dials each entry
  that has an address, as now.
- **The notes port** (question 5). The `link` lines need the selected path and the `relay` lines the
  home relays' states (4.5 part 2; `iroh_carrier.rs:416-462`, `:331-359`), which no port method
  carries. A node-local port, `LinkNotes`, beside `TransportPort` in `assembly.rs`: `path(&TransportId)
  -> Option<PathSeen>`, for the newest live link to that endpoint, and `relay_watch(seen)`, a future
  that reports the states until the port closes, with relays only, holding no endpoint handle, as now.
  `IrohCarrier` implements it; the mesh takes it beside the port, optionally, and a node without it
  notes no path. iroh's types still stop in the adapter. The alternative, `CarrierLink::path()` and a
  relay state on `CarrierPort`, puts iroh's notions into every carrier's contract for the sake of
  status lines.
- **The release wait beyond loopback** (question 7; 4.5's named gap, `:6243-6247`). The adapter's
  `close` binds each socket it held, for an instant, to see that iroh has let it go
  (`iroh_carrier.rs:681-697`). Bound beyond loopback, it binds that address too. Recommend keeping
  it: it binds only sockets this port held a moment before, which the owner's file named, from the
  same process; it sends and reads nothing; and without it CA-004's promise, that `close` frees the
  address, holds on loopback alone. The alternatives: probe loopback sockets only, and beyond loopback
  resolve at iroh's close, so that an in-process re-bind there races iroh's few milliseconds; or no
  probe, with which CA-004 failed 21 runs in 25 (4.5 part 1).
- **Nothing listens beyond loopback by default.** With no file the network is `127.0.0.1:0` with
  relays off (4.5, section 1), and the adapter binds exactly the sockets the root passes, calls for
  relays only when lent `relay n0`, and dials only the addresses an entry names.

### 10. Both roots lend the adapter (question 8)

- **The hand-written root** (`bin/glade-node.rs:371-393`), after adoption, where it binds
  `PeerEndpoint` today: it builds the door as now, then `IrohCarrier::new(Some(Lent { key:
  node.endpoint_key(), door, relays: network.relays, first_word }))`, binds it on the network's sockets
  with the frame limit, prints `peer <tag> <socket>`, and hands the mesh the adapter, as the port and
  as the notes, with the node's identity and endpoint id. The dials follow, as now.
- **The assembled root** (`lifecycle.rs`):
  - `Instance` builds the same adapter, unbound, from the booted values: `Booted` (`:181-192`) holds
    it beside the door;
  - `Assembly` lends it to the module as `peer_carrier_binding`'s parameter, so the module's peer role
    and the node's mesh are one adapter, 3.2's one iroh-facing occurrence (`:69-75` of this note).
    `IrohCarrier` becomes a cheap clone over one shared state, and the component hands the module a
    clone;
  - `PeerCarrier` binds it, where it binds `PeerEndpoint` into an `EndpointSlot` (`:552-578`), and its
    release calls the port's `close`, the by-value discipline the contract states for a port released
    through an `Arc` (`carrier-api/src/lib.rs:53-64`). `EndpointSlot` and `PeerEndpoint::close(self)`
    retire;
  - `Sessions` enables the mesh over the bound port and prints the `peer` line, as now (`:620-626`);
  - the plan's edges do not change: `PeerCarrier` still needs `Instance` and `Storage`.
- **The alternative:** `PeerCarrier` builds an adapter of its own, as it builds `PeerEndpoint` today,
  and the module's peer role stays unlent. Fewer moving parts, but two iroh-facing objects per node,
  one of them inert, where 3.2 said the mesh would move onto the carrier binding (`:107-109`).
- **No process globals.** The key comes from the booted instance (`endpoint.key`, 4.2a), the network
  from the file and flags the entry point read (4.5), and the door from the root; the bounds and the
  frame limit are constants passed down. Nothing reads the environment, and neither the adapter nor
  the conversations keep a static. The adapter spawns no task: its accept, dial and close run in
  their callers' futures, and the relay watch is a future the mesh spawns through `Tasks`. The
  process-globals check stays at 3 permanent entries and no debt.

### 11. What retires, and what stays (question 9)

| Retires | Where | Replaced by |
| --- | --- | --- |
| `PeerEndpoint`: its four binds, `dial`, `accept`, `close`, `relay_watch`, `door`, `identity`, `addr` | `iroh_carrier.rs:246-414` | the adapter and the conversations |
| `PeerLink` | `:237-244` | `Linked` |
| the ALPN `glade/node/3` | `:54-57` | section 5 |
| the exporter read on a `Connection` | `:63-78` | `channel_binding` |
| `EndpointSlot` | `mesh.rs:164-196` | the port's `close` |
| `hello_dial` and `hello_accept` over `AsyncRead`/`AsyncWrite` | `peer.rs:200-268` | HELLO on a link (part 2) |
| the hand-written `u32` writers | `mesh.rs:546-547`, `exchange.rs:264-266` | conversations |
| five sites | `tasks.rs:37-73` | section 8's |
| `PeerEndpoint`'s tests: `dial_and_hello_over_iroh`, `a_protocol_2_node_fails_at_connect`, `sync_over_iroh`, `close_frees_the_bound_port`, `close_ends_an_accept_loop_and_a_kept_clone_keeps_the_port` | `iroh_carrier.rs:1030`, `:1092`, `:1138`, `:1213`, `:1230` | the tests named in section 13 |

**What stays.**

- The library's sync driver, `serve_sync` and `pull_sync`, with `read_frame` and `write_frame` as
  their framing and their five tests (`peer.rs:303-402`, `:629-`). The mesh never called them; they
  are carrier-free by construction, and 4.5c part 2 is ruled to edit `serve_sync`
  (`GladeDirectoryCheckpoints.md:410-414`). Whether a driver with no production caller should go is a
  question of dead code, not of this move. The alternative retires them in part 4, about 120
  production lines and 260 of tests, and 4.5c part 2 then edits `serve_home` alone.
- `bind_endpoint`, `DoorHook`, `bound_addr` and `endpoint_addr`, now the adapter's alone; the notes'
  two types, which move to the notes port; the transcript and its checks (`peer.rs:104-198`); the
  door; the network's types; every line's form.
- **On the wire, nothing** (section 5).

**The async witness.** `glade/dev-docs/async-witness/real` builds on `PeerEndpoint`, `PeerLink` and
`PeerAddr` (`real/src/peer_carrier.rs:49`, `real/src/peer_plan.rs:34`) through a path dependency on
the node (`real/Cargo.toml`), and each node step has type-checked it on a scratch copy. After part 4
it no longer builds against HEAD. Recommend freezing it at the last glade revision that has
`PeerEndpoint`, part 3's, with one line in its README naming that revision and how to check it out in
a worktree; its evidence is Phase 3's record, and is not rewritten. The alternatives: keep
`PeerEndpoint` alive for it, as code no node runs and the gate still builds; or port the witness onto
`IrohCarrier`, which rewrites its evidence (its `WitnessCarrier` is itself a `CarrierPort` over
`PeerEndpoint`, which `IrohCarrier` now is in the node).

### 12. Compatibility

- **A 4.5b node and an older one** fail at connect, whichever dials, and print as section 5 says.
  Nothing is written.
- **A downgrade** from 4.5b: no file, record or format changes, so an older binary starts on an
  instance 4.5b ran, as before, and speaks `glade/node/3` again.
- **The owner's desk.** grazel passes no `--peer` and no `--config` (`:5511-5514`). At its restart on
  part 3:
  - the endpoint is the adapter's, bound by the same recipe on `127.0.0.1:0`, relays off, portmapper
    off, behind the door's hook (`iroh_carrier.rs:92-114`): UDP and TCP on `127.0.0.1` alone;
  - the lines are today's: `peer <tag> 127.0.0.1:<port>` with the same tag, nothing after
    `listening`, and on stderr the recovery warning alone;
  - what differs cannot be seen from the desk: the ALPN.
- **The replay before the desk's restart**, at each part: today's binary, this build, then today's
  again, on a stand-in laid out as grazel lays out the desk, with `lsof`. Parts 1, 2 and 4 must print
  the same lines; part 3 the same lines apart from ports.
- **Clients and suppliers** speak the websocket and see nothing.

### 13. Tests, each begun red

Each is run first against the code with the part it guards switched off, in a scratch copy of the
sources, and the message it prints is recorded, as in the steps before.

**Part 1** (the port's half):

| Test | Proves | Red against |
| --- | --- | --- |
| contracts: `ca_006_each_link_binds_its_transport_session` | on the fixture: both ends of a link derive the same bytes under a label; another label gives other bytes; links to two far ends differ; a fixture with no secret answers `None` on every link | the probe run on a fixture whose links all answer one value, which must fail it |
| contracts: `rejects_a_binding_every_link_shares`, `rejects_a_binding_that_ignores_its_label` | CA-006 refuses those two fixtures | none: they guard the probe |
| node: CA-006 on the fake network (`tests/assembly`) and through the faulty link (`tests/journeys`) | the fakes' checksum passes | a fake deriving its bytes from its own end, not the link's: the two ends disagree |
| `iroh_carrier`: `ca_006_iroh_binds_each_link_to_its_tls_session` | CA-006 on real iroh over loopback | an export that ignores `label` |
| `iroh_carrier`: `the_first_word_is_awaited_within_its_bound` | with a 200 ms bound, a raw dialer on the carrier's ALPN that sends nothing, and one that sends `gc`, are each refused within about the bound and reported by tag, and the port then accepts a genuine link | no bound: the accept still waiting after 2 s |
| `iroh_carrier`: `a_carrier_behind_a_door_refuses_an_unknown_key_at_accept` | the hook on the adapter's endpoint: an unknown key's connection is closed, code 0, no reason, and reported; a configured key links | the adapter bound with no door: the unknown key links |
| `iroh_carrier`: `a_carrier_binds_the_sockets_it_is_given_and_dials_every_address` | `local` naming a `127.0.0.1` and a `[::1]` socket, each found free, binds both and the answer names both; a dial address with an `ip:port` and a relay URL becomes one `EndpointAddr` with both (pure) | the adapter binding the first socket alone, and dialing one address as `PeerAddr::parse` reads it |
| `iroh_carrier`: `a_carrier_notes_each_links_path` | `LinkNotes::path` reads `direct 127.0.0.1:<port>` for a linked endpoint, `None` for another | a notes port answering `None` |

CA-001..005 run on iroh as before, each port lent a key and no door, and pass.

**Part 1, as built** on 2026-09-27 against glade `4f49fe6`. The tests use part 1's API, so each was
run red in one sources-only copy of the final tree (glade's `node`, `wire-rs` and `contracts`, with
`glade-decl-rs` beside them for the contracts' workspace), the part it guards switched off by one edit
and put back before the next. The message is what the red run printed; `<id>` is an endpoint id and
`<tag>` its tag.

| Test | Red first |
| --- | --- |
| contracts: `ca_006_each_link_binds_its_transport_session` | every link of the fixture answering one value under every label: `assertion left != right failed: CA-006 another label gives other bytes` |
| contracts: `rejects_a_binding_every_link_shares`, `rejects_a_binding_that_ignores_its_label` | none: they guard the probe, each expecting its own message, `CA-006 links to two far ends are bound apart` and `CA-006 another label gives other bytes` |
| node: `ca_006_the_fake_network_binds_each_link_to_its_session` (`tests/assembly`); `ca_006_a_faulty_link_binds_its_transport_session` (`tests/journeys`, and `tests/durable`, which includes the file) | the fake's bytes drawn from its own end, not the link's: `assertion left == right failed: CA-006 both ends of a link derive the same bytes`, in each of the three binaries |
| `iroh_carrier`: `ca_006_iroh_binds_each_link_to_its_tls_session` | an export under HELLO's label whatever label is asked: `assertion left != right failed: CA-006 another label gives other bytes`; for its D6 half (below), an export with a context HELLO does not use: `assertion left == right failed: the bytes HELLO signs` |
| `iroh_carrier`: `the_first_word_is_awaited_within_its_bound` | no bound (a day): `the accept still waiting after 5 s: Elapsed(())`; the door's report off: `reported by its tag`, `left: None`, `right: Some("peer refused: endpoint <tag>: no first word within 200 ms")` |
| `iroh_carrier`: `a_carrier_behind_a_door_refuses_an_unknown_key_at_accept` | the adapter bound with no door: `the unknown key linked` |
| `iroh_carrier`: `a_carrier_binds_the_sockets_it_is_given_and_dials_every_address` | the first socket alone bound: `both, IPv4 first`, `left: "<id>@[::1]:61330"`, `right: "<id>@127.0.0.1:60399,[::1]:61330"`; one address dialed, as `PeerAddr::parse` reads it: `linked: Transport("expected <endpoint-id>@<ip:port or relay-url>[,…] to dial")` |
| `iroh_carrier`: `a_carrier_notes_each_links_path` | the notes port answering `None`: `a path noted within 2 s`; with no relays, a watch that never ends: `the watch ends at once: Elapsed(())` |

As built, beside the design:

- **The contract's words.** `ChannelBinding(pub [u8; 32])`, and `channel_binding(&self, label: &[u8]) ->
  Option<ChannelBinding>`, required, in `CarrierLink`'s row of the contracts' policy. A link that has
  ended MAY answer `None`: the iroh adapter answers while the link holds its connection. CA-006 asks
  each link while it lives, under `glade/v1/ca-006` and `glade/v1/ca-006-other`, and a transport binds
  every link or none. The fakes' bytes are FNV-1a over the link's session (the pipe its dialer sends
  on, the same at both ends) and the label, in four lanes; the contract's fixture also runs with no
  session secret, answering `None` on every link.
- **D6, pinned.** `ca_006_iroh_binds_each_link_to_its_tls_session` also shows that under
  `glade/v1/peer-hello` a link answers the very bytes `channel()` exports for HELLO on its connection
  today, so part 2's transcript over the port is D6's, byte for byte.
- **`Lent`** (`key`, `door`, `relays`, `first_word`) is `IrohCarrier::new`'s argument, and
  `Option<Lent>` the component's parameters; `FIRST_WORD` is 10 s. Lent nothing, the adapter refuses
  to bind in the words it had, `the iroh adapter was lent no endpoint key`, so
  `tests/assembly_registration` is unchanged. `IrohCarrier` is a clone over one shared port.
- **The two syntax helpers**: `carrier_addr(&PeerEntry)`, an entry as `<endpoint-id>@<via>[,<via>]`,
  the form `bind` answers, and `entry_of(&CarrierAddr)`, the parse `dial` uses. A dial naming a relay
  URL from a port not lent `relay n0` is refused before anything is sent, `a relay URL needs relay
  n0`. The `peer` line's tag and socket come from `entry_of` and `transport::tag`; a helper for the
  roots waits for part 3, which prints the line.
- **The notes' two types**, `PathSeen` and `RelayState`, moved to `assembly.rs` beside `LinkNotes`, and
  are public; `mesh.rs` imports them from there. The relay watch is one function, `watch_relays`, which
  `PeerEndpoint::relay_watch` and the port's `relay_watch` both call. The port's takes a boxed sink and
  answers a `PortFuture`, so `LinkNotes` is dyn-compatible.
- **The first word's bound** runs from the endpoint's hand-over of the attempt to the word's fourth
  byte. At expiry during the handshake the attempt is dropped and nothing is reported: noq closes a
  connection whose last handle drops with code 0 and no reason (read in its source; no test holds a
  handshake open). After the handshake, the connection is closed, code 0 and no reason, and the door
  reports `peer refused: endpoint <tag>: no first word within 10 s`. A bound that is not whole seconds
  is written in ms, as the tests' 200 ms.
- **The tests dial through one bounded helper**, `dial_accept`, which the existing `linked` now uses:
  a failed dial fails the test at once, with its error. Found in the first red runs: with the dial
  switched to one address, `tokio::join!` of the dial and the accept left the accept waiting, and the
  test binary hung until it was killed.
- **The node's policy text** (`node/architecture-policy.json`) says the adapter runs CA-001..006 on
  real iroh, where it said CA-001..005.

**Part 1's CA-004 finding, fixed** on 2026-09-27 as the owner ruled (plan line 914), on glade `cde604d`.
A pending `accept` or `dial` holds the endpoint on a counted `Loan`. `close` waits for the release only
when no loan is out; otherwise the last loan to end waits for it, within `LINGER`, before its accept or
dial answers. A loan dropped with its future is uncounted and waits for nothing. CA-004 took 3.04 to
3.06 s, its release wait running out every run, and takes 0.03 s; with the release tests it passed 20
runs in a row, 8 of them beside a workspace `cargo test`. Red first, in a sources-only copy:
`close_leaves_the_release_to_a_pending_accept` and `_dial`, unfixed, `close waits on no accept's
handle: 3.006s`, and with the loan's wait off, `the accept answers once the address binds again`;
`only_the_last_loan_to_end_waits_for_the_release`, every loan waiting, `the first waits for no
release: 3.000s`; `a_dropped_accept_leaves_the_release_to_close`, a dropped loan left counted, `close
answers once the address binds again`. The tests hold one handle 100 ms more, so a missing wait fails
every run. For part 3: a root that rebinds an address after `close` lets its accept loop answer first.

**Part 2** (the link's conversations and HELLO on a link):

| Test | Proves | Red against |
| --- | --- | --- |
| `conversation`: `conversations_interleave_on_one_link_and_end_apart` | over an in-memory link pair: three conversations each way interleave; each ends by END while the others go on; each end hands the peer's conversations to handlers chosen by their first frames | one conversation per link: the second read as the first's |
| `conversation`: `a_conversation_dropped_before_its_end_is_reset` | the far end reads an error, not a clean end | a drop that sends END |
| `conversation`: `a_conversation_cancelled_mid_send_never_ends_its_link` | over two `IrohCarrier`s: a conversation's task aborted while its 4 MiB frame is being sent, past the peer's window; another conversation's frame then arrives whole | conversations calling `send` themselves: the link ends |
| `conversation`: `a_slow_conversation_holds_its_queue_and_one_let_go_holds_nothing` | a full queue of 16 holds the reader until it drains; a dropped conversation's later frames are dropped and the others flow | an unbounded queue; a reader waiting on a dropped conversation |
| `conversation`: `the_hundred_and_first_conversation_is_reset` | the cap | no cap |
| `conversation`: `the_links_end_ends_every_conversation_with_an_error` | open conversations read an error, never a clean end | a clean end |
| `peer`: `a_hello_on_a_link_binds_its_transport_session` | over two `IrohCarrier`s: HELLO completes both ways; a HELLO recorded on one link is refused on a second link between the same two ports; a reflected one is refused | a binding of fixed bytes: the replay accepted |
| `peer`: `a_hello_of_another_protocol_is_refused_on_a_link` | a `NodeHello` of protocol 4 is refused and unanswered, reported `protocol 4, not 3`: the gate 4.5c turns | the protocol check switched off |
| `peer`: `a_link_without_an_endpoint_key_or_a_binding_cannot_hello` | refused on a fake link (its `remote_id` is 8 bytes) and on a link with no binding | zeros taken as the channel |
| `peer`: `hello_on_a_link_is_bounded` | a dialer that never sends `NodeHello`, and an acceptor that never answers, are each refused within a 200 ms bound, the acceptor's refusal reported | no bound |

The in-memory HELLO tests (`peer.rs:436-626`) move onto the in-memory link pair and prove what they
proved.

**Part 2, as built** on 2026-09-27 against glade `f895aca`, red in one sources-only copy of the final
tree (glade's `node`, `wire-rs` and `contracts`), the part each test guards switched off and put back
before the next: by one edit, or by a few where the switch is the design's alternative (conversations
sending on the link themselves). The message is what the red run printed.

| Test | Red first |
| --- | --- |
| `conversation`: `conversations_interleave_on_one_link_and_end_apart` | every conversation of an end taking its first number, so the peer reads one: `a0: its own frames, in order`, `left: ["Subscribe(…)", "ExchangeReq(…)", "a0.0"]`, `right: ["a0.0"]` |
| `conversation`: `a_conversation_dropped_before_its_end_is_reset` | a drop that sends END: `the conversation ended`, `left: UnexpectedEof`, `right: ConnectionReset` |
| `conversation`: `a_conversation_cancelled_mid_send_never_ends_its_link` | each conversation sending on the link itself, from a task its drop aborts: the aborted 4 MiB frame, torn, ended the link, and the far end handed on no conversation after it: `called Option::unwrap() on a None value` |
| `conversation`: `a_slow_conversation_holds_its_queue_and_one_let_go_holds_nothing` | an unbounded queue: `the reader went past a full queue`; a conversation let go whose queue stays open and unread: `within 5 s: Elapsed(())` |
| `conversation`: `the_hundred_and_first_conversation_is_reset` | no cap: `within 5 s: Elapsed(())`, the 101st unanswered |
| `conversation`: `the_links_end_ends_every_conversation_with_an_error` | the link's end read as a clean end: `the dialer's: the conversation ended`, `left: UnexpectedEof`, `right: ConnectionAborted` |
| `peer`: `a_hello_on_a_link_binds_its_transport_session` | a binding of fixed bytes: `a HELLO replayed from another link: PeerHello { … }` |
| `peer`: `a_hello_of_another_protocol_is_refused_on_a_link` | the protocol check switched off: `a HELLO of protocol 4 taken: PeerHello { … }` |
| `peer`: `a_link_without_an_endpoint_key_or_a_binding_cannot_hello` | zeros taken for a missing key byte and a missing binding: `an 8-byte key: the dialer completed HELLO: PeerHello { … }` |
| `peer`: `hello_on_a_link_is_bounded` | no bound (a day): `within 5 s: Elapsed(())` |
| `peer`: `hello_on_a_link_exchanges_identities` (moved) | no WELCOME sent: `called Result::unwrap() on an Err value: Custom { kind: TimedOut, error: "no WELCOME within 10 s" }` |
| `peer`: `a_tampered_hello_is_refused_on_a_link` (moved) | the signature check switched off: `a flipped signature byte: accepted` |
| `peer`: `a_hello_replayed_from_another_link_is_refused` (moved) | a binding of fixed bytes: `replayed onto Channel { … exported: [4, …] }: accepted` |
| `peer`: `a_reflected_hello_is_refused_on_a_link` (moved) | the role left out of the transcript: `the dialer took its own HELLO back` |
| `peer`: `a_hello_on_a_link_completes_only_for_a_node_bound_to_its_endpoint_key` (moved) | the door's HELLO check switched off: `unknown: Ok(PeerHello { … })`, `left: true`, `right: false` |

The first red run of `a_hello_replayed_from_another_link_is_refused` stayed green: its HELLO was
signed over the test's channel, not recorded from a link, so a fixed binding could not make the
replay verify. It now records the `NodeHello` a link's dial sends, as the iroh test does, and that
red is the one above.

As built, beside the design:

- **Public.** `conversation` is a `pub` module of `pub` items, where the design sketched `pub(crate)`,
  so the library builds with no caller until part 3 and no dead-code warning. `peer.rs` gains
  `hello_dial_link`, `hello_accept_link`, `HELLO_LABEL` (`glade/v1/peer-hello`) and `HELLO_WITHIN`
  (10 s), public as `hello_dial` is.
- **A conversation takes its number when its first frame is queued**, under the one lock that queues
  it, so each end's numbers reach the peer rising. Numbered at `open`, two tasks could queue their
  first frames out of order, and the peer would drop the lower number as a conversation that is
  over. So a conversation this end opens sends before it receives: a receive before its first frame
  is refused (`InvalidInput`), and one dropped or ended before any frame sends nothing.
- **The tasks** start through a spawner the caller gives, `Spawn`, told which task each is
  (`LinkTask::Writer`, `Reader`, `Inbound`), so part 3 places them at the node's sites; part 2 adds no
  `Site`. `Linked::start(link, node, dialed, max, spawn, handler)`; `Linked::end()` refuses every
  later send and queues the close behind what is queued.
- **A receive answers** `UnexpectedEof` after the peer's END, `ConnectionReset` after its RESET, and
  `ConnectionAborted` after the link's end; a send over the limit, header included, `InvalidInput`.
- **A conversation is forgotten** once both ends are done with it, or at once when this end resets it.
  A frame of an unknown kind ends the link, as one too short for its header does.
- **The reader holds the link's state weakly**: a link whose `Linked` and conversations are all dropped
  closes, its writer's queue closed.
- **HELLO on a link** reports the acceptor's refusals through the door, as `PeerEndpoint::accept` does
  on a stream, its bound's among them: `peer refused: endpoint <tag>: HELLO refused: no HELLO within
  10 s`. The dialer's bound answers `TimedOut`, `no WELCOME within 10 s`. A link that names no 32-byte
  key is refused `HELLO refused: the link names no endpoint key`, one that binds no session `HELLO
  refused: the link binds no session`, neither reported: there is no key to name, and no HELLO yet.
- **`spelled`**, the words for a bound, moved from the adapter to `peer.rs`, so both bounds speak
  alike; the adapter's lines are unchanged.
- **The stream HELLO's tests stay** while the running node speaks it, until part 4. The five rules of
  its in-memory tests are proven again on links by five tests of their own, sharing two case tables
  (`tampered`, `doors`) with them; part 4 removes the stream's.
- **The in-memory link pair** (`conversation::testing`) is test code: two tokio channels, each end
  naming the other's key, and under HELLO's label the test channel's bytes.

**Part 3** (the move):

| Test | Proves | Red against |
| --- | --- | --- |
| `mesh`: `a_dialer_that_never_says_hello_does_not_hold_the_accept_loop` | over real iroh: C links at the carrier and sends nothing; B dials A and links within a second while C's HELLO still waits | HELLO in the accept loop: B waits out C's bound |
| `mesh`: `a_glade_node_3_endpoint_fails_at_connect_either_way` (replaces `a_protocol_2_node_fails_at_connect`) | an endpoint offering only `glade/node/3` fails at the handshake, dialing and dialed | the adapter offering `glade/node/3` too |
| `mesh`: `a_forwarded_gap_crosses_in_chunks_under_the_frame_limit` | with a small test limit, a zone whose gap exceeds it reaches the forwarding node whole, in order, in chunks | the gap in one frame: refused, and the forward lapses |
| `mesh`: `a_newer_link_outlives_the_close_of_an_older_one_to_the_same_node` | two links to one node; the older closes; the newer still serves the node | removal by node id |
| `lifecycle`: `the_module_and_the_mesh_share_one_adapter` | the peer carrier resolved from the module `Assembly` builds answers `AlreadyBound` once the booted adapter is bound: one adapter | the component building an adapter of its own |

Changed and passing: every `mesh` test over real iroh (the convergence, binding, forged-claim, notes,
door, D9, gap, golden-path and grant tests, F5's two among them), `exchange`'s and `claims`' two-node
tests, all through one shared helper that binds a node's adapter; `tests/release_order`, unchanged.
Unchanged and passing, since their lines do not change: `tests/assembled_path` (the door, the notes,
the configuration, no endpoint id in any line), `tests/lifecycle` (a link, then a clean stop with the
ports free; the refused dialer) and `tests/stop_signal`. These, with the three door tests of `mesh`,
are the done-when's "the door's".

**Part 3, as built** on 2026-09-28 against glade `780e751`, red in one sources-only copy of the final tree,
the part each test guards switched off by one edit and put back before the next:

| Test | Red first |
| --- | --- |
| `mesh`: `a_dialer_that_never_says_hello_does_not_hold_the_accept_loop` | HELLO in the accept loop, the driver spawned after it: `B still dialing after 1.00229425s` |
| `mesh`: `a_glade_node_3_endpoint_fails_at_connect_either_way` | the adapter's endpoint offering `glade/node/3` too: `a glade/node/3 dialer connected` |
| `mesh`: `a_forwarded_gap_crosses_in_chunks_under_the_frame_limit` | the gap in one frame: `not sent to peer <id>: an op of ws-razel ws.tree over the frame limit`, then `timed out waiting for the whole gap at A` |
| `mesh`: `a_newer_link_outlives_the_close_of_an_older_one_to_the_same_node` | removal by node id: `each end holds the newer link`, `left: (0, 0)`, `right: (1, 1)` |
| `lifecycle`: `the_module_and_the_mesh_share_one_adapter` | the component building an adapter of its own, unlent: `Err(Transport("the iroh adapter was lent no endpoint key"))` |

As built, beside the design: the mesh takes `mesh::PeerPort` (the port, its notes, the identity, the endpoint
id, the door, the frame limit), which `PeerPort::iroh` makes for both roots; `IrohCarrier::bind_network` binds
the network's sockets at 16 MiB and answers the `peer` line; the component's parameters are
`Option<IrohCarrier>`. **A link lives until it ends**, as a connection did: its reader holds its `Linked`, a
newer link to a node takes the older's numbered entry, and a reader unlinks its own alone. **A subscription's
writer and reader are one loop** in its conversation's task, a conversation having no halves, so
`SubscriptionWriter` retires with `RecordPush`. Chunks are sized by each op's encoding, at most 1 MiB and the
limit less 32 bytes; a lone op refused ends its serve with that line. `PeerEndpoint` loses the relay members
only the mesh used. Gate 9/9, 423 tests each path; contracts 94; the desk replay matched 780e751 but ports.

**Part 4** removes tests only. Each removed test's rule is held by another: `dial_and_hello_over_iroh`
by `a_hello_on_a_link_binds_its_transport_session`; `a_protocol_2_node_fails_at_connect` by
`a_glade_node_3_endpoint_fails_at_connect_either_way`; `sync_over_iroh` by the mesh's convergence
tests over the port; the two close tests by CA-004 on iroh and
`a_closed_carrier_frees_its_port_though_its_links_survive`; the stream HELLO's by part 2's.

**Part 4, as built** on 2026-09-28 against glade `a869697`, with no red run. Removed, with holders that passed
before and after: `dial_and_hello_over_iroh` (`a_hello_on_a_link_binds_its_transport_session`),
`a_protocol_2_node_fails_at_connect` (`a_glade_node_3_endpoint_fails_at_connect_either_way`), `sync_over_iroh`
(`two_booted_nodes_converge_home_share`, `s_discovery_golden_path_end_to_end`), the two close tests (CA-004 on
iroh, `a_closed_carrier_frees_its_port_though_its_links_survive`), the five stream HELLO tests (part 2's five on a
link). Retired: `PeerEndpoint`, `PeerLink`, the ALPN `glade/node/3`, the exporter read `channel` and its label,
`hello_dial`, `hello_accept`; and, their callers gone, `bound_addr`, `PeerAddr` and `NodeIdentity::generate` (the
adapter answers its address with part 1's `bound_at`). Part 3 left no hand-written `u32` writer. Changed: F12's
HELLO case runs on a link; CA-006 reads its connection's exporter itself; two bind tests take `CARRIER_ALPN` and
`bound_at`. Production +21/−309, tests +36/−370. The async witness is frozen at `5e7d238` (its README; on an export
`--locked` is refused, the README's command passes 46 tests). Gate 9/9, 413 tests each path, rustfmt 274, clippy
11; contracts 94.

**4.5's crossing again**, the done-when's third part, after part 4: section 10 of 4.5 as run on
2026-09-27, with the same machines, roles, runs and teardown. Before it, the Pi's sibling checkouts
are pulled `--ff-only` so their two suites count, and each machine's temporary directory is set inside
its scratch directory (the lane owner's proposals of 2026-09-27). Recorded as 4.5's table was, beside
it: the tree, and the ALPN n0 could read.

**The gate** (`node/check.sh`, 9 components) at each part: the node's tests on both paths; rustfmt at
or below its baseline (293 since F5), with no deviation in a line the part writes; clippy at its
baseline; process-globals with nothing new; confinement with no new crate; the contracts gate with
CA-006. Beside it: the six downstream suites against the rebuilt binary, and the desk's replay
(section 12).

**What they do not prove:** Windows and Linux, which the crossing covers; a relay path, save by the
crossing; a change of selected path (a loopback link has one path); two networks; more than two nodes;
what one slow conversation costs its link over a slow relay.

### 14. Size and the split (question 10)

Estimated, in `.rs` lines with doc comments. Recent steps have run over their estimates (4.5 part 1
ran about 530 against 440, `:6395-6396`), so each part is kept well under 500.

| Part | Production | Tests |
| --- | --- | --- |
| 1: `carrier-api` about 70 (the method, its type and its words, CA-006); `iroh_carrier.rs` about 170 (`Lent`, the shared state, `bind` on the lent network, the dial of every address, `channel_binding`, the first word's bound and its report, `LinkNotes` with the relay watch moved in); `assembly.rs` about 30 (the parameters, the port) | ~270 | ~300 |
| 2: `conversation.rs` about 240 (new: the header, `Linked`, the writer, the reader, `Conversation`); `peer.rs` about 45 (HELLO on a link, its bound) | ~280 | ~300 |
| 3: `mesh.rs` about +250/−240; `exchange.rs` about +20/−25; the roots and `lifecycle.rs` about +70/−60; `tasks.rs` about +10/−10 | ~+350/−330 | ~+350/−150 |
| 4: `iroh_carrier.rs` about −250; `peer.rs` about −70; the witness's README +3 | ~+20/−330 | ~−250 |

In that order, foundational first: part 1's method is what part 2's HELLO asks, part 2's conversations
are what part 3's mesh speaks, and part 4 deletes what part 3 leaves unused. Part 1 and part 2's
conversations touch disjoint files (the contracts and `iroh_carrier.rs`; `conversation.rs`), so
they could run side by side once the contract's method lands; in the one node lane they run in order.
Folding part 4 into part 3 would make one commit of about +370/−660; part 3 is the one that changes
behaviour, and it is easier to review with nothing else in it.

### Named gaps (4.5b)

- **One stream's flow control per peer.** A conversation that reads slowly slows its link, where QUIC
  kept streams apart; frames interleave in arrival order, and chunks keep bulk from holding a link for
  long.
- **An op over 16 MiB never crosses a link**; its send is refused with a line. The websocket takes
  such an op from a client, with no limit (`ws.rs:224-238`).
- **The adapter's accept is serial**: a dialer the door admits can hold other inbound links up to
  10 s an attempt.
- **Two links to one node**, both dialing at once: the newer serves the node's conversations, and the
  notes port answers for an endpoint's newest link.
- **A lost link is not dialed again** (4.5's gap, unchanged).
- **The record transport is not the mesh's push.** `CarrierTransport::push` dials a link of its own
  and sends a frame per op with no HELLO (`assembly.rs:698-722`); nothing calls it in production, and
  the mesh's push is a conversation on the HELLO'd link.
- **The release probe** binds, for an instant, each address the port held, beyond loopback when the
  owner's file binds there (section 9).
- **The async witness** no longer builds against HEAD after part 4 (section 11).
- **The conversation header** is defined in this note and the code alone; no other language speaks
  the node protocol, and no vector pins it.

### Default-path changes (4.5b)

At part 3; parts 1, 2 and 4 change nothing a running node does.

1. The node's endpoint offers the ALPN `glade/carrier/1` alone, with the first word `gcl1`. A node on
   `glade/node/3` and this one fail at connect, either way.
2. A peer link is one carrier link carrying the node's conversations (section 3), with HELLO its
   first frame each way.
3. An inbound attempt that sends no first word within 10 s, and a HELLO not completed within 10 s,
   are refused and reported; HELLO no longer holds the accept loop.
4. A frame is at most 16 MiB, and the forwarded gap crosses in chunks.
5. A refused dialer's line names no reason, in new words (section 6); a link's close carries no
   reason.
6. **What the owner's desk sees at its next restart:** the same lines apart from ports, UDP and TCP
   on `127.0.0.1` alone, nothing new on stderr.

### What this changes for 4.5c and 4.6

- **4.5c.**
  - No ALPN is left to move: part 3's `PROTOCOL` 4 is `peer.rs` alone, and its estimate's
    `iroh_carrier.rs` share (`GladeDirectoryCheckpoints.md:651`) goes.
  - Its test row `a_protocol_3_node_fails_at_connect` (`:619`) fails at HELLO only, over the port:
    the acceptor reports `peer refused: endpoint <tag>: HELLO refused: protocol 3, not 4`, and the
    dialer's link ends with no reason. 4.5b's `a_hello_of_another_protocol_is_refused_on_a_link` is
    the gate it turns.
  - The serve order goes into `serve_home`, now a conversation's handler, with the same code; and into
    `serve_sync`, which stays (question 9). A pushed checkpoint rides the push's one `Ops` frame, well
    under the limit. Its line references into `mesh.rs`, `peer.rs` and `iroh_carrier.rs` move.
- **4.6.**
  - "4.4's journeys over the real carrier" can swap the fake network for two `IrohCarrier`s, their
    test code playing the receiving node as it does over the fakes; that proves the carrier, not the
    mesh. The mesh's evidence is the route script over two binaries. 4.6's design should say which it
    means, and whether the record transport is re-based on the mesh's conversations.
  - A lost link is not dialed again, so a restarted accepting node is not linked again until its
    dialer restarts: 4.6's restart outcome needs a re-dial or a stated manual step.
  - The route runs one link per peer, with the 16 MiB frame limit and the two 10 s bounds, at
    protocol 4 once 4.5c lands.

### Questions for the owner (4.5b)

1. **The unit of a link** (section 2). Recommend (a): one carrier link per peer, as 4.2c's adapter
   makes it, with the node's exchanges as conversations on it under a 5-byte header, one writer and
   one reader per link. The alternatives: (b) a link per exchange, the adapter carrying a peer's links
   on one connection, as `dev-docs/IrohGladeMapping.md` §7.1 leans, which keeps QUIC's per-stream
   flow control but makes the adapter a connection manager and needs the contract to promise reuse;
   (c) a connection per exchange, a handshake and HELLO each; (d) sub-streams in the contract.
2. **HELLO's binding through the port** (section 4). Recommend `CarrierLink::channel_binding(label)
   -> Option<ChannelBinding>`, a required method with its line in the contracts' policy for the
   owner's review, and CA-006; HELLO asks it for `glade/v1/peer-hello`, so D6 is unchanged. The
   alternatives: the fixed RFC 9266 label; a nonce each way before HELLO, a round trip more; no
   binding, which lets an endpoint key speak for its node.
3. **The version gate** (section 5). Recommend: `glade/node/3` retires, `PROTOCOL` stays 3, HELLO's
   number is the only gate from here, and there is no window of both ALPNs. An older node then fails
   at the handshake. The alternatives: `PROTOCOL` 4 now and 4.5c at 5; a window that keeps
   `PeerEndpoint`'s protocol beside the new one.
4. **The bounds** (section 7). Recommend 10 s for the first word, over the whole attempt from the
   connection's arrival, refused with code 0 and no reason, and reported by tag once a key is proved;
   and 10 s for HELLO, in the accepted link's own task. The alternatives: 5 s or 30 s; the four bytes
   alone; HELLO left in the accept loop.
5. **The notes** (section 9). Recommend a node-local port, `LinkNotes` (the path of an endpoint's
   newest link, the relay watch), implemented by the iroh adapter. The alternative puts a path and a
   relay state into the carrier contract.
6. **The frame limit** (section 8). Recommend 16 MiB, every serve path chunking at 64 ops or 1 MiB,
   and an op over the limit refused where it would be sent, with a line. The alternatives: a smaller
   limit, near the substrate's 64 KB chunk, which bars more app ops; the wire's `Chunk` frame now.
7. **The release wait beyond loopback** (section 9). Recommend keeping it: it probes only the sockets
   the port held, which the owner's file named. The alternatives: loopback only, leaving CA-004's
   promise to iroh's timing beyond it; none.
8. **One adapter per node** (section 10). Recommend that both roots build the adapter from the booted
   instance, the assembled root lending that same adapter to the module's peer role, binding it in
   `PeerCarrier` and releasing it with the port's `close`. The alternative: `PeerCarrier` builds its
   own and the module's role stays unlent.
9. **What retires** (section 11). Recommend retiring `PeerEndpoint`, `PeerLink`, `EndpointSlot`, the
   ALPN `glade/node/3`, the stream HELLO and the hand-written writers; keeping `serve_sync` and
   `pull_sync` as they are; and freezing the async witness at part 3's revision, with a README line.
   The alternatives: retiring the library's driver too, about 120 production lines and 260 of tests,
   4.5c part 2 then editing `serve_home` alone; keeping `PeerEndpoint` for the witness; porting the
   witness.
10. **The split** (section 14). Recommend four parts, about 270, 280, +350/−330 and +20/−330
    production lines, each gated and replayed, then 4.5's crossing on the Pi and dabeest. The
    alternative folds part 4 into part 3, one commit of about +370/−660.

**Ruled, owner, 2026-09-27 ("all recommended"):** 1 one link per peer, with conversations on it; 2 `CarrierLink::channel_binding(label)`
as a required contract method, with CA-006; 3 `glade/node/3` retires, `PROTOCOL` stays 3, no compatibility window; 4 10 s over the whole
first-word attempt, reported, and HELLO its own 10 s outside the accept loop; 5 the path and relay notes through a node-local `LinkNotes`
port; 6 frames at most 16 MiB, served in chunks of 64 ops or 1 MiB, an op over the limit refused with a line; 7 the release wait kept
beyond loopback; 8 one adapter per node, lent to the module and bound by `PeerCarrier`; 9 `PeerEndpoint`, `PeerLink`, `EndpointSlot`,
the old ALPN and the stream HELLO retire, `serve_sync`/`pull_sync` stay, and the async witness is frozen at part 3's revision; 10 four
parts, then 4.5's crossing again.

### Measured (4.5b, part 1)

2026-09-27, Apple M3 Pro, Rust 1.96.0, on the final tree (glade `4f49fe6` plus part 1):

- **The gate** passes all 9 components, in 107 s from an empty scratch target, with 386 node tests on
  each path across 17 test binaries, where there were 378: `iroh_carrier` 5, `tests/assembly` 1, and
  the faulty link's 1 in `tests/journeys` and again in `tests/durable`, which includes its file.
  - rustfmt: glade-node 293 hunks and glade-wire 43, at their baselines; no hunk is new or gone, so
    no line this part wrote is in one.
  - clippy: glade-node 11 warnings and glade-wire 7, at their baselines, none at a line this part
    wrote.
  - process-globals: 54 files, 3 permanent entries, 0 debt, nothing new.
  - confinement: no new crate; each `Cargo.toml` and `Cargo.lock` untouched.
  - the contracts gate: 94 tests, where there were 91 (CA-006 and its two guards); the checker
    requires `channel_binding`; fmt clean, and clippy clean under `-D warnings`.
- **Nothing reached beyond loopback.** Every socket this part's tests bind is on `127.0.0.1` or
  `[::1]`; the relay URL and `10.1.1.236:4545` in the dial test are parsed, never dialed.
- **Time.** The library's 257 tests take 3.3 s, of which CA-004 alone takes 3.05 s, at HEAD as now
  (below). The five new `iroh_carrier` tests take 0.02 to 0.06 s each, but the first word's, 0.47 s
  for its two 200 ms bounds.
- **The replay**, from `glade-wz/grazel` as grazel starts the desk's node (`--profile local --name grazel
  --app apps/grazel-app.glade --app apps/gyld-app.glade 0`), with only `PATH`, `HOME` and `GLADE_HOME`
  set, on one scratch instance. Each start lived 5 s past `listening`, had its sockets listed with
  `lsof`, and was stopped with SIGTERM to its PID; each ended the same way, exit 143, the hand-written
  root taking SIGTERM's default. Today's binary (inode 408222621, glade `4f49fe6`) and this build
  (inode 408343159, copied from the gate's scratch target):

  | Start | Lines | Sockets |
  | --- | --- | --- |
  | today's, the first boot | `instance`, `node`, `app grazel registered (+12 record(s), 0 unchanged)`, `app gyld registered (+10 record(s), 2 unchanged)`, `registry ready (home served: true)`, `peer 38a3835429 127.0.0.1:<port>`, `workspace ws-razel serving` twice, `listening <port>` | UDP `127.0.0.1:<port>` and TCP `127.0.0.1:<port>` (LISTEN), nothing else |
  | this build | the same, `+0 record(s), 12 unchanged` for each app | the same |
  | today's again | the same as this build's, line for line apart from ports | the same |
  | today's, then this build | the same, line for line apart from ports | the same |

  Every start printed one stderr line, the recovery warning, naming its own binary; nothing followed
  `listening`. The downstream suites were not run: the desk's binary is not rebuilt by this part.
- **CA-004 and the release wait.** Part 1 changes neither `close` nor the probe, and CA-004 takes 3.05 s
  at HEAD as it does now. Measured on a scratch copy with `close`'s phases timed: the drain ends in
  about 1 ms and iroh's close in about 2 ms, and then the release wait runs its whole 3 s and gives up
  with the socket still held, every run. The probe's own pending `accept`, polled once and not again
  until `close` resolves, holds an endpoint handle, and iroh keeps the socket while any handle lives.
  The rebind that follows succeeds only if iroh lets the socket go, a few milliseconds after that
  `accept` ends, before `fresh` binds; under load it can lose, which is, by all signs, the failure of
  F14's first gate run. With the pending `accept` driven alongside `close` (the scratch copy only), the
  wait saw the socket free within 2.5 ms and CA-004 took 0.03 to 0.05 s. Nothing in the ruled design
  makes the release more robust than the probe: question 7 keeps it as it is.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +307/−104, net +203, of which +213/−82 are code (comments counted as the lines that
  begin `//`): `iroh_carrier.rs` +245/−89, `assembly.rs` +44/−13, the carrier contract +16/−1 (the
  type, the method and their words), `mesh.rs` +2/−1; about 30 of the adapter's lines are the relay
  watch and the notes' types moved, not new. Beside them, CA-006 in the contract's `conformance`
  module, +42: net +245 with it, against the design's estimate of about 270;
- tests: +383/−28: `iroh_carrier.rs` +262/−22, the contract's fixture and tests +72/−1,
  `tests/assembly` +34/−2, `tests/journeys/faults.rs` +15/−3;
- beside them, one line in each policy file: `channel_binding` in the contracts' `CarrierLink` row,
  and CA-001..006 in the node's reason text.

### Measured (4.5b, part 2)

2026-09-27, Apple M3 Pro, Rust 1.96.0, on the final tree (glade `f895aca` plus part 2):

- **The gate** passes all 9 components, in 108 s from an empty scratch target, with 401 node tests on
  each path across 17 test binaries, where there were 386: `conversation` 6 and `peer` 9, the design's
  four and the five moved.
  - rustfmt: glade-node 293 hunks and glade-wire 43, at their baselines; no hunk is new or gone.
  - clippy: glade-node 11 warnings and glade-wire 7, at their baselines, none at a line this part
    wrote.
  - process-globals: 55 files (the new module), 3 permanent entries, 0 debt, nothing new.
  - confinement: no new crate; each `Cargo.toml` and `Cargo.lock` untouched. The contracts gate
    passes, untouched, with 94 tests.
- **Nothing reached beyond loopback.** The two iroh tests bind `127.0.0.1` alone; the rest run on the
  in-memory pair.
- **Time.** The six `conversation` tests and the sixteen `peer::hello_tests` take 0.41 s together, in
  each of 5 runs; the library's 272 tests take 3.3 s, CA-004 still the longest (part 1's record).
- **The replay**, as in part 1, on one scratch instance: today's binary (inode 408222621), this build
  (inode 408521518, copied from the gate's scratch target), today's again, and this build again. The
  four starts printed the same lines apart from ports, line for line, but for the first boot's two
  `app` lines, which count the records it seeds (`+12 record(s), 0 unchanged` and `+10 record(s), 2
  unchanged`, then `+0 record(s), 12 unchanged` each); `peer 53336a4858 127.0.0.1:<port>` each time,
  nothing after `listening`, the recovery warning alone on stderr, and UDP and TCP on `127.0.0.1`
  alone (`lsof`). Nothing a running node does calls the new code.

**Size**, in lines added and removed in `.rs` files, doc comments included:

- production: +628/−10, of which +424/−7 are code: `conversation.rs` +457, new (297 code, 121
  comments); `peer.rs` +169/−1 (125 code); `iroh_carrier.rs` +1/−9, `spelled` moved out; `lib.rs` +1.
  The design's estimate was about 280 (240 and 45). Beyond it: the numbering on the first frame,
  each conversation's forgetting, the markers, the reader's and the writer's ends, and HELLO's
  channel, bare frames, bound and report, each with its words;
- tests: +834/−28: `conversation.rs` +437 (the in-memory pair and the iroh helpers 154, the six tests
  283) and `peer.rs` +397/−28 (the four designed, the five moved, their helpers, and the two case
  tables taken out of the stream tests).

### Frames the node cannot take (F15)

The owner's ruling of 2026-09-27; each is refused as a bad frame, not a panic or an abort:
- **Nesting: `MAX_DEPTH`, 32** arrays and maps (`wire-rs/src/wellformed.rs`, by hand beside
  `checked.rs`); the wire's types nest 5 deep. `Frame::from_bytes` walks the message once,
  without recursion, before `cbor::decode`, and refuses it too deep, truncated, with trailing
  bytes, a tag, an indefinite or reserved length, another simple value, a non-int map key or text
  that is not UTF-8: every case on which `cbor::decode` panics or overflows its stack.
- **Size: `MAX_FRAME_BYTES`, 16 MiB** (`frame.rs`; question 6's limit), checked by `frame_len`
  before allocating, in `WsReader::read` and `peer::read_frame`: a longer header ends that
  connection or stream alone. On the websocket a refused message is skipped; its session goes on.
- **Payloads read apart from their frame (F15b):** every other production decode goes through
  `wellformed::decode` (`envelope::record`, `decode_op`, `snapshot_ops`); a source test in
  `envelope.rs` fails the gate on a raw `cbor::decode`. A refused `workspace.create` payload, or
  a peer's refused answer to a forwarded one, answers `ok: false`; an unsealed registry refuses the
  record; a stored op is skipped (in records.json quarantined, closing the grant fold), a folded
  record skipped, each with one stderr line; a nested `home` payload still refuses the start (4.1b).
