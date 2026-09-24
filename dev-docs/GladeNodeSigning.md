# Node signing — the decisions Step 4.1 needs

Decision note, 2026-09-24, for the owner. Read-only: no code was changed, built or
committed. Step 4.1 of `dev-docs/GladeFirstSlicePlan.md` ("Genuine signing and the key",
`:725-742`) cannot start until these choices are made; the slice profile
(`dev-docs/glade/GladeFirstSliceProfile.md`, SP-P3 to SP-P5, §8 items 3–7) records them as
not decided. Each section gives the open options, their costs, and a recommendation, with a
line for the ruling.

Paths are from the glade-wz root; `gryth-wz/` is the sibling workspace; `node/src/…` means
`glade/node/src/…`. Step 4.4 was editing `glade/node` during the read, so line numbers in
`registry.rs`, `store.rs`, `claims.rs`, `sysdir.rs` and `assembly.rs` are as read at about
15:45 and may move.

## Summary

"Before" names the step that cannot start without the ruling; D11 splits 4.1 into 4.1a (key,
identity, HELLO), 4.1b (directory records signed) and 4.1c (custody). So D1–D3, D6, D7's
hello tag and D11 are needed before 4.1 starts; D4, D5, D8 and D9 can wait for 4.1b, which
follows 4.4; D10 can wait for 4.1c.

| # | Decision | Recommendation | Before | Blocks |
| --- | --- | --- | --- | --- |
| D1 | Algorithm and crate | Ed25519, `ed25519-dalek = "=3.0.0"`: stable, resolves offline, no new crate; strict verification | 4.1a | every signature; a policy re-review |
| D2 | Node identity | NodeId = the node key's Ed25519 public key; each `node.key` kept as the seed; every id changes once | 4.1a | HELLO, D8, 4.2 |
| D3 | Which key signs | `node.key`; account-root certification recorded as a slice gap | 4.1a | nothing to build |
| D4 | Where an op signature travels | inside the directory record's payload, a signed envelope in the node's own record schema; no wire change | 4.1b | directory signing |
| D5 | Who signs client ops | no one in the slice: the node signs every home-share record and refuses client writes to `home` | 4.1b | 4.3, in substance |
| D6 | What HELLO signs | a transcript bound to the connection: role, node id, both endpoint ids, TLS-exported bytes; ALPN `glade/node/2` | 4.1a | 4.2 |
| D7 | Domain strings | `glade/v1/peer-hello`, `glade/v1/origin-op`, `glade/v1/local-overlay`, zero-terminated; grants use origin-op; discovery uses the same origin-op tag | 4.1a, 4.1b | discovery's adapter |
| D8 | Existing stores | set unsigned directory records aside once into a history file, then re-mint; app data untouched | 4.1b | the owner's upgrade |
| D9 | Verifier unavailable | never persisted or folded; retried next round; reported as *deferred* | 4.1b | — |
| D10 | Key custody | a separate recovery key, committed in the node's chain, written where the operator names by a one-shot command | 4.1c | nothing; can wait |
| D11 | Budget | does not fit; 4.1a, then 4.1b after 4.4, then 4.1c | now | the order of 4.2, 4.3 |

## What already binds these choices

- `proof_family = taut_grants`: grants stay Glade's CBOR records, "signed under the node
  chain", with one corpus proving Rust, TypeScript and Python agree on the bytes
  (`decisions/glade-decisions-rulings.gyld.py:109-116`).
- `key_custody = recovery_keys` (`:77-85`), one of WD-1's three options: paper backup,
  recovery keys, social recovery (`dev-docs/glade/GladeWorkspaceDirectory.md:267`).
- `transport_key_binding = binding_record`: the iroh key stays transport-only; "identity must
  survive a transport key being replaced" (`:99-106`).
- B5, ratified: grant, revoke, name-claim and membership ops are signed, carry their strict
  predecessor, and are verified before persistence or fold; sessions prove "a device key
  certified by the account root"; unsigned legacy records may be kept as history but never
  govern (`plan-docs/plans/GLP-0006-grazel-gryth-suppliers/RulingWorksheet.md:272-293`;
  `dev-docs/glade/GladeAuthzModel.md:125-150`).
- H-R3: direct client appends only for record kinds with no privileged effect
  (`RulingWorksheet.md:497`).
- Plan §3: no wire-IR change (`GladeFirstSlicePlan.md:38-40`, `:917-918`).

