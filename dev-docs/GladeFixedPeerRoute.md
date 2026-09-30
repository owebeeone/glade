# The fixed-peer route end to end (plan Step 4.6)

The design for plan Step 4.6 (`dev-docs/GladeFirstSlicePlan.md:1015-1036` in the workzone), 2026-09-28.
Nothing here is built. Code references are at glade `2a3c6f8`: a bare file name, `bin/glade-node.rs`
included, is in `glade/node/src/`; `tests/…` and `check.sh` are in `glade/node/`; `client-rs/…` and
`scripts/…` are at glade's root. "Plan `:N`" is a line of the plan.

The step's goal is the build entry's acceptance sentence, verbatim, as one script (plan `:26-33`):

> The final slice passes only when a real registration is discoverable across the selected peer route,
> expired/unauthorized entries are excluded, loss/retry/restart outcomes are honest, and the fast
> independent development path is demonstrated.

It is done when "one script runs the route journey against two configured nodes and exits 0; its log is the
evidence" (plan `:1025-1026`). It stands on 3.4's journeys (plan `:676`), the binding record, the door and the
iroh carrier (4.2, `:777-781`), the grant check (4.3, `:825`), durable acceptance (4.4, `:845`), the two
crossings (4.5, `:893`; 4.5b, `:918`; `GladeNodeAssembly.md:6450-6776`) and signed checkpoints (4.5c,
`:942-944`), whose ruling 9 leaves the lease to this step (`GladeDirectoryCheckpoints.md:787-792`, `:843`).

## Summary

- **Two configured nodes, and a third.** A registers an app, serves its workspaces and dials B; B reads for
  its clients, admits A and enforces client grants; C holds an endpoint key that nothing binds or configures.
  One script runs them in two placements: `local`, all three on one machine over loopback with relays off,
  which the Mac runs for every change; and `crossing`, A and C on the Pi and B on dabeest with `relay n0`, as
  4.5 and 4.5b ran, at the step's done. The desk (ports 5173, 8080, 9099; `~/.glade`) is never one of them.
- **The script** is `glade/scripts/route/route.py`, with its node control in `nodes.py` and a client, the
  probe, in `glade/client-rs/examples/route_probe.rs`. It runs 13 checks, each tied to a clause of the
  sentence, and exits 0 only when all pass. Its one log stamps every line each node, client and suite run
  printed, then gives a verdict per check.
- **Expiry in seconds, not minutes.** A new flag, `--lease-ms`, which the route's nodes pass as 12,000
  (renewed every 4 s). Absent, F1's five-minute lease renewed every 100 s stands. The longest wait in the
  journey is QUIC's idle timeout after a crash, about 35 s.
- **STA-P3-1: the origin in the retraction's scope, and no merged clock.** The binding fold keys by
  `(app, glade_id, origin)`. No registry's answers change; a served store stops letting one node's
  retraction take down another node's declaration.
- **SUR-P3-5:** no retract half in v1, as ruled. For a later format, one for `service` and none for
  `workspace`, whose lingering entry is what makes a retired share answer absent; the route shows it.
- **Also needed:** a forward that ends must tell its subscribers, where today it goes silent
  (`mesh.rs:777-779`); the node the journey restarts is the one that dials, since nothing redials; endpoint
  ids stay out of every log.
- **Size:** about 1,300 lines, 460-640 of them tests, in six parts of at most about 430, then the runs.

## 1. The two configured nodes (question 1)

**Three nodes, three roles.**

- **A registers and serves.** It loads `route-a.glade`: app `route`, bindings `route.notes` (`log`) and
  `route.extra` (`value`), workspaces `ws-route`, `ws-lapse` and `ws-closed`, and seeds granting B's node
  `read.subscribe` on the first two only (a grant names a node by its id, `grants.rs:18-21`). It serves the
  three workspaces at start (`bin/glade-node.rs:403-406`) and takes the writer client. It runs the assembled
  root (`GLADE_NODE_ASSEMBLED=1`), the only one that stops cleanly on SIGTERM; the hand-written root dies by
  the signal (`tests/stop_signal.rs:1-7`). So the journey can tell a stop from a crash.
- **B reads.** It loads `route-b.glade`: the same app `route` with the same two bindings, so two nodes load
  one app (section 3), and seeds granting principal `alice` `read.subscribe` on `ws-route`, `ws-lapse`,
  `ws-closed` and `ws-rogue`, and nothing for `mallory`. It runs the hand-written root the desk runs, with
  `--enforce-client-grants` (`bin/glade-node.rs:363-366`), and takes the reader clients. The journey never
  stops it.
- **C intrudes.** It loads `rogue.glade` (app `rogue`, workspace `ws-rogue`) and dials A. No binding record
  and no peer entry names its key.

**A dials; B admits.** A node dials each configured peer that has an address once, at its start
(`bin/glade-node.rs:397-402`, `mesh.rs:382-392`), and nothing redials: after 4.5 restarted the accepting Pi,
no link came back (`GladeNodeAssembly.md:6563`). A is the node the journey restarts, so A's file names B with
an address, and B's names A's key alone, which admits and dials nothing (`bin/glade-node.rs:52-53`). Each
start of A re-links.

**Two placements.** Recommend both (question 1). `local` puts all three nodes on one machine over loopback,
relays off: about 90 seconds, no network, no firewall prompt (loopback is the no-file default,
`netconf.rs:62-70`). The Mac runs it for every change. `crossing` puts A and C on the Pi and B on dabeest with
`relay n0`, the pairing 4.5 and 4.5b proved: two machines, n0's relay, whatever path iroh picks, and the
Windows node on which 4.5 found faults. It runs at the step's done and after any later change to the route.
No crossing node runs on the Mac.

| | `local` | `crossing` |
| --- | --- | --- |
| the script, its clients, 3.4's suite | this machine | the Mac |
| A, assembled root | `bind 127.0.0.1:<pa>` | the Pi: `relay n0`, `bind 10.1.1.236:4545` |
| B, hand-written, client grants enforced | `bind 127.0.0.1:<pb>` | dabeest: `relay n0`, `bind 10.1.1.239:4545` |
| C, hand-written | `bind 127.0.0.1:<pc>` | the Pi: relays off, `bind 10.1.1.236:4546` |
| A's peer line | `peer <B>@127.0.0.1:<pb>` | `peer <B>@<B's relay URL>` |
| B's peer line | `peer <A>` | `peer <A>` |
| C's peer line | `peer <A>@127.0.0.1:<pa>` | `peer <A>@10.1.1.236:4545` |
| a client reaches a node at | `ws://127.0.0.1:<port>` | an ssh forward from the Mac |

