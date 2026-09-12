#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import unittest
from copy import deepcopy
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VERIFIER_PATH = ROOT / "scripts/local-k8s/verify_evidence.py"
SPEC = importlib.util.spec_from_file_location("verify_evidence", VERIFIER_PATH)
assert SPEC is not None and SPEC.loader is not None
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)

PROFILE_PATH = ROOT / "contracts/local-k8s-platform/instances/RuntimeProfile/valid/ci-kind.json"
EVIDENCE_PATH = ROOT / "contracts/local-k8s-platform/instances/RuntimeEvidence/valid/ci-kind.json"


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


class RuntimeEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.profile = VERIFY.VERIFY_PROFILE.load_profile(PROFILE_PATH)
        self.evidence = read_json(EVIDENCE_PATH)
        self.digest = VERIFY.profile_digest(PROFILE_PATH)

    def validate(self, evidence: dict) -> dict:
        return VERIFY.validate_evidence(
            self.profile,
            evidence,
            expected_profile_sha256=self.digest,
        )

    def test_checked_in_evidence_passes(self) -> None:
        result = self.validate(self.evidence)
        self.assertEqual(result["runtime"], "docker-kind")
        self.assertEqual(result["profileSha256"], self.digest)

    def test_profile_digest_drift_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["profileSha256"] = "0" * 64
        with self.assertRaisesRegex(VERIFY.EvidenceError, "exact RuntimeProfile bytes"):
            self.validate(evidence)

    def test_abbreviated_source_revision_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["sourceSha"] = "deadbeef"
        with self.assertRaisesRegex(VERIFY.EvidenceError, "40-character Git revision"):
            self.validate(evidence)

    def test_runtime_drift_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["runtime"] = "colima-kind"
        with self.assertRaisesRegex(VERIFY.EvidenceError, "runtime does not match"):
            self.validate(evidence)

    def test_secret_read_capability_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["rbacGetSecrets"] = True
        with self.assertRaisesRegex(VERIFY.EvidenceError, "rbacGetSecrets"):
            self.validate(evidence)

    def test_provider_write_evidence_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["providerWrites"] = True
        with self.assertRaisesRegex(VERIFY.EvidenceError, "providerWrites"):
            self.validate(evidence)

    def test_secret_input_evidence_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["secretInputs"] = True
        with self.assertRaisesRegex(VERIFY.EvidenceError, "secretInputs"):
            self.validate(evidence)

    def test_failed_server_admission_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["serverAdmissionPassed"] = False
        with self.assertRaisesRegex(VERIFY.EvidenceError, "serverAdmissionPassed"):
            self.validate(evidence)

    def test_extra_credential_field_is_rejected(self) -> None:
        evidence = deepcopy(self.evidence)
        evidence["providerToken"] = "never-allowed"
        with self.assertRaisesRegex(VERIFY.EvidenceError, "evidence keys differ"):
            self.validate(evidence)


if __name__ == "__main__":
    unittest.main()
