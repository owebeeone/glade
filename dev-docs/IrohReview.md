# Iroh ecosystem review — support tools, libraries and their capabilities

Status: reference, non-normative. Verified 2026-09-12 against crates.io, docs.rs,
docs.iroh.computer, the iroh blog, the n0-computer GitHub org and this repo's
`node/Cargo.lock`. Version numbers are as seen on that date.

Purpose: one place that says what each crate, binary, hosted service and
binding in the iroh ecosystem actually provides, so glade design docs can cite
capabilities rather than assumptions. Scope: the n0-computer ecosystem around
iroh 1.x plus the third-party crates it depends on. Out of scope: alternatives
(libp2p) and glade design decisions themselves (§11 only records observations).

Related: `GladePeerSyncNotes.md` (the carrier), `GladeDirectoryNotes.md`,
`GladeSubstrateV1.md` GQ-2, `../../dev-docs/glade/GladeDiscoveryModel.md`,
`../../dev-docs/research/GLRustiesP2PStory.md` (mid-2026 research, partly
stale — see §11).

## 1. Summary

- **iroh 1.0.0 shipped 2026-06-15.** Point releases since: 1.0.1 (06-29), 1.0.2
  (07-07, `iroh-relay` security fix), 1.0.3 (07-21), 1.1.0 (09-01, three security
  fixes), **1.2.0 (09-09, current)**. n0's stated plan is maintenance releases
  every few weeks, not new features.
- **Glade is on 1.0.2** (`iroh = "1"` in `node/Cargo.toml`; the lockfile resolves
  1.0.2). Superseded: see the 2026-09-21 update under §11 "Version gap".
- **1.0 commits to wire compatibility** across all 1.x minors and across the
  official language bindings; wire changes only at a major.
- **The QUIC stack is n0's own `noq`** (a diverged quinn fork carrying multipath,
  QUIC Address Discovery and NAT-traversal extensions). `iroh-quinn` is legacy
  and unused by 1.x.
- **What the core gives you:** dial-by-public-key, mutually authenticated QUIC
  (TLS 1.3 with raw public keys), hole punching with relay fallback, multipath,
  ALPN-multiplexed protocols, address lookup (DNS/pkarr, mDNS, DHT), tickets.
- **What it does not give you:** authorization/ACLs (only accept/reject hooks),
  reliable broadcast, store-and-forward messaging, a text CRDT. Those are either
  protocol crates (all still 0.x) or the application's job.
- **Protocol crates are 0.x and outside the 1.0 promise:** `iroh-blobs` (whose
  own docs still say it is not production quality), `iroh-gossip`, `iroh-docs`.
  The official Python/Node/Swift/Kotlin bindings expose only the core endpoint
  surface.
- **Hosted infrastructure** is "Iroh Services" (formerly n0des): free
  rate-limited public relays and DNS with no SLA; paid authenticated relays from
  $19/month; everything self-hostable with the same binaries.

## 2. Version snapshot (2026-09-12)

| Crate / binary | Latest | In glade lock | Role | Status |
|---|---|---|---|---|
| `iroh` | 1.2.0 (2026-09-09) | 1.0.2 | core endpoint library | stable 1.x |
| `iroh-base` | 1.2.0 | 1.0.2 | base types: keys, `EndpointId`, `RelayUrl` | stable 1.x |
| `iroh-relay` | 1.2.0 | 1.0.2 | relay client protocol + `iroh-relay` server binary | stable 1.x |
| `iroh-dns` | 1.3.0 | 1.0.2 | DNS + pkarr address lookup (new crate in 1.x) | stable 1.x |
| `iroh-dns-server` | 1.2.0 | — | pkarr relay + DNS server binary | stable 1.x |
| `iroh-metrics` | 1.0.1 | 1.0.1 | metrics registry and export | stable 1.x |
| `iroh-tickets` | 1.0.0 (2026-06-15) | — | ticket encoding, now its own repo | stable 1.x |
| `noq`, `noq-proto`, `noq-udp` | 1.3.0 | 1.0.1 | QUIC implementation | stable 1.x |
| `n0-error` | 1.0.1 | 1.0.0 | error type with call-site location | stable; replaced `n0-snafu` (0.2.3) |
| `n0-watcher` | 1.0.0 | 1.0.0 | async watchable values | stable |
| `n0-future` | 0.3.2 | 0.3.2 | runtime-independent futures, wasm-capable | 0.x |
| `netwatch` | 0.19.3 | 0.19.1 | interface/route change monitoring | 0.x (repo `net-tools`) |
| `portmapper` | 0.19.3 | 0.19.1 | UPnP / NAT-PMP / PCP | 0.x, default feature of `iroh` |
| `iroh-mdns-address-lookup` | 0.4.x | — | mDNS lookup | 0.x |
| `iroh-mainline-address-lookup` | 0.4.x | — | BitTorrent mainline DHT lookup | 0.x |
| `irpc` | 0.17.0 (2026-06-15) | — | RPC over memory / QUIC / iroh | 0.x; successor of `quic-rpc` 0.20.0 (2025-05) |
| `iroh-services` | 1.0.0 (2026-06-15) | — | client for hosted relays / metrics | replaced `iroh-n0des` 0.10.0 (2026-02) |
| `iroh-blobs` | 0.103.0 | — | content-addressed transfer | 0.x protocol |
| `iroh-gossip` | 0.101.0 | — | topic broadcast | 0.x protocol |
| `iroh-docs` | 0.101.0 | — | multi-writer key-value sync | 0.x protocol, no release since 1.0 |
| `iroh-ping` | 1.0.0 (2026-06-15) | — | reference protocol | **1.0** |
| `iroh-smol-kv` | 0.4.0 (2026-06-15) | — | signed key-value over one gossip topic | 0.x protocol |
| `iroh-content-discovery` | 0.3.0 (2025-10) | — | tracker protocol | experimental, on iroh 0.93 |
| `iroh-roq` | 0.1.0 (2025-02) | — | RTP over QUIC | stale, on iroh 0.33 |
| `iroh-willow` | 0.0.1 (2025-02) | — | Willow / Meadowcap | placeholder, dormant |
| `iroh-net-report` | 0.34.1 (2025-04) | — | network probing | **unmaintained**; folded into `iroh` behind `unstable-net-report` |
| `iroh-quinn` | 0.16.1 (2026-01) | — | old quinn fork | legacy, unused by 1.x |
| `dumbpipe` | 0.39.0 | — | pipe / port-forward over iroh | binary |
| `sendme` | 0.36.0 | — | file transfer over `iroh-blobs` | binary |
| `iroh-doctor` | 0.101.0 | — | connectivity diagnostics | binary |
| `swarm-discovery` | 0.6.3 | — | mDNS engine (rkuhn) | third party; link to the 1.x mDNS crate not verified |
| `pkarr` | 8.0.1 | — | pkarr reference crate (pubky) | third party; **not** in iroh 1.x's graph |

