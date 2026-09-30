#!/usr/bin/env python3
"""The fixed-peer route end to end (plan Step 4.6, part 6; `dev-docs/GladeFixedPeerRoute.md`
section 2): the journey and its 13 checks, one per clause of the step's acceptance sentence.

    python3 scripts/route/route.py --placement local|crossing --node <glade-node> \\
        --probe <route_probe> --log <file> [--target-dir <dir>] [--hosts <hosts.json>]

B starts, then A, whose seeds name B's node, then C. Four probe sessions run here: `writer` at A,
and `alice`, `mallory` and `poll`, alice's second session, at B; `poll` subscribes every 500 ms
from A's stop until B tells alice of A's crash. Then 3.4's fast loop, teardown, one `CHECK <id>
PASS|FAIL|SKIP <clause>: <evidence>` line per check, each read from the stamped lines (T1 also
from what teardown found), and the verdict. It exits 0 only when all 13 pass; a setup step that
fails marks the checks that need it SKIP. The fast loop builds only into `--target-dir`, else
`CARGO_TARGET_DIR`, outside the checkout, whose own target holds the desk's glade-node; given
none, or one inside, F1 and F2 are SKIP. `--hosts` gives the crossing's hosts: a JSON object whose
`pi` and `dabeest` hold `nodes.Host`'s fields (`ssh` a list) and `checkout`, the host's glade
tree; this machine's SSH_AUTH_SOCK reaches ssh. Python 3.10 or later, standard library only.
"""
import argparse
import dataclasses
import datetime
import json
import math
import os
import posixpath
import queue
import re
import resource
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

import nodes
from nodes import Line

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.realpath(__file__))))  # the checkout
CHECKS = {'R1': 'a real registration is discoverable', 'U1': 'unauthorized: a principal without a grant',
          'U2': 'unauthorized: a node without a grant', 'U3': 'unauthorized: an unbound key',
          'H1': 'honest stop', 'H2': 'honest restart, exact retry',
          'E1': 'expired entries excluded, route up', 'H3': 'honest loss',
          'E2': 'expired entries excluded, holder gone',
          'H4': 'honest restart after a crash, retry', 'F1': 'the fast path, warm',
          'F2': 'the fast path, one file touched', 'T1': "the run's own hygiene"}
SUITE = ['cargo', 'test', '--offline', '--locked', '--manifest-path', 'node/Cargo.toml',
         '--test', 'journeys', '--test', 'assembly']
TOUCHED = 'node/tests/journeys/leases.rs'
BUDGETS = {'F1': ('warm', 5, 1.0), 'F2': ('touched', 3, 3.0)}  # runs, and the fastest's seconds
LAPSE = (8.0, 12.5)  # after A's end: a 12 s claim, renewed every 4 s, seen by a 500 ms poll
EVENT = re.compile(r'(op|zone-refused) |dropped$')  # a probe's line that answers no command
ANSWER = re.compile(r'acked \S+ \[.*\]|refused \S+ \S+: .+')
RUN = re.compile(r'\w+ run \d+: exit (?P<exit>-?\d+), (?P<passed>\d+) passed, (?P<failed>\d+) failed, '
                 r'wall (?P<wall>[\d.]+) s, cpu (?P<cpu>[\d.]+) s')
CLOSED = r'^link {a} closed$'
TOLD = r'^zone-refused ws-route/\S+ UnknownShare: forward from node {a} ended$'
UNREACHABLE = r'^refused ws-route/\S+ UnknownShare: claim holder {a} unreachable \(no live peer link\)$'
DENIED = ('refused ws-route/route.notes Unauthorized: unauthorized: principal mallory holds no grant of '
          'read.subscribe on ws-route')
PROCESSES = 'for f in /proc/[0-9]*/cmdline; do tr "\\0" " " < "$f"; echo; done 2>/dev/null'
APP = 'glade-app v1\napp route\nbinding route.notes log share commons from-cursor\n'
EXTRA = 'binding route.extra value share commons latest\n'
ROUTE_B = APP + EXTRA + ''.join(f'seed alice {share} read.subscribe\n'
                                for share in ('ws-route', 'ws-lapse', 'ws-closed', 'ws-rogue'))
ROGUE = ('glade-app v1\napp rogue\nbinding rogue.notes log share commons from-cursor\n'
         'workspace ws-rogue rogue\n')
SETUP = (nodes.Refused, OSError, subprocess.SubprocessError)


def route_a(b: str) -> tuple[str, str]:
    """route-a.glade, whose seeds grant B's node `b` a read of ws-route and ws-lapse, and
    route-a2.glade, the same without `binding route.extra` and `workspace ws-lapse`."""
    seeds = ''.join(f'seed {b} {share} read.subscribe\n' for share in ('ws-route', 'ws-lapse'))
    served = ''.join(f'workspace {share} route\n' for share in ('ws-route', 'ws-lapse', 'ws-closed'))
    return APP + EXTRA + seeds + served, APP + seeds + served.replace('workspace ws-lapse route\n', '')


