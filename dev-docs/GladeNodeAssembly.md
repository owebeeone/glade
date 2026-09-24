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
