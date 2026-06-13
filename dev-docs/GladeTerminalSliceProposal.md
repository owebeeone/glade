# Glade Terminal Slice — Transport and Substrate Proposal

Status: working draft — **proposal for pressure test, not yet a contract**

Purpose: capture a plausible end-to-end design for the Phase 1 terminal slice
(browser → Rust provider → PTY → output) and the substrate principles it
forces, so the shape can be attacked before any of it is promoted to a stable
contract. Nothing here is settled; the Open Questions section is the point.

## Core Claim

The terminal slice decomposes into **three surfaces** — a brokered exchange, a
1:1 live channel, and a single-writer lazy append log — carried over libp2p as a
swappable transport, with Glade owning all substrate semantics.

> **libp2p is the carrier ("TCP plus identity, secure muxed streams, NAT reach,
> and fan-out"). Glade is the librarian (what exists, who holds it, what to
> fetch, what to project, how it converges).**

This mirrors the pipeline already named in `glial-dev/dev-docs/GLDevPlan.md`:
`OpenTerminal exchange → TerminalPty live channel → TerminalOutput append log`.

## 1. Scope

In scope:
- the terminal golden-path slice and the substrate principles it forces.

Explicitly out of scope (deferred, see Open Questions):
- the multi-writer / concurrent-document CRDT regime,
- the full Frankenapp composition,
- the runtime-language decision (Python vs Go vs Rust transport).

## 2. Layer Split

- libp2p provides transport only: peer identity, secure muxed streams, NAT
  traversal (relay + hole punch), and pub/sub fan-out.
- Glade provides substrate: share identity/scope, projection, convergence,
  ownership/leases, and exchange semantics.

Normative:
- Glade substrate semantics MUST sit above transport.
- The transport choice MUST remain swappable behind the Substrate plane, per
  `GladeExchangeSemantics.md` (substrate-state separated from execution).

## 3. The Read/Write Asymmetry (key principle)

Glade surfaces split into two classes that MUST NOT be conflated:

| Class | Examples | Realization |
| --- | --- | --- |
| Observable / convergent state | terminal output, documents, records | share / **append log** → **tap** (replicated, lazy, multi-subscriber) |
| Directed / ephemeral / latency-sensitive events | **keystrokes**, control input, signals | **live-stream** → raw 1:1 channel (brokered, **not** replicated) |

Normative:
- Interactive input (keystrokes) MUST be carried as a `live-stream`, not a tap.
- Interactive input MUST NOT be routed through the replicated share substrate.
  Rationale: replication latency on the input path is unacceptable for a
  terminal, and forcing directed events into shared state is the
  RPC-as-shared-state anti-pattern `GladeExchangeSemantics.md` warns against.

## 4. Terminal Decomposition (three surfaces)

### 4.1 OpenTerminal exchange (broker)
- A bounded exchange that grants the right to attach to a terminal session.
- Modeled as shared state (intent → claim → publication), hub-introduced.
- Owns authorization, addressing by `session_id`, and attribution.

### 4.2 TerminalPty live channel (1:1 raw stream)
- A 1:1 **bidirectional** raw libp2p stream: keystrokes up, live output down.
- Ordered and reliable (yamux / QUIC stream semantics).
- SHOULD start relay-routed through the hub and upgrade to a **direct**
  connection via DCUtR when network conditions allow.

### 4.3 TerminalOutput append log (tap)
- A **single-writer** (the provider) segmented append log with a replay cursor.
- Supports reattach, late join, and bounded-window reads ("last 100K").
- Projected to consumers as a **tap**.

## 5. Output Rides Both Paths (and that is the scaling story)

Output appears on the live channel (low latency for the active driver) **and**
in the append log (durable, replayable, reattachable, multi-subscriber).

Therefore:
- **Input is 1:1** — only the active driver holds a live channel.
- **Output fans out via the log/tap** — `N` watchers cost `N` log
  subscriptions, **zero** extra live channels.

This dissolves the earlier "direct connection vs many participants" tension: the
direct 1:1 channel serves the one driver; the log serves the crowd.

## 6. Session Model

Normative proposal:
- A **session is a logically isolated, hub-introduced, directly-synced
  segmented log.**
- A session MUST NOT be realized as its own global pub/sub topic or DHT record.
- Session membership SHOULD be brokered by the hub; replication SHOULD run over
  direct streams among the known participants.
- Per-session cost MUST be `O(participants)`, never `O(total sessions)`.

Rejected alternatives (rationale recorded for the pressure test):
- **session == OrbitDB-style database** (one pub/sub topic + DHT presence each):
  breaks at millions of gossipsub meshes (heartbeat load), DHT churn for
  ephemeral keys, and mesh warm-up latency exceeding session lifetime.
- **session == encrypted row in one shared log**: breaks because encryption is
  confidentiality, not partitioning — every node replicates every session's
  bytes, with no isolation, no lazy window, no clean GC, and metadata leakage.