def pump(log: nodes.Log, stream, source: str, answers: queue.Queue | None = None) -> None:
    """Stamp each line of `stream` as `source`; one that is no event goes to `answers` too."""
    with stream:
        for text in stream:
            line = log.line(source, text.rstrip('\n'))
            if answers is not None and not EVENT.match(line.text):
                answers.put(line)


class Client:
    """A probe session here: its commands stamped as `<name>.in`, its lines as `<name>.out` and
    `.err`. It answers each command in order, after the `welcome` it prints on connecting."""

    def __init__(self, log: nodes.Log, probe: str, url: str, name: str, principal: str):
        self.log, self.name, self.answers, self.owed = log, name, queue.Queue(), 0
        argv, pipe = [probe, url, principal], subprocess.PIPE
        self.popen = subprocess.Popen(argv, stdin=pipe, stdout=pipe, stderr=pipe, encoding='utf-8',
                                      env=dict(nodes.LOCAL_PATH))
        log.line(name, f'started pid {self.popen.pid}: {shlex.join(argv)}')
        for stream, suffix, answers in ((self.popen.stdout, 'out', self.answers),
                                        (self.popen.stderr, 'err', None)):
            args = (log, stream, f'{name}.{suffix}', answers)
            threading.Thread(target=pump, args=args, daemon=True).start()
        try:
            said = self.answers.get(timeout=30).text
        except queue.Empty:
            said = 'nothing'
        if said != f'welcome {principal}':
            self.quit()  # teardown never sees a session that did not start
            raise nodes.Refused(f'{name}: the probe at {url} said {said}, not its welcome')

    def ask(self, command: str, timeout: float = 5.0) -> Line | None:
        """Send `command`; its answer, or None if none comes within `timeout` s."""
        self.log.line(f'{self.name}.in', command)
        try:
            self.popen.stdin.write(f'{command}\n')
            self.popen.stdin.flush()
        except OSError:
            return None
        self.owed, deadline = self.owed + 1, time.monotonic() + timeout
        while self.owed:  # a late answer is an earlier command's
            try:
                answer = self.answers.get(timeout=max(0.0, deadline - time.monotonic()))
            except queue.Empty:
                return None
            self.owed -= 1
        return answer

    def quit(self) -> None:
        """End its stdin, which ends it; kill it if it runs on 5 s later."""
        try:
            self.popen.stdin.close()
            code = self.popen.wait(5)
        except (OSError, subprocess.TimeoutExpired):
            self.popen.kill()
            code = self.popen.wait()
        self.log.line(self.name, f'exit {code}')