Third-party foundations that iroh 1.0.2 brings into glade's graph: `rustls` 0.23,
`ed25519-dalek` 3.0.0-rc.0, `blake3` 1.8, `hickory-resolver` 0.26 and
`simple-dns` (via `iroh-dns`), `tokio-websockets` 0.13 and `ws_stream_wasm`
(via `iroh-relay`, the WebSocket relay transport), `igd-next` (via `portmapper`).

## 3. Core library: `iroh` 1.x

### 3.1 Identity and addressing

- Every endpoint owns an Ed25519 `SecretKey`; its `PublicKey` **is** the address
  (`EndpointId`). The 0.x names `NodeId` / `NodeAddr` are gone.
- `EndpointAddr` = `EndpointId` + a set of `TransportAddr` (IP socket addresses,
  relay URLs, or `CustomAddr` for custom transports); `EndpointAddr::from_parts`.
- Dial: `endpoint.connect(addr, ALPN).await`. An `EndpointId` alone converts to an
  `EndpointAddr`, but that only connects when an address-lookup service is
  configured (the docs warn explicitly).
- Tickets (`iroh-tickets`): string-encoded `EndpointAddr` plus a protocol payload
  (for example a blob hash) — the standard out-of-band bootstrap.

### 3.2 Connection security (what you get for free)

- TLS 1.3 with **raw public keys (RFC 7250)**; no certificates, no CAs
  (`iroh::tls`: "Currently there is one mechanism available: Raw Public Keys").
- Both sides are authenticated. The acceptor learns `Connection::remote_id()`
  (infallible in 1.x), `alpn()` and `side()`.
- Crypto provider: rustls `ring` (default feature `tls-ring`) or `aws-lc-rs`
  (`tls-aws-lc-rs`). The key algorithm is fixed to Ed25519 by design:
  configurable key algorithms and post-quantum *signatures* were explicitly
  rejected for 1.0 (key size).
- Optional post-quantum **key exchange** shipped in 1.0 (core examples
  `pq-only-key-exchange.rs`, `prefer-pq-key-exchange.rs`). The `iroh::tls`
  module docs do not describe it, so the enabling API is not verified here.
- Relay traffic is end-to-end encrypted; relays forward by `EndpointId` and see
  only metadata (which ids talk, when, how much, source/destination IPs).

### 3.3 Authorization: not provided

- iroh has **no ACL, capability or permission system for peers**. The docs use
  "permissions" only for relay access tokens.
- The first-party hook is `EndpointHooks` (`Endpoint::builder(p).hooks(h)`):
  `before_connect` → `BeforeConnectOutcome`; `after_handshake` →
  `AfterHandshakeOutcome::{Accept, Reject { error_code, reason }}`. Hooks can only
  observe or reject; a rejected connection is aborted immediately.
- Otherwise authorize inside `ProtocolHandler::accept` using `remote_id()` and
  `alpn()`. Core examples: `auth-hook.rs`, `incoming-filter.rs`,
  `screening-connection.rs`. A "pre-auth protocol" pattern ships as an example,
  not as a library.

### 3.4 Endpoint lifecycle, presets, Router

- Construction is preset-based: `Endpoint::builder(preset)`.
  `iroh::endpoint::presets`:
  - `Empty` — sets nothing.
  - `Minimal` — "almost empty, besides setting mandatory options" (the crypto
    provider); no relays, no address lookup. **This is what glade uses.**
  - `N0` — n0 defaults: `DnsAddressLookup` (native only), `PkarrPublisher` +
    `PkarrResolver` against n0's DNS / pkarr relay, n0 production relays, rustls
    provider. mDNS and DHT are **not** included.
  - `N0DisableRelay` — as `N0` with relays disabled.
- Builder options: `alpns`, `secret_key`, `relay_mode`, `address_lookup`,
  `transport_config`, `keep_alive`, `idle_timeout`, `hooks`, `bind_addr`.
- Lifecycle: `bind()`, `connect()`, `connect_with_opts(ConnectOptions)` (0-RTT
  and friends), `accept()` → `Incoming` → `Connection`, `close()`. Accessors glade
  relies on: `id()`, `secret_key()`, `bound_sockets()`.
- `iroh::protocol::Router::builder(endpoint).accept(ALPN, handler).spawn()`
  multiplexes protocols by ALPN on one endpoint; each accepted connection runs
  its `ProtocolHandler::accept(Connection) -> Result<(), AcceptError>` on a fresh
  tokio task. Version negotiation = offer several ALPNs (`/proto/2`, `/proto/1`).
  The `custom-router` example covers adding/removing protocols at runtime.

### 3.5 Transport features

- Ordered bidirectional and unidirectional streams (`SendStream`, `RecvStream`);
  `UnorderedRecvStream` for out-of-order receive. A stream becomes visible to the
  acceptor only after the initiator has sent data on it.
