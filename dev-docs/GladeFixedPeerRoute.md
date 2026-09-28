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
