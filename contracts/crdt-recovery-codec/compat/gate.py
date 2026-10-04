"""Fail-closed IC3 representation corpus pins; genuine crypto is a later gate."""
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

REQUIRED = {
    "IdentityBinding", "CreationIntent", "SignedDescriptorBody", "CertificateBody",
    "PermitBody", "PolicyBody", "OperationProofBody", "PermitWrapper",
    "LocalChallengeBody", "LocalResponseBody", "OriginalAdmissionBody",
    "InventoryHeaderBody", "ExchangeHello", "Inventory", "InventoryPage",
    "TransferBundle", "RecordRef", "SignedRecord",
}
LANGUAGES = {"rust", "typescript", "python"}


def validate(root: Path, strict: bool = False) -> None:
    try:
        pins = json.loads((root / "vectors/pins.json").read_text())
        if set(pins["languages"]) != LANGUAGES or pins["version"] != 1:
            raise ValueError("language/version pins")
        if pins["implementation_revision"] is None:
            if pins["status"] != "candidate-content-pins-awaiting-parent-commit":
                raise ValueError("uncommitted candidate status")
        elif not re.fullmatch(r"[0-9a-f]{40}", pins["implementation_revision"]):
            raise ValueError("implementation revision")
        files = pins["files"]
        required_files = {
            "vectors/schema.json", "vectors/input.json", "vectors/expected.json",
            "vectors/negative.json", "compat/python.py", "compat/typescript.ts",
            "tests/vectors.rs", "src/profile.rs", "src/proofs.rs", "src/values.rs",
        }
        if not required_files.issubset(files):
            raise ValueError("missing source pin")
        for name, digest in files.items():
            path = root / name
            if not path.resolve().is_relative_to(root.resolve()):
                raise ValueError("source path escape")
            if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                raise ValueError("source pin mismatch: " + name)
        cases = json.loads((root / "vectors/input.json").read_text())["cases"]
        expected = json.loads((root / "vectors/expected.json").read_text())
        if {case["kind"] for case in cases} != REQUIRED or len(cases) != 21:
            raise ValueError("missing category/assertion")
        if len({case["id"] for case in cases}) != 21 or set(expected) != {case["id"] for case in cases}:
            raise ValueError("missing exact expected assertion")
        negatives = json.loads((root / "vectors/negative.json").read_text())["cases"]
        if len(negatives) != 18 or len({case["id"] for case in negatives}) != 18:
            raise ValueError("missing negative control")
        if strict:
            validate_remote(root, pins)

    except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
        raise ValueError("missing/invalid representation evidence") from exc


def committed_bytes(repository: Path, revision: str, path: str) -> bytes:
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("invalid commit pin")
    if Path(path).is_absolute() or ".." in Path(path).parts:
        raise ValueError("committed source path escape")
    result = subprocess.run(
        ["/usr/bin/git", "-C", str(repository), "show", revision + ":" + path],
        env={}, capture_output=True, check=False,
    )
    if result.returncode:
        raise ValueError("nonexistent committed source pin")
    return result.stdout


def validate_remote(root: Path, pins: dict) -> None:
    revision = pins["implementation_revision"]
    if revision is None or pins["status"] != "committed-content-pins":
        raise ValueError("pre-remote gate refuses uncommitted candidate")
    repository = root.parents[1]
    for name, digest in pins["files"].items():
        raw = committed_bytes(repository, revision, "contracts/crdt-recovery-codec/" + name)
        if hashlib.sha256(raw).hexdigest() != digest:
            raise ValueError("implementation commit content mismatch")
    qualification = pins.get("crypto_qualification")
    if not isinstance(qualification, dict):
        raise ValueError("genuine B1 crypto qualification absent")
    raw = committed_bytes(repository, qualification["evidence_revision"], qualification["path"])
    if hashlib.sha256(raw).hexdigest() != qualification["sha256"]:
        raise ValueError("B1 evidence content pin")
    evidence = json.loads(raw)
    witnesses = {
        "genuine-provider-positive", "bad-signature", "wrong-domain", "revoked",
        "expired", "time-uncertain", "local-session-replay", "historical-admission",
    }
    if (evidence["phase"] != "IC3-B1" or evidence["status"] != "accepted"
            or evidence["implementation_revision"] != revision
            or not witnesses.issubset(evidence["witnesses"])
            or not re.fullmatch(r"[0-9a-f]{40}", evidence["accepted_review_record"])
            or evidence["accepted_review_record"] != qualification["accepted_review_record"]):
        raise ValueError("genuine B1 accepted qualification incomplete")


if __name__ == "__main__":
    strict = len(sys.argv) == 3 and sys.argv[2] == "--pre-remote"
    validate(Path(sys.argv[1]), strict=strict)
    print("Pre-remote committed corpus/B1 accepted-evidence gate PASS" if strict else "Candidate content representation PASS; unqualified for remote use until committed pin and accepted B1 crypto")
