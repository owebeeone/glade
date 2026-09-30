#!/usr/bin/env python3
"""Node control for the fixed-peer route (plan Step 4.6, part 5; `dev-docs/GladeFixedPeerRoute.md`
sections 1 and 2): placements, network files, and starting, stamping, signalling and stopping
the three nodes here or over ssh, for `route.py`. Python 3.10 or later, standard library only.

The desk is never one of them: each node's GLADE_HOME, HOME and temporary directory lie in the
run's scratch under its name, and one at or under the host user's `~/.glade` is refused; every
port is chosen here, never 0, 5173, 8080 or 9099; a PID is signalled only if the harness started
it and its command line names the binary it started; nothing is built. A node runs under
`env -i`. Its lines are stamped `<seconds since the start, to the ms> <source> <line>` as
`<name>.out` and `<name>.err`, and what the harness did to it as `<name>`. Endpoint ids stay out
of the log. Stop the nodes before closing the log.
"""
import os
import posixpath
import re
import shlex
import signal as signals
import socket
import subprocess
import threading
import time
from dataclasses import dataclass, field

DESK_PORTS = frozenset({5173, 8080, 9099})
LEASE_MS = 12000  # a claim lives 12 s and is renewed every 4 s (section 2)
STOP_WITHIN = 10.0  # seconds, the assembled root's stop budget (lifecycle.rs STOP_WITHIN)
LOCAL_PATH = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin'}


class Refused(Exception):
    """A file, port, start or signal the harness will not make."""


@dataclass(frozen=True)
class Line:
    """A stamped line: seconds since the run's start, to the ms, its source and its text."""
    seconds: float
    source: str
    text: str

    def __str__(self) -> str:
        return f'{self.seconds:.3f} {self.source} {self.text}'

    @classmethod
    def parse(cls, stamped: str) -> 'Line':
        seconds, source, text = stamped.split(' ', 2)
        return cls(float(seconds), source, text)


class Log:
    """The run's one log: stamped lines, kept for the checks too, and the unstamped header and
    verdict. It never overwrites a file: a log is evidence."""

    def __init__(self, path, clock=time.monotonic):
        self._file = open(path, 'x', encoding='utf-8')
        self._clock, self._start = clock, clock()
        self._changed = threading.Condition()
        self.lines: list[Line] = []

    def now(self) -> float:
        return self._clock() - self._start

    def line(self, source: str, text: str) -> Line:
        with self._changed:
            stamped = Line(round(self.now(), 3), source, text)
            self.lines.append(stamped)
            self.write(str(stamped))
            self._changed.notify_all()
        return stamped

    def write(self, text: str) -> None:
        with self._changed:
            self._file.write(f'{text}\n')
            self._file.flush()

    def wait_for(self, source: str, pattern: str, timeout: float, after: float = 0.0) -> Line | None:
        """The first line from `source`, stamped at or after `after`, in which `pattern` is
        found, waiting up to `timeout` s; None if none comes."""
        found, deadline, seen = re.compile(pattern).search, time.monotonic() + timeout, 0
        with self._changed:
            while True:
                for line in self.lines[seen:]:
                    if line.source == source and line.seconds >= after and found(line.text):
                        return line
                seen, left = len(self.lines), deadline - time.monotonic()
                if left <= 0:
                    return None
                self._changed.wait(left)

    def close(self) -> None:
        self._file.close()


def free_port() -> int:
    """A loopback port the OS offers for TCP, if UDP can bind it too; else 0."""
    with socket.socket() as tcp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        tcp.bind(('127.0.0.1', 0))
        try:
            udp.bind(tcp.getsockname())
        except OSError:
            return 0
        return tcp.getsockname()[1]


def choose_ports(count: int, offer=free_port, tries: int = 100) -> list[int]:
    """`count` distinct ports from `offer` for the nodes: never 0, and never the desk's."""
    chosen: list[int] = []
    for _ in range(tries):
        port = offer()
        if port and port not in DESK_PORTS and port not in chosen:
            chosen.append(port)
            if len(chosen) == count:
                return chosen
    raise Refused(f'no {count} free ports in {tries} offers')


def check_port(port: int) -> int:
    """`port`, if a node may take it: one the harness chose, so never 0, nor the desk's."""
    if not 0 < port < 65536 or port in DESK_PORTS:
        raise Refused(f'port {port}: a node takes only a port the harness chose, never 0 '
                      'or the desk\'s 5173, 8080 or 9099')
    return port


