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
| signer (`Signer`) | `SignerPort`, `glade-signer-api` | `PendingNodeSigner`: sign and verify `Unavailable` (SI-003), `#[lazy]` | a keyed test signer, FNV over a per-node secret (SI-001..003) | none until 4.1 |
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
configuration. Phase 4 replaces the four `Pending*` stand-ins: 4.1 the signer
(ed25519), 4.3 the grant fold with its serve-hop consult, 4.2 and 4.5 an iroh
`CarrierPort` adapter (4.2 the remote-identity accessor and link tracking, 4.5
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
  The pending grant fold and signer pass the fail-closed half of their suites
  (GR-003, SI-003) and nothing more.
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
  lock, as a separate change with its own failing test.
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
  deterministic test shows it.
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