class Journey:
    """Section 2's journey, a step at a time: the nodes, the probe sessions, and `poll`'s thread."""

    def __init__(self, log: nodes.Log, places: dict[str, nodes.Place], probe: str):
        self.log, self.places, self.probe, self.nodes, self.relay = log, places, probe, None, None
        self.skew = 0.0
        self.made: set[tuple[tuple[str, ...], str]] = set()  # the scratches made new, which teardown deletes
        self.clients: dict[str, Client] = {}
        self.until, self.poller = threading.Event(), threading.Thread(target=self.poll, daemon=True)

    def mark(self) -> float:
        return self.log.now() - 0.001

    def app(self, role: str, name: str, text: str) -> list[str]:
        host = self.places[role].host
        return [host.put(f'{host.scratch}/apps/{name}.glade', text)]

    def expect(self, source: str, pattern: str) -> re.Match:
        line = self.log.wait_for(source, pattern, 60)
        if not line:
            raise nodes.Refused(f'{source} printed no line matching {pattern} within 60 s')
        return re.search(pattern, line.text)

    def setup(self) -> None:
        """B; then A, linked with B; then the probes, and the writer's e1 and e2 at A."""
        self.nodes.start('b', self.app('b', 'route-b', ROUTE_B))
        a, self.route_a2 = route_a(self.expect('route-b.out', r'^node (\S+)$')[1])
        self.relay = self.expect('route-b.out', r'^relay (\S+)$')[1] if self.places['b'].relay else None
        self.expect('route-b.out', '^listening ')
        if not self.start_a(self.app('a', 'route-a', a)):
            raise nodes.Refused('route-a and route-b did not link within 60 s')
        for name, role, principal in (('writer', 'a', 'writer'), ('alice', 'b', 'alice'),
                                      ('mallory', 'b', 'mallory'), ('poll', 'b', 'alice')):
            self.clients[name] = Client(self.log, self.probe, self.nodes.url(role), name, principal)
        for payload in ('e1', 'e2'):
            self.clients['writer'].ask(f'append ws-route/route.notes log {payload}')

    def start_a(self, apps: list[str]) -> bool:
        """Start A on `apps`: whether it listens, and each side links and ends a home round."""
        t, self.a = self.mark(), self.nodes.start('a', apps, self.relay)
        linked = (r'^link \S+ via ', '^home round with node ')
        waits = [('route-a.out', '^listening ')] + [(f'route-{r}.out', p) for r in 'ab' for p in linked]
        return all(self.log.wait_for(source, pattern, 60, t) for source, pattern in waits)

    def read_back(self, payload: str) -> None:
        """alice subscribes at B, again after each second without `payload`; then her log."""
        alice, t = self.clients['alice'], self.mark()
        for _ in range(5):
            alice.ask('subscribe ws-route/route.notes')
            if self.log.wait_for('alice.out', rf'^op ws-route/route\.notes \S+ {payload}$', 1.0, t):
                break
        alice.ask('log ws-route/route.notes')

    def discover(self) -> None:
        """R1, U1, U2 and U3."""
        (alice, mallory), log = (self.clients[name] for name in ('alice', 'mallory')), self.log
        self.read_back('e2')
        mallory.ask('subscribe ws-route/route.notes')
        mallory.ask('log ws-route/route.notes')
        t = self.mark()
        alice.ask('subscribe ws-closed/route.notes')
        log.wait_for('alice.out', '^zone-refused ws-closed/', 5, t)
        t = self.mark()
        self.nodes.start('c', self.app('c', 'rogue', ROGUE))
        log.wait_for('route-a.err', '^peer refused: endpoint ', 10, t)
        alice.ask('subscribe ws-rogue/route.notes')
        time.sleep(1)  # an op of ws-rogue, or one mallory may not have, would come by now

    def poll(self) -> None:
        """`poll` subscribes ws-lapse and ws-route at B every 500 ms, until told to stop."""
        while not self.until.is_set():
            t = time.monotonic()
            for share in ('ws-lapse', 'ws-route'):
                self.clients['poll'].ask(f'subscribe {share}/route.notes', 2.0)
            self.until.wait(t + 0.5 - time.monotonic())

    def restart(self, payload: str) -> None:
        """A starts on route-a2.glade; the writer reconnects, resends its last op and appends
        `payload`; alice reads it back at B."""
        self.start_a(self.app('a', 'route-a2', self.route_a2))
        for command in ('reconnect', 'resend-last', f'append ws-route/route.notes log {payload}'):
            self.clients['writer'].ask(command)
        self.read_back(payload)

    def stop(self) -> None:
        """H1, E1 and H2: SIGTERM to A, `poll` from then on, and A's restart within 2 s."""
        log, t = self.log, self.mark()
        self.nodes.signal(self.a, signal.SIGTERM)
        self.poller.start()
        if self.nodes.wait(self.a, nodes.STOP_WITHIN) is None:
            self.nodes.stop(self.a, 0)
        for source, pattern in (('route-b.out', CLOSED), ('alice.out', TOLD), ('poll.out', UNREACHABLE)):
            log.wait_for(source, pattern.format(a=r'\S+'), t + 2 - log.now(), t)
        self.restart('e3')
        log.wait_for('poll.out', 'no live ServeClaim for ws-lapse', t + 15 + self.skew - log.now(), t)

    def crash(self) -> None:
        """H3, E2 and H4: SIGKILL to A, `poll` until B tells alice, then A's start."""
        log, t = self.log, self.mark()
        self.nodes.signal(self.a, signal.SIGKILL)
        self.nodes.wait(self.a, 5)
        closed = log.wait_for('route-b.out', CLOSED.format(a=r'\S+'), 50, t)
        log.wait_for('alice.out', TOLD.format(a=r'\S+'), 2, closed.seconds - 1 if closed else t)
        log.wait_for('poll.out', 'no live ServeClaim for ws-route', t + 15 + self.skew - log.now(), t)
        self.until.set()
        self.poller.join()
        self.restart('e4')


def refuse_target(target: str | None, checkout: str = ROOT) -> str | None:
    """Why the fast loop may not build into `target`, or None: none is given, or it lies inside
    the checkout, whose own target holds the glade-node the desk runs."""
    if not target:
        return 'no scratch target: pass --target-dir or set CARGO_TARGET_DIR'
    held, root = os.path.realpath(target) + os.sep, os.path.realpath(checkout) + os.sep
    return f'target {target} lies inside the checkout {root}' if held.startswith(root) else None