def check_home(glade_home: str, home: str, resolve=os.path.realpath) -> str:
    """`glade_home`, unless it is relative, or at or under the desk's `.glade` in `home`.
    Here `resolve` follows links; a remote host's paths are only normalised."""
    desk, held = resolve(posixpath.join(home, '.glade')), resolve(glade_home)
    if not posixpath.isabs(glade_home) or held == desk or held.startswith(desk + '/'):
        raise Refused(f'GLADE_HOME {glade_home}: not an absolute path outside the desk\'s {desk}')
    return glade_home


def may_signal(key, started, command: str, binary: str) -> bool:
    """Whether the harness may signal a process: it started it (`key`, its host and PID, is in
    `started`), and its command line, `command`, names the binary it started."""
    return key in started and (command == binary or command.startswith(binary + ' '))


@dataclass(frozen=True)
class Host:
    """Where nodes run: here, through `sh -c`, or where `ssh` reaches, its destination last.
    `binary` is the built glade-node there, `scratch` the run's directory there, `home` its
    user's home, `lan` the address a peer endpoint binds, and `env` the local sh's or ssh's
    environment, never a node's. `msys` marks dabeest's MinGW bash."""
    binary: str
    scratch: str
    home: str
    lan: str = '127.0.0.1'
    ssh: tuple[str, ...] = ()
    msys: bool = False
    env: dict[str, str] = field(default_factory=lambda: dict(LOCAL_PATH))

    def argv(self, command: str, forward: tuple[int, int] | None = None) -> list[str]:
        """The argv that runs the shell command `command` there; over ssh, `forward` takes a
        port here to a port on the host's loopback."""
        if not self.ssh:
            return ['sh', '-c', command]
        tunnel = ['-L', f'127.0.0.1:{forward[0]}:127.0.0.1:{forward[1]}'] if forward else []
        return [*self.ssh[:-1], *tunnel, self.ssh[-1], command]

    def run(self, command: str, stdin: str | None = None, check: bool = True) -> str:
        done = subprocess.run(self.argv(command), input=stdin, capture_output=True, text=True,
                              env=self.env, timeout=60)
        if check and done.returncode:
            where = self.ssh[-1] if self.ssh else 'here'
            raise Refused(f'{where}: `{command}` exited {done.returncode}: {done.stderr.strip()}')
        return done.stdout

    def put(self, path: str, text: str) -> str:
        """Write `text` there to `path`, a new file at 0600, its directory made at 0700."""
        where, q = posixpath.dirname(path), shlex.quote
        self.run(f'umask 077 && mkdir -p {q(where)} && rm -f {q(path)} && cat > {q(path)}', text)
        return path

    def command_line(self, pid: int) -> str:
        """The command line of `pid` there, read afresh; '' if there is no such process."""
        ps = f'tr "\\0" " " < /proc/{pid}/cmdline' if self.msys else f'ps -ww -o command= -p {pid}'
        return self.run(ps, check=False).strip()


@dataclass(frozen=True)
class Place:
    """A node's place: its instance name, host, peer (UDP) and client (TCP) ports and root;
    `forward` is the port here that an ssh forward takes to its client port."""
    name: str
    host: Host
    peer_port: int
    client_port: int
    relay: bool = False
    assembled: bool = False
    enforce: bool = False
    forward: int | None = None


def local(binary: str, scratch: str, home: str, ports: list[int] | None = None) -> dict[str, Place]:
    """All three nodes here over loopback, relays off. `ports`, chosen here if not given, are
    A's, B's and C's peer ports, then their client ports."""
    here = Host(binary, scratch, home)
    pa, pb, pc, ca, cb, cc = ports or choose_ports(6)
    return {'a': Place('route-a', here, pa, ca, assembled=True),
            'b': Place('route-b', here, pb, cb, enforce=True),
            'c': Place('route-c', here, pc, cc)}


def crossing(pi: Host, dabeest: Host, forwards: list[int] | None = None) -> dict[str, Place]:
    """A and C on the Pi, B on dabeest, A and B with `relay n0`: section 1's peer ports, client
    ports 4555 and 4556, and `forwards` here to A, B and C, chosen here if not given."""
    fa, fb, fc = forwards or choose_ports(3)
    return {'a': Place('route-a', pi, 4545, 4555, relay=True, assembled=True, forward=fa),
            'b': Place('route-b', dabeest, 4545, 4555, relay=True, enforce=True, forward=fb),
            'c': Place('route-c', pi, 4546, 4556, forward=fc)}