## 7. Lazy Append Log

The single-writer log SHOULD be **segmented on the time axis** (chunk + bitmap,
applied to time):
- Each segment is a small, self-contained Merkle log plus a **signed anchor**
  `{ prev_segment, state_hash, clock_range }`.
- "Last 100K" = fetch the last `K` segments; trust the boundary anchor's
  `state_hash` for older history (never fetched). Old segments are GC-eligible.

The cut into the core CRDT is shallow and localized:
- bounded join/verify: stop fetching `next`/`refs` at a segment horizon and
  treat absent ancestry as a sealed anchor (touches the log traversal);
- additive checkpoint/anchor entry (no rewrite);
- anchor-aware sync bootstrap ("latest anchor + tail since anchor");
- conflict-resolution and the Lamport clock are **untouched within a segment**.

Single-writer is what makes this cheap and safe (unambiguous boundaries, no
concurrency spanning the horizon). The multi-writer regime is deferred.

## 8. Reuse from OrbitDB

- Glade SHOULD **port the ~1,500-line oplog + sync spine** (Merkle-CRDT log +
  head exchange) into Glade's typed runtime, faithfully, with tests against
  adversarial concurrent histories.
- Glade MUST NOT adopt the OrbitDB JS library or the `go-orbit-db` port as a
  dependency. Rationale: runtime mismatch, fixed store types, full-log
  replication (no native laziness), governance gap versus Glade's planes, and
  full IPFS-stack coupling. `go-orbit-db` is additionally unmaintained and
  tracks the superseded OrbitDB design.

## 9. libp2p Lever Set (for this slice)

- **raw stream** — the live channel and bulk transfer.
- **request-response** — windowed catch-up and exchange handshakes.
- **gossipsub** — output fan-out to many watchers (only where fan-out exists).
- **relay + DCUtR** — reach, then direct upgrade for the hot edge.
- **rendezvous / mDNS** — discovery.
- **Kademlia DHT** — NOT used for known-participant sessions.
- The hub is a thin, shardable, **introduction-only** component; the data plane
  stays peer-to-peer.

## 10. Consumer Seam

A terminal component:
- **reads** via a tap bound to `TerminalOutput`, and
- **writes** keystrokes to a `TerminalPty` channel handle.

Mock → real MUST require no consumer rewrite:
- mock = local PTY loopback + in-memory log,
- real = brokered live channel + replicated append log.

## Open Questions / Pressure Tests

The honest risk list. None of these is resolved.

1. **Reattach cutover (primary correctness seam).** Exact semantics of replay
   cursor → live tail: dedup and ordering between replayed log entries and the
   live channel. Define before anything else.
2. **Lazy/partial replication is novel and unproven** — the main design risk.
   Validate the segment/anchor scheme end to end, not just on paper.
3. **Anchor trust model.** Who signs anchors; single-writer signature vs quorum;
   what stops a malicious writer lying about `state_hash`.
4. **Horizon reconciliation.** Trivial under single-writer; reopens hard under
   multi-writer. Confirm the terminal slice is strictly single-writer.
5. **Retention** of live-channel logs and segments — open as `DecisionLog`
   `GDL-012`.
6. **Hub vs P2P-first.** A semi-central introduction hub is in tension with
   `GladeP2PFirstTopology.md`. Reconcile: is introduction-only centralization
   acceptable if the data plane stays p2p?
7. **Runtime-language decision** (port the spine to Python vs adopt `go-libp2p`
   vs `rust-libp2p`) is open and affects everything downstream.
8. **Key management** for millions of ephemeral sessions.
9. **Is gossipsub even needed for this slice?** Request-response plus a raw
   stream may suffice until multi-watcher fan-out exists. Validate before adding
   a mesh.
10. **Multi-writer document regime is deferred** — confirm the terminal slice
    never needs it, so this proposal is not silently undersized.

## Proposed DecisionLog entries (for the root log)

- Substrate spine: port OrbitDB oplog/sync vs build fresh — decision needed.
- Session realization: log + hub-introduction vs topic-per-session — proposed
  resolved toward the former; record rationale.
- Transport runtime language — open.

## References

- `glial-dev/dev-docs/GLDevPlan.md` — terminal pipeline, replay cursor.
- `glial-dev/dev-docs/glade/GladeExchangeSemantics.md` — exchange planes.
- `glial-dev/dev-docs/GladeHypothenicalApiStudy.md` — Scenario 4, `live-stream`.
- `glial-dev/dev-docs/Phase1Libp2pTest.md` — transport classification, gates.
- `glial-dev/dev-docs/glade/GladeP2PFirstTopology.md` — topology stance.
- `glial-dev/dev-docs/DecisionLog.md` — `GDL-012` retention.
- `glial-dev/third-party/orbitdb` — `src/oplog/*`, `src/sync.js` spine reference.
- `glial-dev/third-party/rust-libp2p`, `glial-dev/third-party/py-libp2p` —
  transport options.