- Unreliable datagrams (`SendDatagram`, `ReadDatagram`).
- 0-RTT connects (`OutgoingZeroRttConnection` / `IncomingZeroRttConnection`).
- **QUIC multipath shipped in 1.0**: several routes inside one connection, hot
  swapped as conditions change (`PathList`, `PathListStream`, `PathEvent`,
  `PathStats`, `LocalTransportAddr`). Direct↔relay migration is automatic.
- `QuicTransportConfig`, `IdleTimeout` and keep-alive are configurable; the
  numeric defaults were not verified.

### 3.6 NAT traversal and relays

- Hole punching = coordinated simultaneous open. The relay learns each side's
  public address via **QUIC Address Discovery (QAD)** and brokers the exchange.
  STUN is gone since 0.90. NAT traversal itself is a QUIC extension (QNT) in
  `noq`.
- Fallback: if punching fails, "iroh automatically falls back to the relay". n0
  claims direct connections in roughly nine of ten network configurations.
- `RelayMode::{Disabled, Default, Staging, Custom(RelayMap)}`; `Default` = n0's
  production relays. Each endpoint still keeps a "home relay" (example
  `home-relay-status.rs`).
- Browsers are relay-only over WebSocket (no UDP from the sandbox).

### 3.7 Address lookup (formerly "discovery")

- Trait `AddressLookup` (renamed from `Discovery`), `AddressLookupBuilder`,
  registry `AddressLookupServices` (several services run concurrently),
  `AddrFilter` / `FilteredAddressLookup` (filter or reorder addresses before
  publishing), `UserData` (a small application payload in published records).
- In-crate: `address_lookup::dns::DnsAddressLookup` (native only),
  `address_lookup::pkarr::{PkarrPublisher, PkarrResolver}`,
  `address_lookup::memory` (static / manual).
- Separate crates, not features: `iroh-mdns-address-lookup`
  (`MdnsAddressLookup`), `iroh-mainline-address-lookup` (`DhtAddressLookup`,
  BitTorrent mainline DHT).
- n0 hosts the DNS origin `dns.iroh.link` and a pkarr relay
  (`N0_DNS_PKARR_RELAY_PROD`). Self-hosting = run `iroh-dns-server`.
- pkarr record handling lives in `iroh-dns` (built on `simple-dns` +
  `hickory-resolver`); glade's 1.0.2 graph contains no external `pkarr` crate.

### 3.8 Observability

- `Watcher` values (`n0-watcher`) for addresses, paths and online state
  (`endpoint.online()`); `PathListStream` for path events.
- Metrics via `iroh-metrics` (default feature `metrics`); `qlog` feature
  (`noq/qlog`); `tracing` throughout; net report behind `unstable-net-report`.
- Examples: `monitor-connections.rs`, `remote-info.rs`, `home-relay-status.rs`.

### 3.9 Platforms, build, features

- Listed as supported: Linux, macOS, Windows, Android, iOS, WebAssembly (browser),
  FreeRTOS. MSRV Rust 1.91, edition 2024 (as of 1.2.0). Native needs tokio.
- 1.2.0 features — default: `metrics`, `fast-apple-datapath`, `portmapper`,
  `tls-ring`; optional: `tls-aws-lc-rs`, `platform-verifier`, `qlog`,
  `test-utils`, `unstable-custom-transports`, `unstable-net-report`.
- Browser build: `iroh = { version = "1", default-features = false }` (metrics
  breaks wasm) plus wasm-bindgen.

### 3.10 Stability and support policy

- Wire protocol: any 1.x endpoint talks to any other 1.x endpoint regardless of
  minor version or language; wire changes only at a major; 2.x must stay
  compatible with the non-deprecated parts of 1.x.
- Cadence: majors at least 6 months apart, minors at least 4 weeks, patches as
  needed. Support: a major gets 1 year full support then 1–3 years maintenance;
  a minor 3 months full then up to 1 year maintenance.
- Not covered: anything behind `unstable-*` features, canary / experimental
  channels, release candidates. Public-relay access ends 2026-09-30 for 0.9x /
  RC builds and 2026-12-31 for 0.35.

## 4. Support and infrastructure crates

- **`iroh-base`** — key types (`SecretKey`, `PublicKey`, `EndpointId`),
  `RelayUrl`, encodings shared by every crate; split out so protocol crates can
  name ids without depending on the endpoint.
- **`iroh-tickets`** (own repo since 1.0) — the `Ticket` trait and string
  encoding (`EndpointAddr` + payload) that protocol tickets such as `BlobTicket`
  build on.
- **`iroh-relay`** — two things in one crate: the relay *client* protocol every
  endpoint speaks, and the `iroh-relay` *server* binary (`iroh-relay -c
  config.toml`; Docker image `n0computer/iroh-relay`). Server capabilities: TLS
  with Let's Encrypt (`[tls] cert_mode = "LetsEncrypt"`) or own certificates;
  WebSocket transport for browsers; QAD server; per-connection receive rate
  limiting (`[limits.client.rx]`, **off by default**, backpressure rather than
  drops; 1.1.0 adds a `Status::RateLimited` message); client authentication by
  signed capability tokens bound to an endpoint key (issuer, expiry,
  permissions) or, self-hosted, an allowlist of endpoint ids or a shared
  password; `GET /healthz`; Prometheus metrics. Security fixes: 1.0.2 (relay),
  1.1.0 (CPU pin via a crafted batch message).
- **`iroh-dns`** (new in 1.x) — DNS-based endpoint lookup and pkarr record
  publish / resolve with `tls-ring` / `tls-aws-lc-rs` backends. Replaces the
  external `pkarr` dependency in the endpoint's graph.
- **`iroh-dns-server`** — "a pkarr relay and DNS server": accepts signed pkarr
  packets over HTTP and serves them as DNS so `DnsAddressLookup` can resolve an
  endpoint id under an origin. n0 runs one behind `dns.iroh.link`; the binary is
  the self-hosting path for private discovery.
