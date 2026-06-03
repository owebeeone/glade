# Glade Rust Orbit Strategy

Status: working draft, proposal for pressure test. This is not yet a stable
contract.

Purpose: specify a strategy for porting the OrbitDB oplog/sync spine to Rust,
testing it heavily, exposing the Glade terminal surface through a Python module,
and preserving a path to a browser/wasm target.

## 1. Status / Purpose / Core Claim

Glade SHOULD port the OrbitDB oplog/sync spine faithfully into Rust, but Glade
MUST NOT adopt OrbitDB JS, go-orbit-db, or a libp2p-coupled design as the
semantic core.

The core claim is:

```text
Rust oplog core + simulated Substrate first + Glade API + Python binding +
optional wasm/libp2p adapters
```

The test framework is not supporting work. It is the main product of this
strategy. The Rust core MUST be testable without libp2p through a deterministic,
pure in-process Substrate simulator. libp2p integration MUST come after that
simulator can drive correctness, fault, churn, and scale tests.

Risk calibration:

| Work | Calibration | Why |
| --- | --- | --- |
| PyO3/maturin packaging | Commodity | Known path for native Python modules, provided async/GIL boundaries are kept narrow. |
| Browser OrbitDB JS/js-libp2p spike | Low | The vendored OrbitDB JS stack already supports browser use; use it to validate interop and UI shape cheaply. |
| Entry/log/clock/head port | Moderate | Small source footprint, but exact canonical bytes, ordering, and signature behavior MUST match fixtures. |
| libp2p adapter | Moderate to hard | The protocol levers exist, but stream lifecycle, backpressure, browser transport, and diagnostics are integration risks. |
| Lazy segmented replication | Hard | It changes traversal and sync bootstrap while relying on partial history and signed anchors. |
| Reattach cutover | Hard | Replay and live output race at the user-visible ordering boundary. |
| Rust wasm browser substrate | Medium to high | Useful portability proof, but browser storage, wasm async, bundle size, and browser transport interop are not free. |
| py-libp2p port to browser APIs | High | It would combine Python runtime-in-browser work with browser transport APIs and should not be treated as cheaper than existing js-libp2p. |
| Million-scale simulation | Hard | Requires compressed models that still preserve enough detail to falsify `O(participants)` and correctness claims. |

This document follows Glade's documentation rules: `dev-docs/` is for internal
engineering contracts, `scratch/` is non-authoritative, and specs MUST use
explicit `MUST` / `SHOULD` / `MAY` language
(`glade/AGENTS.md:23-38`, `glade/dev-docs/README.md:23-28`).

## 2. Scope

In scope:

- A Rust port of the OrbitDB oplog/sync spine: entries, Lamport clocks, heads,
  deterministic ordering, append, join, traversal, and head exchange.
- Lazy segmented single-writer logs with signed anchors.
- A very thorough test framework, including property tests, JS conformance,
  deterministic simulated transport, and large-scale Monte Carlo simulation.
- The Glade terminal surface: `OpenTerminal` exchange, `TerminalPty` live
  channel, and `TerminalOutput` append-log/tap.
- Python packaging through PyO3/maturin, embedded in the provider process.
- A browser/wasm path for the same core where practical.

Out of scope:

- Multi-writer document CRDT semantics.
- Full Frankenapp composition.
- Non-terminal product surfaces.
- Adopting OrbitDB database store types, manifests, IPFS storage, or OrbitDB JS
  as runtime dependencies.
- Making libp2p a semantic dependency of the core.

## 3. Source Analysis

### 3.1 Glade Constraints

The strategy MUST preserve the read/write asymmetry in the terminal proposal:
terminal output is observable/convergent state carried by append-log/tap, while
keystrokes and control input are directed live-stream events and MUST NOT be
routed through replicated shared state
(`glade/dev-docs/GladeTerminalSliceProposal.md:45-59`).

The terminal slice decomposes into three surfaces: `OpenTerminal` exchange,
`TerminalPty` live channel, and `TerminalOutput` append log
(`glade/dev-docs/GladeTerminalSliceProposal.md:61-78`). GLDevPlan names the same
pipeline and requires create/attach/input/resize/output/replay/restart
behaviors (`dev-docs/GLDevPlan.md:217-230`).

Sessions MUST be isolated, hub-introduced, directly-synced segmented logs with
per-session cost `O(participants)`, not `O(total sessions)`
(`glade/dev-docs/GladeTerminalSliceProposal.md:92-100`). The lazy append-log
proposal is a segmented single-writer log with signed anchors and a bounded
join/verify horizon (`glade/dev-docs/GladeTerminalSliceProposal.md:110-127`).

Exchange work MUST remain represented as share state, not hidden request/reply.
Glade separates intent, substrate, ownership, execution, publication,
observation, and diagnostics planes
(`dev-docs/glade/GladeExchangeSemantics.md:67-145`). The p2p topology also
requires identity separation: libp2p `PeerId` is transport identity and MUST NOT
be treated as Glade authorization identity by default
(`dev-docs/glade/GladeP2PFirstTopology.md:36-48`).

The current GripLab consumer is a mock xterm wrapper: interactive mode writes a
mock prompt and echoes input locally (`grip-lab/src/lab/terminalController.ts:43-49`).
The replacement target is therefore a real terminal handle that preserves the
consumer shape: read output chunks and send input frames.

### 3.2 OrbitDB Spine To Port

The required OrbitDB spine is 1,498 lines:

- `third-party/orbitdb/src/oplog/log.js`
- `third-party/orbitdb/src/oplog/entry.js`
- `third-party/orbitdb/src/oplog/heads.js`
- `third-party/orbitdb/src/oplog/conflict-resolution.js`
- `third-party/orbitdb/src/oplog/clock.js`
- `third-party/orbitdb/src/oplog/oplog-store.js`
- `third-party/orbitdb/src/sync.js`

Critical-core inventory:

| File | Raw LOC | Semantic function count | Critical role | Concurrency / correctness risks |
| --- | ---: | ---: | --- | --- |
| `oplog/log.js` | 565 | 18 named functions plus 6 local closures/tasks | Append, join, traversal, iterator, references, log-facing API | Separate `appendQueue` and `joinQueue` serialize within append and join respectively, but do not globally serialize append-vs-join (`third-party/orbitdb/src/oplog/log.js:84-86`). Bare `Log` callers can race head/storage updates unless wrapped by a higher-level queue. |
| `oplog/entry.js` | 240 | 6 | Entry creation, canonical encoding, CID calculation, signing, verify, encode/decode | Canonical bytes and mutation of encrypted/decrypted payload fields are compatibility-sensitive (`third-party/orbitdb/src/oplog/entry.js:73-94`, `third-party/orbitdb/src/oplog/entry.js:163-231`). |
| `oplog/heads.js` | 113 | 9 | Persistent head set and `findHeads` | `add` and `remove` are read-modify-write over storage without their own queue (`third-party/orbitdb/src/oplog/heads.js:25-39`). |
| `oplog/conflict-resolution.js` | 89 | 4 exported functions plus 4 local closures | Deterministic ordering | Pure code, but zero comparator results are fatal and MUST be preserved (`third-party/orbitdb/src/oplog/conflict-resolution.js:71-82`). |
| `oplog/clock.js` | 58 | 3 | Lamport clock | Mostly pure, but `tickClock` increments the passed clock before returning a new `Clock` object (`third-party/orbitdb/src/oplog/clock.js:37-39`). |
| `oplog/oplog-store.js` | 116 | 11 | Entry bytes, verified index, heads storage | Writes entry bytes, verified index, and heads in separate awaits; `addHead`, `addVerified`, and `removeHeads` are not transactional as a group (`third-party/orbitdb/src/oplog/oplog-store.js:49-87`). |
| `sync.js` | 317 | 11 named functions plus 2 local tasks | Head exchange, pubsub update notification, peer lifecycle | Pubsub event handling is queued with concurrency 1, but direct stream handlers, `add`, and stop/start lifecycle run outside that queue (`third-party/orbitdb/src/sync.js:118-127`, `third-party/orbitdb/src/sync.js:169-224`, `third-party/orbitdb/src/sync.js:257-300`). |

The Rust port SHOULD treat `log.js`, `entry.js`, `heads.js`,
`conflict-resolution.js`, `clock.js`, and the storage contract in
`oplog-store.js` as the critical semantic core: about 1,181 raw LOC before sync,
with roughly 51 named/semantic functions plus local closures/tasks. `sync.js` adds 317 raw
LOC and about 13 sync/lifecycle functions/tasks, but it is transport-coupled and
SHOULD become a transport-independent sync state machine rather than a literal
libp2p/IPFS port.

The largest concurrency risk is not algorithmic complexity. It is state
serialization and atomicity at the boundary between append, join, storage, heads,
and sync:

- `Log.append` and `Log.joinEntry` each use a single-concurrency queue, but those
  are different queues (`third-party/orbitdb/src/oplog/log.js:84-86`). The Rust
  core SHOULD use one per-log operation sequencer unless tests prove split queues
  are safe under concurrent append/join.
- `joinEntry` verifies and collects hashes, then calls `addHead`, `addVerified`,
  and `removeHeads` in separate storage steps
  (`third-party/orbitdb/src/oplog/log.js:299-307`). The Rust port MUST define
  whether this is atomic, recoverable, or replay-repaired after crash.
- `traverse` and `iterator` fetch entries while storage may be changing
  (`third-party/orbitdb/src/oplog/log.js:319-379`,
  `third-party/orbitdb/src/oplog/log.js:414-483`). The Rust port MUST define
  snapshot or weakly-consistent iterator semantics.
- `sync.add` publishes head bytes directly when `started` is true, outside the
  sync event queue (`third-party/orbitdb/src/sync.js:257-261`). The Glade
  Substrate adapter MUST model publish-vs-stop races and duplicate head delivery.
- `sync.js` explicitly does not guarantee delivery, order, or timing
  (`third-party/orbitdb/src/sync.js:33-37`), so the Rust core MUST treat all
  transport input as advisory and idempotent.

`entry.js` SHOULD be ported closely. It defines the entry fields `id`,
`payload`, `next`, `refs`, `clock`, `v`, `key`, `identity`, and `sig`
(`third-party/orbitdb/src/oplog/entry.js:11-23`). It encodes entries as
dag-cbor blocks with sha256 and base58btc CIDs
(`third-party/orbitdb/src/oplog/entry.js:1-9`), signs the canonical entry bytes
before adding `key`, `identity`, and `sig`
(`third-party/orbitdb/src/oplog/entry.js:58-94`), verifies by reconstructing the
signed value (`third-party/orbitdb/src/oplog/entry.js:106-126`), and computes
hashes during encode/decode (`third-party/orbitdb/src/oplog/entry.js:163-231`).

`clock.js` SHOULD be ported verbatim in behavior. It compares Lamport clocks by
time, then id (`third-party/orbitdb/src/oplog/clock.js:20-29`), and ticks by
incrementing time (`third-party/orbitdb/src/oplog/clock.js:37-55`).

`conflict-resolution.js` SHOULD be ported verbatim in behavior inside a segment.
The default sort is Last Write Wins, using clock ordering and a clock-id
tiebreaker (`third-party/orbitdb/src/oplog/conflict-resolution.js:14-23`,
`third-party/orbitdb/src/oplog/conflict-resolution.js:37-60`). The comparator
MUST reject zero results (`third-party/orbitdb/src/oplog/conflict-resolution.js:71-82`).

`heads.js` SHOULD be ported. It persists current heads as `{ hash, next }`
records (`third-party/orbitdb/src/oplog/heads.js:18-23`), adds heads by unioning
with current heads and recomputing `findHeads`
(`third-party/orbitdb/src/oplog/heads.js:25-31`), and defines heads as entries
not referenced by another entry's `next`
(`third-party/orbitdb/src/oplog/heads.js:94-110`).