`<A>` and `<B>` are endpoint ids from `glade-node endpoint-id --name route-a|route-b`, the pattern of
`client-rs/tests/integration.rs:598-634`. Each network file is written at 0600, as `--config` requires
(`netconf.rs:1-7`), just before its node starts. In the crossing, A's file is written once B has printed
`relay <url>` (3.1-3.6 s after a start, `GladeNodeAssembly.md:6503-6505`, `:6722`); a dialer needs no home
relay of its own to use it (`:6627-6628`). A client needs a forward because each node's client port listens
on loopback alone (`bin/glade-node.rs:409`). The alternative crossing (question 1) is B on the Mac,
loopback-bound with `relay n0`: one remote host and no forward, every packet through n0 as in 4.5's run 1,
and no Windows node.

**The desk is never one of them.** Every node runs with `GLADE_HOME` and `HOME` inside the run's scratch
directory, under the name `route-a`, `route-b` or `route-c`. The script refuses a `GLADE_HOME` at or under
`~/.glade` and any port in 5173, 8080 or 9099, and chooses every port itself. It signals only a PID it
started, after checking that the PID's command line names the scratch binary, as 4.5 did
(`GladeNodeAssembly.md:6560`). It builds nothing: its caller passes built binaries, from a scratch target
that the same command deletes. In the crossing, TMPDIR (the Pi) and TMP and TEMP (dabeest) stay inside the
scratch, as ruled (plan `:895`), and both machines pull `--ff-only` to a tree pushed to GitHub. Pushing is
the owner's call.

## 2. The one script (question 2)

**Where it lives and what it starts.** `glade/scripts/route/`, beside `scripts/checks/`: `route.py`, the
journey and its checks; `nodes.py`, the placements (writing files, starting, signalling and stopping nodes
here or over ssh, and stamping their lines); `test_nodes.py` and `test_route.py`. Python 3.10 or later,
standard library only. One command runs it:

```
python3 scripts/route/route.py --placement local|crossing --node <glade-node> --probe <route_probe> --log <file>
```

It starts the three nodes, three clients (a writer at A, `alice` and `mallory` at B) and 3.4's suite. The
client is the probe, an example on `glade-client`, which has no node internals. It is a long-lived session
that says `hello` with its principal (`client-rs/src/client.rs:434`) and takes one command per line
(`subscribe`, `append`, `resend-last`, which sends the last op again byte for byte, `log`, `reconnect`,
`quit`). It prints one line per answer and one per event: ops as they arrive, `zone-refused <zone> <code>:
<message>` (`client-rs/src/client.rs:571`) and `dropped`. The script exits 0 when all 13 checks pass and 1 otherwise; a
setup step that fails marks the checks that need it SKIP, which also exits 1.

**The journey.** B starts first, since A's seeds need B's node id from its `node` line
(`bin/glade-node.rs:299`); then A, which dials B. The link is up once each prints `link <node> via …` and
`home round with node <node>` (`bin/glade-node.rs:59-65`). The writer appends `e1` and `e2` to
`ws-route/route.notes` at A. Then:

| Check | Clause | Step | Passes when |
| --- | --- | --- | --- |
| R1 | a real registration is discoverable | `alice` subscribes `ws-route/route.notes` at B, again after each second without data | her log is `[e1 e2]` within 5 s of A's `workspace ws-route serving`; only A's forward can bring them (`mesh.rs:296-298`) |
| U1 | unauthorized: a principal without a grant (4.3) | `mallory` subscribes the same at B | refused, `Unauthorized`, `unauthorized: principal mallory holds no grant of read.subscribe on ws-route` (`server.rs:393-404`, `grants.rs:170`); no op |
| U2 | unauthorized: a node without a grant (4.3, the route's own hop) | `alice` subscribes `ws-closed/route.notes` at B | acked, then `zone-refused`, `Unauthorized`, `refused by node <A>, which serves ws-closed: unauthorized: node <B> holds no grant …` (`mesh.rs:643-672`, `:780-786`); no op |
| U3 | unauthorized: an unbound key (4.2) | C starts and dials A | within 10 s A prints `peer refused: endpoint <C's tag>: unknown endpoint key` (`transport.rs:292`, `:386`); no `link` line names C's node; `alice` subscribing `ws-rogue` at B is acked with no op, the answer for a share B's directory never heard of (`mesh.rs:304`) |
| H1 | honest stop | SIGTERM to A | A exits 0 within `STOP_WITHIN`, 10 s (`lifecycle.rs:88`); within 2 s B prints `link <A> closed`, `alice`'s zone is told `forward from node <A> ended` (section 5), and a new subscribe is refused, `UnknownShare`, `claim holder <A> unreachable (no live peer link)` (`mesh.rs:300`) |
| H2 | honest restart, exact retry | A restarts on `route-a2.glade`, the same file without `binding route.extra` and `workspace ws-lapse`; the writer reconnects, resends `e2` and appends `e3`; `alice` subscribes again | A prints `app route registered (+1 record(s), …)`, the retraction (`appdecl.rs:801-806`), and re-links; the resend is `ok`; her log is `[e1 e2 e3]`, each once, within 5 s |
| E1 | expired entries excluded, route up | from the SIGTERM on, `alice` subscribes `ws-lapse` and `ws-route` at B every 500 ms | the first `no live ServeClaim for ws-lapse` (`mesh.rs:303`) comes after A's `link` line and between the stop plus 8 s and the stop plus 12.5 s, widened by the clocks' offset; `ws-route` is never refused so |
| H3 | honest loss | SIGKILL to A | every subscribe at B is answered within 2 s, acked from B's replica or refused with its reason, never hanging; B prints `link <A> closed` within 45 s, and `alice`'s zone is told within 1 s of that line |
| E2 | expired entries excluded, holder gone | the same polling of `ws-route` | the first `no live ServeClaim for ws-route` falls between the kill plus 8 s and plus 12.5 s, widened likewise |
| H4 | honest restart after a crash, retry | A starts again on `route-a2.glade`; the writer reconnects, resends `e3` and appends `e4`; `alice` subscribes again | A boots, since a crash leaves no lock (`sysdir.rs:109-116`); the resend is `ok`; her log is `[e1 e2 e3 e4]`, each once |
| F1 | the fast path, warm | 3.4's suite five times after one untimed build | every run passes (43 tests at `2a3c6f8`); the fastest takes at most 1.0 s |
| F2 | the fast path, one file touched | three runs, each after touching `tests/journeys/leases.rs` (its mtime alone, restored after) | the fastest takes at most 3.0 s |
| T1 | the run's own hygiene | teardown | every node stopped by the script; no process left whose command line holds the scratch path; the bound ports free; no endpoint id in any log; the scratch deleted |

4.4's journeys over the real carrier: `publish` is R1; `exact_retry` is the resends in H2 and H4;
`restart_mid_round` and `lost_acknowledgement` are H3 with H4, where B has taken A's ops, A dies and returns,
and each op is held once (`tests/journeys/restart.rs:71`, `tests/journeys/delivery.rs:83`).
`retry_after_a_failed_save` (`tests/journeys/restart.rs:142`) needs an injected disk fault, so it stays in
process and in `tests/durable` (named gaps).

**Expiry without waiting five minutes.** Each route node passes `--lease-ms 12000` (section 5): a claim lives
12 s and is renewed every 4 s (`claims.rs:200-208`, `:342-361`). A's last claim before a stop was minted
within 4 s of it, so the claim expires between 8 and 12 s after the stop, judged at the reader's clock
(`mesh.rs:1249-1266`). E1 and E2 hold that window, with the half-second poll added, widened in the crossing
by the offsets the script measures before and after the run, as 4.5 did (74-90 ms,
`GladeNodeAssembly.md:6490-6492`, `:6701-6702`). E1 sees a lapse after A has re-linked, which tells expiry
from unreachability; E2 sees one after a crash, whether or not B has noticed the crash yet. That notice,
QUIC's idle timeout (33.6 and 35.3 s on the relay path, `GladeNodeAssembly.md:6540-6543`, `:6736`), is the
journey's longest wait, and H3 bounds it at 45 s.

**The log.** One file at `--log`, outside the scratch. A header gives the date, the placement and hosts, the
glade revision on each host and whether its tree is clean, each binary's SHA-256 (12 digits), the leases,
the roots, the ports, the three endpoint tags (10 digits, as the nodes print them) and, in the crossing, the
clock offsets. Then come every line each node printed on stdout and stderr, each client's commands, answers
and events, and each suite run's command with its wall and CPU time, stamped `<seconds since the start, to
the ms> <source> <line>` on the script's clock. Node ids may appear, as in 4.5's logs; endpoint ids may not
(`bin/glade-node.rs:54-55`): they live in 0600 files in the scratch, and T1 searches every log for each.
Last come one `CHECK <id> PASS|FAIL|SKIP <clause>: <evidence>` line per check and a verdict worded as the
gate words its own (`check.sh:758-761`): `ROUTE: PASS -- all 13 checks passed`, or `ROUTE: FAIL -- <n> of 13
checks failed: <ids>`.

**The fast path.** F1 and F2 run 3.4's fast loop, `cargo test --offline --locked --manifest-path
node/Cargo.toml --test journeys --test assembly`, on the script's machine, in the checkout the script lives
in. Its budget is 1.0 s warm and 3.0 s after touching one journey file (plan `:676`). The fastest run is
judged, since wall time on the Mac follows its load (plan `:676`); each run's CPU time, from `getrusage` for
the children, is logged beside it, for comparing trees as 3.4 did. The suite's fakes are what make the path
independent: a fake network, clock and engine, with no file, socket, runtime or sleep
(`tests/journeys/main.rs:12-15`).

## 3. STA-P3-1: one node's retraction, another node's declaration (question 3)

**The issue.** The binding family, `dir.bindings` with `dir.binding-retractions`, folds per
`(app, glade_id)`, the highest `(lamport, origin, retraction, seq)` winning (`registry.rs:867-883`,
`:910-944`). A registry holds one node's records, since a sealed one appends under its own origin alone
(`:647-656`), and one clock numbers the family there (`:664-667`, `:687-700`), so its highest is its newest.
A served store holds every node's records, each numbered on its own node's clock, and a retraction's key
names no origin. So one node's retraction takes down another node's live declaration whenever its clock runs
ahead (`:891-902`; pinned, not endorsed, at `:1478-1511`). The declaring node never repairs it: `register`
diffs its file against its own registry (`appdecl.rs:787-800`), where the declaration is live.

**Who sees it today.** In the node, only `declared_exchange` reads a served store's binding family
(`exchange.rs:65-85`), and only for `exchange`-shaped bindings, which a v1 file cannot declare
(`appdecl.rs:399-404`, `:543-555`). On a v1 route the defect reaches only a reader that folds `home`'s two
binding streams itself by the documented rule (`registry.rs:867-869` cites glade-gyld's README). It must still
close before two nodes load one app, which the route's first run does: the rule is the directory's, and a
fold that lets any bound node withdraw another node's declaration is wrong for every reader.

**Recommend the origin in the retraction's scope, alone.** The fold keys by `(app, glade_id, origin)`, the
origin being the op's, which the record's seal proves (plan Step 4.1b; `registry.rs:647-651`). Each node's
declaration then stands or falls by that node's own records, ordered on its own clock, where highest already
means newest, and no node can withdraw another's. `live()` still chooses one declaration per glade id across
origins by the same stamp, which matters only when two nodes declare one glade id differently (named gaps).
Every registry answers as before, since it holds one origin, so `register`'s diff and `bindings_of` do not
move. No record, format, `PROTOCOL` (4) or ALPN changes.

**Why not a merged clock.** It is not enough. Numbering a node's binding records above the highest its
served store holds at boot orders a later record over an earlier one, but A's retraction, numbered above
everything A held, still outranks B's older declaration, which B's file still makes. That is the pinned
test's first half, and B never declares again. With the origin in scope it is not needed: across origins,
order only picks between two live declarations, where "whoever booted last" is no truer than a fixed stamp.
And it costs: the hand-written root registers apps (`bin/glade-node.rs:326-338`) before it opens the served
store (`:359`), so the boot would reorder, and a node's clock would follow its peers' records, which a faulty
peer could inflate. "Both" pays those costs to settle only the conflict, which wants a warning, not a winner.

## 4. SUR-P3-5: `service` and `workspace` lines (question 4)

`service` and `workspace` lines register as ordinary records, diffed by their bytes, and a removed line
appends nothing (`appdecl.rs:808-840`); only `binding` lines have a retract half (R9(a)). The route shows the
`workspace` case. H2 drops `workspace ws-lapse`, its `WorkspaceEntry` stays in every store, and once its
claim lapses B answers `no live ServeClaim for ws-lapse` (E1): the directory knows a share with no live host
(`mesh.rs:1271-1289`, `:303`). A lingering `ServiceDefinition` keeps its glade id a declared exchange at
every node that holds it (`exchange.rs:69-77`), so a subscribe there attaches a provider instead of reading
(`server.rs:361-370`), for good.

**Recommend** v1 as ruled, with no format change in 4.6. For a later format: a retract half for `service`,
scoped to its origin as section 3 scopes bindings, since a stale service changes routing and nothing else
can withdraw it; and none for `workspace`, whose entry is the history that lets a retired share answer absent
with its reason, where a retraction would make the share unknown and so served locally as an empty zone
(`mesh.rs:304`). Revisit `workspace` when failover needs a share's eligible hosts to change. Alternatives:
both, R9(s) as first proposed; or neither, with liveness from claims alone and exchange surfaces read from
live bindings only.

## 5. What else the route needs (question 5)

1. **A forward's end is told** (part 3). A forward that ends without a refusal leaves the forwarded set and
   tells no one (`mesh.rs:774-779`). A reader at B subscribed through A's forward stays subscribed, across
   A's stop, crash and restart, to a zone nothing feeds, and learns nothing unless it subscribes again. That
   fails "honest". Change: at such an end, each local subscriber of the zone gets one Error, `UnknownShare`,
   `forward from node <A> ended`, and leaves the zone through `refuse_subscription` (`server.rs:230-245`), as
   a refused forward's subscribers already do (`mesh.rs:780-786`); a later subscribe routes afresh
   (`:765-767`). The code is the one an absent route answers with (`server.rs:377`), and clients already take
   a refusal after an ack (F13, `client-rs/tests/integration.rs:644-700`). A node without links never
   forwards, so the desk sees nothing.
2. **`--lease-ms`** (part 2). The entry point takes `--lease-ms <n>` out of the arguments before either root
   sees them, since both read an unknown word as positional (`bin/glade-node.rs:277`, `assembly.rs:302`):
   today `--lease-ms 12000` would make `12000` the app-data store directory. It accepts 3,000 to 3,600,000
   ms, renews at a third, the default's rule (`claims.rs:58-60`), and leaves `checkpoint_after` alone, ruled
   a setting with no flag (`GladeDirectoryCheckpoints.md:805-807`, `:839-840`). Any other value refuses the
   start before anything is written, as a bad `--peer` does (`netconf.rs:6-7`). Given, each root prints
   `leases <n> ms, renewed every <n/3> ms` after `node`; absent, the leases are `Leases::default()`
   (`bin/glade-node.rs:225`) and nothing new is printed. The legacy form ignores it, as it ignores
   `--config` and `--peer` (`assembly.rs:272-274`).
3. **Restart by the dialer**, configuration only (section 1): no redial is built.
4. **Endpoint ids out of the logs**, the script's own rule, checked by T1 (section 2).
5. **The probe** (part 4). No client tool exists: client-rs's two-node test
   (`client-rs/tests/integration.rs:644-700`) shows the pattern, but it is a test, not a tool.

## 6. Tests, each begun red

**Part 1, STA-P3-1.**

- The pinned test (`registry.rs:1478-1511`), turned round as
  `across_two_origins_a_retraction_takes_down_only_its_own_origins_declaration`: folded together, B's `g`
  stays live, and B's later `log` declaration is the live one. Red against today's fold, which answers `h`
  alone.
- New in `registry.rs`: a retraction from an origin that never declared `(app, g)` retracts nothing. Red:
  today it takes B's declaration down whenever its lamport is higher.
- New in `exchange.rs`: `declared_exchange` over a store holding B's `exchange`-shaped binding, appended
  through `Registry` since no v1 file can write one, and A's retraction of it, answers true. Red today.
- Guards, green before and after: the one-origin fold tests (`registry.rs:1405-1476`, `:1515-1533`) and
  `appdecl.rs`'s diff tests.

**Part 2, `--lease-ms`.**

- `claims.rs`: the flag's value to `Leases`. 12000 gives 12,000, 4,000 and 1,000 (`checkpoint_after`);
  3,000 and 3,600,000 are accepted; 2,999, 3,600,001, `12s`, `-1` and an empty value are refused, naming the
  range. Red: no such function.
- `tests/start_refusals.rs`: `--lease-ms 2999` exits 1, prints the refusal and writes nothing under
  `GLADE_HOME`; `--lease-ms 12000` starts, prints the `leases` line and makes no `12000` directory. Red
  today: the node starts, with `12000` as its store directory.
- Guards: `claims.rs:837-838` (the defaults), and every test that passes no flag.

**Part 3, a forward's end.**

- `mesh.rs`, on its two-node harness (`mesh.rs:1295`): a client of B subscribed through B's forward to a
  share A serves; A's links are released (`release_links`, `:796`). The client gets one Error,
  `UnknownShare`, `forward from node <A> ended`, and B's router no longer holds it in the zone. Red: nothing
  arrives within the test's 5 s bound.
- Guards: a forward that ends because its last local subscriber left sends nothing; a subscribe after the
  told end forwards again once A is linked.

**Part 4, the probe.** A client-rs integration test drives it against one spawned node with
`--enforce-client-grants` and one seed: `subscribe` accepted and refused, `append` and `resend-last` both
`ok`, `log` in order. Red: no probe.

**Part 5, the harness.** `test_nodes.py`: the stamp format; each placement's network files, at 0600; the
port chooser never returns 5173, 8080 or 9099; a `GLADE_HOME` at or under `~/.glade` is refused; a PID whose
command line does not name the scratch binary is never signalled. Red: no module.

**Part 6, the journey.** `test_route.py` runs each of the 13 checks against a passing fixture of stamped
lines and at least one failing fixture: R1 with `e2` missing; U1 with `mallory` acked; U2 fed; U3 with a
`link` line between A and C, or with no refusal line; H1 with the zone told after 2 s, which is today's
behaviour; H2 and H4 with an entry twice; E1 lapsing before its window, or `ws-route` lapsing; E2 after its
window; H3 with the link closing after 45 s; F1 and F2 over budget; T1 with an endpoint id in a log. Red: no
checks. End to end, the local run on today's tree fails: E1 and E2, since a node given `--lease-ms 12000`
today takes the default lease and a stray store directory, and H1 and H3, since no subscriber is told.
Parts 2 and 3 turn those green, and after part 6 the local run exits 0.

## 7. The gate and a desk replay

- **Parts 1-3** change `glade/node`. Each runs `check.sh` to 9 of 9 components (`check.sh:687-695`), with
  the test count on both paths and no baseline raised, and the downstream suites against the rebuilt binary
  at 4.5c's counts (client-rs 32+12+1, client-ts 60, grip-share 19, grazel 30+3+5+1, glade-gwz 10+1+10+1,
  glade-gyld 251 (1 ignored)+1+1+41+1; plan `:944`). Part 4 runs client-rs's suites; parts 5 and 6 run
  their own tests and a local route run.
- **The desk replay**, after parts 1-3: the desk is restarted from the owner's terminal on the rebuilt
  binary, and its start lines and Streams census are compared with 4.5c part 3's. Expected unchanged: grazel
  passes no `--lease-ms`, the desk's stores hold one origin, and it has no link to forward over.
- **Not a gate component** (question 6). The route runs at this step's done, and again after any later
  step that changes the mesh, the carrier, the door, claims, the binding fold or subscribe routing.
- **Part 7, the runs.** The local placement on the Mac, then the crossing, each log kept, recorded here as
  4.5's crossings were recorded ("The route, <date>").

### The route, 2026-09-30

Run by an agent for the lane owner, on glade `ad0855c` (parts 1-6), as section 1 and "Part 7, the runs" lay out:
the local placement on the Mac, then the crossing, A and C on the Pi and B on dabeest with `relay n0`. Times are
the script's stamps, seconds since the run began, on the Mac's clock. `<A>` and `<B>` stand for A's and B's node
ids, and `<C's tag>` for C's endpoint tag, which the logs carry in full. The logs stay in the Mac's scratchpad:
`route-local.log`, `route-crossing.log` and `route-local-changed.log`, with the build logs `build-local.log`,
`build-crossing.log`, `build-local-changed.log`, `build-pi.log` and `build-dabeest.log`.

**The machines**

| | the Mac | the Pi | dabeest |
| --- | --- | --- | --- |
| system | macOS (Darwin 25.6), Rust 1.96.0 | Raspberry Pi 5, Debian 13 aarch64, 4 cores, Rust 1.96.0 | Windows 11, MSYS bash (runtime 3.6.9), 24 cores, Rust 1.98.1 (MSVC) |
| glade | `ad0855c`: clean for the local run; for the crossing and the local re-run, with the harness change below (4 paths) | `8bed929` to `ad0855c` by `pull --ff-only`; clean | `8bed929` to `ad0855c` by `pull --ff-only`; clean |
| runs | local: A, B and C, the script, its clients and 3.4's suite; crossing: the script, its clients and the suite, no node | the crossing's A and C | the crossing's B |
| build, empty target | `glade-node` 33 s, then the probe 4 s, into one target; for the crossing, the probe alone, 6 s; for the re-run, both, 38 s | `--offline --locked`: 191.3 s | `--offline --locked`: 56.7 s |
| before | the desk running, untouched; 3.8 GiB free | load 0.00 before the build, 0.04 before the run; no `glade-node`; UDP 4545 and 4546 free; `wlan0` read; 4.5 GB free before the build | 7% CPU, no build or other heavy job (the idle ollama service aside); no `glade-node.exe`; UDP 4545 and 4546 free; Wi-Fi read by `ipconfig` |

- **Binaries,** SHA-256 to 12 digits: the local run's `glade-node` `55d28448a167` and probe `4c8aa471bd94`; the
  crossing's probe `edf1c96a6350`, the Pi's `glade-node` `d97010676976` and dabeest's `13b4ef13ef6e`.
- **Builds.** Each went to a scratch target outside the checkout, the Pi's outside `~/git`, with the temporary
  directory inside that scratch. The Pi's and dabeest's targets (2.1 and 2.6 GB) were deleted once the binary was
  copied out, and the copies after the run. The Mac's targets also took the fast loop's build, and each was
  deleted with its copies in the command that ran its placement (2.1 GB locally, 1.8 GB for the crossing). The
  Pi's non-interactive shell has no `cargo` on its `PATH` (4.5's adaptation 1): the first attempt stopped at
  `cargo: command not found`, and the build ran with `PATH=$HOME/.cargo/bin:$PATH`. `--offline --locked` held on
  both hosts.
