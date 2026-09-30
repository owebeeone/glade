#!/usr/bin/env python3
"""Tests of nodes.py, the route's node control (plan Step 4.6, part 5).

Run `python3 -B scripts/route/test_nodes.py`. They start no glade-node and reach no other host: a
crossing's hosts are this machine's shell, and `sleep` or a script stands in for a node.
"""
import dataclasses
import os
import re
import signal
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

import nodes

A, B = 'a' * 64, 'b' * 64
PORTS = [40001, 40002, 40003, 40011, 40012, 40013]


class NodesTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.tmp = Path(scratch.name)
        self.home, self.run_dir = str(self.tmp / 'home'), str(self.tmp / 'run')
        self.bin = str(self.tmp / 'bin' / 'sleep')
        os.makedirs(self.tmp / 'bin')
        os.symlink('/bin/sleep', self.bin)
        self.log = nodes.Log(self.tmp / 'route.log')
        self.addCleanup(self.log.close)

    def make(self, places, **ids):
        run = nodes.Nodes(places, self.log)
        run.ids.update(ids)
        return run

    def files(self, places, relay_url=None):
        run = self.make(places, a=A, b=B)
        paths = {role: run.write_network(role, relay_url) for role in 'abc'}
        self.assertEqual({stat.S_IMODE(os.stat(path).st_mode) for path in paths.values()}, {0o600})
        return {role: Path(path).read_text().splitlines() for role, path in paths.items()}

    def test_a_line_is_stamped_with_seconds_since_the_start_to_the_ms_then_its_source(self):
        ticks = iter([50.0, 51.2504, 172.0])
        log = nodes.Log(self.tmp / 'stamped.log', clock=lambda: next(ticks))
        log.line('route-b.out', 'node 9f3e')
        log.line('route-a.err', '')
        log.close()
        self.assertEqual((self.tmp / 'stamped.log').read_text(),
                         '1.250 route-b.out node 9f3e\n122.000 route-a.err \n')
        line = nodes.Line.parse('1.250 route-b.out link x via direct 127.0.0.1:1')
        self.assertEqual(line, nodes.Line(1.25, 'route-b.out', 'link x via direct 127.0.0.1:1'))

    def test_local_files_bind_loopback_with_relays_off_at_0600(self):
        self.assertEqual(self.files(nodes.local(self.bin, self.run_dir, self.home, PORTS)), {
            'a': ['relay off', 'bind 127.0.0.1:40001', f'peer {B}@127.0.0.1:40002'],
            'b': ['relay off', 'bind 127.0.0.1:40002', f'peer {A}'],
            'c': ['relay off', 'bind 127.0.0.1:40003', f'peer {A}@127.0.0.1:40001']})

    def test_crossing_files_put_a_and_c_on_the_pi_and_b_on_dabeest_at_0600(self):
        pi = nodes.Host(self.bin, str(self.tmp / 'pi'), self.home, lan='10.9.0.6')
        dabeest = nodes.Host(self.bin, str(self.tmp / 'dab'), self.home, lan='10.9.0.9', msys=True)
        places = nodes.crossing(pi, dabeest, [40021, 40022, 40023])
        self.assertRaises(nodes.Refused, self.files, places)  # A's waits for the relay URL B prints
        relay = 'https://relay.example./'
        self.assertEqual(self.files(places, relay), {
            'a': ['relay n0', 'bind 10.9.0.6:4545', f'peer {B}@{relay}'],
            'b': ['relay n0', 'bind 10.9.0.9:4545', f'peer {A}'],
            'c': ['relay off', 'bind 10.9.0.6:4546', f'peer {A}@10.9.0.6:4545']})
        ssh = dataclasses.replace(pi, ssh=('ssh', '-o', 'ConnectTimeout=10', 'pi'))  # never run
        want = ['ssh', '-o', 'ConnectTimeout=10', '-L', '127.0.0.1:40021:127.0.0.1:4555', 'pi', 'true']
        self.assertEqual(ssh.argv('true', (40021, 4555)), want)

    def test_the_harness_chooses_every_port_and_never_the_desks(self):
        offers = iter([5173, 8080, 9099, 0, 40001, 40001, 40002])
        self.assertEqual(nodes.choose_ports(2, offer=lambda: next(offers)), [40001, 40002])
        self.assertRaises(nodes.Refused, nodes.choose_ports, 1, offer=lambda: 8080)
        self.assertEqual(len(set(nodes.choose_ports(6)) - nodes.DESK_PORTS - {0}), 6)
        for port in (0, 5173, 8080, 9099, 65536):
            self.assertRaises(nodes.Refused, nodes.check_port, port)
        places = nodes.local(self.bin, self.run_dir, self.home, [8080, *PORTS[1:]])
        self.assertRaises(nodes.Refused, nodes.Nodes, places, self.log)

    def test_a_glade_home_at_or_under_the_desks_is_refused(self):
        desk = os.path.join(self.home, '.glade')
        os.makedirs(desk)
        os.symlink(desk, self.tmp / 'link')
        for held in (desk, f'{desk}/sys', f'{self.home}/x/../.glade/y', f'{self.tmp}/link/z', 'run/glade'):
            self.assertRaises(nodes.Refused, nodes.check_home, held, self.home)
        for held in (f'{desk}-route', f'{self.run_dir}/route-a/glade'):
            self.assertEqual(nodes.check_home(held, self.home), held)
        places = nodes.local(self.bin, f'{desk}/run', self.home, PORTS)
        self.assertRaises(nodes.Refused, nodes.Nodes, places, self.log)

    def test_only_a_process_it_started_on_the_scratch_binary_is_signalled(self):
        run, here = self.make({}), nodes.Host(self.bin, self.run_dir, self.home)
        ours, spare = (run.spawn(here, name, [self.bin, '30'], {}) for name in ('sleeper', 'spare'))
        self.addCleanup(run.stop_all)
        stranger = subprocess.Popen([self.bin, '30'])
        self.addCleanup(stranger.wait)
        self.addCleanup(stranger.kill)
        for proc in (dataclasses.replace(ours, pid=stranger.pid),
                     dataclasses.replace(ours, binary=f'{self.tmp}/target/glade-node')):
            self.assertRaises(nodes.Refused, run.signal, proc, signal.SIGTERM)
        self.assertFalse(nodes.may_signal(((), 7), {((), 7)}, f'{self.bin}2 30', self.bin))
        self.assertEqual((stranger.poll(), run.wait(ours, 0.2)), (None, None))
        run.signal(ours, signal.SIGTERM)
        self.assertEqual(run.wait(ours, 5), -signal.SIGTERM)
        reused = dataclasses.replace(ours, pid=spare.pid)  # an ended process's PID, taken again
        self.assertRaises(nodes.Refused, run.signal, reused, signal.SIGTERM)

    def test_the_harness_builds_nothing_and_a_node_starts_in_its_scratch_with_the_route_flags(self):
        fake = self.tmp / 'target' / 'glade-node'
        places = nodes.local(str(fake), self.run_dir, self.home, PORTS)
        self.assertRaises(nodes.Refused, nodes.Nodes, places, self.log)  # not built: nothing to run
        fake.parent.mkdir()
        fake.write_text('#!/bin/sh\necho "$*"\necho "$GLADE_HOME $HOME $TMPDIR '  # and PATH's count: 0
                        '${GLADE_NODE_ASSEMBLED-no} $(/usr/bin/env | /usr/bin/grep -c ^PATH=)" >&2\n')
        fake.chmod(0o700)
        run = self.make(places, a=A, b=B)
        for role, tail, root in (('a', '40011', '1'), ('b', '--enforce-client-grants 40012', 'no')):
            name, at = f'route-{role}', f'{self.run_dir}/route-{role}'
            self.assertEqual(run.wait(run.start(role, [f'/apps/{name}.glade']), 5), 0)
            argv = f'--profile local --name {name} --app /apps/{name}.glade --config {at}/{name}.conf'
            for source, text in ((f'{name}.out', f'{argv} --lease-ms 12000 {tail}'), (name, 'exit 0'),
                                 (f'{name}.err', f'{at}/glade {at}/home {at}/tmp {root} 0')):
                self.assertTrue(self.log.wait_for(source, f'^{re.escape(text)}$', 0), (source, text))


if __name__ == '__main__':
    unittest.main()