`log.js` SHOULD be ported as the behavioral center. It describes the log as a
verifiable append-only Merkle-CRDT (`third-party/orbitdb/src/oplog/log.js:1-7`),
serializes append and join operations through queues
(`third-party/orbitdb/src/oplog/log.js:84-86`), appends by taking current heads
as `next`, collecting historical `refs`, ticking the clock, checking access, and
setting the new head (`third-party/orbitdb/src/oplog/log.js:156-195`). Join
verifies the incoming entry, walks reachable missing entries through `next` and
`refs`, verifies them, adds them to the index, and removes connected heads
(`third-party/orbitdb/src/oplog/log.js:238-311`). Traversal and iterator
semantics are the most sensitive code paths for lazy windows
(`third-party/orbitdb/src/oplog/log.js:319-379`,
`third-party/orbitdb/src/oplog/log.js:414-483`).

`oplog-store.js` SHOULD be replaced at the concrete backend level but ported as a
trait contract. It separates entry bytes, a verified-entry index, and head
storage (`third-party/orbitdb/src/oplog/oplog-store.js:23-47`), and updates
storage/index/head state during append and join
(`third-party/orbitdb/src/oplog/oplog-store.js:49-87`).

`sync.js` SHOULD NOT be ported as a libp2p/IPFS dependency. It is valuable as a
state-machine reference: OrbitDB sends and receives heads on open and update, and
its own comments explicitly do not guarantee message ordering, delivery, or
timing (`third-party/orbitdb/src/sync.js:12-37`). However, it directly captures
`ipfs.libp2p`, pubsub, and an OrbitDB heads protocol address
(`third-party/orbitdb/src/sync.js:109-118`), sends head bytes over streams
(`third-party/orbitdb/src/sync.js:146-167`), reacts to pubsub subscription
events by dialing peers (`third-party/orbitdb/src/sync.js:183-224`), publishes
updates over pubsub (`third-party/orbitdb/src/sync.js:257-261`), and starts by
subscribing and registering a libp2p handler
(`third-party/orbitdb/src/sync.js:289-300`). Glade SHOULD extract the head
exchange semantics into a transport-independent sync state machine and implement
libp2p only as one adapter.

### 3.3 OrbitDB Peripherals To Replace

OrbitDB database wrappers SHOULD NOT be ported. `database.js` combines log,
storage, IPFS, sync, events, and access control into a database abstraction
(`third-party/orbitdb/src/database.js:47-115`), then applies operations by
calling `log.append`, `sync.add`, and `log.joinEntry`
(`third-party/orbitdb/src/database.js:130-161`). Glade already owns the exchange,
append-log, live-channel, declaration, lease, route, and diagnostic semantics
(`glade/AGENTS.md:7-14`).

Storage backends SHOULD be replaced by Glade storage traits. OrbitDB exposes
memory, LRU, Level, composed, and IPFS block storage
(`third-party/orbitdb/src/storage/index.js:1-10`), and its IPFS block storage
requires an IPFS instance and blockstore operations
(`third-party/orbitdb/src/storage/ipfs-block.js:29-31`,
`third-party/orbitdb/src/storage/ipfs-block.js:42-88`).

Identities and access controllers SHOULD be replaced by Glade identity,
capability, and record-envelope rules. OrbitDB identities create or load a
keystore, optionally backed by IPFS storage
(`third-party/orbitdb/src/identities/identities.js:34-48`), create identities and
store them by hash (`third-party/orbitdb/src/identities/identities.js:72-98`),
and verify provider-specific identity signatures
(`third-party/orbitdb/src/identities/identities.js:100-131`). Glade record
envelopes instead require self-describing, transport-independent validation
(`dev-docs/glade/GladeRecordEnvelope.md:10-15`) and canonical signature/hash
rules before cross-language compatibility
(`dev-docs/glade/GladeRecordEnvelope.md:71-89`).

OrbitDB manifests and addresses SHOULD NOT define Glade session identity. The
manifest store writes `{ name, type, accessController, meta }` as dag-cbor and
returns its CID (`third-party/orbitdb/src/manifest-store.js:33-50`), while
OrbitDB addresses are `/orbitdb/<manifest-cid>`
(`third-party/orbitdb/src/address.js:17-37`). Glade sessions are declaration and
capability scoped, not OrbitDB database addresses.

### 3.4 Wire Compatibility Stance

Recommendation: Glade SHOULD preserve entry-level OrbitDB wire compatibility for
the oplog spine at first, but SHOULD NOT preserve full OrbitDB database
wire-compatibility.

Specifically:

- Entry encoding SHOULD preserve dag-cbor, sha256, base58btc CID strings, and
  the signed entry field set. This enables differential tests against vendored
  JS OrbitDB and a cheap JS cooperating spike.
- Log ordering SHOULD preserve Lamport clock and conflict-resolution behavior
  within a segment.
- Glade SHOULD NOT preserve OrbitDB manifests, `/orbitdb/<cid>` addresses, IPFS
  block storage, IPFS pubsub topic behavior, or store-type APIs.
- Lazy segment anchors MAY be represented as ordinary Glade payload entries in
  the OrbitDB-compatible entry envelope. Vanilla OrbitDB will not understand the
  lazy semantics, but conformance tooling can still validate encoding, CIDs,
  ordering, and head behavior for non-anchor entries.

The hard boundary is compatibility for the Merkle-oplog core, not compatibility
for OrbitDB as a database product.

### 3.5 go-orbit-db Reference Only

`go-orbit-db` is useful as a cautionary reference, not a base. Its README says it
intends to provide a compatible Go port of the JavaScript version and is a "P2P
Database on IPFS" (`third-party/go-orbit-db/README.md:39-43`). Its module pulls
in `go-ipfs-log`, `go-libipfs`, `kubo`, go-libp2p, go-libp2p-pubsub, and many
IPFS dependencies (`third-party/go-orbit-db/go.mod:7-18`). Its interfaces expose
Kubo `coreiface.CoreAPI` as the IPFS API (`third-party/go-orbit-db/iface/interface.go:75-79`),
and the common store interface includes store types, replication status, cache,
load/sync/snapshot, IPFS, and access-controller methods
(`third-party/go-orbit-db/iface/interface.go:178-246`).

