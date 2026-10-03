"""Regression checks for the selected fast loop and ALL-member framework guard."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
ENV = {key: os.environ[key] for key in ('PATH', 'HOME', 'CARGO_HOME', 'RUSTUP_HOME') if key in os.environ}


class ContractToolingTests(unittest.TestCase):
    def test_new_pure_selection_is_explicit_and_all_keeps_contracts(self):
        result = subprocess.run(['sh', str(ROOT / 'check.sh'), '--list', 'crdt-admission'], capture_output=True, text=True, env=ENV)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ['glade-crdt-storage-attempt-api', 'glade-crdt-admission-core'])
        selected = subprocess.run(['sh', str(ROOT / 'check.sh'), '--list', 'all'], capture_output=True, text=True, env=ENV)
        self.assertEqual(len(selected.stdout.splitlines()), 12)
        self.assertIn('glade-signer-api', selected.stdout.splitlines())

    def test_storage_selector_includes_affected_pure_consumer(self):
        selected = subprocess.run(['sh', str(ROOT / 'check.sh'), '--list', 'crdt-storage-attempt'], capture_output=True, text=True, env=ENV)
        self.assertEqual(selected.returncode, 0, selected.stderr)
        self.assertEqual(selected.stdout.splitlines(), ['glade-crdt-storage-attempt-api', 'glade-crdt-admission-core'])

    def test_framework_guard_handles_pure_member_and_keeps_exact_refusals(self):
        with tempfile.TemporaryDirectory(prefix='icd-tooling-') as temporary:
            work = Path(temporary)
            copy = work / 'glade/contracts'
            shutil.copytree(ROOT, copy, ignore=shutil.ignore_patterns('target'))
            (work / 'glade-decl-rs').symlink_to(ROOT.parents[1] / 'glade-decl-rs', target_is_directory=True)
            (work / 'glade/wire-rs').symlink_to(ROOT.parent / 'wire-rs', target_is_directory=True)
            (work / 'glade-discover').symlink_to(ROOT.parents[1] / 'glade-discover', target_is_directory=True)
            fixture = copy / 'fixture-pure'
            (fixture / 'src').mkdir(parents=True)
            (fixture / 'src/lib.rs').write_text('pub fn identity(value: u8) -> u8 { value }\n')
            (fixture / 'Cargo.toml').write_text('[package]\nname = "glade-fixture-pure"\nversion="0.0.0"\nedition="2024"\n')
            manifest = copy / 'Cargo.toml'
            manifest.write_text(manifest.read_text().replace('members = [', 'members = ["fixture-pure", '))
            policy = copy / 'architecture-policy.json'
            data = json.loads(policy.read_text())
            data['packages']['glade-fixture-pure'] = {'role': 'pure', 'reason': 'deterministic identity regression fixture', 'dependencies': []}
            policy.write_text(json.dumps(data))
            lock = subprocess.run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)], capture_output=True, text=True, env=ENV)
            self.assertEqual(lock.returncode, 0, lock.stderr)
            guarded = subprocess.run(['sh', str(copy / 'arch002-fixture.sh')], capture_output=True, text=True, env=ENV)
            self.assertEqual(guarded.returncode, 0, guarded.stdout + guarded.stderr)
            self.assertIn('ARCH-002 fixture: the gate refused', guarded.stdout)
            self.assertIn('glade-fixture-pure', guarded.stdout)
            self.assertIn('all 13 classified libraries', guarded.stdout)
            self.assertIn('glade-crdt-storage-attempt-api: undeclared dependency normal:shaku', guarded.stdout)
            self.assertNotIn('cargo metadata failed', guarded.stdout)


if __name__ == '__main__':
    unittest.main()