def fast_path(log: nodes.Log, target: str | None, checkout: str = ROOT) -> None:
    """F1 and F2: 3.4's fast loop in the checkout, built into `target` alone, once untimed; five
    warm runs, then three, each after touching one journey file's mtime alone, restored after.
    Incremental builds stay on, whatever the caller's environment says: F2 needs them."""
    refused = refuse_target(target, checkout)
    if refused:
        log.line('suite', f'not run: {refused}')
        return
    target = os.path.realpath(target)
    env = {key: value for key, value in os.environ.items() if key != 'CARGO_INCREMENTAL'}
    env['CARGO_TARGET_DIR'], command = target, [*SUITE, '--target-dir', target]
    log.line('suite', f'in {checkout}: CARGO_TARGET_DIR={target} {shlex.join(command)}')
    if suite(log, 'build', [*command, '--no-run'], env, checkout):
        return
    for n in range(1, 6):
        suite(log, f'warm run {n}', command, env, checkout)
    touched = os.path.join(checkout, TOUCHED)
    kept = os.stat(touched)
    try:
        for n in range(1, 4):
            os.utime(touched, ns=(kept.st_atime_ns, time.time_ns()))
            suite(log, f'touched run {n}', command, env, checkout)
    finally:
        os.utime(touched, ns=(kept.st_atime_ns, kept.st_mtime_ns))


def suite(log: nodes.Log, label: str, command: list[str], env: dict[str, str], cwd: str) -> int:
    """One run of the fast loop, its lines stamped; then its exit, its tests, its wall time and the
    CPU time of the children it waited for. Its exit code."""
    t, before, start = log.now() - 0.001, resource.getrusage(resource.RUSAGE_CHILDREN), time.monotonic()
    popen = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, encoding='utf-8', errors='replace')
    pumps = [threading.Thread(target=pump, args=(log, popen.stdout, 'suite.out')),
             threading.Thread(target=pump, args=(log, popen.stderr, 'suite.err'))]
    for thread in pumps:
        thread.start()
    code, wall = popen.wait(), time.monotonic() - start
    for thread in pumps:
        thread.join()
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime
    counts = [re.search(r'(\d+) passed; (\d+) failed', line.text)
              for line in find(log.lines, r'suite\.out', '^test result: ', t)]
    passed, failed = (sum(int(count[n]) for count in counts if count) for n in (1, 2))
    log.line('suite', f'{label}: exit {code}, {passed} passed, {failed} failed, wall {wall:.3f} s, '
                      f'cpu {cpu:.3f} s')
    return code


def find(lines: list[Line], source: str, pattern: str, after=0.0, before=math.inf) -> list[Line]:
    """The lines from `source`, a pattern matched whole, stamped from `after` to `before`, in
    which `pattern` is found."""
    return [line for line in lines if re.fullmatch(source, line.source)
            and after <= line.seconds <= before and re.search(pattern, line.text)]


def first(lines, source, pattern, after=0.0, before=math.inf) -> Line | None:
    return next(iter(find(lines, source, pattern, after, before)), None)


def texts(lines, source, pattern) -> list[str]:
    return [line.text for line in find(lines, source, pattern)]


def node(lines, role: str) -> str:
    """The role's node id from its `node` line, as a pattern; if it has none, one matching nothing."""
    said = first(lines, rf'route-{role}\.out', r'^node \S+$')
    return re.escape(said.text[5:]) if said else '(?!)'


def since(line: Line | None, t: float) -> str:
    return f'+{line.seconds - t:.3f} s' if line else 'never'


def marks(lines) -> tuple[list[float], float | None, float | None]:
    """A's starts, its first SIGTERM, and the first SIGKILL after its first restart."""
    starts = [line.seconds for line in find(lines, 'route-a', '^started pid ')]
    stop = first(lines, 'route-a', '^SIGTERM to pid ')
    kill = first(lines, 'route-a', '^SIGKILL to pid ', starts[1] if len(starts) > 1 else math.inf)
    return starts, stop and stop.seconds, kill and kill.seconds


def logged(lines, served: Line | None, want: str) -> tuple[bool, str]:
    """alice's log at B reads `want` within 5 s of `served`, A's `workspace ws-route serving`."""
    if not served:
        return False, 'A did not serve ws-route'
    t = served.seconds
    logs = find(lines, r'alice\.out', r'^log ws-route/', t, t + 5)
    got = next((line for line in logs if line.text == f'log ws-route/route.notes [{want}]'), None)
    tries = len(find(lines, r'alice\.in', '^subscribe ws-route/', t, got.seconds if got else t + 5))
    said = f'{since(got, t)} after' if got else f'not within 5 s ({logs[-1].text if logs else "no log"}) of'
    return bool(got), f"alice's log at B [{want}] {said} A's workspace ws-route serving, {tries} subscribe(s)"


def r1(lines, facts):
    return logged(lines, first(lines, r'route-a\.out', '^workspace ws-route serving$'), 'e1 e2')


def u1(lines, facts):
    said = texts(lines, r'mallory\.out', r'^(acked|refused|zone-refused|op) ')
    return said == [DENIED], f"mallory at B: {'; '.join(said) or 'no answer'}"


