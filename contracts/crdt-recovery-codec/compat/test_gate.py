"""Missing evidence must never look like a qualified language/corpus gate."""
import json
import tempfile
import unittest
from pathlib import Path

from gate import validate, validate_remote


class PinGateTests(unittest.TestCase):
    def test_missing_pin_manifest_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                validate(Path(directory))

    def test_missing_language_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "vectors").mkdir()
            (root / "vectors/pins.json").write_text(json.dumps({"languages": {}}))
            with self.assertRaises(ValueError):
                validate(root)
    def test_pre_remote_refuses_uncommitted_candidate_content_pins(self):
        import hashlib
        import shutil
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            copied = Path(directory) / "codec"
            shutil.copytree(root, copied, ignore=shutil.ignore_patterns("__pycache__"))
            path = copied / "vectors/pins.json"
            pins = json.loads(path.read_text())
            pins["implementation_revision"] = None
            pins["status"] = "candidate-content-pins-awaiting-parent-commit"
            pins["crypto_qualification"] = None
            pins["files"] = {name: hashlib.sha256((copied / name).read_bytes()).hexdigest() for name in pins["files"]}
            path.write_text(json.dumps(pins))
            validate(copied)
            with self.assertRaises(ValueError):
                validate(copied, strict=True)

    def test_candidate_content_can_be_checked_but_nonhex_revision_refuses(self):
        import shutil
        root = Path(__file__).resolve().parents[1]
        validate(root)
        with tempfile.TemporaryDirectory() as directory:
            copied = Path(directory) / "codec"
            shutil.copytree(root, copied, ignore=shutil.ignore_patterns("__pycache__"))
            path = copied / "vectors/pins.json"
            pins = json.loads(path.read_text())
            pins["implementation_revision"] = "z" * 40
            path.write_text(json.dumps(pins))
            with self.assertRaisesRegex(ValueError, "implementation revision"):
                validate(copied)

    def test_nonexistent_commit_and_absent_crypto_evidence_refuse_remote(self):
        import hashlib
        from unittest.mock import patch
        root = Path(__file__).resolve().parents[1]
        pins = json.loads((root / "vectors/pins.json").read_text())
        pins["implementation_revision"] = "0" * 40
        pins["status"] = "committed-content-pins"
        pins["crypto_qualification"] = None
        pins["files"] = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in pins["files"]}
        with self.assertRaisesRegex(ValueError, "nonexistent committed source pin"):
            validate_remote(root, pins)
        with patch("gate.committed_bytes", side_effect=lambda repository, revision, path: (root / path.removeprefix("contracts/crdt-recovery-codec/")).read_bytes()):
            with self.assertRaisesRegex(ValueError, "genuine B1 crypto qualification absent"):
                validate_remote(root, pins)

    def test_python_and_shell_options_are_closed_before_qualification(self):
        import hashlib
        import shutil
        import subprocess
        import sys
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            copied = Path(directory) / "codec"
            shutil.copytree(root, copied, ignore=shutil.ignore_patterns("__pycache__"))
            # Prevent a regressed legacy shell driver from recursively running this test.
            (copied / "compat/test_gate.py").write_text("raise SystemExit(0)\n")
            path = copied / "vectors/pins.json"
            pins = json.loads(path.read_text())
            pins["files"] = {name: hashlib.sha256((copied / name).read_bytes()).hexdigest() for name in pins["files"]}
            path.write_text(json.dumps(pins))
            for arguments in (["--unknown"], ["--pre-remtoe"], ["--pre-remote", "EXTRA"]):
                for command in ([sys.executable, str(copied / "compat/gate.py"), str(copied)], ["/bin/sh", str(copied / "check-vectors.sh")]):
                    with self.subTest(arguments=arguments, command=command):
                        result = subprocess.run(command + arguments, capture_output=True, text=True, env={})
                        self.assertEqual(result.returncode, 2, command + arguments)
                        self.assertIn("usage:", result.stderr)



if __name__ == "__main__":
    unittest.main()
