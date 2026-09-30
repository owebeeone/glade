#!/usr/bin/env python3
"""Tests of route.py, the route's journey and its 13 checks (plan Step 4.6, part 6).

Run `python3 -B scripts/route/test_route.py`. They start no node and no probe, and build nothing:
each check reads a fixture of stamped lines, a passing journey and a failing change to it, and
the fast loop runs a fake cargo in a scratch checkout.
"""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import nodes
import route

IDS = {'a': 'e' * 64, 'b': 'f' * 64, 'c': '9' * 64}  # endpoint ids, which no log line may hold
DENIED = ('refused ws-route/route.notes Unauthorized: unauthorized: principal mallory holds no grant of '
          'read.subscribe on ws-route')
CLOSED = ('zone-refused ws-closed/route.notes Unauthorized: refused by node aaaa, which serves ws-closed: '
          'unauthorized: node bbbb holds no grant of read.subscribe on ws-closed')
# a fake cygpath's body, after `rest=` strips the test's scratch: the path as MSYS spells E:
CYGPATH = r'''case "$1" in -w) printf 'E:%s\n' "$rest" | tr / '\\' ;; -m) printf 'E:%s\n' "$rest" ;; esac
'''


def runs(label, walls):
    return ''.join(f'70.000 suite {label} run {n}: exit 0, 43 passed, 0 failed, wall {wall} s, '
                   f'cpu 0.500 s\n' for n, wall in enumerate(walls, 1))


