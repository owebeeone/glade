# Application-side Glade contracts — first complete draft tranche

Date: 2026-09-05. Status: **provisional traits and canonical tests, no production
adapters, wire changes or demo migration**. GDL-046 records the owner's go-ahead.
This extends the earlier persistence draft and is separate from `glade-discover`.
“Complete tranche” means the five requested boundaries now exist; it does not
mean the entire Glade SDK, security architecture or adapter suite is complete.

## Scope and dependency boundaries

| Package under `contracts/` | Required public traits | Direct library dependencies |
| --- | --- | --- |
| `binding-api` | `BindingResolver::resolve` | existing `glade-decl` only |
| `invocation-api` | `Invoker::invoke` | none |
| `subscription-api` | `Subscriber::open`; `Subscription::next/close` | none |
| `sync-api` | `ReplicaSync::read/ingest` | none |
| `lifecycle-api` | `ManagedResource::phase/shutdown` | none |
| `persistence-api` (earlier tranche) | `SnapshotStore::load/compare_exchange` | none |

Binding reuses the existing generated declaration types rather than creating a
competing schema. Its dependency includes that leaf's codec; no code generation
runs at build time. Other interfaces use associated binding/cursor types. This
keeps their callers independent of declaration codecs and one another, while
composition supplies compatible types. It is intentional dependency inversion,
not evidence that cross-boundary wiring has been implemented or tested.

All six libraries have reviewed `contract` classifications and meaningful required
methods. There are no node/client, database, network, async-runtime or third-party
library dependencies added by the new contracts. The GWZ-local checker is a
development tool in a sibling repository, not a Cargo library dependency.

## Behavioral contract and staging decisions

**Bindings.** Resolution MUST validate the registered declaration, exact version,
supported capability, canonical parameters and authorized scope. A manifest MUST
NOT replace runtime grant folds. Resolution is read-only and MUST NOT instantiate
a service, attach a supplier or create authority. Its result is descriptive data,
not an unforgeable bearer permission or promise of a reachable provider. Current
authorization MUST be checked again on actual operations. The fixture is value-only;
recognition of other catalogue names MUST NOT imply implemented adapter support.

**Invocation.** This is directed exchange, not a delivery fold. An invoker is bound
to an authenticated caller/session; payload identity fields MUST NOT override that
context. Provider epoch, scope and authorization checks remain mandatory. Completed
responses preserve correlation; application failure is data and may follow effects.
Rejected means positively not dispatched. Once execution might have begun, timeout
or connection loss MUST report Unknown rather than promise a safe retry. Automatic
retries of possibly executed commands are forbidden. Dropping a future is neither
remote cancellation nor rollback. Correlation is not durable idempotency identity.

**Subscriptions.** Opening validates binding/profile/cursor scope and bounded buffer
limits. Fresh sessions start with a snapshot; resume starts with an explicit resume
handshake or gap, never a silent reset. Deltas connect to the previously delivered
cursor. Gap is terminal: subsequent reads MUST return an error without awaiting new
data, until a new session is opened. Overflow/retention loss MUST NOT silently lose
updates. Closing stops local delivery and releases owned local resources; it does
not certify remote teardown. Cursor/bytes belong to the versioned delivery profile,
not a newly invented universal sequence, CRDT or terminal-channel format.

**Replica synchronization.** A handle is bound to one authenticated replica scope.
Reads page exact canonical signed operation bytes with both count and byte bounds.
Local opaque cursors are not global source revisions; profiles MAY represent
per-origin frontiers. Expired/foreign cursors MUST be explicit errors. Ingest MUST
validate the full batch before atomic durable retention. Exact duplicates are
idempotent; invalid/conflicting operations MUST NOT partially mutate the batch.
Receipt means local retention, not source-command authority, applied projection or
peer replication. The owner/factory binds immutable scope; no universal namespace
or cursor wire encoding is specified. Anti-entropy optimizations and checkpoint
retention profiles remain separate, and MUST preserve authority/fencing history.

**Lifecycle.** The resource owner exposes Open/Closing/Closed and an explicit
shutdown report. First polling shutdown stops admission. Drain and cooperative
Cancel are distinct; neither rolls back effects. Budget expiry MUST report unfinished
ownership, not detach invisible work. Partial cleanup is retryable and completed
cleanup MUST NOT be repeated. Dropping a pending shutdown leaves ownership/progress
available for another attempt. This specifies obligations, not a dependency-DAG
executor: independent cleanup MAY run in parallel, but no channels, threads,
macros or sdax-like runtime are mandated.

These are in-process contracts. Authenticated context construction, provider-side
attachment/handoff and forwarding codecs MUST still satisfy the existing supplier
B1–B3 rulings before effectful adapters are connected. Public data fields are not
security tokens. The draft does not resolve ASCR-D01/D02/D04 by silently freezing
new definition/binding/advertisement records, role schemas or domain mappings.

## Requirement-to-test trace

Each new crate has an opt-in `conformance` module and a `public_contract` integration
target, plus compile-fail documentation for missing required trait methods.

