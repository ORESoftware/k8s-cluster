#!/usr/bin/env python3
"""Fail-closed admission for evidence emitted by a local Kubernetes runtime.

RuntimeEvidence is downstream evidence bound to one exact RuntimeProfile. The
profile authorities remain TypeSpec and independently authored Draft 2020-12
JSON Schema; this verifier only enforces cross-object/runtime invariants that
cannot be expressed by either one-object schema alone.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import re
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
PROFILE_VERIFIER_PATH = ROOT / "scripts/local-k8s/verify_profile.py"
SPEC = importlib.util.spec_from_file_location("verify_profile", PROFILE_VERIFIER_PATH)
assert SPEC is not None and SPEC.loader is not None
VERIFY_PROFILE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY_PROFILE)

EVIDENCE_SCHEMA = "ores.local-k8s-runtime-evidence/v1"
EVIDENCE_SOURCES = {"github-actions-kind", "local-kind"}
EXPECTED_KEYS = {
    "schema",
    "profileName",
    "profileSha256",
    "evidenceSource",
    "sourceSha",
    "runtime",
    "architecture",
    "resourceClass",
    "topology",
    "kubernetesVersion",
    "runtimeArtifact",
    "apiServerReady",
    "serverAdmissionPassed",
    "rbacListPods",
    "rbacGetSecrets",
    "providerWrites",
    "secretInputs",
    "cloudMutationPolicy",
    "cloudWrites",
}
SHA40_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
PROFILE_FIELDS = (
    "runtime",
    "architecture",
    "resourceClass",
    "topology",
    "kubernetesVersion",
    "runtimeArtifact",
    "cloudMutationPolicy",
    "cloudWrites",
)


class EvidenceError(ValueError):
    """A deterministic runtime-evidence rejection."""


def _require_string(evidence: dict[str, Any], key: str) -> str:
    value = evidence.get(key)
    if not isinstance(value, str) or not value:
        raise EvidenceError(f"{key} must be a non-empty string")
    return value


def profile_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_evidence(
    profile: dict[str, Any],
    evidence: Any,
    *,
    expected_profile_sha256: str,
) -> dict[str, Any]:
    if not isinstance(evidence, dict):
        raise EvidenceError("evidence must be a JSON object")

    keys = set(evidence)
    missing = sorted(EXPECTED_KEYS - keys)
    extra = sorted(keys - EXPECTED_KEYS)
    if missing or extra:
        raise EvidenceError(f"evidence keys differ: missing={missing} extra={extra}")

    if _require_string(evidence, "schema") != EVIDENCE_SCHEMA:
        raise EvidenceError("unsupported evidence schema")
    if _require_string(evidence, "profileName") != profile["name"]:
        raise EvidenceError("profileName does not match the admitted RuntimeProfile")

    declared_profile_digest = _require_string(evidence, "profileSha256")
    if SHA256_RE.fullmatch(declared_profile_digest) is None:
        raise EvidenceError("profileSha256 must be a lowercase 64-character SHA-256 digest")
    if declared_profile_digest != expected_profile_sha256:
        raise EvidenceError("profileSha256 does not match the exact RuntimeProfile bytes")

    evidence_source = _require_string(evidence, "evidenceSource")
    if evidence_source not in EVIDENCE_SOURCES:
        raise EvidenceError(f"unsupported evidenceSource: {evidence_source}")

    source_sha = _require_string(evidence, "sourceSha")
    if SHA40_RE.fullmatch(source_sha) is None:
        raise EvidenceError("sourceSha must be an immutable lowercase 40-character Git revision")

    for key in PROFILE_FIELDS:
        if evidence.get(key) != profile.get(key):
            raise EvidenceError(f"{key} does not match the admitted RuntimeProfile")

    required_true = ("apiServerReady", "serverAdmissionPassed", "rbacListPods")
    for key in required_true:
        if evidence.get(key) is not True:
            raise EvidenceError(f"{key} must be the boolean true")

    required_false = (
        "rbacGetSecrets",
        "providerWrites",
        "secretInputs",
        "cloudWrites",
    )
    for key in required_false:
        if evidence.get(key) is not False:
            raise EvidenceError(f"{key} must be the boolean false")

    if evidence.get("cloudMutationPolicy") != "denied":
        raise EvidenceError("cloudMutationPolicy must be denied")

    return evidence


def load_evidence(profile_path: Path, evidence_path: Path) -> dict[str, Any]:
    try:
        profile = VERIFY_PROFILE.load_profile(profile_path)
        evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
    except VERIFY_PROFILE.ProfileError as exc:
        raise EvidenceError(f"RuntimeProfile rejected: {exc}") from exc
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"unable to read evidence: {exc.__class__.__name__}") from exc
    return validate_evidence(
        profile,
        evidence,
        expected_profile_sha256=profile_digest(profile_path),
    )


def main() -> int:
    if len(sys.argv) != 1:
        print(
            "verify_evidence.py accepts no command-line arguments; set LOCAL_K8S_PROFILE and LOCAL_K8S_EVIDENCE",
            file=sys.stderr,
        )
        return 2

    raw_profile = os.environ.get("LOCAL_K8S_PROFILE", "")
    raw_evidence = os.environ.get("LOCAL_K8S_EVIDENCE", "")
    if not raw_profile or not raw_evidence:
        print("LOCAL_K8S_PROFILE and LOCAL_K8S_EVIDENCE are required", file=sys.stderr)
        return 2

    try:
        evidence = load_evidence(Path(raw_profile), Path(raw_evidence))
    except EvidenceError as exc:
        print(f"local Kubernetes evidence rejected: {exc}", file=sys.stderr)
        return 2

    print(
        json.dumps(
            {
                "ok": True,
                "profileName": evidence["profileName"],
                "evidenceSource": evidence["evidenceSource"],
                "sourceSha": evidence["sourceSha"],
                "runtime": evidence["runtime"],
                "topology": evidence["topology"],
                "providerWrites": False,
                "secretInputs": False,
                "rbacGetSecrets": False,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