- **`iroh-metrics`** — counter / gauge registry with postcard, HTTP-service and
  static-core features; every iroh crate exposes its metrics structs through it.
- **`noq` / `noq-proto` / `noq-udp`** — "general purpose implementation of the
  QUIC transport protocol in pure Rust"; started as a quinn fork, now diverged,
  carrying the draft extensions iroh needs: multipath, QAD, NAT traversal (QNT),
  0-RTT / 0.5-RTT, custom and zero-length connection ids. `noq-proto` is sans-io.
- **`n0-future`** — runtime-independent futures / streams / time abstractions so
  the same code runs on tokio and in wasm.
- **`n0-watcher`** — `Watcher` / `Watchable`: observe a value and await changes;
  used for endpoint addresses, relay state and path lists.
- **`n0-error`** — error type with call-site locations; the 1.x error idiom.
  Supersedes `n0-snafu` (still published, no formal deprecation).
- **`netwatch`** and **`portmapper`** (repo `net-tools`) — interface / route
  change detection (triggers re-probing) and UPnP / NAT-PMP / PCP port mapping
  (`portmapper` is a default feature of `iroh`).
- **`irpc`** — request / response and streaming RPC with derive macros over
  in-memory, QUIC or iroh transports; it underpins the `api` modules of
  `iroh-blobs` and `iroh-docs`. Successor to `quic-rpc`, which has had no
  release since 2025-05.
- **`iroh-mdns-address-lookup`**, **`iroh-mainline-address-lookup`** — see §3.7.
- **`iroh-net-report`** — marked unmaintained on crates.io; folded into `iroh`
  behind `unstable-net-report`.
- **`iroh-quinn`** — legacy fork at 0.16.1; not used by 1.x.
- Third party in the graph: `swarm-discovery` (rkuhn; mDNS engine, relationship
  to the 1.x mDNS crate not verified), `pkarr` (pubky; reference crate, not used
  by iroh 1.x), `hickory` (DNS), `rustls`, `ed25519-dalek`, `blake3`.

## 5. Protocol crates

All of these build on the core, plug into the `Router` by ALPN, and are
versioned independently of `iroh`. Only `iroh-ping` has reached 1.0; the rest
are 0.x and outside the wire-stability promise.

### 5.1 `iroh-blobs` 0.103.0 — content-addressed transfer

- **Model**: a blob is opaque bytes of any size addressed by its 32-byte BLAKE3
  hash; a *HashSeq* is a blob that is a concatenation of hashes; a *Collection*
  is a HashSeq whose first element is a metadata blob (`format::collection`).
  Verified streaming via bao: every 1024-byte chunk is checked against the root
  hash on both ends as it flows, so a completed response provably covers the
  requested ranges and a blob never has to fit in memory. Block size 16 KiB
  (`IROH_BLOCK_SIZE`).