| IDs | Canonical scenario |
| --- | --- |
| BI-001 | Exact request, share, key and supported capability; no provider needed |
| BI-002 | Denied caller, unsupported shape, unknown declaration and version mismatch |
| BI-003 | Unauthorized domain, invalid parameters and altered declaration rejected |
| IV-001–002 | Correlated success/application failure; denial/zero-budget rejection before dispatch |
| IV-003–004 | Lost reply is Unknown with one attempt; unpolled future is inert |
| IV-005–006 | Injected post-dispatch deadline remains Unknown; duplicate live correlation rejected and local waiter released on drop |
| SU-001–002 | Snapshot/delta continuity; gap then terminal error |
| SU-003–004 | Idempotent terminal close; unpolled read does not consume data |
| SU-005–006 | Fresh/resumed opening, explicit resume gap/denial, buffer-product overflow rejected |
| SY-001–002 | Duplicate ingest, bounded paging, conflicting identity, empty no-op and invalid-batch atomicity |
| SY-003–004 | Invalid cursor/undersized byte budget; adapter-supplied bytes and a distinct cursor type |
| LC-001–003 | Terminal idempotent cleanup, visible retryable failure, unpolled shutdown inert |
| LC-004–006 | Zero-budget unfinished report, pending-poll/drop/retry retains ownership, Cancel does not drain fixture work |

Rejecting mutants cover wrong binding scope, mismatched correlation, delta after
gap, broken delta chain, delivery after close, silent resume reset, partial ingest,
and falsely reported cleanup completion. Tests precede implementation of each
contract/helper. The initial unresolved-interface RED run was followed by semantic
RED failures for unauthorized domain binding, delivery after gap and buffer-product
overflow. The architecture gate rejected all five unclassified crates before exact
entries were added. No existing dependency rule or classification was relaxed.

## Reuse and evidence limits

Real adapters SHOULD use `assert_resolution`, `assert_outcome`, `assert_events`,
subscription gap/close assertions and sync `assert_ingest_and_replay` /
`assert_rejection` with their real declarations, bindings, cursors and signed corpus.
The named scenario wrappers additionally document fixture setup requirements.
Sync's generic assertions are exercised with a distinct String cursor and a rejecting
partial-write model. Inspection requests in atomicity tests MUST cover all affected
records; a truncated page is not sufficient evidence of global non-mutation.

Private fixtures are deterministic, volatile models. Their synthetic operation
bytes are NOT a Glade wire encoding or cryptographic proof. They MUST NOT be used
as production adapters or claimed as real storage durability. Ready-only runners
and explicit single-poll probes avoid sleeps and do not exercise an executor.
Injected deadline results/zero budgets are not elapsed-timer verification.

Before production integration, adapters MUST add real authentication/forwarding and
provider-epoch tests; authorization revocation during delivery; actual bounded-buffer
pressure; pending-open/read cancellation; deadline and scheduler behavior;
concurrent handles/ingests; physical crash/restart durability; source/replica role
separation; and each exact shape/retention/checkpoint profile's conformance corpus.
Lifecycle dependency ordering and parallel cleanup are still orchestration work.
No demo or end-to-end node test is claimed for this interface-only tranche.

## Adversarial review

Independent review found no architectural dependency blocker. It identified four
test gaps: post-gap delivery, lifecycle mode/budget/polled cancellation, invocation
deadline/live-correlation behavior, and synthetic-only synchronization assertions.
All were addressed. Re-review passed the five-crate suite and requested one wording
alignment: terminal reads after Gap. That wording is now explicit. Buffer-product
overflow received an additional RED/GREEN test after re-review.

## Fast verification and enforcement

From the GWZ root:

```sh
sh glade/contracts/check.sh                 # all six small contract crates
sh glade/contracts/check.sh subscription    # one independently selectable boundary
sh glade/contracts/test-selection.sh        # default and explicit selector regression
```

Each selection runs the architecture gate, opt-in tests, formatting and all-target,
all-feature clippy. The selector test prevents the former persistence-only default
from silently omitting new contracts. Future public contract changes MUST also run
affected consumers. There are currently no production consumers of these new traits.

Gate coverage is only the nested `glade/contracts` workspace. Legacy Glade libraries,
other workspaces and hosted CI are not implicitly adopted. The existing GWZ sibling
checker must be materialized; absence fails closed. Distribution/pinning of this
checker for standalone Glade CI remains open, as do hosted required-check settings.

Local verification on 2026-09-05:

- **34 new integration tests and six compile-fail doctests passed**. Including the
  earlier persistence contract, this workspace has **42 integration tests and seven
  compile-fail doctests**, passing on Rust 1.85 and 1.96.
- Formatting and all-target/all-feature clippy passed on the affected crates;
  clippy also passed on MSRV Rust 1.85. The architecture gate, selector regression,
  shell syntax and tracked whitespace checks passed.
- A warm all-six gate/test/format/clippy command measured **1.28 seconds** wall time;
  isolated lifecycle measured **0.94 seconds**. Test bodies reported 0.00 seconds at
  harness display precision. These are local observations, not guaranteed budgets.
- A fresh target-directory build and all-six tests/doctests on Rust 1.85 took
  **9.58 seconds** wall time (Cargo compilation: 1.03 seconds), using
  `/tmp/glade-app-contract-cold.0AZLgz`. Sources were already local/offline; this
  excludes downloads and a cold architecture-checker build. Other local verification
  was active during the measurement.
- Existing node/client/demo implementations and generated declarations were not
  modified. No production consumer, full-system run, real I/O or hosted CI run is
  claimed. Changes remain uncommitted.

Sources: [declaration surface](../../dev-docs/glade/GladeDeclSurface.md),
[supplier model](../../dev-docs/glade/GladeSupplierModel.md),
[shape catalogue adoption](../../dev-docs/TautShapeCatalogAdoption.md), and
[library policy](../../dev-docs/LibraryBoundaryAndTestingPolicy.md).
