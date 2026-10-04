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
        root = Path(__file__).resolve().parents[1]
        with self.assertRaises(ValueError):
            validate(root, strict=True)

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
        from unittest.mock import patch
        root = Path(__file__).resolve().parents[1]
        pins = json.loads((root / "vectors/pins.json").read_text())
        pins["implementation_revision"] = "0" * 40
        pins["status"] = "committed-content-pins"
        with self.assertRaisesRegex(ValueError, "nonexistent committed source pin"):
            validate_remote(root, pins)
        with patch("gate.committed_bytes", side_effect=lambda repository, revision, path: (root / path.removeprefix("contracts/crdt-recovery-codec/")).read_bytes()):
            with self.assertRaisesRegex(ValueError, "genuine B1 crypto qualification absent"):
                validate_remote(root, pins)



if __name__ == "__main__":
    unittest.main()
