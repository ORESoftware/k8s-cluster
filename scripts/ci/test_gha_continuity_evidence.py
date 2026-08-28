#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("gha_continuity_evidence.py")
SPEC = importlib.util.spec_from_file_location("gha_continuity_evidence", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

REVISION = "0123456789abcdef0123456789abcdef01234567"
PLAN_ID = "sha256:" + "a" * 64
DIGEST = "sha256:" + "b" * 64


def evidence(lane: str, *, status: str = "succeeded", digest: str = DIGEST):
    return MODULE.parse_evidence(
        {
            "schemaVersion": MODULE.SCHEMA_VERSION,
            "lane": lane,
            "repository": "ORESoftware/k8s-cluster",
            "revision": REVISION,
            "workflowPath": ".github/workflows/gha-continuity-parity.yml",
            "planId": PLAN_ID,
            "status": status,
            "artifacts": {"workflow-plan.json": digest},
        }
    )


class EvidenceValidationTests(unittest.TestCase):
    def test_exact_four_lane_parity(self) -> None:
        report = MODULE.compare(
            [evidence(lane) for lane in sorted(MODULE.LANES)],
            MODULE.LANES,
        )
        self.assertTrue(report["parity"])
        self.assertEqual(report["lanes"], sorted(MODULE.LANES))
        self.assertEqual(report["mismatches"], [])

    def test_status_and_artifact_mismatches_are_both_reported(self) -> None:
        report = MODULE.compare(
            [
                evidence("hosted"),
                evidence("arc-aws", status="failed", digest="sha256:" + "c" * 64),
            ],
            {"hosted", "arc-aws"},
        )
        self.assertFalse(report["parity"])
        self.assertEqual(
            {mismatch["kind"] for mismatch in report["mismatches"]},
            {"status", "artifacts"},
        )

    def test_duplicate_and_missing_lanes_fail_closed(self) -> None:
        with self.assertRaisesRegex(MODULE.EvidenceError, "duplicate evidence"):
            MODULE.compare([evidence("hosted"), evidence("hosted")], {"hosted"})
        with self.assertRaisesRegex(MODULE.EvidenceError, "missing required lanes"):
            MODULE.compare(
                [evidence("hosted"), evidence("independent")],
                {"hosted", "arc-aws"},
            )

    def test_mutable_or_malformed_identity_is_rejected(self) -> None:
        raw = evidence("hosted").to_json()
        raw["revision"] = "main"
        with self.assertRaisesRegex(MODULE.EvidenceError, "40-hex"):
            MODULE.parse_evidence(raw)
        raw = evidence("hosted").to_json()
        raw["workflowPath"] = "../ci.yml"
        with self.assertRaisesRegex(MODULE.EvidenceError, "workflowPath"):
            MODULE.parse_evidence(raw)

    def test_unexpected_or_secret_bearing_fields_are_rejected(self) -> None:
        raw = evidence("hosted").to_json()
        raw["notes"] = "not part of the stable schema"
        with self.assertRaisesRegex(MODULE.EvidenceError, "unexpected evidence fields"):
            MODULE.parse_evidence(raw)
        raw = evidence("hosted").to_json()
        raw["githubToken"] = "redacted"
        with self.assertRaisesRegex(MODULE.EvidenceError, "forbidden"):
            MODULE.parse_evidence(raw)

    def test_emit_hashes_files_and_round_trips(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            artifact = root / "plan.json"
            artifact.write_text('{"plan":"stable"}\n', encoding="utf-8")
            output = root / "hosted.json"
            status = MODULE.main(
                [
                    "emit",
                    "--lane",
                    "hosted",
                    "--repository",
                    "ORESoftware/k8s-cluster",
                    "--revision",
                    REVISION,
                    "--workflow-path",
                    ".github/workflows/gha-continuity-parity.yml",
                    "--plan-id",
                    PLAN_ID,
                    "--status",
                    "succeeded",
                    "--artifact",
                    f"workflow-plan.json={artifact}",
                    "--output",
                    str(output),
                ]
            )
            self.assertEqual(status, 0)
            parsed = MODULE.load_evidence(output)
            self.assertEqual(parsed.lane, "hosted")
            self.assertEqual(parsed.artifact_map()["workflow-plan.json"], MODULE.sha256_file(artifact))
            self.assertEqual(json.loads(output.read_text())["schemaVersion"], MODULE.SCHEMA_VERSION)


if __name__ == "__main__":
    unittest.main()
