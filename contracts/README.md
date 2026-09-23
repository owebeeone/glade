# Glade application-side draft contracts

This independent Cargo workspace adds contracts alongside the working node/client,
not inside discovery. It does not replace or link into the demo. It now contains
ten draft contracts: binding, invocation, subscription, replica synchronization,
resource lifecycle and persistence, plus the four assembly ports below (carrier,
clock, grant, signer). These are a first reviewable boundary layer, not a complete
stable Glade SDK or working implementation.

The five new contracts, test IDs, review disposition, dependency rationale and
remaining adapter obligations are recorded in
[ApplicationContractDraft.md](../dev-docs/ApplicationContractDraft.md).

## Persistence boundary

`glade-persistence-api` exposes `SnapshotStore::load` and `compare_exchange`, with
opaque caller-encoded snapshots and explicit local revision, conflict, corruption,
capacity, exhaustion and uncertain-outcome semantics. It has **zero dependencies**.
It follows the snapshot storage seam in GDL-036 without treating the storage engine
as a replication mechanism. CAS is a provisional strengthening, not a change to
the existing node's `StoreApi`, its serialization, or any public wire format.

No deduplicated command acceptance, source authority, replication acknowledgement,
database engine, channels or async runtime are selected. Futures are Send; explicit
outcomes and cancellation semantics do not require an sdax-like implementation.

## Test coverage and limits

PS-001–003 cover roundtrip bytes, create/replace revision checks, empty-but-present
snapshots and unpolled write laziness. PS-004–005 distinguish unavailable/corrupt
storage from absence. PS-006 covers a lost reply after committing; PS-007 checks
reopen retention; PS-008 checks revision exhaustion without mutation. Negative
fixtures demonstrate rejection of ignored CAS and lost snapshots. Missing required
trait methods fail a compile-fail doctest.

Tests were written and run RED before the interface was added. Private fixture
models are volatile memory, not production implementations. Snapshot-copy reopen
tests MUST NOT be presented as physical crash or fsync verification. Real adapters
MUST also test concurrent handles, interrupted writes, pending-future cancellation,
capacity limits and recovery after known failure. Caller schema/authenticity checks
remain outside the storage port. There are no current production consumers.

## Assembly ports (first-slice plan Step 3.1)

Four zero-dependency ports the node's assembly needs and did not have
(`dev-docs/GladeFirstSlicePlan.md` Step 3.1). Each is `Send + Sync`, names no `Any`,
and bridges onto an injector with `impl<T: Port + 'static> Facade for T {}`: each
trait's doctests compile that bridge against a local copy of `shaku::Interface` and
show the witness's form failing (E0310; rustdoc checks the code only on nightly).

- `glade-carrier-api`: `CarrierPort` (bind, dial, accept, close) and `CarrierLink`
  (send, recv, close), from the async witness. CA-001 framing and order, CA-002 the
  frame limit both ways, CA-003 lazy futures and a cancel-safe `recv`, CA-004 close
  by value: the address re-binds while a clone of the port and a link survive.
- `glade-clock-api`: `ClockPort::now_ms`, wall-clock epoch milliseconds. CL-001 one
  instant behind every handle, CL-002 unit and epoch.
- `glade-grant-api`: `GrantPort::check`, the `grants_for` question as one decision.
  GR-001 exact match, GR-002 revocation wins in either fold order, GR-003 an
  unreadable fold is `Unavailable`.
- `glade-signer-api`: `SignerPort` (node_id, sign, verify) for the HELLO, origin-op
  and local-overlay seams. SI-001 round trip, SI-002 bytes, purpose and signer
  binding with an unknown key `Unavailable`, SI-003 unavailable key material. It
  follows the discovery `Signer`/`Verifier` outcomes without depending on them.

Each suite passes on a volatile fixture and fails, naming its requirement, on
deliberately wrong ones. The fixtures (a fake network, an atomic clock, an in-memory
fold, a keyed checksum) are not transports, time sources, registries or cryptography.
Real adapters MUST add I/O, partial-frame, backpressure, fault, algorithm-vector and
key-resolution tests.

## Fast checks and gate scope

From the GWZ root: `sh glade/contracts/check.sh` runs all ten contract packages.
Pass `binding`, `invocation`, `subscription`, `sync`, `lifecycle`, `persistence`,
`carrier`, `clock`, `grant` or `signer` for one package.
`sh glade/contracts/test-selection.sh` verifies those selections.
All selections include the local architecture gate and enable conformance tests.
`sh glade/contracts/arch002-fixture.sh` injects `shaku` into each contract crate in
turn, on a temporary copy, and requires the gate to refuse each with ARCH-002; it
is a separate command, not part of `check.sh`.
There are no concrete consumers yet; future contract changes MUST also test their
affected consumers, not just the selected interface package.

The script explicitly adopts the existing architecture checker for **this nested
workspace only**, using `architecture-policy.json`. The checker is obtained from
the sibling `glade-discover` GWZ member and is not a runtime/build dependency of the
contract. Missing tooling fails closed. This is a local gate, not a claim that
Glade's legacy crates or hosted CI are covered. Standalone Glade CI adoption needs
a packaged/pinned checker distribution; no new cross-repository checkout workflow
is silently introduced here. Required merge checks remain hosting configuration.

For the original persistence tranche, independent review found no architectural blocker; its two requested
corrections (PS-004 coverage naming and acknowledged-success durability wording)
are applied. Eight integration/fixture tests and one compile-fail doctest pass.
The suite passed on Rust 1.85 and 1.96. Local gate/test/format/clippy verification
took 1.71 seconds with cached dependencies and an incremental build on 2026-09-05;
this is not a cold-build measurement or a guaranteed budget.

The reusable engineering policy is
[LibraryBoundaryAndTestingPolicy.md](../../dev-docs/LibraryBoundaryAndTestingPolicy.md).
New contracts MUST have explicit reviewed classifications and minimal dependencies.