WARM, TOUCHED = runs('warm', (1.3, 0.9, 1.1, 0.95, 1.2)), runs('touched', (2.5, 3.2, 2.9))
FIXTURE = f'''\
0.100 route-b started pid 11: glade-node --name route-b
0.200 route-b.out node bbbb
1.000 route-a started pid 12: glade-node --name route-a
1.100 route-a.out node aaaa
1.200 route-a.out link bbbb via direct 127.0.0.1:2, rtt 1 ms
1.300 route-a.out workspace ws-route serving
1.900 writer.out welcome writer
2.000 alice.out welcome alice
2.050 poll.out welcome alice
2.100 alice.in subscribe ws-route/route.notes
2.110 alice.out acked ws-route/route.notes []
2.120 alice.out op ws-route/route.notes writer:1 e2
2.200 alice.in log ws-route/route.notes
2.210 alice.out log ws-route/route.notes [e1 e2]
2.300 mallory.out {DENIED}
2.400 alice.in subscribe ws-closed/route.notes
2.410 alice.out acked ws-closed/route.notes []
2.500 alice.out {CLOSED}
3.000 route-c started pid 13: glade-node --name route-c
3.100 route-c.out node cccc
3.200 route-c.out peer 0123456789 127.0.0.1:3
3.500 route-a.err peer refused: endpoint 0123456789: unknown endpoint key
3.610 alice.out acked ws-rogue/route.notes []
10.000 route-a SIGTERM to pid 12
10.100 poll.in subscribe ws-route/route.notes
10.110 poll.out refused ws-route/route.notes UnknownShare: claim holder aaaa unreachable (no live peer link)
10.200 route-b.out link aaaa closed
10.210 alice.out zone-refused ws-route/route.notes UnknownShare: forward from node aaaa ended
10.300 route-a exit 0
11.000 route-a started pid 14: glade-node --name route-a
11.100 route-a.out app route registered (+1 record(s), 6 unchanged)
11.200 route-a.out link bbbb via direct 127.0.0.1:2, rtt 1 ms
11.300 route-a.out workspace ws-route serving
11.400 writer.in resend-last
11.410 writer.out ok ws-route/route.notes writer:1 e2
11.610 alice.out log ws-route/route.notes [e1 e2 e3]
19.000 poll.in subscribe ws-lapse/route.notes
19.010 poll.out refused ws-lapse/route.notes UnknownShare: no live ServeClaim for ws-lapse
30.000 route-a SIGKILL to pid 14
30.100 route-a exit -9
30.500 poll.in subscribe ws-route/route.notes
30.510 poll.out acked ws-route/route.notes [writer:2]
40.000 poll.in subscribe ws-route/route.notes
40.010 poll.out refused ws-route/route.notes UnknownShare: no live ServeClaim for ws-route
63.600 route-b.out link aaaa closed
63.610 alice.out zone-refused ws-route/route.notes UnknownShare: forward from node aaaa ended
65.000 route-a started pid 15: glade-node --name route-a
65.200 route-a.out link bbbb via direct 127.0.0.1:2, rtt 1 ms
65.300 route-a.out workspace ws-route serving
65.400 writer.in resend-last
65.410 writer.out ok ws-route/route.notes writer:2 e3
65.600 alice.out log ws-route/route.notes [e1 e2 e3 e4]
{WARM}{TOUCHED}90.000 route-a SIGTERM to pid 15
90.100 route-a exit 0
90.200 route-b SIGTERM to pid 11
90.300 route-b exit -15
90.400 route-c SIGTERM to pid 13
90.500 route-c exit -15
'''
FAILING = {  # each check's failing journeys: a line of the passing one, and what replaces it
    'R1': [('[e1 e2]\n', '[e1]\n')],
    'U1': [(DENIED, 'acked ws-route/route.notes [writer:1]')],
    'U2': [('2.500 alice.out', '2.450 alice.out op ws-closed/route.notes writer:0 e0\n2.500 alice.out')],
    'U3': [('3.200 route-c.out', '3.300 route-a.out link cccc via direct 127.0.0.1:3\n3.200 route-c.out'),
           ('peer refused: endpoint 0123456789', 'peer said: endpoint 0123456789')],
    'H1': [('10.210 alice.out', '12.500 alice.out')],
    'H2': [('[e1 e2 e3]\n', '[e1 e2 e2 e3]\n')],
    'E1': [('19.000 poll.in subscribe ws-lapse/route.notes\n19.010',
            '16.000 poll.in subscribe ws-lapse/route.notes\n16.010'),
           ('19.000 poll.in', '20.000 poll.out refused ws-route/route.notes UnknownShare: no live ServeClaim '
            'for ws-route\n19.000 poll.in')],
    'H3': [('63.600 route-b.out link aaaa closed\n63.610', '80.000 route-b.out link aaaa closed\n80.010')],
    'E2': [('40.000 poll.in subscribe ws-route/route.notes\n40.010',
            '44.000 poll.in subscribe ws-route/route.notes\n44.010')],
    'H4': [('[e1 e2 e3 e4]', '[e1 e2 e3 e3 e4]')],
    'F1': [(WARM, runs('warm', (1.3, 1.2, 1.1, 1.05, 1.2)))],
    'F2': [(TOUCHED, runs('touched', (3.4, 3.2, 3.1)))],
    'T1': [('90.000 route-a', f'85.000 route-a.err peer {IDS["b"]}@127.0.0.1:2: gone\n90.000 route-a')],
}


def lines(text):
    return sorted((nodes.Line.parse(row) for row in text.splitlines()), key=lambda line: line.seconds)


def changed(old, new):
    assert old in FIXTURE, old
    return FIXTURE.replace(old, new, 1)


class RouteTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.tmp = Path(os.path.realpath(scratch.name))
        self.checkout = self.tmp / 'glade'
        self.touched = self.checkout / route.TOUCHED
        self.touched.parent.mkdir(parents=True)
        self.touched.write_text('// a journey\n')
        os.utime(self.touched, ns=(1_000_000_000, 2_000_000_000))
        self.log = nodes.Log(self.tmp / 'route.log')
        self.addCleanup(self.log.close)

    def judge(self, key, text=FIXTURE):
        return route.JUDGES[key](lines(text), {'ids': IDS})

    def cargo(self):
        """A fake cargo on PATH, which notes each call's directory, arguments, target and whether
        incremental builds are off; the caller's environment turns them off."""
        bin_dir, calls = self.tmp / 'bin', self.tmp / 'calls'
        bin_dir.mkdir()
        (bin_dir / 'cargo').write_text('#!/bin/sh\necho "$(pwd -P) $* $CARGO_TARGET_DIR '
                                       '${CARGO_INCREMENTAL-on}" >> "$CALLS"\n'
                                       'echo "test result: ok. 2 passed; 0 failed; 0 ignored"\n')
        (bin_dir / 'cargo').chmod(0o700)
        env = {'PATH': f'{bin_dir}:{os.environ["PATH"]}', 'CALLS': str(calls), 'CARGO_INCREMENTAL': '0'}
        patched = mock.patch.dict(os.environ, env)
        patched.start()
        self.addCleanup(patched.stop)
        return calls

    def test_each_check_passes_a_passing_journey(self):
        for key in route.CHECKS:
            ok, evidence = self.judge(key)
            self.assertIs(ok, True, f'{key}: {evidence}')

    def test_each_check_fails_each_of_its_failing_journeys(self):
        self.assertEqual(set(FAILING), set(route.CHECKS))
        for key, variants in FAILING.items():
            for old, new in variants:
                ok, evidence = self.judge(key, changed(old, new))
                self.assertIs(ok, False, f'{key}, {new}: {evidence}')

    def test_the_verdict_names_each_check_not_passed(self):
        report = route.report(route.evaluate(lines(FIXTURE), {'ids': IDS}))
        passed = [['CHECK', key, 'PASS'] for key in route.CHECKS]
        self.assertEqual([line.split()[:3] for line in report[:13]], passed)
        self.assertTrue(report[0].startswith('CHECK R1 PASS a real registration is discoverable: '))
        self.assertEqual(report[13:], ['ROUTE: PASS -- all 13 checks passed'])
        slow = lines(changed(WARM, runs('warm', [2.0] * 5)))
        skipped = route.evaluate(slow, {'ids': IDS, 'skip': {'H1': 'x'}})
        self.assertEqual(skipped['H1'], ('SKIP', 'x'))
        self.assertEqual(route.report(skipped)[-1], 'ROUTE: FAIL -- 2 of 13 checks failed: H1 F1')

    def test_the_fast_loop_builds_nothing_without_a_scratch_target_outside_the_checkout(self):
        calls = self.cargo()
        os.symlink(self.checkout, self.tmp / 'link')
        inside = (None, '', self.checkout, self.checkout / 'node' / 'target', self.tmp / 'link' / 'target')
        for target in inside:
            self.assertTrue(route.refuse_target(target and str(target), self.checkout), target)
            route.fast_path(self.log, target and str(target), self.checkout)
        self.assertIsNone(route.refuse_target(str(self.tmp / 'target'), self.checkout))
        self.assertFalse(calls.exists())  # cargo never ran
        said = [line.text for line in self.log.lines]
        self.assertEqual([text[:8] for text in said], ['not run:'] * len(inside), said)
        for key in ('F1', 'F2'):
            self.assertEqual(route.JUDGES[key](self.log.lines, {}), (None, said[0]))

    def test_the_fast_loop_builds_into_its_scratch_target_and_restores_the_touched_mtime(self):
        calls, target = self.cargo(), str(self.tmp / 'target')
        route.fast_path(self.log, target, self.checkout)
        want = f'{self.checkout} {" ".join(route.SUITE[1:])} --target-dir {target}'
        self.assertEqual(calls.read_text().splitlines(), [f'{want} --no-run {target} on']
                         + [f'{want} {target} on'] * 8)
        self.assertEqual(self.touched.read_text(), '// a journey\n')
        self.assertEqual(self.touched.stat().st_mtime_ns, 2_000_000_000)
        for key in ('F1', 'F2'):
            ok, evidence = route.JUDGES[key](self.log.lines, {})
            self.assertIs(ok, True, evidence)


    def test_a_crossing_runs_in_a_new_scratch_of_its_own_and_deletes_only_that(self):
        """Teardown deletes a host's scratch with `rm -rf`, so a crossing works in a directory of its
        own, made new under the scratch the hosts file names, and deletes only a scratch it made:
        whatever else the named one holds stays."""
        given, home = self.tmp / 'scratch', self.tmp / 'home'
        given.mkdir()
        home.mkdir()
        (given / 'kept').write_text('not the run\'s\n')
        host = {'binary': '/bin/sleep', 'scratch': str(given), 'home': str(home), 'ssh': ['sh', '-c'],
                'checkout': str(self.checkout)}
        spec = self.tmp / 'hosts.json'
        spec.write_text(json.dumps({'pi': host, 'dabeest': host}))
        places, hosts, _ = route.crossing(str(spec))
        made = {host.scratch for host in hosts.values()}
        self.assertEqual(len(made), 1, made)
        scratch = made.pop()
        self.assertEqual(os.path.dirname(scratch), str(given))
        self.assertRegex(os.path.basename(scratch), r'^glade-route-\d{8}-\d{6}-\d+$')
        journey = route.Journey(self.log, places, '')
        route.make_scratch(journey, hosts['pi'])
        with self.assertRaises(nodes.Refused):
            route.make_scratch(journey, hosts['pi'])  # never one that already exists
        unmade = route.Journey(self.log, places, '')
        problems = route.teardown(unmade)
        self.assertTrue(os.path.isdir(scratch), 'a scratch this run did not make is left')
        self.assertEqual(problems, [f'scratch not deleted: {scratch} is not one this run made'])
        self.assertEqual(route.teardown(journey), [])
        self.assertFalse(os.path.exists(scratch))
        self.assertEqual((given / 'kept').read_text(), 'not the run\'s\n')

    def test_teardown_finds_a_native_process_holding_the_scratch_as_windows_spells_it(self):
        """On dabeest /proc/<pid>/cmdline spells a native node's path arguments as `cygpath -m`
        does (seen there 2026-09-30), so teardown looks for the scratch so spelled too. This
        machine's sh stands in, `cygpath` faked on its PATH and dabeest's processes listed."""
        fakes, listed = self.tmp / 'fakes', []
        fakes.mkdir()
        (fakes / 'cygpath').write_text(f'#!/bin/sh\nrest=${{2#"{self.tmp}"}}\n' + CYGPATH)
        (fakes / 'cygpath').chmod(0o700)

        class Dabeest(nodes.Host):
            def run(self, command, stdin=None, check=True):  # what only dabeest's /proc can list
                return '\n'.join(listed) if command == route.PROCESSES else super().run(command, stdin, check)

        home, env = str(self.tmp / 'home'), {'PATH': f'{fakes}:{nodes.LOCAL_PATH["PATH"]}'}
        pi = nodes.Host('/bin/sleep', str(self.tmp / 'pi' / 'glade-route-1'), home, ssh=('sh', '-c'))
        dabeest = Dabeest('/bin/sleep', str(self.tmp / 'dab' / 'glade-route-1'), home, ssh=('sh', '-c'),
                          msys=True, env=env)
        journey = route.Journey(self.log, nodes.crossing(pi, dabeest), '')
        for host in (pi, dabeest):
            route.make_scratch(journey, host)
        native = 'E:\\bin\\glade-node.exe --name route-b --app E:/dab/glade-route-1/apps/route-b.glade 4555'
        listed.extend(['C:\\Windows\\System32\\svchost.exe -k netsvcs', native])
        self.assertEqual(route.teardown(journey), [f'left running: {native}'])
        self.assertFalse(os.path.exists(dabeest.scratch))


if __name__ == '__main__':
    unittest.main()