Useful ideas to inspect: package boundaries such as `stores/`, `iface/`, event
types, and `replicator/`. Not useful for Glade: adopting its IPFS/Kubo core,
store-type model, or full database API.

## 4. Rust Architecture

### 4.1 Crate Layout

The Rust workspace SHOULD be split so the core has no libp2p or Python
dependency:

- `glade-orbit-core`: `Entry`, `Clock`, `Log`, `Heads`, `Segment`, `Anchor`,
  validation, join, traversal, iterator, and sync state machine.
- `glade-orbit-codec`: dag-cbor/CID/multihash encoding, canonical bytes, and
  JS conformance fixtures.
- `glade-orbit-storage`: storage traits and in-memory/file-backed test storage.
- `glade-substrate`: transport-independent events and traits for streams,
  request/response, peer/session lifecycle, and backpressure.
- `glade-substrate-sim`: deterministic in-process simulator used by tests.
- `glade-substrate-libp2p`: rust-libp2p adapter, gated behind simulator
  acceptance.
- `glade-python`: PyO3 module exposing the Glade surface.
- `glade-wasm`: wasm bindings and browser adapter proof.

### 4.2 Core Types

The core SHOULD define:

- `Entry`: OrbitDB-compatible entry fields plus typed Glade payload envelope.
- `EntryHash`: CID string or binary CID wrapper. It MUST round-trip with JS
  OrbitDB fixtures.
- `Clock`: Lamport `{ id, time }`, ordered exactly as OrbitDB orders it.
- `Heads`: current head set plus deterministic sorted projection.
- `Log`: append, join, join_entry, traverse, iterator, and values.
- `Segment`: bounded range of entries with one writer.
- `Anchor`: signed checkpoint over segment metadata and state hash.
- `Identity`: signing and verifying trait, not tied to libp2p `PeerId`.
- `AccessPolicy`: can-append/can-anchor validation, backed later by Glade
  capabilities.

### 4.3 Storage Trait

Storage MUST be abstract:

```rust
#[async_trait]
pub trait EntryStore {
    async fn get_entry_bytes(&self, hash: &EntryHash) -> Result<Option<Bytes>>;
    async fn put_entry_bytes(&self, hash: &EntryHash, bytes: Bytes) -> Result<()>;
    async fn mark_verified(&self, hash: &EntryHash) -> Result<()>;
    async fn is_verified(&self, hash: &EntryHash) -> Result<bool>;
    async fn load_heads(&self, session: &SessionId) -> Result<Vec<HeadRef>>;
    async fn store_heads(&self, session: &SessionId, heads: &[HeadRef]) -> Result<()>;
}
```

Tests MUST include in-memory storage, fault-injecting storage, and bounded-cache
storage. File-backed storage MAY come later.

### 4.4 Substrate / Transport Boundary

The core MUST depend on a narrow Substrate contract, not libp2p:

```rust
#[async_trait]
pub trait Substrate {
    async fn open_stream(&self, peer: PeerRef, purpose: StreamPurpose) -> Result<DynStream>;
    async fn request(&self, peer: PeerRef, request: SyncRequest) -> Result<SyncResponse>;
    async fn publish(&self, group: FanoutGroup, frame: Frame) -> Result<()>;
    fn events(&self) -> Pin<Box<dyn Stream<Item = SubstrateEvent> + Send>>;
}
```

The core MUST assume only:

- A stream is ordered within that stream, until it fails.
- Messages can be delayed, duplicated, dropped, reordered across streams, and
  delivered after reconnect.
- Request/response can timeout, fail after remote processing, or race with a
  duplicate retry.
- Peer lifecycle events are hints, not authorization facts.
- Backpressure is explicit and may reject writes.
- The transport identity is not the Glade principal.

The simulator and libp2p adapter MUST implement the same trait. Any behavior that
cannot be expressed in the simulator is not a core assumption.

### 4.5 rust-libp2p Binding

rust-libp2p is a plausible adapter, not the core. The vendored workspace exposes
the facade `libp2p` v0.57.0 and relevant protocol crates including gossipsub,
request-response, relay, rendezvous, stream, mDNS, TCP, QUIC, WebSocket, WebRTC,
WebSocket-websys, WebTransport-websys, and Yamux
(`third-party/rust-libp2p/Cargo.toml:73-119`). The facade features expose the
same levers (`third-party/rust-libp2p/libp2p/Cargo.toml:13-90`), with wasm32
browser transport dependencies separated from native dependencies
(`third-party/rust-libp2p/libp2p/Cargo.toml:121-145`).

The adapter SHOULD map:

- live channel -> `libp2p_stream` raw streams;
- catch-up and exchange handshakes -> request-response;
- fan-out where actually needed -> gossipsub;
- reachability -> relay plus DCUtR;
- discovery -> rendezvous or mDNS;
- known-participant sessions -> no Kademlia dependency.

This matches the terminal proposal's libp2p lever set
(`glade/dev-docs/GladeTerminalSliceProposal.md:140-149`) and Phase 1's gate
surface (`dev-docs/Phase1Libp2pTest.md:49-57`).

The stream protocol is a useful fit because libp2p streams are the fundamental
primitive and the Rust stream behavior exposes `Control` for accept/open-stream
operations (`third-party/rust-libp2p/protocols/stream/README.md:1-12`,
`third-party/rust-libp2p/protocols/stream/README.md:47-64`). Its resource notes
are also a stress requirement: incoming streams are dropped if the application
falls behind (`third-party/rust-libp2p/protocols/stream/README.md:33-45`), and
the stream example warns that spawning unbounded per-stream tasks can break
backpressure and force OOM (`third-party/rust-libp2p/examples/stream/src/main.rs:44-55`).