def u2(lines, facts):
    said = texts(lines, r'alice\.out', r'^(acked|refused|zone-refused|op) ws-closed/')
    told = (rf'zone-refused ws-closed/\S+ Unauthorized: refused by node {node(lines, "a")}, which serves '
            rf'ws-closed: unauthorized: node {node(lines, "b")} holds no grant ')
    ok = len(said) == 2 and said[0].startswith('acked ws-closed/') and re.match(told, said[1])
    return bool(ok), f"alice's subscribe of ws-closed at B: {'; '.join(said) or 'no answer'}"


def u3(lines, facts):
    start, bound_at = first(lines, 'route-c', '^started pid '), first(lines, r'route-c\.out', r'^peer \w+ ')
    if not (start and bound_at):
        return False, 'C did not start and bind'
    tag, t = bound_at.text.split()[1], start.seconds
    refused = first(lines, r'route-a\.(out|err)', rf'^peer refused: endpoint {tag}: unknown endpoint key$',
                    t, t + 10)
    links = [*find(lines, r'route-a\.out', rf'^link {node(lines, "c")} '),
             *find(lines, r'route-c\.out', '^link ')]
    rogue = texts(lines, r'alice\.out', r'^(acked|refused|zone-refused|op) ws-rogue/')
    ok = refused and not links and rogue == ['acked ws-rogue/route.notes []']
    return bool(ok), (f"A refused C's endpoint {tag} {since(refused, t)} after C's start; {len(links)} link "
                      f"line(s) between A and C; alice at B: {'; '.join(rogue) or 'no answer'}")


def h1(lines, facts):
    stop, a = marks(lines)[1], node(lines, 'a')
    if stop is None:
        return False, 'A was not sent SIGTERM'
    ended = first(lines, 'route-a', '^exit ', stop)
    said = [first(lines, source, pattern.format(a=a), stop) for source, pattern in (
        (r'route-b\.out', CLOSED), (r'alice\.out', TOLD), (r'poll\.out', UNREACHABLE))]
    ok = ended and ended.text == 'exit 0' and ended.seconds - stop <= nodes.STOP_WITHIN
    closed, told, refused = (since(line, stop) for line in said)
    return bool(ok and all(line and line.seconds - stop <= 2 for line in said)), (
        f"A {ended.text if ended else 'ran on'} {since(ended, stop)} after SIGTERM; B's link to A closed "
        f"{closed}, alice told {told}, a subscribe refused as unreachable {refused} (each within 2 s)")


def again(lines, n: int, resent: str, want: str) -> tuple[bool, str]:
    """A's `n`th restart: it registers (the first time, the retraction alone), re-links and serves
    ws-route; the writer's resend of `resent` is ok; alice's log at B reads `want`."""
    starts = marks(lines)[0]
    if len(starts) <= n:
        return False, f'A started {len(starts)} time(s)'
    t, until = starts[n], (starts[n + 1] if len(starts) > n + 1 else math.inf)
    registered, relinked, served = (first(lines, r'route-a\.out', pattern, t, until) for pattern in (
        r'^app route registered ', r'^link \S+ via ', '^workspace ws-route serving$'))
    resend = first(lines, r'writer\.out', rf'^\S+ ws-route/\S+ \S+ {resent}$', t, until)
    ok, read = logged(lines, served, want)
    retracted = n > 1 or registered and registered.text.startswith('app route registered (+1 record(s), ')
    return bool(ok and retracted and relinked and resend and resend.text.startswith('ok ')), (
        f"A: {registered.text if registered else 'no registration'}, re-linked {since(relinked, t)} "
        f"after its start; resend: {resend.text if resend else 'no answer'}; {read}")


def lapse(lines, share: str, t: float, facts, before=math.inf) -> tuple[Line | None, bool, str]:
    """`poll`'s first `no live ServeClaim for <share>` after `t`, whether it falls in the window
    after `t` widened by the clocks' skew, and the evidence."""
    pattern = rf'^refused {share}/\S+ UnknownShare: no live ServeClaim for {share}$'
    line, skew = first(lines, r'poll\.out', pattern, t, before), facts.get('skew', 0.0)
    low, high = LAPSE[0] - skew, LAPSE[1] + skew
    inside = bool(line) and low <= line.seconds - t <= high
    return line, inside, f'{since(line, t)} (window +{low:.3f}-{high:.3f} s)'


def e1(lines, facts):
    starts, stop, kill = marks(lines)
    if stop is None or len(starts) < 2:
        return False, 'A was not stopped and started again'
    relinked = first(lines, r'route-a\.out', r'^link \S+ via ', starts[1])
    lapsed, inside, window = lapse(lines, 'ws-lapse', stop, facts)
    route = lapse(lines, 'ws-route', stop, facts, kill or math.inf)[0]
    ok = inside and relinked and relinked.seconds < lapsed.seconds and not route
    held = f'lapsed {since(route, stop)}' if route else 'did not lapse before SIGKILL'
    return bool(ok), (f'ws-lapse lapsed {window} after SIGTERM, A re-linked {since(relinked, stop)}; '
                      f'ws-route {held}')