@dataclass
class Proc:
    """A process the harness started: `pid` on `host`, running `binary`; `popen`, the local sh
    or ssh, carries its lines, which `watcher` stamps."""
    name: str
    host: Host
    binary: str
    pid: int
    popen: subprocess.Popen
    watcher: threading.Thread | None = None


class Nodes:
    """One run's nodes: their files, processes and lines. `ids` holds the endpoint ids, which
    no log line holds; `started`, the host and PID of each process the harness started."""

    def __init__(self, places: dict[str, Place], log: Log, lease_ms: int = LEASE_MS):
        for place in places.values():
            host = place.host
            resolve = posixpath.normpath if host.ssh else os.path.realpath
            check_home(self.dirs(place)[0], host.home, resolve)
            for port in (place.peer_port, place.client_port, place.forward):
                if port is not None:
                    check_port(port)
            if not host.ssh and not (os.path.isfile(host.binary) and os.access(host.binary, os.X_OK)):
                raise Refused(f'{host.binary} is not a built binary; the harness builds nothing')
        self.places, self.log, self.lease_ms = places, log, lease_ms
        self.ids: dict[str, str] = {}
        self.procs: list[Proc] = []
        self.started: set[tuple[tuple[str, ...], int]] = set()

    @staticmethod
    def dirs(place: Place) -> list[str]:
        """The node's GLADE_HOME, HOME and temporary directory: in the scratch, under its name."""
        return [f'{place.host.scratch}/{place.name}/{leaf}' for leaf in ('glade', 'home', 'tmp')]

    def env(self, place: Place) -> dict[str, str]:
        glade, home, tmp = self.dirs(place)
        env = {'GLADE_HOME': glade, 'HOME': home}
        env.update({'TMP': tmp, 'TEMP': tmp} if place.host.msys else {'TMPDIR': tmp})
        if place.assembled:
            env['GLADE_NODE_ASSEMBLED'] = '1'
        return env

    @staticmethod
    def command(host: Host, argv: list[str], env: dict[str, str], dirs=(), pid=False) -> str:
        """The shell command that runs `argv` there under `env -i` with `env` alone (and on
        Windows the SYSTEMROOT its programs need), once `dirs` are made; with `pid`, it first
        prints `pid <n>`, the PID that exec hands on to `argv`."""
        steps = ['umask 077', *([f'mkdir -p {shlex.join(dirs)}'] if dirs else [])]
        steps += ['echo pid $$'] if pid else []
        keep = ['SYSTEMROOT="$SYSTEMROOT"'] if host.msys else []
        pairs = [f'{name}={shlex.quote(value)}' for name, value in env.items()]
        return ' && '.join([*steps, ' '.join(['exec env -i', *keep, *pairs, shlex.join(argv)])])

    def endpoint_id(self, role: str) -> str:
        """The role's endpoint id, from `glade-node endpoint-id`, which mints its key first."""
        place = self.places[role]
        argv = [place.host.binary, 'endpoint-id', '--name', place.name]
        ident = place.host.run(self.command(place.host, argv, self.env(place), self.dirs(place))).strip()
        if not re.fullmatch(r'[0-9a-f]{64}', ident):
            raise Refused(f'{place.name}: endpoint-id printed no endpoint id')
        self.ids[role] = ident
        return ident

    def network(self, role: str, relay_url: str | None = None) -> str:
        """The role's network file, as section 1's table has it: A dials B, at B's relay URL if
        B has relays, else at its bind; B names A and dials nothing; C dials A at its bind."""
        place, a, b = self.places[role], self.places['a'], self.places['b']
        if role == 'a' and b.relay and not relay_url:
            raise Refused('route-a\'s network waits for the relay URL route-b prints')
        peer, via = {'a': ('b', relay_url if b.relay else f'{b.host.lan}:{b.peer_port}'),
                     'b': ('a', None), 'c': ('a', f'{a.host.lan}:{a.peer_port}')}[role]
        if peer not in self.ids:
            raise Refused(f'{place.name}\'s network needs {self.places[peer].name}\'s endpoint id')
        entry, relay = self.ids[peer] + (f'@{via}' if via else ''), 'n0' if place.relay else 'off'
        return f'relay {relay}\nbind {place.host.lan}:{place.peer_port}\npeer {entry}\n'

    def write_network(self, role: str, relay_url: str | None = None) -> str:
        """Write the role's network file in its scratch, at 0600; its path, for `--config`."""
        place = self.places[role]
        path = f'{place.host.scratch}/{place.name}/{place.name}.conf'
        return place.host.put(path, self.network(role, relay_url))

    def start(self, role: str, apps: list[str], relay_url: str | None = None) -> Proc:
        """Write the role's network file, then start its node: booted as its name with `apps`,
        the route's leases and, for B, client grants enforced, on its client port."""
        place = self.places[role]
        argv = [place.host.binary, '--profile', 'local', '--name', place.name]
        for app in apps:
            argv += ['--app', app]
        argv += ['--config', self.write_network(role, relay_url), '--lease-ms', str(self.lease_ms)]
        argv += ['--enforce-client-grants'] if place.enforce else []
        forward = (place.forward, place.client_port) if place.forward else None
        argv.append(str(place.client_port))
        return self.spawn(place.host, place.name, argv, self.env(place), self.dirs(place), forward)

    def spawn(self, host: Host, name: str, argv: list[str], env: dict[str, str], dirs=(),
              forward: tuple[int, int] | None = None) -> Proc:
        """Start `argv` there, stamping its lines, then `exit <code>`: a negative code is the
        signal that ended a local process; over ssh, the code is ssh's."""
        popen = subprocess.Popen(host.argv(self.command(host, argv, env, dirs, pid=True), forward),
                                 stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 encoding='utf-8', errors='replace', env=host.env)
        err = threading.Thread(target=self._pump, args=(popen.stderr, f'{name}.err'), daemon=True)
        err.start()
        said = re.fullmatch(r'pid (\d+)\n', popen.stdout.readline())
        if not said:
            popen.kill()
            popen.wait()
            err.join()
            popen.stdout.close()
            raise Refused(f'{name}: nothing started by {argv[0]}')
        proc = Proc(name, host, argv[0], int(said[1]), popen)
        self.started.add((host.ssh, proc.pid))
        self.procs.append(proc)
        shown = [f'{key}={value}' for key, value in env.items()]
        self.log.line(name, f'started pid {proc.pid}: {" ".join([*shown, shlex.join(argv)])}')
        proc.watcher = threading.Thread(target=self._watch, args=(proc, err), daemon=True)
        proc.watcher.start()
        return proc

    def _pump(self, stream, source: str) -> None:
        with stream:
            for text in stream:
                self.log.line(source, text.rstrip('\n'))

    def _watch(self, proc: Proc, err: threading.Thread) -> None:
        self._pump(proc.popen.stdout, f'{proc.name}.out')
        err.join()
        self.log.line(proc.name, f'exit {proc.popen.wait()}')

    def url(self, role: str) -> str:
        """Where a client here reaches the role's node: its client port, or the forward to it."""
        place = self.places[role]
        return f'ws://127.0.0.1:{place.forward or place.client_port}'

    def wait(self, proc: Proc, timeout: float) -> int | None:
        """Its exit code, once it has ended and its lines are all stamped; None while it runs."""
        proc.watcher.join(timeout)
        return None if proc.watcher.is_alive() else proc.popen.returncode

    def signal(self, proc: Proc, sig: signals.Signals) -> None:
        """Send `sig`, SIGTERM or SIGKILL, to a process the harness started, once its command
        line names the binary it started; on Windows either is a forced end (named gap 12)."""
        host, pid = proc.host, proc.pid
        ended = not host.ssh and proc.popen.returncode is not None  # reaped: the PID is free
        command = '' if ended else host.command_line(pid)
        if not may_signal((host.ssh, pid), self.started, command, proc.binary):
            self.log.line(proc.name, f'not signalled: pid {pid} is not {proc.binary}, started here')
            raise Refused(f'{proc.name}: pid {pid} is not a process the harness started on {proc.binary}')
        self.log.line(proc.name, f'{sig.name} to pid {pid}')
        if host.msys:
            host.run(f'taskkill //F //PID "$(cat /proc/{pid}/winpid)"')
        else:
            host.run(f'kill -s {sig.name[3:]} {pid}')

    def stop(self, proc: Proc, within: float = STOP_WITHIN) -> int | None:
        """SIGTERM, then SIGKILL if it runs on `within` s later; its exit code."""
        for sig, grace in ((signals.SIGTERM, within), (signals.SIGKILL, 5.0)):
            if self.wait(proc, 0) is None:
                self.signal(proc, sig)
                self.wait(proc, grace)
        return self.wait(proc, 0)

    def stop_all(self) -> None:
        """Teardown: stop each process the harness started that still runs."""
        for proc in self.procs:
            try:
                self.stop(proc)
            except Refused as refusal:
                self.log.line(proc.name, f'not stopped: {refusal}')