- **ssh to dabeest.** The owner's configuration gives dabeest, by name and by address, LocalForwards on 11434 and
  1919, which another session holds, and `ClearAllForwardings=yes` would also drop the harness's `-L`. The
  crossing used `ssh -F /dev/null -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=10` with the user and host
  that `ssh -G dabeest` resolves. With `-o HostKeyAlias=dabeest` host-key verification failed, since `known_hosts`
  holds dabeest's keys under its address; without it, a test connection bound its own `-L` port and nothing else,
  and 11434 and 1919 stayed with the other session. The Pi's was `ssh -o BatchMode=yes -o ConnectTimeout=10`.

**The harness change, for Windows.** Part 5 left three Windows details untested. Before the crossing, a preflight
on dabeest started a native program the harness's way (`echo pid $$`, then `exec env -i SYSTEMROOT="$SYSTEMROOT"
…`), read its command line and ended it, then did the same with the scratch `glade-node.exe`, loopback only:

- `SYSTEMROOT` under `env -i` suffices: the node started and listened.
- `taskkill //F //PID "$(cat /proc/<pid>/winpid)"` ends it, and its ssh exits 1.
- `/proc/<pid>/cmdline` of a native program reads as Windows holds it: the program as `cygpath -w` spells it
  (`E:\…\bin\glade-node.exe`) and each path argument as `cygpath -m` does (`E:/…`). So `may_signal` refused,
  the binary the harness started being `/e/…/bin/glade-node.exe`: teardown could not have stopped B, and T1
  would have failed with B left running. Teardown's scan, which looks for `/e/…`, could not see a native node
  holding the scratch.