def h3(lines, facts):
    starts, _, kill = marks(lines)
    if kill is None:
        return False, 'A was not sent SIGKILL'
    closed = first(lines, r'route-b\.out', CLOSED.format(a=node(lines, 'a')), kill)
    told = first(lines, r'alice\.out', TOLD.format(a=node(lines, 'a')), kill)
    until = starts[2] if len(starts) > 2 else math.inf
    commands = find(lines, r'poll\.in', '')
    answers = [line for line in find(lines, r'poll\.out', '') if not EVENT.match(line.text)][1:]  # no welcome
    polls = [(asked, said) for asked, said in zip(commands, answers + [None] * len(commands))
             if kill <= asked.seconds < until]
    answered = [said for asked, said in polls
                if said and said.seconds - asked.seconds <= 2 and ANSWER.fullmatch(said.text)]
    ok = closed and closed.seconds - kill <= 45 and told and abs(told.seconds - closed.seconds) <= 1
    return bool(ok and polls and len(answered) == len(polls)), (
        f"B's link to A closed {since(closed, kill)} after SIGKILL (budget 45 s), alice told "
        f"{since(told, closed.seconds) if closed else 'with no close'} after it; "
        f'{len(answered)} of {len(polls)} subscribes at B answered within 2 s')


def e2(lines, facts):
    kill = marks(lines)[2]
    if kill is None:
        return False, 'A was not sent SIGKILL'
    _, inside, window = lapse(lines, 'ws-route', kill, facts)
    return inside, f'ws-route lapsed {window} after SIGKILL'


def fast(lines, key: str) -> tuple[bool | None, str]:
    label, count, budget = BUDGETS[key]
    refused = first(lines, 'suite', '^not run: ')
    if refused:
        return None, refused.text
    runs = [RUN.fullmatch(line.text) for line in find(lines, 'suite', f'^{label} run ')]
    passed = [run for run in runs if run and run['exit'] == run['failed'] == '0' and int(run['passed'])]
    best = min(passed, key=lambda run: float(run['wall']), default=None)
    ok = len(passed) == count and best and float(best['wall']) <= budget
    fastest = f", {best['passed']} tests; the fastest {best['wall']} s (cpu {best['cpu']} s)" if best else ''
    return bool(ok), f'{len(passed)} of {count} {label} runs passed{fastest}, budget {budget} s'


def t1(lines, facts):
    text = facts.get('text', '\n'.join(map(str, lines)))
    leaked = [f"route-{role}'s endpoint id is in the log" for role, ident in facts.get('ids', {}).items()
              if ident in text]
    problems = [*facts.get('problems', ()), *leaked]
    count = 0
    for role in 'abc':
        pid, signalled = None, False
        for word in (line.text.split() for line in find(lines, f'route-{role}', '')):
            if word[0] == 'started':
                pid, signalled, count = word[2].rstrip(':'), False, count + 1
            elif word[0] in ('SIGTERM', 'SIGKILL'):
                signalled = True
            elif word[0] == 'exit' and pid:
                problems += [] if signalled else [f'route-{role} pid {pid} ended unsignalled, exit {word[1]}']
                pid = None
        problems += [f'route-{role} pid {pid} still runs'] if pid else []
    return count > 0 and not problems, '; '.join(problems) or (
        f'{count} node processes, each ended after the script signalled it; no process holds the scratch, '
        'its ports are free, it is deleted, and no endpoint id is in the log')


JUDGES = {'R1': r1, 'U1': u1, 'U2': u2, 'U3': u3, 'H1': h1, 'E1': e1, 'H3': h3, 'E2': e2, 'T1': t1,
          'H2': lambda lines, _: again(lines, 1, 'e2', 'e1 e2 e3'),
          'H4': lambda lines, _: again(lines, 2, 'e3', 'e1 e2 e3 e4'),
          'F1': lambda lines, _: fast(lines, 'F1'), 'F2': lambda lines, _: fast(lines, 'F2')}


def evaluate(lines, facts) -> dict[str, tuple[str, str]]:
    """Each check's status and evidence; one whose setup failed is SKIP, with the reason."""
    skip, status = facts.get('skip', {}), {True: 'PASS', False: 'FAIL', None: 'SKIP'}
    judged = {key: (None, skip[key]) if key in skip else JUDGES[key](lines, facts) for key in CHECKS}
    return {key: (status[ok], said) for key, (ok, said) in judged.items()}


def report(results: dict[str, tuple[str, str]]) -> list[str]:
    """The CHECK lines, then the verdict, worded as the gate words its own."""
    failed = [key for key, (status, _) in results.items() if status != 'PASS']
    verdict = (f'ROUTE: FAIL -- {len(failed)} of {len(CHECKS)} checks failed: {" ".join(failed)}' if failed
               else f'ROUTE: PASS -- all {len(CHECKS)} checks passed')
    return [f'CHECK {key} {status} {CHECKS[key]}: {said}' for key, (status, said) in results.items()] + [
        verdict]