- **Requests** (`protocol`): `GetRequest` (one hash + `ChunkRangesSeq`),
  `GetManyRequest`, `PushRequest` (provider-initiated upload), `ObserveRequest`
  (watch a provider's availability bitfield). Ranges are expressed in chunks,
  not bytes (`ChunkRanges::bytes()` / `chunk()` / `last_chunk()`), which is what
  makes resumption and partial fetches cheap. Request messages are capped at
  100 MiB (`MAX_MESSAGE_SIZE`); blobs are not.
- **Stores**: `MemStore` (small mutable data), `readonly_mem` (static data),
  `FsStore` (feature `fs-store`; `redb` for metadata and inlined small blobs,
  large blobs on disk; owns a runtime, so call `shutdown()`).
- **Local API** (`api::blobs::Blobs`): `add_bytes` / `add_path` / `add_slice` /
  `add_stream`, `get_bytes`, `export`, `export_ranges`, `reader`, `has`,
  `status`, `observe`, `list`, `delete`; progress streams for add, export and
  download.
- **Tags and GC**: persistent `Tag`s and process-lifetime `TempTag`s pin data;
  `GcConfig { interval, add_protected }` sweeps unpinned blobs, with a protect
  callback that can abort a sweep when its own source failed.
- **Downloader** (`api::downloader`): multi-provider downloads with a
  `SplitStrategy`, `AddProviderRequest` mid-download, shuffled provider order,
  and a pluggable `ContentDiscovery` trait.
- **Provider side** (`provider::events`): an event handler selected by
  `EventMask` receives `ClientConnected`, `ConnectionClosed`,
  `TransferStarted` / `TransferProgress` / `TransferCompleted` and
  `RequestUpdate`, and can answer with `Throttle` or an `AbortReason`. This is
  the only authorization and rate-limit point: **any peer may request any hash
  the store holds unless the handler rejects it.**
- **Router**: `ALPN = b"/iroh-bytes/4"`; `BlobsProtocol::new(&store, events)`
  implements `ProtocolHandler` and derefs to the store API (`blobs()`, `tags()`,
  `remote()`, `downloader()`).
- **Tickets**: `BlobTicket` = endpoint address + hash + `BlobFormat`, built on
  `iroh-tickets`.
- **Status**: repo active (pushed 2026-08-31), but the 0.103.0 crate docs still
  say "this version of iroh-blobs is not yet considered production quality. For
  now, if you need production quality, use iroh-blobs 0.35". Whether that
  sentence is stale boilerplate could not be verified; there is no 1.0 and no
  stability statement.

### 5.2 `iroh-gossip` 0.101.0 — topic broadcast

- **Algorithms**: HyParView membership (partial views, active 5 and passive 30
  by default, so at most about five connections per topic) plus PlumTree
  epidemic broadcast trees (eager push along a spanning tree, lazy "I have"
  repair). Both are IO-free state machines in `proto`; `net` binds them to iroh
  connections.
- **Topics**: `TopicId` is 32 bytes; each topic is its own swarm and broadcast
  scope. Guidance: derive it by hashing a meaningful string; one topic scales to
  "a few thousand peers".
- **API**: `Gossip::builder().max_message_size(n).membership_config(..)
  .broadcast_config(..).alpn(..).spawn(endpoint)`; `subscribe(topic, bootstrap)`,
  `subscribe_and_join` (waits for a first neighbor), `subscribe_with_opts`;
  `GossipTopic::split()` gives a `GossipSender` (`broadcast` to the whole swarm,
  `broadcast_neighbors` to direct neighbors only, `join_peers`) and a
  `GossipReceiver` (a stream of `Event`).
- **Events**: `NeighborUp`, `NeighborDown`, `Received(Message)`, `Lagged`.
  `Message { content, scope: DeliveryScope, delivered_from }`, where
  `delivered_from` is the relaying neighbor, **not the original author**; there
  is no author field and no per-message signature. Sender authenticity beyond
  "my authenticated neighbor relayed this" is the application's job
  (`iroh-smol-kv` shows the pattern).
- **Limits and guarantees**: `DEFAULT_MAX_MESSAGE_SIZE = 4096` bytes
  (configurable). Best effort only: no persistence, no ordering, no delivery or
  membership guarantee; outbound queues drop the **oldest** messages when full,
  inbound overload surfaces as `Lagged`. Duplicate suppression follows from
  PlumTree but is not documented as a guarantee.
- **Router**: `ALPN = b"/iroh-gossip/1"`; `Gossip` implements
  `ProtocolHandler`; `handle_connection(conn)` serves hand-written accept
  loops; `shutdown()` leaves all topics. Optional `rpc` feature; `metrics()`.
- **Status**: the most recently pushed protocol repo (2026-09-11); tracks iroh
  1.0 since 0.101.0; no 1.0, no stability statement. Compiles to browser wasm.

### 5.3 `iroh-docs` 0.101.0 — multi-writer key-value replicas

- **Model**: a replica (namespace) holds entries keyed by (namespace, author,
  key); the value is a record = BLAKE3 content hash + length + timestamp.
  Content bytes never travel with the replica; they move through `iroh-blobs`.
  Every entry is signed twice: by the namespace secret (write capability) and by
  the author key (authorship).
- **Sync**: range-based set reconciliation (Meyer): recursive partitioning and
  fingerprint comparison, so two replicas exchange only their difference.
- **Conflicts and deletes**: last-writer-wins on the author timestamp;
  future-dated entries are accepted at most 10 minutes ahead
  (`MAX_TIMESTAMP_FUTURE_SHIFT`); `delete_prefix` writes an empty entry that
  shadows every key under the prefix (a tombstone, no hard delete). It is a
  keyed LWW register set, **not** a sequence or text CRDT.
- **Storage and queries**: in-memory and `redb` file stores; a `Query` builder
  with key and author filters, sorting, and "single latest per key".
- **Access**: `Capability` is either write (holds the `NamespaceSecret`) or
  read-only (holds the `NamespacePublicKey`); a `DocTicket` is a capability plus
  a peer list, so a ticket minted from a read-only capability grants read-only
  access. This is the only first-party capability model in the ecosystem, and it
  is per namespace, not per key or per author.
- **Live engine** (`engine`): one gossip swarm per open document announces
  changes, reconciliation repairs divergence, blobs fetch content under a
  `DownloadPolicy::{NothingExcept, EverythingExcept}` filter, and a
  `ProtectCallbackHandler` keeps synced content out of blobs GC.
- **Router**: `ALPN = b"/iroh-sync/1"`; `protocol::Docs` implements
  `ProtocolHandler`; `iroh-blobs` and `iroh-gossip` must be registered on the
  same router.
- **Status**: released in lockstep with iroh 1.0 (0.101.0); last push
  2026-08-19; the lowest download count of the three protocols and no GitHub
  release entry for 0.101.0. No n0 statement of deprecation or maintenance mode
  was found, so "de-emphasised" is an inference, not a fact. Excluded from the
  official language bindings.

### 5.4 `iroh-smol-kv` 0.4.0 — signed key-value over one gossip topic

"A tiny replicated kv store that syncs over an iroh-gossip topic": scope
(`PublicKey`) → key → value with a timestamp and a signature over (key, value,
timestamp). It is the lightweight alternative to `iroh-docs` when per-writer
authenticity on top of gossip is needed without blobs or set reconciliation.
Released 2026-06-15 alongside iroh 1.0.

### 5.5 `iroh-willow` 0.0.1 — dormant

A crates.io placeholder ("This work is not released yet"), not listed on the
protocol directory, only dependabot commits in 2026. It was to bring Willow's
path-structured namespaces, Meadowcap delegatable and attenuable capabilities,
and private-set-intersection sync. Not usable.

### 5.6 `iroh-roq` 0.1.0 and `iroh-live` — media

`iroh-roq` (RTP over QUIC; `Session`, `SendFlow`, `ReceiveFlow`) still depends
on iroh 0.33 and last saw a push in 2025-06. n0's current media work is
`iroh-live` (Media-over-QUIC: `iroh-moq`, `moq-media`, `iroh-live-cli`),
self-described as an early tech preview, unpublished on crates.io, repo pushed
2026-09-10.

### 5.7 `iroh-content-discovery` 0.3.0 — tracker protocol (experimental)

Nodes announce hashes to a tracker with signed announces; requesters query for
providers and fetch with blobs; trackers probe announcers for a random chunk
before trusting them; queries use 0-RTT. The code lives in `iroh-experiments`
and depends on iroh 0.93. No n0-hosted tracker is verified (stated intent
only). Pluggable discovery now also exists inside `iroh-blobs`
(`ContentDiscovery` trait).

### 5.8 `iroh-ping` 1.0.0 — reference protocol

The documented starting point for writing a protocol: `Ping` implements
`ProtocolHandler`, `ALPN = b"iroh/ping/0"`, a ticket-driven quickstart and an
optional metrics push. The only protocol crate at 1.0.

### 5.9 Other first-party repos worth knowing

- `pigeons` (`iroh-pigeons`): SSH tunnelling over iroh with service installers;
  an application, active.
- `iroh-dht-experiment`: a Kademlia DHT over iroh connections; an experiment.
- `iroh-address-lookups`: home of the DHT and mDNS lookup crates (§3.7).
- `iroh-tor-transport`, `iroh-nym-transport`: experimental custom transports
  (Bluetooth is documented alongside) behind `unstable-custom-transports`.
- `imsg`: "a base protocol providing streams of messages", a WIP prototype,
  unpublished (the `imsg` crate on crates.io is an unrelated iMessage tool).
- Archived: `iroh-sync` (became `iroh-docs`), `beetle` (the old IPFS
  implementation), `web-transport-iroh`; the standalone `iroh-dns-server` repo
  is archived because the crate moved into the main `iroh` repo.

## 6. Binaries and operator tools

| Tool | Version | What it does |
|---|---|---|
| `iroh-relay` | 1.2.0 | Relay server; see §4. |
| `iroh-dns-server` | 1.2.0 | pkarr relay + DNS server for address lookup; see §4. |
| `dumbpipe` | 0.39.0 | netcat over iroh. `listen` / `connect <ticket>` pipe stdin/stdout; `listen-tcp --host` / `connect-tcp --addr` forward TCP; `listen-unix` / `connect-unix` forward Unix sockets (unix only); `generate-ticket`. `--custom-alpn` talks to any iroh service. No UDP mode. |
| `sendme` | 0.36.0 | `sendme send <path>` / `sendme receive <ticket>`: files and directories over `iroh-blobs`, BLAKE3-verified, resumable. CLI only (a third-party egui GUI exists). |
| `iroh-doctor` | 0.101.0 | Diagnostics: `report` (net report), `accept` / `connect` (paired throughput and hole-punch test), `port-map-probe`, `port-map`, `relay-urls` (relay latencies), `plot` (metrics); CSV metrics dump. |

There is no general `iroh` CLI in 1.x; it was removed in 0.29 ("iroh is no
longer a CLI").

## 7. Hosted services ("Iroh Services", formerly n0des)

- **Community tier, free**: public relays in the US, EU and Singapore; DNS and
  pkarr relay at `dns.iroh.link`; 7-day metrics. Terms: latest stable iroh only,
  rate limited, no uptime guarantee, "not suitable for production", and n0
  advises against public relays for sensitive data because connection metadata
  is visible to the relay operator. Relay access for 0.9x / RC builds ends
  2026-09-30, for 0.35 on 2026-12-31.
- **Pro, $19/month**: authenticated shared relays (per-project API key,
  launched 2026-09-08), 10,000 concurrent connections, 5 MB/s rate limit,
  100 GB egress, 30-day metrics, support tickets.
- **Dedicated, $199/month per region** (add-on to Pro): dedicated relays, 60,000
  concurrent connections, no rate limit, 250 GB egress, version pinning, custom
  DNS.
- **Enterprise**: custom; BYOC / on-prem / multi-cloud, SLAs.
- Overages: $0.003 per endpoint, $0.09 per GB egress, $1.49 per 1k metric data
  points per minute.
- **Authenticated relays** (2026-07-30): relays admit only endpoints presenting
  a signed capability token bound to their key; a leaked relay URL alone is
  useless. Self-hosted relays get the same via an allowlist or a shared
  password.
- **Client crate `iroh-services` 1.0.0**: `iroh_services::preset()` with
  `.relays([...])` and `.api_secret_from_env()` (`IROH_SERVICES_API_SECRET`);
  metrics are reported after `endpoint.online().await`, the dashboard updates
  once a minute. Its predecessor `iroh-n0des` stopped at 0.10.0 (2026-02-11);
  the n0des name survives only in older material and the metrics host name.
- Self-hosting everything (relay + DNS server) is supported and free; the
  binaries are the ones n0 runs.

## 8. Language bindings and platform support

- **Official bindings** (repo `iroh-ffi`, UniFFI-generated; announced
  2026-06-18): Python `iroh` 1.1.0 on PyPI, Node `@number0/iroh` 1.1.0 on npm,
  Kotlin `computer.iroh:iroh` 1.0.0 and Android `computer.iroh:iroh-android`
  1.1.0 on Maven Central, Swift via SwiftPM / CocoaPods `IrohLib` (release
  v1.1.0, 2026-07-16). A C surface is listed; Go (`iroh-go`) is
  community-maintained.
- **Surface**: "the stabilized iroh 1.0 surface (endpoints, connections, paths,
  tickets, relays, services)": dial by id, `openBi()` / `acceptBi()`.
  **Explicitly excluded**: `iroh-blobs`, `iroh-docs`, `iroh-gossip` (planned
  "later"); mDNS and Bluetooth "once underlying APIs stabilize".
- Bindings trail core (1.1.0 against `iroh` 1.2.0 today). Wire compatibility is
  the 1.0 promise, so a 1.1 binding still talks to a 1.2 Rust endpoint.
- **Browser / wasm32**: the core compiles with `default-features = false`;
  connections are **relay-only over WebSocket** (no UDP, so no hole punching),
  still end-to-end encrypted. WebTransport / WebRTC direct paths are named as
  possible future work, not shipped. `iroh-gossip` compiles to wasm (since
  0.33); `iroh-blobs` compiles but only the in-memory store works. For Node,
  Deno or Bun use the NAPI binding rather than wasm to get direct connections.

## 9. Examples and community ecosystem

**`iroh-examples` repo (12, exhaustive):** `browser-echo` (wasm echo protocol,
live demo), `browser-chat` (gossip chat, wasm + CLI, live demo), `browser-blobs`
(blobs in the browser, memory store), `custom-router` (runtime protocol
management), `dumbpipe-web` (expose a local HTTP server through dumbpipe),
`extism` (iroh from wasm plugins), `framed-messages` (tokio-util codec framing
on one bidirectional stream), `frosty` (FROST threshold signatures),
`iroh-automerge` and `iroh-automerge-repo` (Automerge CRDT sync, the latter via
`samod`), `iroh-gateway` (stateless HTTP gateway over blobs with range
requests), `tauri-todos` (iroh-docs + Tauri).

**Core repo `iroh/examples/` (18):** `connect` / `listen`,
`connect-unreliable` / `listen-unreliable` (datagrams), `echo` /
`echo-no-router`, `0rtt`, `auth-hook` (`EndpointHooks`), `incoming-filter`,
`screening-connection`, `custom-transport`, `home-relay-status`,
`monitor-connections`, `remote-info`, `search` (address lookup), `transfer`
(throughput), `pq-only-key-exchange`, `prefer-pq-key-exchange`.

**Protocol repos:** `iroh-blobs/examples` has `get-blob`, `transfer`,
`transfer-collection`, `custom-protocol`, `compression`, `expiring-tags`,
`limit` (request limiting), `mdns-address-lookup`, `random_store`;
`iroh-gossip/examples` has `chat` and `setup`; `iroh-docs/examples` has `setup`
only.

**Curated list:** `github.com/n0-computer/awesome-iroh` (updated 2026-09-10).
Verified users and projects: Delta Chat (multi-device sync and webxdc realtime
channels on `iroh-gossip`, "hundreds of thousands of devices"), Weird / Muni
Town (local-first CMS on iroh + Willow), Fedimint, p2panda, Prime Intellect and
Psyche (distributed AI training), the Oku browser, Fish Folk / Bones engine,
Mesh LLM (GPU pooling over iroh, n0 blog 2026-07-11). Community protocols and
tools: `godot-iroh` (multiplayer), `iroh-ssh` (plus an Android client),
`iroh-lan` (LAN emulation), `distributed-topic-tracker` (serverless gossip
bootstrap), `iroh-rings`, Teamtype, Zeco, p2pmux, Rayfish (mesh VPN), and
several file-sharing GUIs (Sendme-egui, Dropwire, ringdrop, Strada).

## 10. Capability matrix

| Need | Provided by | Guarantees / limits |
|---|---|---|
| Peer identity, mutual authentication | `iroh` core | Ed25519 raw-public-key TLS; `remote_id()` is proven. |
| Authorization / ACL / capabilities | **not provided** | `EndpointHooks` accept/reject plus checks in the handler; `iroh-docs` namespace secrets cover documents only. |
| Reliable ordered point-to-point | `iroh` QUIC streams | Only while both peers are online; the app frames its own messages (`framed-messages` example). |
| Unreliable datagrams | `iroh` core | QUIC datagrams, MTU-bounded. |
| Request / response RPC | `irpc` (0.x) | Typed, streaming; in-memory, QUIC or iroh transports. |
| Topic broadcast | `iroh-gossip` (0.x) | Best effort, no persistence, no ordering, no author binding, 4 KiB default messages; §5.2. |
| Store-and-forward, offline delivery, queues | **not provided** | Build on blobs/docs (mailbox pattern) or an external system. |
| Bulk content transfer | `iroh-blobs` (0.x) | BLAKE3-verified streaming, resumable, range requests; "not production quality" per its own docs; §5.1. |
| Multi-writer key-value sync | `iroh-docs` (0.x) | Keyed last-writer-wins + set reconciliation; not a text CRDT; §5.3. |
| Small signed key-value over gossip | `iroh-smol-kv` (0.x) | Per-value signatures; no blobs; §5.4. |
| Text / sequence CRDT | **not provided** | Automerge examples; Willow-based work dormant. |
| Global discovery by key | `iroh-dns` + n0's or a self-hosted `iroh-dns-server` | DNS / pkarr; records signed by the endpoint key. |
| LAN discovery | `iroh-mdns-address-lookup` | Separate crate. |
| Serverless global discovery | `iroh-mainline-address-lookup` | BitTorrent DHT; separate crate. |
| NAT traversal | `iroh` core + relays | Hole punch with relay fallback; multipath migration. |
| Browser participation | `iroh` wasm | Relay-only; gossip works, blobs in memory only. |
| Media streaming | `iroh-live` (preview) / `iroh-roq` (stale) | §5.6. |
| Hosted infrastructure | Iroh Services | Free public relays without SLA; paid authenticated relays. |

## 11. Observations for glade (non-normative)

- **Current use** (`node/src/iroh_carrier.rs`): `presets::Minimal`, relays and
  address lookup disabled, direct localhost dial by
  `EndpointAddr::from_parts(id, [TransportAddr::Ip(sock)])`, ALPN
  `glade/node/1`, one bidirectional stream per peer link, `Endpoint::accept`
  driven by hand (no `Router`). The glade node id is derived separately from the
  iroh key (`GladeDirectoryNotes.md`), so iroh's authentication proves the
  transport key and the HELLO seam binds it to the directory identity.
- **Version gap**: the lock resolves 1.0.2; 1.1.0 (2026-09-01) fixed a
  deserialization panic on crafted `EndpointAddr` `CustomAddr` variants, a relay
  batch-message CPU pin, and NAT-probe misrouting. The relay and NAT items are
  moot while `Minimal` stays; the `EndpointAddr` one matters as soon as
  addresses are parsed from untrusted input (tickets, directory records).
  `iroh = "1"` already admits 1.2.0; nothing was bumped in this review.
  - Update 2026-09-21: `node/Cargo.lock` is not tracked (`node/.gitignore`), so
    the 1.0.2 above was this checkout's stale resolution and not a pin. The
    manifest now requires `iroh = "1.2"` (glade `74ffeb0`); the node's 61 tests
    pass on iroh 1.2.0, iroh-dns 1.3.0 and noq 1.3.0.
- **Unused capabilities that map onto open design items**: `EndpointHooks`
  (accept-time rejection before any glade frame is read); `RelayMode::Custom`
  with a self-hosted `iroh-relay` (the discovery model's "public iroh relay"
  assumption meets n0's "not suitable for production" terms for the free tier);
  `iroh-dns-server` as a private pkarr / DNS origin if node discovery ever
  moves off the directory; `iroh-tickets` for out-of-band bootstrap strings;
  `irpc` as an alternative to the hand-framed protocol.
- **Stale statements in existing docs**: `GLRustiesP2PStory.md` still says iroh
  is pre-1.0 and that 1.0 slipped; 1.0 shipped 2026-06-15 and 1.2.0 is current.
  Its browser findings (relay-only, no WebTransport) and its characterization of
  `iroh-docs` as keyed LWW rather than a text CRDT remain accurate.
- **Bindings**: the official Python / Node / Swift / Kotlin bindings expose the
  endpoint surface only, and browser iroh stays relay-only; both are consistent
  with GQ-2 (browser↔node over websocket, iroh node-to-node only).

## 12. Sources

Core and policy: [Iroh 1.0 announcement](https://www.iroh.computer/blog/v1),
[The road to iroh 1.0](https://www.iroh.computer/blog/the-road-to-iroh-1-0),
[iroh 1.1.0 security fixes](https://www.iroh.computer/blog/iroh-1-1-0),
[release policy](https://docs.iroh.computer/about/release-policy),
[compatibility](https://docs.iroh.computer/compatibility),
[endpoints](https://docs.iroh.computer/concepts/endpoints),
[protocols](https://docs.iroh.computer/concepts/protocols),
[writing a protocol](https://docs.iroh.computer/protocols/writing-a-protocol),
[endpoint hooks](https://docs.iroh.computer/connecting/endpoint-hooks),
[security and privacy](https://docs.iroh.computer/concepts/security-privacy),
[NAT traversal](https://docs.iroh.computer/concepts/nat-traversal),
[relays](https://docs.iroh.computer/concepts/relays),
[QAD](https://www.iroh.computer/blog/qad),
[address lookup](https://docs.iroh.computer/concepts/address-lookup),
[local address lookup](https://docs.iroh.computer/connecting/local-address-lookup),
[DHT address lookup](https://docs.iroh.computer/connecting/dht-address-lookup),
[wasm / browser](https://docs.iroh.computer/languages/wasm-browser),
[docs.rs iroh](https://docs.rs/iroh/latest/iroh/) (`endpoint`, `endpoint::presets`,
`RelayMode`, `tls`, `address_lookup`), [noq](https://github.com/n0-computer/noq),
crates.io API for every crate in §2.

Infrastructure and services:
[self-hosted relays](https://docs.iroh.computer/iroh-services/relays/self-hosted),
[public relays](https://docs.iroh.computer/iroh-services/relays/public),
[rate limiting](https://docs.iroh.computer/relays/rate-limiting),
[authenticated relays](https://www.iroh.computer/blog/authenticated-relays),
[shared relays](https://www.iroh.computer/blog/shared-relays),
[metrics](https://docs.iroh.computer/iroh-services/metrics/how-it-works),
[pricing](https://www.iroh.computer/pricing),
[iroh-services crate](https://crates.io/crates/iroh-services),
[iroh-n0des crate](https://crates.io/crates/iroh-n0des).

Protocols: [protocol directory](https://www.iroh.computer/proto),
[iroh-blobs docs](https://docs.rs/iroh-blobs/latest/iroh_blobs/),
[blobs guide](https://docs.iroh.computer/protocols/blobs),
[iroh-gossip docs](https://docs.rs/iroh-gossip/latest/iroh_gossip/),
[gossip guide](https://docs.iroh.computer/connecting/gossip),
[iroh-docs docs](https://docs.rs/iroh-docs/latest/iroh_docs/),
[kv-crdts guide](https://docs.iroh.computer/protocols/kv-crdts),
[iroh-smol-kv](https://github.com/n0-computer/iroh-smol-kv),
[iroh-willow](https://github.com/n0-computer/iroh-willow),
[iroh-roq](https://crates.io/crates/iroh-roq),
[iroh-live](https://github.com/n0-computer/iroh-live),
[content discovery](https://www.iroh.computer/blog/iroh-content-discovery),
[iroh-ping](https://docs.rs/iroh-ping/latest/iroh_ping/),
[imsg](https://github.com/n0-computer/imsg).

Tools, bindings, examples, community:
[dumbpipe](https://github.com/n0-computer/dumbpipe),
[sendme](https://github.com/n0-computer/sendme),
[iroh-doctor](https://github.com/n0-computer/iroh-doctor),
[0.29 release post (CLI removal)](https://www.iroh.computer/blog/iroh-0-29-net-is-the-new-iroh),
[iroh-ffi](https://github.com/n0-computer/iroh-ffi),
[language support announcement](https://www.iroh.computer/blog/iroh-language-support),
[PyPI iroh](https://pypi.org/project/iroh/),
[npm @number0/iroh](https://www.npmjs.com/package/@number0/iroh),
[iroh-examples](https://github.com/n0-computer/iroh-examples),
[awesome-iroh](https://github.com/n0-computer/awesome-iroh),
[Delta Chat case study](https://www.iroh.computer/solutions/delta-chat),
[Weird](https://github.com/muni-town/weird),
[roadmap](https://www.iroh.computer/roadmap) (stale: last updated 2026-02, ends at 1.0).

Local: `node/Cargo.toml`, `node/Cargo.lock`, `node/src/iroh_carrier.rs`,
`node/src/mesh.rs`.