The change is in the Mac's working tree, uncommitted. `Host.spellings(path)` in `nodes.py` gives the path as given
and, on msys, its `cygpath -w` and `-m` forms; `Nodes.signal` signals once the command line names any spelling of
the binary it started, and `teardown` in `route.py` looks for any spelling of the scratch. Off msys there is one
spelling and no call. Two tests came first, red before the change and green after:
`test_on_msys_a_node_is_known_by_its_command_line_as_windows_spells_the_binary` in `test_nodes.py` and
`test_teardown_finds_a_native_process_holding_the_scratch_as_windows_spells_it` in `test_route.py`, with `cygpath`
and `taskkill` faked on the local shell's `PATH`. `test_nodes.py` passes 8 of 8 and `test_route.py` 7 of 7. A second
preflight, the node started by `Nodes.spawn` and stopped by `Nodes.stop`, ended it (`exit 1`) and left nothing. No
check, budget, node or client code changed, and every node ran `ad0855c`.

**The local placement**, 23:23, on the committed tree (`route-local.log`, 1,124 lines): all three nodes on
loopback, relays off. A's first life lasted 1.59 s, R1 to U3 taking under a second of it; B saw the crash
33.8 s after the SIGKILL, and the run ended at 68.0 s. The suite's untimed build took 8.9 s, the node's build
having made its dependencies.