def crossing(path: str) -> tuple[dict[str, nodes.Place], dict[str, nodes.Host], dict[str, str]]:
    """The crossing's places, hosts and glade trees from `--hosts`. Teardown deletes each host's
    scratch, so the one named may be neither a home nor under the desk's `.glade`, and the run
    works in a directory of its own under it, which `make_scratch` makes new."""
    with open(path, encoding='utf-8') as file:
        spec = json.load(file)
    agent = {key: os.environ[key] for key in ('SSH_AUTH_SOCK',) if key in os.environ}
    hosts, trees = {}, {}
    own = f'glade-route-{time.strftime("%Y%m%d-%H%M%S")}-{os.getpid()}'
    for name in ('pi', 'dabeest'):
        fields = dict(spec[name])
        trees[name] = fields.pop('checkout')
        fields.update(ssh=tuple(fields.get('ssh', ())), env=fields.get('env', {**nodes.LOCAL_PATH, **agent}))
        host = nodes.Host(**fields)
        scratch = posixpath.normpath(nodes.check_home(host.scratch, host.home, posixpath.normpath))
        if not host.ssh or (posixpath.normpath(host.home) + '/').startswith(scratch.rstrip('/') + '/'):
            raise nodes.Refused(f'{name}: the crossing takes ssh, and a scratch with no home in it: '
                                f'{host.scratch}')
        hosts[name] = dataclasses.replace(host, scratch=posixpath.join(scratch, own))
    return nodes.crossing(hosts['pi'], hosts['dabeest']), hosts, trees


def make_scratch(journey: Journey, host: nodes.Host) -> None:
    """Make the run's scratch on `host`, new: `mkdir` refuses one that exists already, and
    teardown deletes only a scratch made so."""
    q = shlex.quote
    host.run(f'umask 077 && mkdir -p {q(posixpath.dirname(host.scratch))} && mkdir {q(host.scratch)}')
    journey.made.add((host.ssh, host.scratch))


def offsets(hosts: dict[str, nodes.Host]) -> tuple[float, str]:
    """E1's and E2's widening: the Pi's clock against dabeest's, each read to within half its
    round trip; and each offset from this clock, as the log shows it."""
    read = {}
    for name in ('pi', 'dabeest'):
        before = time.time()
        there = float(hosts[name].run('date +%s.%N'))
        read[name] = (there - (before + time.time()) / 2, (time.time() - before) / 2)
    (pi, pi_half), (dab, dab_half) = read.values()
    shown = ', '.join(f'{name} {off:+.3f} s (+-{half:.3f})' for name, (off, half) in read.items())
    return abs(pi - dab) + pi_half + dab_half, shown


def header(journey: Journey, args, hosts: dict[str, nodes.Host], trees: dict[str, str]) -> list[str]:
    q, now = shlex.quote, datetime.datetime.now().astimezone().isoformat(timespec='seconds')
    out = [f'route {now}: placement {args.placement}, {shlex.join(sys.argv)}',
           f'leases {nodes.LEASE_MS} ms, renewed every {nodes.LEASE_MS // 3} ms']
    for name, host in hosts.items():
        said = host.run(f'cd {q(trees[name])} && git rev-parse --short=12 HEAD && '
                        'git --no-optional-locks status --porcelain', check=False).splitlines()
        tree = 'unknown' if not said else 'clean' if len(said) == 1 else f'{len(said) - 1} path(s) changed'
        via = f' via {shlex.join(host.ssh)}' if host.ssh else ''
        out.append(f'host {name}{via}: glade {said[0] if said else "unknown"}, tree {tree}')
        for binary in filter(None, (host.binary, args.probe if name == 'here' else '')):
            digest = host.run(f'shasum -a 256 {q(binary)} 2>/dev/null || sha256sum {q(binary)}', check=False)
            out.append(f'binary {binary} on {name}: sha256 {digest[:12] or "unknown"}')
    for role, place in journey.places.items():
        root = ('assembled' if place.assembled else 'hand-written') + ' root'
        root += ', client grants enforced' if place.enforce else ''
        forward = f', forward {place.forward}' if place.forward else ''
        out.append(f'{place.name}: {root}, peer {place.host.lan}:{place.peer_port}, client port '
                   f'{place.client_port}{forward}, endpoint tag {journey.nodes.ids[role][:10]}, '
                   f'scratch {place.host.scratch}')
    return out


def bound(port: int) -> bool:
    """Whether a TCP listener or a UDP socket holds `port` on loopback here."""
    for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
        with socket.socket(socket.AF_INET, kind) as held:
            held.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, int(kind == socket.SOCK_STREAM))
            try:
                held.bind(('127.0.0.1', port))
            except OSError:
                return True
    return False