Request-response is a fit for catch-up and bounded exchanges because its behavior
sends each request on a new substream and emits explicit timeout, connection
closed, unsupported protocol, response omission, and IO failures
(`third-party/rust-libp2p/protocols/request-response/src/lib.rs:21-65`,
`third-party/rust-libp2p/protocols/request-response/src/lib.rs:175-235`).
Gossipsub is fan-out only. It is a pub/sub routing layer with mesh metadata and
topic mechanics, and it does not provide peer discovery by itself
(`third-party/rust-libp2p/protocols/gossipsub/src/lib.rs:21-38`,
`third-party/rust-libp2p/protocols/gossipsub/src/lib.rs:58-68`).

### 4.6 Build vs Reuse

Recommended reuse:

- CID/multihash/dag-cbor: reuse Rust crates such as `cid`, `multihash`, and a
  dag-cbor/IPLD crate. Do not hand-roll canonical encoding.
- Signing: use `ed25519-dalek` or `k256` only behind an `Identity` trait. If
  OrbitDB JS conformance requires secp256k1 parity, support that as a feature.
- libp2p: reuse only in the adapter crate.
- PyO3/maturin: reuse for Python packaging; this is commodity compared to lazy
  sync.
- `proptest`: use for model/property tests.
- `turmoil` or `madsim`: evaluate for deterministic network simulation, but the
  Glade simulator MUST remain at the Substrate trait layer so it does not depend
  on a real transport stack.
- `automerge`/`crdts`: study only for testing ideas. They SHOULD NOT enter the
  terminal append-log core.

Build in Glade:

- Entry/log/head/join semantics, because this is the port target.
- Lazy segmentation and anchors, because this is Glade-specific.
- The simulated Substrate and Monte Carlo scale model, because this is the risk
  reducer and acceptance harness.
- The Glade exchange/live-channel/tap API surface.

## 5. Lazy / Segmented Design In Rust

The segment design MUST be additive and shallow:

```rust
pub struct Segment {
    pub session_id: SessionId,
    pub segment_id: SegmentId,
    pub writer: IdentityId,
    pub first_clock: u64,
    pub last_clock: u64,
    pub first_entry: EntryHash,
    pub last_entry: EntryHash,
    pub prev_segment: Option<SegmentId>,
    pub anchor: Option<Anchor>,
}

pub struct Anchor {
    pub session_id: SessionId,
    pub segment_id: SegmentId,
    pub prev_segment: Option<SegmentId>,
    pub prev_anchor_hash: Option<Hash>,
    pub state_hash: Hash,
    pub clock_range: RangeInclusive<u64>,
    pub last_entry: EntryHash,
    pub retention_epoch: RetentionEpoch,
    pub policy_ref: PolicyRef,
    pub signer: IdentityId,
    pub signature: Signature,
}
```

Normative rules:

- A session MUST have one accepted writer for the terminal output log.
- Entries inside a segment MUST use unchanged OrbitDB entry, clock, head, and
  conflict-resolution semantics.
- An anchor MUST be signed by the current log writer or an explicitly authorized
  anchor signer.
- The bounded join/verify path MUST stop at a trusted anchor horizon instead of
  recursively requiring older `next` or `refs`.
- Sync bootstrap SHOULD exchange "latest accepted anchor plus tail heads since
  anchor", not unbounded full-log heads.
- Retention MAY delete old segment entries only after the successor anchor and
  retention policy make the deletion valid.
- Multi-writer beyond the active terminal controller MUST be detected and refused
  for this slice. It MUST NOT silently converge with an unproven horizon rule.

What breaks under multi-writer:

- Segment boundaries are no longer unambiguous.
- Concurrent entries can span the anchor horizon.
- A shallow state hash cannot prove unseen concurrent branches without a more
  complex proof or quorum rule.
- Retention can delete history still needed to validate a remote branch.

Anchor trust is a real open risk. A signed anchor proves who asserted the
checkpoint and protects against transport tampering. It does not prove that a
malicious authorized writer chose an honest `state_hash`. The terminal slice can
accept this only if writer authority is provider-owned and attributable.

## 6. Glade Surface

The Python/Rust API MUST enforce the read/write split at the boundary:

```python
terminal = await glade.open_terminal(workspace_id, provider_hint=None)

output = await terminal.output_tap(
    cursor=ReplayCursor.tail(bytes=100_000),
)

live = await terminal.attach_pty(role="writer")

async for chunk in output:
    xterm_write(chunk.data)

await live.send_input(StdinFrame(data=b"ls\n"))
await live.send_resize(cols=120, rows=30)
```

Surfaces:

- `OpenTerminal`: exchange plane bundle. It maps to intent, substrate,
  ownership, execution, publication, observation, and diagnostics planes.
- `TerminalPty`: live channel handle. It carries stdin, resize, signals, and hot
  output. It is not replicated state.
- `TerminalOutput`: append-log/tap. It carries durable output chunks, replay
  cursors, segment anchors, retention state, and diagnostics.

The API MUST make it impossible to send keystrokes through `TerminalOutput`.
Likewise, durable reattach MUST read from the append-log/tap, not from a live
channel backlog. This matches Scenario 4, where Glade owns the live-channel
declaration, controller authority, output stream identity, replay cursor,
optional persisted output log, presence, and diagnostics, but not PTY behavior or
terminal rendering (`dev-docs/GladeHypothenicalApiStudy.md:470-551`).

Mock-to-real swap:

- Current mock: xterm local echo in `terminalController.ts`.
- First adapter: same UI calls into generated TypeScript helpers over the Python
  provider bridge.
- Real Glade-backed path: `OpenTerminal` resolves a provider, `TerminalPty`
  opens a live stream, and `TerminalOutput` opens a replayable tap.

The UI SHOULD NOT need to know whether the tap is in-memory, simulated, local
Python, wasm, or libp2p-backed.

## 7. Python Binding Strategy

PyO3/maturin SHOULD be the default packaging path. This is commodity work
relative to lazy replication and simulation, but it still has concrete risks:
async runtime ownership, GIL discipline, cancellation, and wheel compatibility.

Recommended shape:

