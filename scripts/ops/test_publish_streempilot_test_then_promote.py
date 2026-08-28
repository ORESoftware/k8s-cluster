#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts/ops/publish_streempilot_test_then_promote.py"
os.environ.setdefault("GH_TOKEN", "unit-test-installation-token")
SPEC = importlib.util.spec_from_file_location("streempilot_test_then_promote", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


SEALED = {
    "streempilot-compositor.rs": "ea7c1c8042122b4e4a7689aee026113fb607421d",
    "streempilot-destinations": "acaff0ab1fcbbae82eb52c72120a6edb10243a77",
    "streempilot-recording.rs": "8145e4717a9906c759ada560bff28adbda68667c",
    "streempilot-webrtc-adapter.rs": "471360a2ceba65432e542f5ef58caac83b645210",
}


def record(name: str) -> dict[str, object]:
    return {
        "org": "streempilot",
        "name": name,
        "full_name": f"streempilot/{name}",
        "commit": SEALED[name],
        "default_branch": "main",
        "description": f"sealed {name}",
    }


class StreemPilotTestThenPromoteTests(unittest.TestCase):
    def setUp(self) -> None:
        self.records = [record(name) for name in SEALED]

    def test_exact_four_canonical_records_are_selected_in_reviewed_order(self) -> None:
        noise = {
            "org": "streempilot",
            "name": "streempilot-api-server.rs",
            "full_name": "streempilot/streempilot-api-server.rs",
            "commit": "a" * 40,
            "default_branch": "main",
        }
        selected = MODULE.canonical_records([noise, *reversed(self.records)])
        self.assertEqual(
            [MODULE.canonical_full_name(item) for item in selected],
            list(MODULE.EXPECTED_REPOSITORIES),
        )

    def test_target_mapping_is_fixed_and_cannot_be_redirected(self) -> None:
        compositor = record("streempilot-compositor.rs")
        self.assertEqual(
            MODULE.target_full_name(compositor, "stage"),
            "StreemPilot-test/streempilot-compositor.rs",
        )
        self.assertEqual(
            MODULE.target_full_name(compositor, "production"),
            "StreemPilot/streempilot-compositor.rs",
        )
        with self.assertRaisesRegex(RuntimeError, "invalid target"):
            MODULE.target_organization("other")

    def test_selection_rejects_cross_org_or_commit_drift(self) -> None:
        cross_org = self.records.copy()
        cross_org[0] = dict(cross_org[0])
        cross_org[0]["full_name"] = "other/streempilot-compositor.rs"
        with self.assertRaisesRegex(RuntimeError, "missing from sealed manifest"):
            MODULE.canonical_records(cross_org)

        invalid_sha = self.records.copy()
        invalid_sha[1] = dict(invalid_sha[1])
        invalid_sha[1]["commit"] = "A" * 40
        with self.assertRaisesRegex(RuntimeError, "invalid sealed SHA"):
            MODULE.canonical_records(invalid_sha)

    def test_stage_evidence_must_cover_same_four_private_main_shas(self) -> None:
        rows = []
        for item in self.records:
            canonical = MODULE.canonical_full_name(item)
            rows.append(
                {
                    "canonical_full_name": canonical,
                    "target_full_name": MODULE.target_full_name(item, "stage"),
                    "repository_id": 1000 + len(rows),
                    "visibility": "private",
                    "default_branch": "main",
                    "main_sha": item["commit"],
                    "expected_sealed_sha": item["commit"],
                    "disposition": "created-or-reconciled",
                }
            )
        document = {
            "schema_version": 1,
            "target": "stage",
            "target_organization": MODULE.STAGE_ORGANIZATION,
            "canonical_organization": MODULE.CANONICAL_ORGANIZATION,
            "sealed_source_repository": MODULE.EXACT.FLEET_SOURCE_REPOSITORY,
            "sealed_source_sha": MODULE.EXACT.FLEET_SOURCE_SHA,
            "repositories": rows,
        }
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "stage.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            parsed = MODULE.validate_stage_evidence(path, self.records)
            self.assertEqual(parsed["target"], "stage")

            mutated = dict(document)
            mutated["repositories"] = [dict(row) for row in rows]
            mutated["repositories"][2]["main_sha"] = "f" * 40
            path.write_text(json.dumps(mutated), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "stage SHA mismatch"):
                MODULE.validate_stage_evidence(path, self.records)

    def test_source_has_no_public_creation_force_push_or_arbitrary_target(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertNotIn('"private": False', source)
        self.assertNotIn("git push --" + "force", source)
        self.assertNotIn("--visibility " + "public", source)
        self.assertNotIn("parser.add_argument(\"--organization\"", source)
        self.assertIn("STAGE_ORGANIZATION = \"StreemPilot-test\"", source)
        self.assertIn("PRODUCTION_ORGANIZATION = \"StreemPilot\"", source)
        self.assertIn("production promotion requires --stage-evidence", source)
        self.assertIn("ensure_private_repository", source)

    def test_production_gate_rejects_stage_receipt_for_wrong_target_org(self) -> None:
        rows = []
        for item in self.records:
            rows.append(
                {
                    "canonical_full_name": MODULE.canonical_full_name(item),
                    "target_full_name": MODULE.target_full_name(item, "stage"),
                    "repository_id": 10,
                    "visibility": "private",
                    "default_branch": "main",
                    "main_sha": item["commit"],
                    "expected_sealed_sha": item["commit"],
                }
            )
        document = {
            "schema_version": 1,
            "target": "stage",
            "target_organization": "StreemPilot",
            "sealed_source_repository": MODULE.EXACT.FLEET_SOURCE_REPOSITORY,
            "sealed_source_sha": MODULE.EXACT.FLEET_SOURCE_SHA,
            "repositories": rows,
        }
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "stage.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "stage evidence organization mismatch"):
                MODULE.validate_stage_evidence(path, self.records)


if __name__ == "__main__":
    unittest.main()
