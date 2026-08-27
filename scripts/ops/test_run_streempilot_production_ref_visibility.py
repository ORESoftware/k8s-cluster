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
    "den896_production_ref_visibility",
    MODULE_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class ProductionRefVisibilityTests(unittest.TestCase):
    def setUp(self) -> None:
        MODULE._CURRENT_TARGET = None
        MODULE._RECOVERABLE_EMPTY_APPROVED = False
        MODULE._CURRENT_RUN_STAGE_CREATIONS.clear()
        MODULE._CURRENT_RUN_PRODUCTION_CREATIONS.clear()

    @staticmethod
    def target(name: str = "streempilot-compositor.rs") -> str:
        return f"StreemPilot/{name}"

    def test_records_only_absent_exact_production_target_in_current_run(self) -> None:
        full_name = self.target()
        expected = "a" * 40
        MODULE._CURRENT_TARGET = "production"

        result = MODULE._record_current_stage_creation(full_name, expected, None)

        self.assertIsNone(result)
        self.assertIn(
            MODULE._stage_creation_key(full_name, expected),
            MODULE._CURRENT_RUN_PRODUCTION_CREATIONS,
        )
        self.assertEqual(MODULE._CURRENT_RUN_STAGE_CREATIONS, set())

    def test_existing_production_repository_is_never_recorded_for_retry(self) -> None:
        full_name = self.target()
        expected = "a" * 40
        existing = {"full_name": full_name, "main_sha": expected}
        MODULE._CURRENT_TARGET = "production"

        result = MODULE._record_current_stage_creation(
            full_name,
            expected,
            existing,
        )

        self.assertIs(result, existing)
        self.assertEqual(MODULE._CURRENT_RUN_PRODUCTION_CREATIONS, set())

    def test_rejects_repository_outside_exact_production_allowlist(self) -> None:
        MODULE._CURRENT_TARGET = "production"
        with self.assertRaisesRegex(
            RuntimeError,
            "outside exact production allowlist",
        ):
            MODULE._record_current_stage_creation(
                "StreemPilot/unreviewed-repository",
                "a" * 40,
                None,
            )

    def test_retry_kind_requires_exact_same_run_production_creation(self) -> None:
        full_name = self.target("streempilot-destinations")
        expected = "b" * 40
        MODULE._CURRENT_TARGET = "production"

        self.assertIsNone(MODULE._post_push_retry_kind(full_name, expected))

        MODULE._CURRENT_RUN_PRODUCTION_CREATIONS.add(
            MODULE._stage_creation_key(full_name, expected)
        )
        self.assertEqual(
            MODULE._post_push_retry_kind(full_name, expected),
            "current-run-production-creation",
        )
        self.assertIsNone(
            MODULE._post_push_retry_kind(full_name, "c" * 40)
        )

    def test_bounded_retry_accepts_eventually_visible_exact_production_ref(self) -> None:
        full_name = self.target("streempilot-recording.rs")
        expected = "c" * 40
        MODULE._CURRENT_TARGET = "production"
        MODULE._CURRENT_RUN_PRODUCTION_CREATIONS.add(
            MODULE._stage_creation_key(full_name, expected)
        )
        initial_failure = RuntimeError(
            f"remote verification failed for {full_name}: None != {expected}"
        )

        with (
            mock.patch.object(
                MODULE,
                "ORIGINAL_PUSH_EXACT_MAIN",
                side_effect=initial_failure,
            ) as original_push,
            mock.patch.object(
                MODULE,
                "safe_main_ref",
                side_effect=[None, expected],
            ) as main_ref,
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            MODULE.recovery_push_exact_main(
                Path("/tmp/sealed"),
                full_name,
                expected,
            )

        original_push.assert_called_once_with(
            Path("/tmp/sealed"),
            full_name,
            expected,
        )
        self.assertEqual(main_ref.call_count, 2)
        sleep.assert_called_once_with(MODULE.POST_PUSH_REF_DELAY_SECONDS)

    def test_preexisting_production_repository_never_receives_retry_exception(self) -> None:
        full_name = self.target("streempilot-webrtc-adapter.rs")
        expected = "d" * 40
        MODULE._CURRENT_TARGET = "production"
        initial_failure = RuntimeError(
            f"remote verification failed for {full_name}: None != {expected}"
        )

        with (
            mock.patch.object(
                MODULE,
                "ORIGINAL_PUSH_EXACT_MAIN",
                side_effect=initial_failure,
            ),
            mock.patch.object(MODULE, "safe_main_ref") as main_ref,
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "remote verification failed",
            ):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"),
                    full_name,
                    expected,
                )

        main_ref.assert_not_called()

    def test_wrong_sha_after_creation_fails_immediately_without_sleep(self) -> None:
        full_name = self.target()
        expected = "e" * 40
        MODULE._CURRENT_TARGET = "production"
        MODULE._CURRENT_RUN_PRODUCTION_CREATIONS.add(
            MODULE._stage_creation_key(full_name, expected)
        )
        initial_failure = RuntimeError(
            f"remote verification failed for {full_name}: None != {expected}"
        )

        with (
            mock.patch.object(
                MODULE,
                "ORIGINAL_PUSH_EXACT_MAIN",
                side_effect=initial_failure,
            ),
            mock.patch.object(
                MODULE,
                "safe_main_ref",
                return_value="f" * 40,
            ) as main_ref,
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "changed after first push",
            ):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"),
                    full_name,
                    expected,
                )

        main_ref.assert_called_once_with(full_name)
        sleep.assert_not_called()

    def test_source_has_no_delete_force_or_public_visibility_path(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertIn("_CURRENT_RUN_PRODUCTION_CREATIONS", source)
        self.assertIn("_is_exact_production_target", source)
        self.assertNotIn('BASE.api("DELETE"', source)
        self.assertNotIn("git push --" + "force", source)
        self.assertNotIn("--visibility " + "public", source)
        self.assertNotIn('"private": False', source)
        self.assertNotIn("while True", source)


if __name__ == "__main__":
    unittest.main()