- Build a Rust library with `cdylib` Python bindings in `glade-python`.
- Keep `glade-orbit-core` free of PyO3 types.
- Run the Rust core on a Tokio runtime owned by the Python extension or by an
  explicit `GladeRuntime`.
- Prefer command/event channels across the Python/Rust boundary for the first
  version. Expose Python `async for` streams by reading from Rust event queues.
- Evaluate `pyo3-async-runtimes` only after the command/event bridge is stable.
- Release the GIL around Rust work and reacquire only to deliver Python objects.
- Use `abi3` wheels if the exported Python API can avoid version-specific CPython
  dependencies.
- Embed in the provider server as one process. There SHOULD be no Node sidecar
  for the core path.

`py-libp2p` remains useful for contrast, not as the primary core. Its local
package is `libp2p` v0.6.0, depends on Trio/AnyIO and a broad Python networking
stack (`third-party/py-libp2p/pyproject.toml:6-49`), and its README says it is
progressing toward production readiness rather than claiming full maturity
(`third-party/py-libp2p/README.md:15-17`). It has many useful protocol surfaces
(`third-party/py-libp2p/README.md:31-96`), but the Rust core plus Python module
keeps provider ergonomics without making Python the transport spine.

### Browser Runtime Alternatives

The lowest-cost browser path is not Rust wasm. OrbitDB JS and js-libp2p already
run in the browser in the vendored stack: OrbitDB's README says the JavaScript
implementation works in browsers and Node.js
(`third-party/orbitdb/README.md:20`), supports a browser `<script>` build
(`third-party/orbitdb/README.md:31-35`), and documents module use with Helia,
js-libp2p, gossipsub, and identify
(`third-party/orbitdb/README.md:43-65`). The package also has a browser test
path using webpack and Playwright (`third-party/orbitdb/package.json:54-59`).

Therefore:

- Browser V0 SHOULD use OrbitDB JS / js-libp2p as the cooperating spike and
  conformance harness.
- The native provider path SHOULD use the Rust core exposed as a Python module.
- Rust wasm SHOULD be preserved as an optional future portability proof, not as
  the low-cost browser delivery path.
- Porting `py-libp2p` to browser APIs SHOULD be rejected or deferred as a
  high-unknown alternative. It would require Python-in-browser or Python-to-wasm
  runtime choices, browser WebSocket/WebRTC/WebTransport integration, and a
  second async/runtime bridge, while js-libp2p already provides the browser-side
  libp2p stack.

### Browser / wasm Path

The Rust core SHOULD remain wasm-compatible where it is cheap:

- no direct Tokio dependency in `glade-orbit-core`;
- injected clock and randomness;
- no filesystem assumption in core storage;
- no OS threads in core logic;
- wasm bindings in a separate crate;
- browser substrate adapter separate from native libp2p adapter.

The wasm path is not free. rust-libp2p separates wasm/browser transports in
feature and target sections (`third-party/rust-libp2p/libp2p/Cargo.toml:121-145`),
and the workspace pins wasm-bindgen-futures because of wasm dependency breakage
(`third-party/rust-libp2p/Cargo.toml:153-157`). The strategy SHOULD preserve the
option, but native Python module delivery and the OrbitDB JS/js-libp2p browser
spike are the first delivery gates.

## 8. Test Framework And Failure-Mode Study

### 8.1 Testing Posture

Every implementation phase MUST start with tests. For the port, "test" means
more than unit tests:

- deterministic unit tests for exact JS-compatible semantics;
- property/model tests for convergence and ordering;
- differential tests against vendored JS OrbitDB;
- adversarial tests for malformed and forged data;
- lazy-specific tests for segments and anchors;
- simulated-substrate tests with no libp2p;
- large-scale Monte Carlo stress tests;
- adapter tests for libp2p only after the simulator passes.

### 8.2 Stress Points And Failure Modes

The following failure modes MUST have explicit tests before implementation is
accepted:

| Area | Failure mode | Required test shape |
| --- | --- | --- |
| Canonical entry encoding | Rust produces different dag-cbor bytes, CID, or signed payload than JS | JS fixture -> Rust decode -> re-encode; Rust fixture -> JS decode; compare CIDs and signature verification |
| Signature verification | Missing key, missing sig, tampered payload, swapped signature | Regression tests matching OrbitDB signed-log failures (`third-party/orbitdb/test/oplog/signed-log.test.js:82-150`) |
| Lamport ordering | Comparator returns zero or changes tie ordering | Unit tests from OrbitDB clock/conflict tests plus proptest over random clocks |
| Head maintenance | Old head not removed, duplicate head kept, disconnected head lost | Model tests over random DAGs; compare `findHeads` with reference model |
| Append/join serialization | Concurrent local append and remote join interleave through split queues | Simulator and unit tests that schedule append/join at every await boundary; assert deterministic heads or defined conflict/retry behavior |
| Store atomicity | Crash between entry put, index mark, head add, verified mark, and head removal | Fault-injecting storage tests that crash after each storage await and replay/recover |
| Iterator consistency | Traversal observes a moving head/storage set while append or join is active | Snapshot-vs-weak iterator contract tests; assert no duplicate, missing, or panic outside the chosen semantics |
| Join traversal | Missing dependency accepted, duplicate delivery mutates state, bad log id accepted | Adversarial join tests with malformed `next`/`refs`; assert atomic rejection |
| Partial replication | Traversal crosses lazy horizon or fails to stop at trusted anchor | Segment tests with old history absent; verify bounded traversal |
| Anchor integrity | Forged anchor, stale anchor, wrong state hash, wrong clock range | Anchor signature/hash tests; byzantine writer simulations |
| Single-writer invariant | Two writers create valid-looking branches across horizon | Must detect/refuse multi-writer segment or session |
| Reattach cutover | Replay cursor and live tail duplicate or reorder chunks | Simulated PTY output with replay/live race; assert exactly-once ordered UI chunks |
| Transport abstraction leak | Core depends on libp2p-specific peer, topic, timing, or delivery | Compile/core tests using only `glade-substrate-sim`; no libp2p features enabled |
| Backpressure | Unbounded queues or per-stream tasks cause memory growth | Simulator queue caps and libp2p stream-adapter tests |
| Restart/churn | Peer stops mid-sync, restarts with stale heads, repeats messages | Deterministic random starts/stops with reproducible seeds |
| Retention | Old segment deleted before all needed proofs are sealed | Retention tests tied to GDL-012 |
| Scale claim | Per-session cost drifts toward `O(total sessions)` | Monte Carlo accounting for active participants, queues, storage, and CPU steps |