## D1. Signature algorithm and crate

No ruling picks one; GDL-007 is open (`dev-docs/DecisionLog.md:25`). Ed25519, named by the
plan and `GladeAuthzModel.md:58`, fits: 32-byte keys match the 32-byte `node_id` on the wire;
signatures are deterministic, so a retried append re-signs to the same bytes and the registry
takes it as a duplicate, not a fork (`node/src/registry.rs:332-344`); browsers (WebCrypto)
and Python have it.

| Option | Lock change | Cost |
| --- | --- | --- |
| (a) `ed25519-dalek =3.0.0-rc.0` | none: iroh 1.2 already brings it (`glade/node/Cargo.lock:569-570`) | a release candidate, pulling another, `curve25519-dalek 5.0.0-rc.0` (`:403-404`) |
| (b) `ed25519-dalek =3.0.0` | both dalek crates move to stable; no new crate | none found |
| (c) `ed25519-dalek 2.x` | ten new crates; fails offline (`const-oid` not cached) | a second Ed25519 stack beside iroh's |
| (d) `ring =0.17.14` (in the lock via rustls and iroh's QUIC, `:2342-2343`) | none | 0.x, so an exact pin that also freezes theirs; a second Ed25519 implementation |

The stable release needs no fetch: it is in the local cargo cache, the async witness already
builds iroh 1.2.0 on it (`glade/dev-docs/async-witness/Cargo.lock:434-435`, `:600-601`), and
iroh 1.2 accepts `>=3.0.0-rc.0,<4.0.0` (iroh-base 1.2.0's `Cargo.toml:92-93` in the cache).
Pin exactly: another verification library changes
which signatures count, and the pin also governs iroh's copy. Verify with `verify_strict`
(D4 says why).

Policy: the checker demands an exact match with `glade/node/architecture-policy.json`
(`glade/node/check.sh:184`), so `normal:ed25519-dalek` needs the owner's re-review, as shaku
and sdax did. Also add confinement rows (`check.sh:65-77`) `node ed25519-dalek glade-node` and
`contracts ed25519-dalek -`, keeping the ports algorithm-free
(`glade/contracts/signer-api/src/lib.rs:50-54`).

**Recommend (b).** **Ruling:** open.

## D2. Node identity, and how a verifier finds a node's key

Today `node_id = hex(sha256(node.key))` (`node/src/sysdir.rs:218-223`), a hash of 32 secret
random bytes (`:253-259`); no verifier can get a public key from it. The code calls it "a
deterministic stand-in for the ed25519 pubkey" (`:21-23`, `:218`). An Ed25519 seed is 32
bytes, so every existing `node.key` becomes the signing seed unchanged, under any option.

| | (A) id = public key | (B) id = sha256(public key) | (C) keep today's id; bind a key by record |
| --- | --- | --- | --- |
| Existing ids | all change once | all change once | unchanged |
| Records naming ids: `NodeRecord`, `ServeClaim.node`, `WorkspaceEntry.eligible_hosts`, every home op's origin (grants name principals like `owner`, `grazel/apps/grazel-app.glade:50-51`) | old records name an id no one can sign for; D8 decides | as A | unsigned; their chains take signed ops only after an unsigned prefix, or by rewriting from seq 0, a self-fork to any peer holding them |
| Where a verifier gets the key | the id | carried in every signature (96 bytes) or in a record | a `NodeKey{node, key}` record no one can check, since the id hashes a secret: first seen wins, or configuration |
| HELLO: `node_id` is 32 bytes (`glade/wire-rs/src/generated.rs:285-289`) | fits; first contact verifies | fits; the key rides in `sig` by convention | fits; first contact cannot verify, because the record arrives by the sync HELLO opens |

Under A or B the owner's instance prints a new `node <id>` (`node/src/bin/glade-node.rs:148`),
writes fresh presence (`sysdir.rs:174-187`) and re-registers its apps. Kept old records would
leave a second id in `nodes_of` (`registry.rs:522-533`) and could win `replicas_of`, which
orders by origin, not time (`:480-493`); D8 prevents both. Under C nothing visible changes.

**Recommend A.** It is what the stand-in stood for; verification needs no lookup and works on
first contact; and a later account-root certificate (D3) can name the same id, because the id
is the device key. B buys an algorithm-independent id, like E-users-1's account
fingerprint (`RulingWorksheet.md:489`), for 32 bytes per signature and a convention hidden in
`sig`. C links id and key by assertion only; B5's premise is that "a claimed device key is
not proof of possession" (`:274-275`).

Knock-on: `SignerPort::NodeId` stays `[u8; 32]`; its doc changes (`signer-api/src/lib.rs:13`).
SI-002 wants an unknown signer to be `Unavailable` (`:136-164`); under A the adapter knows
itself and the nodes it has authenticated at HELLO on configured links, the slice's operator
trust. **Ruling:** open.

## D3. Which key signs for a node in the slice

(a) `node.key`, trusted by operator configuration (`scope_model = node_trust`). (b) A device
key certified under an account root, as E-users-1 and B5 ask (`RulingWorksheet.md:489`,
`:277-278`). (b) needs what does not exist: account roots, a certification record, root
custody, and the "merge/root transition" E-users-1 says must land first; principals are made
by hand in the slice.

**Recommend (a)**, recording B5's certification clause as a slice gap for 5.1. Under D2(A) a
certificate added later names the same id: no second identity change. **Ruling:** open.

## D4. Where an op's signature travels

The facts the options turn on:

- **Renderings.** Rust is generated: `python -m taut.corpus.glade_build` writes
  `glade/wire-rs/src/generated.rs` and `cbor.rs`, `taut/corpus/glade.ir.json` and the golden
  vectors from `taut/ir/glade.taut.py` (`taut/src/taut/corpus/glade_build.py:12`,
  `:115-134`), using the legacy decoder, which panics on a missing key
  (`taut/src/taut/gen/rust.py:245-256`; `glade/wire-rs/src/cbor.rs:22-31`). TypeScript and
  Python are codecs that read the IR JSON at run time (`glade/client-ts/src/taut/codec.ts`,
  `taut/src/taut/wire/codec.py`).
- **TS decoders.** Only `@glade/client-ts` decodes ops. glade/demo loads the live IR
  (`glade/demo/src/glial.ts:14-19`); gryth-ui loads a vendored copy
  (`gryth-wz/gryth-ui/packages/glade/src/runtime.ts:31-35`), already stale: its `Shape` enum
  lacks `swmr` and `crdt`. glial never decodes the wire (`glial/src/session.ts:6-10`);
  grip-core touches only the declaration vocabulary (`grip-core/src/core/share_decl.ts:1-17`).
- **An extra map key.** Rust ignores it and drops it at the next encode
  (`generated.rs:202-229`), and the node re-encodes what it stores (`store.rs:324`,
  `registry.rs:535`). TS and Python keep and re-emit it (`codec.ts:35-37`, `:72-75`;
  `codec.py:90`, `:167-169`), and TS hashes it (`glade/client-ts/src/hash.ts:9-11`), so the
  client's chain check disagrees with the node and reports a break (`store.ts:57-66`).
- **Hashing.** Every rendering emits optional fields always, `null` when absent
  (`codec.ts:5-6`, `codec.py:12`, `rust.py:218-219`), and hashes the whole encoding
  (`node/src/chain.rs:11-13`, `hash.ts:9-11`, `taut/src/taut/crdt/glade_chain.py:31-33`).
- No client, TS or Rust, reads or writes the `home` share today.

The options:

- **(a) Optional field 11 on the wire `Op`**, discovery's frozen form
  (`dev-docs/glade/GladeDiscoveryDesign.md:180-184`). Every op's bytes change (`11: null`) and
  so every `op_hash`, unless all three renderings and taut's oracle hash fields 1–10 instead.
  Regenerated Rust cannot decode an op without key 11: every stored journal and snapshot, and
  every op from a client on the old IR. Every store and client moves on one day, gryth-ui's
  copy included. A wire amendment, excluded by plan §3, with its own review.
- **(b) Beside the ops, or only in sync frames.** A second store kept in step, and signatures
  in the snapshot. Peers need them, and `Ops{ops, pri}` has no slot
  (`generated.rs:380-383`): a new frame or field, a wire change after all. Frames alone also
  lose them at rest, so a restarted node cannot re-serve verifiable records.
- **(c) Inside the directory record's payload.** The node's record schema
  (`glade/node/ir/sysdata.taut.py`, regenerated with `--legacy-codec`,
  `dev-docs/GladeProgramStatus.md:29`) gains one message, a signed envelope `{record, sig}`,
  and every home payload becomes one. It signs the op's fields 1–10 with the inner record as
  payload, so it covers the chain position (`origin`, `seq`, `prev`). No wire, rendering,
  client or hash rule changes; the op hash covers the envelope, so the chain commits to the
  signatures. Costs: the 29 places that decode a home payload (`claims.rs`, `exchange.rs`,
  `mesh.rs`, `registry.rs`) and the byte comparisons in `register` and `Registry::contains`
  (`registry.rs:437-439`) go through one helper; unsigned records are recognised before
  decoding, because the legacy decoders panic on a type mismatch (every current kind's field
  1 is text, profile `:32-42`; the envelope's is bytes); verification must refuse
  non-canonical signatures (`verify_strict`), or a third party could re-encode one and so
  manufacture a "fork" of an honest chain; old and new nodes must not sync (D6), since an old
  one panics on an envelope. It is not discovery's form, but no
  translation between the families exists (profile SP-R4): any cutover rewrites the records.
- **(d) None in the slice**, HELLO only. Cheapest; B5 unmet for grants and revocations; 4.1's
  "forged origin signature is rejected at ingest" (`:740-741`) dropped; 4.3's grant check
  trusts whatever a trusted peer holds, under any origin.

**Recommend (c)**, leaving (a) to the wire amendment that app-op signatures will need.
**Ruling:** open.

## D5. Who signs the ops that clients originate

Clients send finished ops under their own origins and the node stores them as they arrive
(`node/src/server.rs:262-282`). A browser origin is six random characters per tab
(`gryth-wz/gryth-ui/packages/glade/src/runtime.ts:42-50`).

- **(a) Clients sign.** Each tab holds a key (WebCrypto has Ed25519) and its origin becomes
  that key; app payloads belong to the apps, so this needs D4(a)'s field 11, and B5 wants the
  keys certified by an account root. Not slice-sized.
- **(b) The node signs on accept.** The signature then says "node N took this from some
  session", not "the origin wrote it": an operator-vouched statement
  (`GladeAuthzModel.md:297-305`) that B5 bars from strong security actions (`:148-150`). It
  still needs a carriage for app ops.
- **(c) The node re-originates client ops.** Breaks every client chain, the one-writer rule of
  `swmr` surfaces (`store.rs:176-181`), the value fold's tie-break by origin
  (`lww_lamport_origin`, `taut/corpus/glade.ir.json:46`), and attribution.
- **(d) Only the node's own directory records are signed.** B5's list (grant, revoke,
  name-claim, membership; `GladeAuthzModel.md:135-137`) covers `CapabilityGrant` and
  `CapabilityRevocation`; no name-claim or membership kind exists yet. Sign all nine home kinds
  and 4.2's binding anyway: the node writes them all and they all steer routing or authority.
  The node also refuses client writes to `home`, as H-R3 requires; no client writes it today.

**Recommend (d).** App ops stay unsigned, a 5.1 gap: the one-op tamper window
(`glade/dev-docs/GladePeerSyncNotes.md:77-84`) stays open for them. **Ruling:** open.

## D6. What HELLO signs

Today's "signature" is `sha256("glade/peer/hello" ‖ node.key)` (`node/src/peer.rs:73-81`),
the same bytes on every connection; `verify_peer` accepts anything (`:96-101`).

- **(a) A transcript of both node ids and the protocol.** The dialer speaks first and knows
  only the peer's endpoint id (`node/src/iroh_carrier.rs:50-59`), so it can name only itself,
  and nothing in the transcript changes between connections: a recorded HELLO replays.
- **(b) The transport session.** Sign the protocol, the role (dialer or acceptor), the node
  id, both iroh endpoint ids, and 32 bytes of keying material exported from the connection's
  TLS session (`Connection::export_keying_material`, iroh 1.2
  `src/endpoint/connection.rs:1076-1092`). Both ends compute the same bytes without sending
  anything. A replay on another connection fails (other bytes), a reflection fails (role),
  and a relay through a man in the middle fails (two TLS sessions, two sets of bytes and ids).
- **(c) A challenge.** Each side sends a nonce for the other to sign. `NodeHello` and
  `NodeWelcome` carry only `node_id`, `protocol` and `sig`, so a nonce is a wire change; the
  exported bytes are a challenge both sides already share.

With 4.2: 4.1a takes `remote_id()` and the exported bytes where `dial` and `accept` hold the
connection (`iroh_carrier.rs:120-136`); in-memory tests pass a fixed value. 4.2 adds the
`CarrierPort` accessor (plan `:761-764`), the check that `node_id` is bound to `remote_id()`,
and refusal at accept. A completed HELLO is itself a signed "node N speaks through endpoint E
here"; the binding record is still needed before HELLO (the accept hook sees only an
endpoint id) and for publishing addresses. Under (a) 4.2's check would rest on a replayable
statement; under (c) it waits for the wire change.

**Recommend (b)**, with the ALPN moved to `glade/node/2` (`iroh_carrier.rs:24`) and
`PROTOCOL` to 2 (`peer.rs:31`), so old and new nodes fail at connect, not mid-sync. `NodeHello`
keeps its fields; `sig` carries 64 bytes. **Ruling:** open.

## D7. Domain strings

`SignerPort` requires the purpose to be part of what is signed (`signer-api/src/lib.rs:50-54`)
and has three purposes (`:16-25`). Recommend an ASCII tag ending in a zero byte, prepended:

| Purpose | Tag | Message |
| --- | --- | --- |
| `PeerHello` | `glade/v1/peer-hello\0` | D6's transcript, canonical CBOR |
| `OriginOp` | `glade/v1/origin-op\0` | the op's fields 1–10, canonical CBOR, inner record as payload |
| `LocalOverlay` | `glade/v1/local-overlay\0` | the overlay file's canonical bytes |

No tag is a prefix of another, so no signature crosses purposes (SI-002). Ed25519's context
variant is avoided: WebCrypto lacks it. Grants get no tag: a grant is signed as the op that
carries it, which is all "signed under the node chain" means while grants have no parent
links (profile SP-P1). `glade/v1/grant\0` is reserved for detached grants such as invite
tickets (`dev-docs/IrohGladeMapping.md:399-400`). Each tag gets vectors in the `proof_family`
corpus.

Discovery must use the same origin-op tag, or the one adapter meant to "implement both and
run both suites" (`signer-api/src/lib.rs:7-11`) cannot exist. Its `Signer` takes raw bytes
with no purpose (`glade-discover/crates/glade-discover-signature-api/src/lib.rs:33-42`), so
its Ed25519 implementation prepends the tag. The families cannot be confused: discovery
rejects all nine node kinds as malformed (profile `:88-90`). **Ruling:** open.

## D8. Existing stores

The owner runs one node: gyld-ui starts grazel `--mode both`
(`gryth-wz/gryth-ui/gyld-ui.py:71`, `:1487-1490`), which starts
`glade-node --profile local --name grazel --app …` under `GLADE_HOME=<data>/sys` on the
hand-written root, with no peers (`grazel/src/lib.rs:67-72`, `:223-250`). There
`records.json` holds only the node's own writes (`sysdir.rs:174-187`, `glade-node.rs:157-165`,
`claims.rs:273`), and
`cache/store` holds home-share copies plus all app data (`glade-node.rs:180-183`). Both roots
share `sysdir.rs`, `peer.rs` and the `Server` (`node/src/lifecycle.rs:176-181`), so 4.1
reaches this instance at its next rebuild and restart. B5 already rules that unsigned records
may be kept as history but never govern (`RulingWorksheet.md:277-283`).

- **(a) Quarantine.** Unsigned records stay out of every fold; since the first save rewrites
  `records.json` from the valid set (`sysdir.rs:186`), keeping them means writing them once to
  a side file. The node re-mints at boot: presence (`sysdir.rs:174-187`), registrations
  (`node/src/appdecl.rs:561-617` finds the fold empty), claims as it serves, principals as
  tabs reconnect.
- **(b) The node vouches once**, with an origin-signed checkpoint over the old chains' heads
  (AZ-12's shape, `GladeAuthzModel.md:341`). Under D2(A) the voucher is not the old origin,
  so it asserts a succession no one else can check, and it keeps unsigned grants governing,
  which B5 forbids. The owner loses nothing.
- **(c) Re-sign.** Impossible across an id change; and under D4(c) a signature changes the
  payload and every later `prev`, so re-signing is re-minting, with (a)'s losses.
- **(d) Start fresh, by hand.** Deleting `records.json` and the home-share files in
  `cache/store` loses nothing the node does not re-mint, but it is manual and the store's
  layout is 4.4's. Deleting the whole instance also loses `node.key` and all app data in
  `cache/store`: chat, terminal logs, gyld output.

Under (a) the owner loses the old id (D2), principal records until tabs reconnect, and claim
epochs; keeps `node.key`, all app data and the UI.

**Recommend (a), automatic:** at 4.1b's first boot, unsigned home records are written once
to `records.legacy-<date>.json` and never folded; the served store's home copies go with them
(its `open` replays the journal unchecked, `store.rs:98-126`, so 4.1b makes home ops verify
there too); the node re-mints. The store half waits for 4.4. **Ruling:** open.

## D9. "Verifier unavailable"

`SignerPort` keeps "cannot tell" (`Err(Unavailable)`) apart from `Invalid`
(`signer-api/src/lib.rs:33-38`, `:56-59`). Under D2(A) it means the origin is not a node this
node knows, or its own key cannot be read.

- (a) Treat it as a bad signature: quarantines a possibly valid op; the port forbids it.
- (b) Withhold and retry: never persist or fold; stop that origin's chain from that peer for
  the round, since later ops chain on it; count it as *deferred*, beside *applied* and
  *rejected* in `SyncOutcome` (`peer.rs:154-166`); the next round re-fetches it.
- (c) Fail the round or link: one unknown origin stalls every other chain.
- (d) Accept now, check later: breaks B5's verify-before-persist.

**Recommend (b)**, reported as `deferred N` beside today's `quarantined N` at boot
(`glade-node.rs:149-151`), one stderr line per chain per round, and on the assembled path's
console. An unreadable own key already refuses the start; HELLO closes the link. Discovery's
kernel has no such result (profile SP-P5); its host withholds the op the same way.
**Ruling:** open.

## D10. Key custody

`node.key` stays 0600: created so (`sysdir.rs:242-247`) and refused if group- or
world-accessible (`:225-236`).

- **(a) A recovery key**, the ruling's form: a separate Ed25519 key; the node appends a signed
  record committing to its public half, writes the secret half where the operator names, and
  keeps no copy. Nothing uses it until rotation exists (a 5.1 gap), but the commitment must be
  made while the node key is trusted. Once account roots exist, recovery belongs to the root.
- **(b) A paper backup of `node.key`**: simplest, restores the same id, as dangerous as the
  key. WD-1 offered it; the ruling passed it over.
- **(c) Defer**: contradicts "minted at first setup" (plan `:737-739`).

A node that already has a key keeps it (D2 reuses the seed). Its "first setup" is the
operator's first run of a one-shot command on the stopped instance,
`glade-node recovery --name <name> --out <path>`: it takes the instance lock
(`sysdir.rs:79-106`), mints, commits, writes, exits. A new node can take
`--recovery-out <path>` at first boot. The place is named on the command line only; the node
refuses a path inside `GLADE_HOME`, never overwrites, writes 0600, and says to move the file
offline. Until then each boot warns and starts, so grazel is untouched.

**Recommend (a).** **Ruling:** open.

## D11. Budget

Recent steps ran 619, 773 and 887 lines against about 500 (plan `:629`, `:652`, `:676`). 4.1
does not fit. Estimates, tests included:

| Step | Contents | Lines | After |
| --- | --- | --- | --- |
| 4.1a — the key and HELLO | pin and policy rows; the Ed25519 `SignerPort` adapter with tags and SI-001..003; id = public key; HELLO over the session; ALPN bump; the adapter in place of `PendingNodeSigner` (`assembly.rs:757-790`), movable to 4.1b if long | ~550 | D1–D3, D6, D7 |
| 4.1b — directory records signed | the envelope; sign at append; verify at every home ingest (registry load and ingest, the served store's append and open, sync pull, seeding); `prev` required; client writes to `home` refused; legacy set aside; deferred reporting; two commits if needed | ~600 | 4.1a, 4.4, D4, D5, D8, D9 |
| 4.1c — custody and overlay | recovery key and command; the local overlay's check (`sysdir.rs:265-271`) | ~250 | 4.1a, D10 |

4.2 can start after 4.1a; 4.3 should follow 4.1b, or its tests should name F2's bypass.
**Ruling:** open.

## Findings beyond the questions

- **F1. The iroh endpoint key is new on every start.** `bind_endpoint` sets none
  (`iroh_carrier.rs:32-40`), so iroh generates one (iroh 1.2 `src/endpoint.rs:228`,
  `:522-534`). 4.2's binding record, 4.5's `--peer <endpoint-id>@…` and the relay crossing all
  need a stable key: a second class-1 secret, or one derived from `node.key`. It needs no
  recovery material; the binding ruling expects transport keys to be replaced.
- **F2. Any websocket client can write the directory.** The client path appends ops for every
  share, `home` included (`server.rs:262-282`), and routing reads the served store's home share
  (`mesh.rs:121-142`, `:512-524`): a forged `ServeClaim` with a high epoch redirects a share,
  and a malformed home payload panics every session that folds it. So 4.3's grant check means
  little before 4.1b. The handshake checks no `Origin` header (`node/src/ws.rs:93-118`;
  "trusted localhost", `:6`), so a page from another site may reach the node if the browser
  allows it. D5 closes the directory part; record the `Origin` check.
- **F3. The strict predecessor is not enforced.** The served store and the registry accept an
  op with no `prev` after seq 0 (`store.rs:277-281`, `registry.rs:347`); B5 requires one.
- **F4. Boot verification grows with renewals**: a `ServeClaim` per served share every 10 s
  (`claims.rs:42-45`, `:273`), 8,640 a day; at tens of microseconds a check, a month of one
  share adds seconds to every boot. AZ-12's checkpoints are the remedy; record it.
- **F5. The standing `cfg` rule.** `sysdir.rs:225-251` has bare `#[cfg(unix)]` and
  `#[cfg(not(unix))]` on four functions; touched by 4.1, they move into platform modules as in
  `glade-node.rs:261-309`. Off Unix, `node.key` gets default permissions and no check.
- **F6. Stale comments** for 4.1a: `peer.rs:9-11`, `:57-60`, `iroh_carrier.rs:10-12`,
  `sysdir.rs:21-26`; the wire IR's (`taut/ir/glade.taut.py:115`) waits for a wire amendment.

## Corrections the recommendations imply

Not made here; each follows the owner's ruling.

| Where | Now | Becomes |
| --- | --- | --- |
| Plan 4.1 `:733-734`; §4 item 4 `:938` | NodeId stays `sha256(key)` | the node key's Ed25519 public key (D2), ruled |
| Plan 4.1 `:725-742` | one step | 4.1a–c (D11); origin signatures scoped to home records; added: replayed and reflected HELLOs refused, client writes to `home` refused, signed home ops need `prev` |
| Plan 4.1 `:734-736` | discovery's `Signer`/`Verifier` mismatch is a blocker | the slice implements `SignerPort` only; discovery's traits wait for a family translation (SP-R4) |
| Plan 4.1 `:737-739` | recovery material at first setup | D10's recovery key and command |
| Plan 4.2 `:749-758` | — | a stable endpoint key first (F1) |
| Plan §2 `:890-908` | 4.1 → 4.2; 4.3 free | 4.1a → 4.2; 4.4 → 4.1b → 4.3; 4.1c after 4.1a |
| Plan 5.1 `:870` | rotation, session identity, checker blind spot | add: unsigned app ops, account-root certification, F2's `Origin` check, F4, F5 |
| Profile SP-P3(a) `:135-138` | field 11 blocks per-op signatures | solved for home records by the envelope; still blocks app-op signatures and `SignedOp` interop |
| Profile SP-P3(d), SP-P4, SP-P5, §8 items 5–7 | not decided | decided by the ruling, as D1–D3, D6, D7, D9 state |
| `glade/contracts/signer-api/src/lib.rs:13` | `NodeId` is `sha256` of the key | the node key's public key; SI-002 unchanged |

## Evidence

- `cargo tree --offline` on a scratch copy of the node manifest and lock (path dependencies
  pointed at the checkout; nothing built; copy deleted): `=3.0.0-rc.0` changes no package;
  `=3.0.0` moves `curve25519-dalek` and `ed25519-dalek` to `5.0.0` and `3.0.0`, adds none, and
  resolves with `--locked --target all`; `"2"` adds ten packages and fails offline on
  `const-oid 0.9.6`.
- Searched glial, glade/client-ts, glade/demo, glade-chat, gryth-ui, glade/client-rs,
  grazel, glade-gyld and glade-gwz: no client reads or writes `home`. The 29 decode sites were
  counted in `node/src` by record type.