def teardown(journey: Journey) -> list[str]:
    """Stop the probes and nodes; then what T1 finds: a process holding a scratch, a port still
    bound here (the nodes', or the forwards to them) and a scratch not deleted."""
    journey.until.set()
    if journey.poller.is_alive():
        journey.poller.join(10)
    for client in journey.clients.values():
        client.quit()
    if journey.nodes:
        journey.nodes.stop_all()
    places, problems = journey.places.values(), []
    hosts = {(place.host.ssh, place.host.scratch): place.host for place in places}.values()
    here = subprocess.run(['ps', '-axww', '-o', 'command='], capture_output=True, text=True).stdout
    here = here.splitlines()
    for host in hosts:
        listed = here + (host.run(PROCESSES, check=False).splitlines() if host.ssh else [])
        held = host.spellings(host.scratch)  # on dabeest, a native node's arguments are E:/…
        problems += [f'left running: {line.strip()}' for line in listed if any(path in line for path in held)]
    ports = [place.forward for place in places if place.forward] or [
        port for place in places for port in (place.peer_port, place.client_port)]
    problems += [f'port {port} still bound' for port in ports if bound(port)]
    for host in hosts:
        if (host.ssh, host.scratch) not in journey.made:
            problems.append(f'scratch not deleted: {host.scratch} is not one this run made')
            continue
        try:
            host.run(f'rm -rf {shlex.quote(host.scratch)} && test ! -e {shlex.quote(host.scratch)}')
        except nodes.Refused as refusal:
            problems.append(f'scratch not deleted: {refusal}')
    return problems


def run(args, log: nodes.Log) -> int:
    here = nodes.Host(args.node or '', '', os.path.expanduser('~'))
    hosts, trees = {'here': here}, {'here': ROOT}
    if args.placement == 'local':
        places = nodes.local(args.node, tempfile.mkdtemp(prefix='glade-route-'), here.home)
    else:
        places, remote, remote_trees = crossing(args.hosts)
        hosts.update(remote)
        trees.update(remote_trees)
    journey, facts, reason = Journey(log, places, args.probe), {'skip': {}}, None
    if args.placement == 'local':
        journey.made.add(((), places['a'].host.scratch))  # mkdtemp made it new

    def prepare():
        journey.nodes = nodes.Nodes(places, log)
        for host in hosts.values():
            if host.ssh:
                make_scratch(journey, host)
        for role in 'abc':
            journey.nodes.endpoint_id(role)
        journey.skew, shown = offsets(hosts) if args.placement == 'crossing' else (0.0, '')
        for text in header(journey, args, hosts, trees) + ([f'clock offsets: {shown}'] if shown else []):
            log.write(text)

    try:
        for ids, step in (((), prepare), ((), journey.setup), (('R1', 'U1', 'U2', 'U3'), journey.discover),
                          (('H1', 'E1', 'H2'), journey.stop), (('H3', 'E2', 'H4'), journey.crash)):
            if reason is None:
                try:
                    step()
                    continue
                except SETUP as failure:
                    reason = f'not run: {step.__name__} failed: {failure}'
                    log.line('route', reason)
            facts['skip'].update(dict.fromkeys(ids, reason))  # the failed step's, and each later one's
        try:
            fast_path(log, args.target_dir)
        except SETUP as failure:
            log.line('suite', f'not run: {failure}')
    finally:
        facts['problems'] = teardown(journey)
    if args.placement == 'crossing':
        skew, shown = offsets(hosts)
        journey.skew = max(journey.skew, skew)
        log.line('route', f'clock offsets after: {shown}')
    facts.update(skew=journey.skew, ids=dict(journey.nodes.ids) if journey.nodes else {})
    with open(args.log, encoding='utf-8') as file:
        facts['text'] = file.read()
    said = report(evaluate(log.lines, facts))
    for text in said:
        log.write(text)
    print('\n'.join(said))
    return 0 if said[-1].startswith('ROUTE: PASS') else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='The fixed-peer route journey and its 13 checks.')
    parser.add_argument('--placement', choices=('local', 'crossing'), required=True)
    parser.add_argument('--node', help='the built glade-node, for the local placement')
    parser.add_argument('--probe', required=True, help='the built route_probe')
    parser.add_argument('--log', required=True, help='a new file, outside the scratch')
    parser.add_argument('--target-dir', default=os.environ.get('CARGO_TARGET_DIR'),
                        help="the fast loop's scratch target, outside the checkout")
    parser.add_argument('--hosts', help="the crossing's hosts, a JSON file")
    args = parser.parse_args(argv)
    if not (args.node if args.placement == 'local' else args.hosts):
        parser.error('the local placement takes --node, and the crossing --hosts')
    log = nodes.Log(args.log)
    try:
        return run(args, log)
    finally:
        log.close()


if __name__ == '__main__':
    sys.exit(main())