```
CHECK R1 PASS a real registration is discoverable: alice's log at B [e1 e2] +0.438 s after A's workspace ws-route serving, 1 subscribe(s)
CHECK U1 PASS unauthorized: a principal without a grant: mallory at B: refused ws-route/route.notes Unauthorized: unauthorized: principal mallory holds no grant of read.subscribe on ws-route
CHECK U2 PASS unauthorized: a node without a grant: alice's subscribe of ws-closed at B: acked ws-closed/route.notes []; zone-refused ws-closed/route.notes Unauthorized: refused by node <A>, which serves ws-closed: unauthorized: node <B> holds no grant of read.subscribe on ws-closed
CHECK U3 PASS unauthorized: an unbound key: A refused C's endpoint <C's tag> +0.041 s after C's start; 0 link line(s) between A and C; alice at B: acked ws-rogue/route.notes []
CHECK H1 PASS honest stop: A exit 0 +0.015 s after SIGTERM; B's link to A closed +0.011 s, alice told +0.012 s, a subscribe refused as unreachable +0.012 s (each within 2 s)
CHECK H2 PASS honest restart, exact retry: A: app route registered (+1 record(s), 5 unchanged), re-linked +0.039 s after its start; resend: ok ws-route/route.notes writer:1 e2; alice's log at B [e1 e2 e3] +0.009 s after A's workspace ws-route serving, 1 subscribe(s)
CHECK E1 PASS expired entries excluded, route up: ws-lapse lapsed +10.588 s (window +8.000-12.500 s) after SIGTERM, A re-linked +0.091 s; ws-route did not lapse before SIGKILL
CHECK H3 PASS honest loss: B's link to A closed +33.822 s after SIGKILL (budget 45 s), alice told +0.001 s after it; 134 of 134 subscribes at B answered within 2 s
CHECK E2 PASS expired entries excluded, holder gone: ws-route lapsed +9.548 s (window +8.000-12.500 s) after SIGKILL
CHECK H4 PASS honest restart after a crash, retry: A: app route registered (+0 record(s), 5 unchanged), re-linked +0.041 s after its start; resend: ok ws-route/route.notes writer:2 e3; alice's log at B [e1 e2 e3 e4] +0.011 s after A's workspace ws-route serving, 1 subscribe(s)
CHECK F1 PASS the fast path, warm: 5 of 5 warm runs passed, 43 tests; the fastest 0.148 s (cpu 0.154 s), budget 1.0 s
CHECK F2 PASS the fast path, one file touched: 3 of 3 touched runs passed, 43 tests; the fastest 1.687 s (cpu 1.784 s), budget 3.0 s
CHECK T1 PASS the run's own hygiene: 5 node processes, each ended after the script signalled it; no process holds the scratch, its ports are free, it is deleted, and no endpoint id is in the log
ROUTE: PASS -- all 13 checks passed
```

The same placement on the changed harness, after the crossing (`route-local-changed.log`, 1,128 lines): `ROUTE:
PASS -- all 13 checks passed`, with R1 +0.433 s, H1's exit +0.014 s, E1 +10.584 s, H3 +33.835 s, E2 +9.551 s, F1
0.168 s and F2 2.111 s.

**The crossing**, 23:37 (`route-crossing.log`, 1,385 lines). The clocks, each read from the Mac over a new ssh
connection: the Pi +0.110 s (±0.105) and dabeest +0.063 s (±0.135) before the run, +0.123 s (±0.118) and +0.061 s
(±0.135) after. E1's and E2's windows were widened by the larger skew, 0.315 s, to +7.685-12.815 s. Every start
took the same n0 relay, B's 3.63 s in and each of A's three 3.09 s in.

| s | the Pi | dabeest | the clients, on the Mac |
| --- | --- | --- | --- |
| 5.18 | | B starts, hand-written root, client grants enforced | |
| 8.80 | | `relay …` | |
| 9.48 | A starts, assembled root, and dials B at that relay URL | | |
| 10.79 | | `link <A> via relay …, rtt 348 ms` | |
| 10.99 | `link <B> via direct …, rtt 22 ms`, 1.42 s after A's `peer` line | | |
| 11.00-11.05 | home round, 11 records in 15 ms; ws-route, ws-lapse and ws-closed served | home round, 11 records in 207 ms; `via direct`, rtt 5 ms; two gaps in A's `dir.claims` healed | |
| 11.54-11.59 | | | the writer appends `e1` and `e2` at A; alice reads `[e1 e2]` at B (R1); mallory is refused (U1); alice's ws-closed is acked, then refused (U2) |
| 12.28-12.37 | C starts, relays off, and dials A: A prints `peer refused: endpoint <C's tag>: unknown endpoint key`, C `the link ended before a WELCOME` | | alice's ws-rogue is acked `[]` (U3) |
| 13.61 | SIGTERM to A | | |
| 13.83-13.85 | | `link <A> closed` | the writer's session `dropped`; alice told; `poll` refused, `claim holder <A> unreachable` |
| 14.20 | A `exit 0` | | |
| 14.85-16.35 | A starts on `route-a2.glade` (`+1 record(s), 5 unchanged`) and re-links, `via direct` | `via relay`, then `via direct` | |
| 16.39-16.43 | | | the writer reconnects, resends `e2` (`ok … writer:1 e2`) and appends `e3`; alice reads `[e1 e2 e3]` |
| 25.99 | | | `poll`: ws-lapse `no live ServeClaim` (acked at 25.49) |
| 26.23 | SIGKILL to A; its ssh `exit 255` at 26.45 | | the writer's session `dropped` |
| 35.13 | | | `poll`: ws-route `no live ServeClaim` (acked at 34.61) |
| 44.54 | | `link <A> via relay …, rtt 315 ms` | |
| 59.28 | | `link <A> closed` | alice told, 1 ms later |
| 59.96-61.53 | A starts again on `route-a2.glade` (`+0 record(s), 5 unchanged`), re-links `via direct`, and pulls 9 of B's `dir.claims` records after a gap | `via relay`, then `via direct`; one gap of A's healed | |
| 61.51-61.55 | | | the writer reconnects, resends `e3` (`ok … writer:2 e3`) and appends `e4`; alice reads `[e1 e2 e3 e4]` |
| 61.6-102.9 | | | 3.4's suite: its build 33.7 s, untimed, then five warm runs and three touched |
| 103.56 | | SIGTERM, a forced end: `exit 1` 0.38 s later | |
| 104.17-112.61 | C: SIGTERM, `exit 255` 0.21 s later. A: SIGTERM at 104.63, `exit 0` 7.98 s later | | |

