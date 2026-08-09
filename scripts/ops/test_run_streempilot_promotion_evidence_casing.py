#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import sys
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts/ops/run_streempilot_test_promotion_with_empty_repo_recovery.py"
os.environ.setdefault("GH_TOKEN", "unit-test-installation-token")
SPEC = importlib.util.spec_from_file_location(
    "den896_promotion_evidence_casing",
    MODULE_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class PromotionEvidenceCasingTests(unittest.TestCase):
    def record(self, name: str = "streempilot-compositor.rs") -> dict[str, object]:
        return {
            "org": "streempilot",
            "name": name,
            "full_name": f"streempilot/{name}",
            "commit": "a" * 40,
            "default_branch": "main",
            "description": f"sealed {name}",
        }

    def test_normalizes_only_owner_case_after_base_validation(self) -> None:
        record = self.record()
        original_row = {
            "canonical_full_name": "StreemPilot/streempilot-compositor.rs",
            "target_full_name": "streempilot-test/streempilot-compositor.rs",
            "repository_id": 1327442276,
            "visibility": "private",
            "default_branch": "main",
            "main_sha": "a" * 40,
            "expected_sealed_sha": "a" * 40,
        }
        with mock.patch.object(
            MODULE,
            "ORIGINAL_PUBLISH_ONE",
            return_value=original_row,
        ) as publish_one:
            result = MODULE.canonical_evidence_publish_one(
                Path("/tmp/sealed"),
                record,
                "stage",
            )

        publish_one.assert_called_once_with(Path("/tmp/sealed"), record, "stage")
        self.assertEqual(
            result["target_full_name"],
            "StreemPilot-test/streempilot-compositor.rs",
        )
        self.assertEqual(
            original_row["target_full_name"],
            "streempilot-test/streempilot-compositor.rs",
        )

    def test_normalizes_production_owner_case(self) -> None:
        record = self.record("streempilot-recording.rs")
        with mock.patch.object(
            MODULE,
            "ORIGINAL_PUBLISH_ONE",
            return_value={
                "target_full_name": "streempilot/streempilot-recording.rs",
                "repository_id": 42,
                "visibility": "private",
                "default_branch": "main",
                "main_sha": "a" * 40,
                "expected_sealed_sha": "a" * 40,
            },
        ):
            result = MODULE.canonical_evidence_publish_one(
                Path("/tmp/sealed"),
                record,
                "production",
            )

        self.assertEqual(
            result["target_full_name"],
            "StreemPilot/streempilot-recording.rs",
        )

    def test_rejects_repository_name_drift(self) -> None:
        record = self.record()
        with mock.patch.object(
            MODULE,
            "ORIGINAL_PUBLISH_ONE",
            return_value={
                "target_full_name": "streempilot-test/streempilot-recording.rs",
            },
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "escaped the exact repository identity",
            ):
                MODULE.canonical_evidence_publish_one(
                    Path("/tmp/sealed"),
                    record,
                    "stage",
                )

    def test_rejects_missing_target_identity(self) -> None:
        with mock.patch.object(
            MODULE,
            "ORIGINAL_PUBLISH_ONE",
            return_value={},
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "escaped the exact repository identity",
            ):
                MODULE.canonical_evidence_publish_one(
                    Path("/tmp/sealed"),
                    self.record(),
                    "stage",
                )


if __name__ == "__main__":
    unittest.main()
