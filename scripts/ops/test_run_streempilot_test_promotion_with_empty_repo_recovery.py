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
SPEC = importlib.util.spec_from_file_location("den896_empty_repo_recovery", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class EmptyRepositoryRecoveryTests(unittest.TestCase):
    def metadata(self) -> dict[str, object]:
        return {
            "id": MODULE.RECOVERABLE_EMPTY_STAGE["repository_id"],
            "full_name": MODULE.RECOVERABLE_EMPTY_STAGE["full_name"],
            "private": True,
            "visibility": "private",
            "default_branch": "main",
            "size": 0,
            "created_at": MODULE.RECOVERABLE_EMPTY_STAGE["created_at"],
        }

    def test_safe_main_ref_maps_only_exact_empty_repository_409_to_none(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        exact = RuntimeError(
            f'GitHub API 409 for GET /repos/{full_name}/git/ref/heads/main: '
            '{"message":"Git Repository is empty.","status":"409"}'
        )
        with mock.patch.object(MODULE, "ORIGINAL_MAIN_REF", side_effect=exact):
            self.assertIsNone(MODULE.safe_main_ref(full_name))

        other = RuntimeError(
            f'GitHub API 409 for GET /repos/{full_name}/git/ref/heads/main: '
            '{"message":"Repository rule conflict","status":"409"}'
        )
        with mock.patch.object(MODULE, "ORIGINAL_MAIN_REF", side_effect=other):
            with self.assertRaisesRegex(RuntimeError, "Repository rule conflict"):
                MODULE.safe_main_ref(full_name)

    def test_exact_failed_run_empty_repo_is_deleted_and_verified_absent(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        calls: list[tuple[str, str]] = []
        responses = iter(
            [
                (200, self.metadata()),
                (204, None),
                (404, None),
            ]
        )

        def fake_api(method: str, path: str, body=None):
            del body
            calls.append((method, path))
            return next(responses)

        with (
            mock.patch.object(MODULE.BASE, "api", side_effect=fake_api),
            mock.patch.object(MODULE, "safe_main_ref", return_value=None),
        ):
            self.assertEqual(
                MODULE.recover_failed_empty_stage_repository(),
                "deleted",
            )

        self.assertEqual(
            calls,
            [
                ("GET", f"/repos/{full_name}"),
                ("DELETE", f"/repos/{full_name}"),
                ("GET", f"/repos/{full_name}"),
            ],
        )

    def test_recovery_rejects_wrong_repository_id_without_delete(self) -> None:
        metadata = self.metadata()
        metadata["id"] = int(MODULE.RECOVERABLE_EMPTY_STAGE["repository_id"]) + 1
        calls: list[tuple[str, str]] = []

        def fake_api(method: str, path: str, body=None):
            del body
            calls.append((method, path))
            return 200, metadata

        with mock.patch.object(MODULE.BASE, "api", side_effect=fake_api):
            with self.assertRaisesRegex(RuntimeError, "repository id changed"):
                MODULE.recover_failed_empty_stage_repository()

        self.assertFalse(any(method == "DELETE" for method, _ in calls))

    def test_recovery_preserves_exact_main_without_delete(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        calls: list[tuple[str, str]] = []

        def fake_api(method: str, path: str, body=None):
            del body
            calls.append((method, path))
            return 200, self.metadata()

        with (
            mock.patch.object(MODULE.BASE, "api", side_effect=fake_api),
            mock.patch.object(MODULE, "safe_main_ref", return_value=expected),
        ):
            self.assertEqual(
                MODULE.recover_failed_empty_stage_repository(),
                "already-exact",
            )

        self.assertFalse(any(method == "DELETE" for method, _ in calls))
        self.assertEqual(calls, [("GET", f"/repos/{full_name}")])

    def test_recovery_rejects_any_other_nonempty_main(self) -> None:
        calls: list[tuple[str, str]] = []

        def fake_api(method: str, path: str, body=None):
            del body
            calls.append((method, path))
            return 200, self.metadata()

        with (
            mock.patch.object(MODULE.BASE, "api", side_effect=fake_api),
            mock.patch.object(MODULE, "safe_main_ref", return_value="f" * 40),
        ):
            with self.assertRaisesRegex(RuntimeError, "refusing to delete non-empty"):
                MODULE.recover_failed_empty_stage_repository()

        self.assertFalse(any(method == "DELETE" for method, _ in calls))

    def test_source_hardcodes_only_test_org_recovery_and_no_force_or_public_path(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertIn('"StreemPilot-test/streempilot-compositor.rs"', source)
        self.assertIn("1327442276", source)
        self.assertIn('"2026-08-08T05:25:37Z"', source)
        self.assertNotIn("StreemPilot/streempilot-compositor.rs\"", source)
        self.assertNotIn("git push --" + "force", source)
        self.assertNotIn("--visibility " + "public", source)
        self.assertNotIn('"private": False', source)
        self.assertNotIn("--organization", source)


if __name__ == "__main__":
    unittest.main()
