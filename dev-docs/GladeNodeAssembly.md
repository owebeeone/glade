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
| `peer_carrier_binding` (`PeerCarrier`) | `CarrierPort`, `glade-carrier-api` | `PendingIrohAdapter`: fail-closed (`bind` refused, `dial` `Closed`, `accept` `Ok(None)`); built with the record transport `Records` injects, never called | port A on a fake network (CA-001..004) | `Sessions` (peer role); the record transport |
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
every provider marked real above, and a process-wide counter shows none built.

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
configuration, and since 4.1a the signer (Ed25519). Phase 4 replaces the three
remaining `Pending*` stand-ins: 4.3 the grant fold with its serve-hop consult,
4.2 and 4.5 an iroh `CarrierPort` adapter (4.2 the remote-identity accessor and link tracking, 4.5
the bind address CA-004's re-bind needs), and a WebSocket one; the mesh and the
WS server then move onto the carrier bindings, and 4.4 makes the served store the
record host. Step 3.3's sdax plan takes over the root's three acquisitions
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
- No carrier adapter implements `CarrierPort`: CA-001..004 run on the fake only.
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
| the served store, `store::Store` | `<instance>/cache/store/`, or the second positional (a temp directory in the legacy form) | every op the node serves, in per-(share, origin) op logs: its own records (seeded from records.json at adoption, then published), peers' records, clients' ops | `Server` (`Frame::Ops`); the mesh (pull, push, forward); `claims::publish` | every serve path; the peer pull announces its heads | the operation-store port, `DurableOperationStore` |

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
(landed)", below.

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
  - Not done: they wait for the route.
- gyld-app.glade `:63` and `:66` already name `ws-razel`.
- The fixtures name the app: `glade-gyld/tests/fixtures/gyld-test-app.glade:27`
  (`seed owner gyld gyld.*`) and `glade-gwz/tests/fixtures/gwz-test-app.glade:18`
  (`seed owner gwz gwz.*`). Each declares `workspace ws-razel` (`:30` and
  `:21`).
  - **They must change, to `ws-razel`, in the commit that adds precondition
    4's warning.**
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
- **Part 2: the check.** About 300 production lines and 500 test lines:
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
  adapter that tracks its links.

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
    from one that names the far end; a third can.
  - The contract's fixture gains a deliberately wrong link that names its
    own end, which CA-005 refuses. The node's fake network and its faulty
    link implement the method.
  - `glade/contracts/architecture-policy.json` lists `remote_id` among
    `CarrierLink`'s methods, so the checker requires it. The lane owner
    added it in 4.2b's commit, for the owner's review, as the policy's
    earlier changes were.
- **The iroh adapter's links:** 4.2c, ruled a step of its own.
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

### Measured (4.2b)

2026-09-25, Apple M3 Pro, Rust 1.96.0, on the final tree:

- **The gate** (`glade/node/check.sh`) passes all 8 components, in 42 s warm
  and 89 s from an empty target. There are 256 node tests on each path,
  across 15 test binaries, where there were 246. The ten new ones are section 10's rows, less the two changed
  tests and the contract's own two.
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