### 8.3 Property And Model-Based Tests

`proptest` SHOULD generate:

- append histories across one or more writers for the base OrbitDB-compatible
  core;
- delivery permutations: missing, duplicate, delayed, and out-of-order heads;
- random DAG shapes constrained by valid `next`/`refs`;
- random clock ties and identity ids;
- random segment sizes and anchor horizons;
- random retention and reattach points.

For the terminal slice, multi-writer generation MUST expect rejection once the
test crosses the single-writer boundary. It MUST NOT expect convergence.

Acceptance:

- all replicas converge to identical ordered entry hashes for valid histories;
- all invalid histories are rejected atomically;
- comparator output is deterministic and never zero;
- adding duplicate messages is idempotent;
- removing old segments is impossible before anchor and retention criteria pass.

### 8.4 Differential / Conformance Tests Against JS OrbitDB

The harness SHOULD run the vendored JS package `third-party/orbitdb` to generate
fixtures:

1. Create identities using the vendored test key fixtures.
2. Generate single-writer and multi-writer oplogs with chosen payloads.
3. Export entry bytes, CIDs, heads, and expected iterator order.
4. Decode in Rust and assert exact fields, signature validity, CIDs, head sets,
   and iterator order.
5. Generate Rust entries and decode in JS OrbitDB.
6. Store every failing fixture with a deterministic name and seed.

Existing OrbitDB tests provide categories to preserve: CRDT associativity and
commutativity (`third-party/orbitdb/test/oplog/crdt.test.js:44-109`), concurrent
join consistency (`third-party/orbitdb/test/oplog/join-concurrent.test.js:39-85`),
signature/access rejection (`third-party/orbitdb/test/oplog/signed-log.test.js:82-172`),
and sync eventual consistency under out-of-order update publication
(`third-party/orbitdb/test/sync.test.js:243-278`).

If full entry-wire compatibility becomes too expensive, the decision MUST be
recorded and the conformance target MUST downgrade explicitly to
order-equivalent logs, not silently drift.

### 8.5 Simulated Substrate

The simulated substrate MUST be a first-class crate. It MUST model the same
Substrate API as the libp2p adapter and MUST run without libp2p.

It MUST simulate:

- bidirectional streams;
- request/response;
- fan-out groups;
- delayed, dropped, duplicated, reordered, and corrupted frames;
- stream open failure, close, half-close, and protocol mismatch;
- peer crash, restart, suspend, and clock skew;
- process restart with persisted or lost storage;
- hub introduction success, delay, duplication, and wrong-peer bugs;
- session join/leave churn;
- partitions and healing;
- backpressure, queue caps, and slow consumers.

It SHOULD expose deterministic seeds, event traces, minimized counterexamples,
and a replay command for every failure.

### 8.6 Large-Scale Monte Carlo Simulator

The Monte Carlo simulator MUST be designed for up to millions of clients and
sessions. It MAY use compressed/session-sampled state, but it MUST preserve
enough detail to test correctness for sampled hot sessions.

Model:

- `N` clients, `S` sessions, sparse client-session membership.
- A session has one provider/writer and `P` participants.
- Most sessions are inactive or summarized.
- A small sampled frontier runs full entry/log/sync detail.
- Aggregate counters track CPU steps, queued frames, storage bytes, live streams,
  requests, fan-out, anchors, and retained segments.

Random events:

- client start/stop;
- provider crash/restart;
- session create/attach/detach;
- live input frames;
- output append bursts;
- network partitions and heal;
- peer reconnect;
- anchor creation;
- retention/GC;
- replay cursor attach;
- hub introduction delay or duplicate;
- adapter backpressure;
- malicious or malformed frames.

Required assertions:

- active-session cost is `O(participants)`;
- inactive sessions do not accumulate transport work;
- no global topic-per-session heartbeat appears in the core model;
- queue and memory caps hold under churn;
- every detailed sampled session preserves log order and reattach correctness;
- every failure has a seed and compact event trace.

This simulator is the direct answer to the scale risk in the terminal proposal:
session cost MUST NOT become `O(total sessions)`
(`glade/dev-docs/GladeTerminalSliceProposal.md:94-100`).

### 8.7 Reattach Cutover Tests

Reattach is the primary correctness seam. Tests MUST define a total output order
for the UI:

1. Open a replay cursor at log hash/segment/byte offset.
2. Fetch persisted output chunks through the tap.
3. Open or resume live output.
4. Deduplicate by output sequence and log entry hash.
5. Deliver exactly once in provider-accepted order.

Cases:

- live chunk arrives before replay catches up;
- replay includes a chunk already seen live;
- provider restarts after writing log but before live send;
- provider sends live before log persistence;
- segment boundary occurs during reattach;
- retention deletes an old segment while a replay is in progress.

### 8.8 Adapter Tests

The libp2p adapter MUST be tested after simulator gates:

- stream adapter preserves ordered bytes per stream;
- request-response maps timeouts and connection failures into Substrate errors;
- gossipsub is used only for declared fan-out;
- relay/DCUtR failures are diagnostics, not semantic state changes;
- transport `PeerId` never authorizes Glade writes;
- backpressure is surfaced to the core and Python API.

The Phase 1 libp2p gates remain useful, especially browser-to-Rust terminal
(`dev-docs/Phase1Libp2pTest.md:49-57`), but they MUST validate transport
viability only. They are not substitutes for core correctness tests.

### 8.9 Definition Of Done Per Layer

Core oplog:

