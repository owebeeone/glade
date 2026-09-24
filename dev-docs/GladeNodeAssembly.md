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