```
CHECK R1 PASS a real registration is discoverable: alice's log at B [e1 e2] +0.549 s after A's workspace ws-route serving, 1 subscribe(s)
CHECK U1 PASS unauthorized: a principal without a grant: mallory at B: refused ws-route/route.notes Unauthorized: unauthorized: principal mallory holds no grant of read.subscribe on ws-route
CHECK U2 PASS unauthorized: a node without a grant: alice's subscribe of ws-closed at B: acked ws-closed/route.notes []; zone-refused ws-closed/route.notes Unauthorized: refused by node <A>, which serves ws-closed: unauthorized: node <B> holds no grant of read.subscribe on ws-closed
CHECK U3 PASS unauthorized: an unbound key: A refused C's endpoint <C's tag> +0.084 s after C's start; 0 link line(s) between A and C; alice at B: acked ws-rogue/route.notes []
CHECK H1 PASS honest stop: A exit 0 +0.586 s after SIGTERM; B's link to A closed +0.228 s, alice told +0.229 s, a subscribe refused as unreachable +0.239 s (each within 2 s)
CHECK H2 PASS honest restart, exact retry: A: app route registered (+1 record(s), 5 unchanged), re-linked +1.509 s after its start; resend: ok ws-route/route.notes writer:1 e2; alice's log at B [e1 e2 e3] +0.058 s after A's workspace ws-route serving, 1 subscribe(s)
CHECK E1 PASS expired entries excluded, route up: ws-lapse lapsed +12.380 s (window +7.685-12.815 s) after SIGTERM, A re-linked +2.742 s; ws-route did not lapse before SIGKILL
CHECK H3 PASS honest loss: B's link to A closed +33.056 s after SIGKILL (budget 45 s), alice told +0.001 s after it; 130 of 130 subscribes at B answered within 2 s
CHECK E2 PASS expired entries excluded, holder gone: ws-route lapsed +8.901 s (window +7.685-12.815 s) after SIGKILL
CHECK H4 PASS honest restart after a crash, retry: A: app route registered (+0 record(s), 5 unchanged), re-linked +1.497 s after its start; resend: ok ws-route/route.notes writer:2 e3; alice's log at B [e1 e2 e3 e4] +0.052 s after A's workspace ws-route serving, 1 subscribe(s)
CHECK F1 PASS the fast path, warm: 5 of 5 warm runs passed, 43 tests; the fastest 0.149 s (cpu 0.156 s), budget 1.0 s
CHECK F2 PASS the fast path, one file touched: 3 of 3 touched runs passed, 43 tests; the fastest 1.744 s (cpu 1.741 s), budget 3.0 s
CHECK T1 PASS the run's own hygiene: 5 node processes, each ended after the script signalled it; no process holds the scratch, its ports are free, it is deleted, and no endpoint id is in the log
ROUTE: PASS -- all 13 checks passed
```

**The timings that matter**

| | local | crossing | bound |
| --- | --- | --- | --- |
| R1: alice's `[e1 e2]` at B after A's `workspace ws-route serving` | +0.438 s | +0.549 s | 5 s |
| H1: A's `exit 0` after its SIGTERM | +0.015 s | +0.586 s | 10 s |
| H1: B's `link <A> closed`, alice told, a subscribe refused as unreachable | +0.011, 0.012, 0.012 s | +0.228, 0.229, 0.239 s | 2 s each |
| H2 and H4: A re-linked after its start | +0.039 and 0.041 s | +1.509 and 1.497 s | none (the script waits 60 s) |
| E1: ws-lapse's first `no live ServeClaim` after the SIGTERM | +10.588 s | +12.380 s | +8.000-12.500 s; crossing +7.685-12.815 s |
| H3: B's `link <A> closed` after the SIGKILL; subscribes at B answered within 2 s | +33.822 s; 134 of 134 | +33.056 s; 130 of 130 | 45 s; every one |
| E2: ws-route's first `no live ServeClaim` after the SIGKILL | +9.548 s | +8.901 s | as E1 |
| F1: the fastest of five warm runs, 43 tests | 0.148 s (cpu 0.154) | 0.149 s (cpu 0.156) | 1.0 s |
| F2: the fastest of three touched runs | 1.687 s (cpu 1.784) | 1.744 s (cpu 1.741) | 3.0 s |

**Teardown.** T1 passed in both placements, in the crossing with the change above, so that dabeest was also
scanned for the scratch as Windows spells it. Then, by hand:

- **The Pi:** no process under its scratch (an anchored `pgrep`), no `glade-node`, UDP 4545 and 4546 and TCP 4555
  and 4556 free, and the run's `glade-route-*` directory gone.
- **dabeest:** `tasklist` found no `glade-node.exe`; UDP 4545 and 4546 and TCP 4555 free; the run's directory gone.
- Each build log was copied to the Mac, identical by `md5`; then `rm -rf` of each host's scratch, 255 MB on the Pi
  and 31 MB on dabeest, a binary and a build log each. Nothing of the run's is left in either machine's temporary
  directory. Both checkouts are clean at `ad0855c`, and dabeest's `scratch/` holds what it held before.
- **The Mac:** no `glade-node` or `route_probe` from a scratch path, the desk's node untouched, 3.8 GiB free again.
- **No endpoint id in any log:** T1 for each route log; by hand, the only 64-digit hex strings in each are its
  three node ids, and the build logs hold none.

**Seen, and no check covers it**

- **A's last stop was slow:** 7.98 s at the crossing's teardown and 3.11 s at the local one, against H1's 0.586 and
  0.015 s. Teardown stops the processes in the order they started, so B has gone (by force on dabeest, by the
  signal locally) and A drains toward a peer that no longer answers. That is within `STOP_WITHIN`, but nothing
  times it: past 10 s `stop` would send SIGKILL, and T1 would still pass.