- JS conformance fixtures pass.
- Property convergence tests pass.
- Malformed entries are rejected atomically.
- No libp2p or PyO3 dependency.

Lazy segments:

- Anchor tests pass.
- Bounded traversal never fetches beyond a trusted horizon.
- Multi-writer terminal sessions are refused.
- Retention tests pass or are blocked on an explicit DecisionLog item.

Simulated substrate:

- All sync tests run without libp2p.
- Deterministic replay exists for failures.
- Monte Carlo runs include random starts/stops and churn.
- Scale accounting validates `O(participants)` for active sessions.

Glade API:

- Keystrokes cannot enter the append log.
- Durable output cannot depend only on live stream delivery.
- Reattach cutover tests pass.
- Terminal mock-to-real swap requires no UI rewrite.

Python:

- Async stream cancellation is tested.
- Rust panics do not cross into Python unsafely.
- GIL is not held during long Rust work.
- Provider restart behavior is visible through diagnostics.

libp2p adapter:

- Adapter passes the same Substrate contract tests as the simulator where
  physically possible.
- Browser-to-Rust gate is classified green/yellow/red.
- Transport failures map to diagnostics.

## 9. Phased Plan And Gates

Phase 0: contract and fixture setup.

- Create JS fixture generator from vendored OrbitDB.
- Define Rust crate boundaries.
- Define Substrate trait and simulator event model.
- Gate: fixtures exist before implementation.

Phase 1: OrbitDB-compatible core.

- Port `Entry`, `Clock`, `Heads`, conflict resolution, append, join, traversal,
  and iterator.
- Gate: unit, JS conformance, and property tests pass.

Phase 2: pure simulated substrate.

- Implement transport-independent sync state machine.
- Implement deterministic simulator for streams/request-response/fan-out.
- Gate: out-of-order, duplicate, partition/heal, restart, and churn tests pass
  without libp2p.

Phase 3: lazy segments and anchors.

- Add segment metadata, anchor entry/payload, bounded traversal, anchor-aware
  bootstrap, and retention guards.
- Gate: lazy-specific tests and multi-writer refusal pass.

Phase 4: Glade terminal surface.

- Add `OpenTerminal`, `TerminalPty`, and `TerminalOutput` API.
- Add replay cursor and live-tail dedup semantics.
- Gate: reattach cutover tests pass in simulator.

Phase 5: Python wheel.

- Add PyO3/maturin packaging and provider-process embedding.
- Gate: async cancellation, diagnostics, wheel build, and provider restart tests
  pass.

Phase 6: libp2p adapter.

- Map Substrate trait to rust-libp2p streams, request-response, optional
  gossipsub, relay/DCUtR, rendezvous/mDNS.
- Gate: adapter contract tests plus Phase 1 `Gate B` and `Gate C` classifications
  (`dev-docs/Phase1Libp2pTest.md:49-57`).

Phase 7: browser/wasm proof.

- Use OrbitDB JS/js-libp2p first as the browser cooperating spike and
  conformance harness.
- Compile the Rust core to wasm only after native core, Python module,
  simulation, and browser JS spike gates are understood.
- Gate: browser can read a tap and send live input through the same Glade-shaped
  surface or an explicitly documented subset. If this uses Rust wasm, the gate
  MUST also report bundle size, startup cost, storage constraints, and browser
  transport limitations.

## 10. Open Questions And DecisionLog Candidates

Carry forward:

- Reattach cutover exact semantics.
- Lazy validation and anchor trust.
- Horizon size and reconciliation rule.
- Retention defaults for terminal logs, diagnostics, observations, and leases
  (`dev-docs/DecisionLog.md:30`).
- Hub introduction versus p2p-first topology.
- Runtime-language boundary for transport adapters.

New candidates:

- Entry-level wire compatibility versus order-equivalent divergence.
- Canonical Rust dag-cbor/IPLD crate choice.
- Anchor signing authority: provider-only, delegated anchor signer, or quorum.
- Simulator API contract and failure trace format.
- Monte Carlo scale model and minimum nightly/CI run profiles.
- Python async bridge: command/event channel versus direct
  `pyo3-async-runtimes`.
- wasm support level: core-only, browser substrate, or full browser libp2p.
- Browser runtime stance: OrbitDB JS/js-libp2p spike versus Rust wasm versus any
  py-libp2p-in-browser experiment.

## 11. References

Local Glade/G*:

- `glade/AGENTS.md`
- `glade/dev-docs/README.md`
- `glade/dev-docs/GladeTerminalSliceProposal.md`
- `dev-docs/GLDevPlan.md`
- `dev-docs/glade/GladeExchangeSemantics.md`
- `dev-docs/GladeHypothenicalApiStudy.md`
- `dev-docs/Phase1Libp2pTest.md`
- `dev-docs/glade/GladeP2PFirstTopology.md`
- `dev-docs/glade/GladeRecordEnvelope.md`
- `dev-docs/DecisionLog.md`
- `grip-lab/src/lab/terminalController.ts`

Local third-party:

- `third-party/orbitdb/src/oplog/*`
- `third-party/orbitdb/src/sync.js`
- `third-party/orbitdb/src/database.js`
- `third-party/orbitdb/src/storage/*`
- `third-party/orbitdb/src/identities/*`
- `third-party/orbitdb/src/access-controllers/*`
- `third-party/orbitdb/src/manifest-store.js`
- `third-party/orbitdb/src/address.js`
- `third-party/orbitdb/test/oplog/*`
- `third-party/orbitdb/test/sync.test.js`
- `third-party/go-orbit-db`
- `third-party/rust-libp2p`
- `third-party/py-libp2p`

External crate docs checked for candidate evaluation:

- `https://pyo3.rs/`
- `https://www.maturin.rs/`
- `https://docs.rs/pyo3-async-runtimes/`
- `https://docs.rs/proptest/`
- `https://docs.rs/turmoil/`
- `https://docs.rs/madsim/`
- `https://docs.rs/cid/`
- `https://docs.rs/multihash/`
- `https://docs.rs/serde_ipld_dagcbor/`