- **Gaps healed on `dir.claims`.** At the crossing's first link B refused two of A's claim records that arrived
  ahead of an earlier one (`a gap: expected seq 2, got 4`, then `got 3`), then pulled 2 records 5 ms later and
  reported the gaps healed. After the crash A's store held B's records to seq 6; B's live 15 came first (`expected
  seq 7, got 15`), and A pulled 9 within 17 ms; B then healed one gap of A's. The local run shows the first heal
  too, of one record. Each healed at once, and no check reads these lines.
- **Paths.** A dialled B at B's relay URL, yet at each of its three starts A's first `link` line read `via direct`
  (rtt 22, 5 and 6 ms), 1.50-1.51 s after it began, while B's read `via relay` (rtt 345-357 ms), then `via direct`
  0.26 s later, the shape of the 2026-09-28 crossing's run 2. After the SIGKILL, B's path fell back to the relay
  18.3 s in, and the link closed 33.1 s in (4.5's relay-path closes took 33.6 and 35.3 s).
- **E1 near its window's top.** The crossing's +12.380 s is 0.435 s inside +12.815 s. The polls either side,
  +11.873 s (acked) and +12.380 s (refused), put A's last renewal of ws-lapse within about 0.13 s of the SIGTERM,
  the case the window's top allows for: the 12 s lease, the poll's 0.5 s and the skew. E2's +8.901 s fits a
  renewal 3.4 s before the kill.
- **The clocks' reads.** Each offset was read over a new ssh connection, so half its round trip, 0.105-0.135 s,
  is most of the 0.315 s widening; 4.5's reads over one held session were good to 5-10 ms.
- **The first warm run** took 1.70-1.91 s in all three runs and the other four 0.148-0.195 s; F1 judges the
  fastest, as designed.
- **dabeest's lines** still mix separators in the `instance` line (`E:/…/route-b/glade\sys\route-b`) and quote the
  program as `'E:\…\glade-node.exe'` in the recovery warning, as the 2026-09-28 crossing recorded.
- **Exit codes over ssh are ssh's:** A's clean stops `exit 0`, its SIGKILL `exit 255`, C's SIGTERM `exit 255` (the
  hand-written root dies by the signal) and B's forced end `exit 1`; locally 0, -9, -15 and -15. T1 needs only
  that each ended after a signal.

## 8. Size and the split

| Part | What | Files | Production | Tests |
| --- | --- | --- | --- | --- |
| 1 | STA-P3-1: the origin-scoped binding fold | `registry.rs`, `exchange.rs` | 30-50 | 90-120 |
| 2 | `--lease-ms` and its `leases` line | `bin/glade-node.rs`, `claims.rs`, `lifecycle.rs`, `tests/start_refusals.rs` | 40-60 | 70-100 |
| 3 | a forward's end told | `mesh.rs` | 15-30 | 60-90 |
| 4 | the probe | `client-rs/examples/route_probe.rs`, `client-rs/tests/` | 180-230 | 50-70 |
| 5 | placements, processes and the stamped log | `scripts/route/nodes.py`, `test_nodes.py` | 230-280 | 60-90 |
| 6 | the journey and its 13 checks | `scripts/route/route.py`, `test_route.py` | 200-260 | 130-170 |
| 7 | the runs, recorded in this note | this note | — | — |

About 700-900 production lines and 460-640 of tests; no part passes about 430. Part 1 goes first: STA-P3-1
closes before two nodes load one app, which the route's first run does. Parts 2 and 3 touch other files and
can run in any order or in parallel; parts 4 and 5 touch no node code and can run beside parts 1-3; part 6
needs parts 2-5, and part 7 needs all six. Each part is gated as section 7 says. Merging parts 2 and 3 gives
about 190-280 lines; merging parts 5 and 6 would pass 500.

## Named gaps

1. **Nothing redials.** A link lost while both nodes stay up is not restored until the dialer restarts
   (`bin/glade-node.rs:397-402`, `mesh.rs:382-392`). The route restarts only its dialer.
2. **A crash is noticed only at QUIC's idle timeout.** Until then B forwards into a dead link and answers
   from its replica (H3). Ending a forward when its holder's claim lapses is not attempted.
3. **No freshness.** The node's lookup cannot report partiality (slice profile §8 item 11, plan `:676`), so
   B's answers between a crash and the lapse are true but possibly stale.
4. **A subscribe that races the registration.** One that reaches B before A's registration does is served
   locally, as a share the directory never heard of (`mesh.rs:304`), and is not moved to the forward when the
   registration lands. R1 subscribes again after each second without data and logs how many tries it took.
5. **No failed save on the real route.** `retry_after_a_failed_save` stays in process and in
   `tests/durable`; the route injects no disk fault.
6. **No skew bound.** The windows are widened by the measured offsets; how much clock uncertainty a lookup
   must report is not decided (slice profile SP-C2).
7. **Principals are the client's word** (4.3, plan `:825`). U1 proves the check, not the identity.
8. **No checkpoint folds during a run.** A's four shares renewed every 4 s supersede 1,000 claims in about
   17 minutes (`claims.rs:61-64`); 4.5c's own tests and replay cover folding.
9. **Cross-origin conflicts.** Two nodes declaring one glade id differently resolve by the stamp
   (`registry.rs:867-883`), with no warning. The route's two files declare alike.
10. **glade-gyld's README** states the binding rule that `registry.rs:867-869` cites; it needs the origin
    scope too. That repository is outside this step.
11. **The probe's build is not `--locked`**: client-rs keeps no lockfile (`client-rs/.gitignore`).
12. **Windows runs only B,** which the journey never stops cleanly; its teardown is a forced end, as in 4.5.

## Default-path changes

1. **`--lease-ms <n>`** on the booted form, both roots. Absent, the leases stay F1's (`claims.rs:52-60`) and
   nothing new is printed; given, the node prints its `leases` line; out of range or malformed, the start is
   refused before anything is written. The desk passes none. The entry point's documentation, "No flag
   changes them" (`bin/glade-node.rs:102-104`, `:205-209`), changes.
2. **The binding fold scopes a retraction to its op's origin.** A store holding one origin, every registry
   and the desk's served store, answers as before; a store holding several nodes' records no longer lets one
   node's retraction take down another's declaration.
3. **A forward's end is told** to the zone's local subscribers: one Error, `UnknownShare`, `forward from
   node <A> ended`. A node without links, as the desk runs, never forwards.

Nothing else: no record, format, `PROTOCOL` or ALPN change, and the new files (`scripts/route/`,
`client-rs/examples/route_probe.rs`) are off every node's path.

## Questions for the owner

1. **The two configured nodes** (section 1). Recommend both placements: `local` on the Mac for every run,
   and `crossing` with A and C on the Pi and B on dabeest at the done. Alternatives: `local` alone, which
   never crosses a machine or n0; `crossing` alone, which needs both machines for every run; or a crossing
   with B on the Mac, loopback-bound with `relay n0`, one remote host and no Windows node.
2. **Expiry without five minutes** (sections 2 and 5). Recommend `--lease-ms`, 3,000 to 3,600,000 ms,
   renewal at a third, the route passing 12,000. Alternatives: a `lease` line in the `--config` file, which
   holds the node's network and nothing else; an environment variable, read once at the entry point, which
   would reach the desk's node from whatever shell starts the desk; or no setting, and a journey of over ten
   minutes on F1's lease.
3. **STA-P3-1** (section 3). Recommend the origin in the retraction's scope, alone. Alternatives: a merged
   clock alone, which leaves one node's retraction able to take down another's declaration; or both.
4. **SUR-P3-5** (section 4). Recommend no retract half in v1; later, one for `service`, scoped to its
   origin, and none for `workspace`. Alternatives: both, R9(s) whole; or neither.
5. **A forward's end** (section 5). Recommend telling its subscribers, in this step. The alternative leaves
   it silent, has the journey check only fresh subscribes after each stop, and records the silence as a gap.
6. **The gate** (section 7). Recommend the route outside the gate, run at the done and after route-touching
   steps. The alternative is a tenth component, the local placement, about 90 s with loopback sockets.
7. **The roots** (section 1). Recommend A on the assembled root and B and C on the hand-written one, so one
   run carries both. The alternative runs all three on the desk's hand-written root, where SIGTERM kills A
   by the signal and H1's clean stop becomes a second crash.
8. **The split** (section 8). Recommend six parts and the runs, in that order. Alternatives: merge parts 2
   and 3 (about 190-280 lines), or build the journey in Rust within the probe, dropping `nodes.py` but
   putting ssh control and log stamping in Rust.

**Ruled, owner, 2026-09-30 ("all recommended"):** 1-8 as recommended. Both placements, `local` for
every run and `crossing` at the done; `--lease-ms`, with the route at 12,000; the origin alone in a
retraction's scope; no retract half in v1; a forward's end told to its subscribers in this step; the
route outside the gate; A on the assembled root, B and C on the hand-written one; six parts, then the
runs. The build starts in the node lane once glade's move onto taut v0.10.0 lands.
