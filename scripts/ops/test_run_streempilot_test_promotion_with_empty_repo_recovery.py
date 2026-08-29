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
    "den896_empty_repo_recovery",
    MODULE_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class EmptyRepositoryRecoveryTests(unittest.TestCase):
    def setUp(self) -> None:
        MODULE._RECOVERABLE_EMPTY_APPROVED = False
        MODULE._CURRENT_TARGET = None
        MODULE._CURRENT_RUN_STAGE_CREATIONS.clear()

    def metadata(self, *, size: int = 0) -> dict[str, object]:
        return {
            "id": MODULE.RECOVERABLE_EMPTY_STAGE["repository_id"],
            "full_name": MODULE.RECOVERABLE_EMPTY_STAGE["full_name"],
            "private": True,
            "visibility": "private",
            "default_branch": "main",
            "size": size,
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

    def test_exact_failed_run_empty_repo_is_approved_without_delete(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        calls: list[tuple[str, str]] = []

        def fake_api(method: str, path: str, body=None):
            del body
            calls.append((method, path))
            return 200, self.metadata()

        with (
            mock.patch.object(MODULE.BASE, "api", side_effect=fake_api),
            mock.patch.object(MODULE, "safe_main_ref", return_value=None),
        ):
            self.assertEqual(
                MODULE.prepare_failed_empty_stage_repository(),
                "approved-empty",
            )

        self.assertTrue(MODULE._RECOVERABLE_EMPTY_APPROVED)
        self.assertEqual(calls, [("GET", f"/repos/{full_name}")])
        self.assertFalse(any(method == "DELETE" for method, _ in calls))

    def test_recovery_existing_repository_reuses_only_preapproved_empty_identity(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        MODULE._RECOVERABLE_EMPTY_APPROVED = True

        with (
            mock.patch.object(MODULE.BASE, "api", return_value=(200, self.metadata())),
            mock.patch.object(MODULE, "safe_main_ref", return_value=None),
        ):
            self.assertIsNone(
                MODULE.recovery_existing_repository(full_name, expected)
            )

        other = "StreemPilot-test/streempilot-destinations"
        with mock.patch.object(
            MODULE,
            "ORIGINAL_EXISTING_REPOSITORY",
            return_value={"full_name": other},
        ) as original:
            result = MODULE.recovery_existing_repository(other, "a" * 40)
            self.assertEqual(result, {"full_name": other})
            original.assert_called_once_with(other, "a" * 40)

    def test_absent_exact_stage_repo_is_recorded_for_current_run_retry(self) -> None:
        full_name = "StreemPilot-test/streempilot-recording.rs"
        expected = "a" * 40
        MODULE._CURRENT_TARGET = "stage"

        with mock.patch.object(
            MODULE,
            "ORIGINAL_EXISTING_REPOSITORY",
            return_value=None,
        ):
            self.assertIsNone(
                MODULE.recovery_existing_repository(full_name, expected)
            )

        self.assertIn(
            MODULE._stage_creation_key(full_name, expected),
            MODULE._CURRENT_RUN_STAGE_CREATIONS,
        )

    def test_absent_production_repo_is_never_recorded_for_retry(self) -> None:
        full_name = "StreemPilot/streempilot-recording.rs"
        expected = "a" * 40
        MODULE._CURRENT_TARGET = "production"

        with mock.patch.object(
            MODULE,
            "ORIGINAL_EXISTING_REPOSITORY",
            return_value=None,
        ):
            self.assertIsNone(
                MODULE.recovery_existing_repository(full_name, expected)
            )

        self.assertEqual(MODULE._CURRENT_RUN_STAGE_CREATIONS, set())

    def test_absent_stage_repo_outside_allowlist_is_rejected(self) -> None:
        MODULE._CURRENT_TARGET = "stage"
        with mock.patch.object(
            MODULE,
            "ORIGINAL_EXISTING_REPOSITORY",
            return_value=None,
        ):
            with self.assertRaisesRegex(RuntimeError, "outside exact staging allowlist"):
                MODULE.recovery_existing_repository(
                    "StreemPilot-test/unapproved-repository",
                    "a" * 40,
                )

    def test_recovery_existing_repository_preserves_race_to_exact_sha(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        MODULE._RECOVERABLE_EMPTY_APPROVED = True
        metadata = self.metadata()

        with (
            mock.patch.object(MODULE.BASE, "api", return_value=(200, metadata)),
            mock.patch.object(MODULE, "safe_main_ref", return_value=expected),
        ):
            self.assertEqual(
                MODULE.recovery_existing_repository(full_name, expected),
                metadata,
            )

    def test_recovery_existing_repository_rejects_content_without_visible_main(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        MODULE._RECOVERABLE_EMPTY_APPROVED = True

        with (
            mock.patch.object(
                MODULE.BASE,
                "api",
                return_value=(200, self.metadata(size=8)),
            ),
            mock.patch.object(MODULE, "safe_main_ref", return_value=None),
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "content without the exact sealed main ref",
            ):
                MODULE.recovery_existing_repository(full_name, expected)

    def test_post_push_retry_accepts_eventually_visible_exact_ref(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        MODULE._RECOVERABLE_EMPTY_APPROVED = True
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
                side_effect=[None, None, expected],
            ) as ref,
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            MODULE.recovery_push_exact_main(Path("/tmp/sealed"), full_name, expected)

        original_push.assert_called_once_with(Path("/tmp/sealed"), full_name, expected)
        self.assertEqual(ref.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_current_run_stage_creation_gets_same_bounded_ref_retry(self) -> None:
        full_name = "StreemPilot-test/streempilot-webrtc-adapter.rs"
        expected = "b" * 40
        MODULE._CURRENT_TARGET = "stage"
        MODULE._CURRENT_RUN_STAGE_CREATIONS.add(
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
                side_effect=[None, expected],
            ) as ref,
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            MODULE.recovery_push_exact_main(Path("/tmp/sealed"), full_name, expected)

        self.assertEqual(ref.call_count, 2)
        sleep.assert_called_once_with(MODULE.POST_PUSH_REF_DELAY_SECONDS)

    def test_current_run_stage_retry_fails_immediately_on_wrong_sha(self) -> None:
        full_name = "StreemPilot-test/streempilot-recording.rs"
        expected = "c" * 40
        MODULE._CURRENT_TARGET = "stage"
        MODULE._CURRENT_RUN_STAGE_CREATIONS.add(
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
            mock.patch.object(MODULE, "safe_main_ref", return_value="f" * 40),
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            with self.assertRaisesRegex(RuntimeError, "changed after first push"):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"), full_name, expected
                )
        sleep.assert_not_called()

    def test_post_push_retry_fails_immediately_on_wrong_sha(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        MODULE._RECOVERABLE_EMPTY_APPROVED = True
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
            ) as ref,
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            with self.assertRaisesRegex(RuntimeError, "changed after first push"):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"),
                    full_name,
                    expected,
                )

        ref.assert_called_once_with(full_name)
        sleep.assert_not_called()

    def test_post_push_retry_fails_closed_after_bounded_absence(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        MODULE._RECOVERABLE_EMPTY_APPROVED = True
        initial_failure = RuntimeError(
            f"remote verification failed for {full_name}: None != {expected}"
        )

        with (
            mock.patch.object(
                MODULE,
                "ORIGINAL_PUSH_EXACT_MAIN",
                side_effect=initial_failure,
            ),
            mock.patch.object(MODULE, "POST_PUSH_REF_ATTEMPTS", 3),
            mock.patch.object(MODULE, "safe_main_ref", return_value=None) as ref,
            mock.patch.object(MODULE.time, "sleep") as sleep,
        ):
            with self.assertRaisesRegex(RuntimeError, "after 3 bounded checks"):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"),
                    full_name,
                    expected,
                )

        self.assertEqual(ref.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_post_push_retry_never_applies_to_untracked_repository_or_error(self) -> None:
        other = "StreemPilot-test/streempilot-destinations"
        expected = "a" * 40
        failure = RuntimeError(
            f"remote verification failed for {other}: None != {expected}"
        )
        MODULE._CURRENT_TARGET = "stage"

        with (
            mock.patch.object(
                MODULE,
                "ORIGINAL_PUSH_EXACT_MAIN",
                side_effect=failure,
            ),
            mock.patch.object(MODULE, "safe_main_ref") as ref,
        ):
            with self.assertRaisesRegex(RuntimeError, "remote verification failed"):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"),
                    other,
                    expected,
                )
        ref.assert_not_called()

        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        recoverable_sha = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])
        other_failure = RuntimeError("git push failed")
        MODULE._RECOVERABLE_EMPTY_APPROVED = True
        with (
            mock.patch.object(
                MODULE,
                "ORIGINAL_PUSH_EXACT_MAIN",
                side_effect=other_failure,
            ),
            mock.patch.object(MODULE, "safe_main_ref") as ref,
        ):
            with self.assertRaisesRegex(RuntimeError, "git push failed"):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"),
                    full_name,
                    recoverable_sha,
                )
        ref.assert_not_called()

    def test_production_never_gets_current_run_stage_retry(self) -> None:
        full_name = "StreemPilot/streempilot-recording.rs"
        expected = "d" * 40
        MODULE._CURRENT_TARGET = "production"
        MODULE._CURRENT_RUN_STAGE_CREATIONS.add(
            MODULE._stage_creation_key(
                "StreemPilot-test/streempilot-recording.rs",
                expected,
            )
        )
        failure = RuntimeError(
            f"remote verification failed for {full_name}: None != {expected}"
        )
        with (
            mock.patch.object(
                MODULE,
                "ORIGINAL_PUSH_EXACT_MAIN",
                side_effect=failure,
            ),
            mock.patch.object(MODULE, "safe_main_ref") as ref,
        ):
            with self.assertRaisesRegex(RuntimeError, "remote verification failed"):
                MODULE.recovery_push_exact_main(
                    Path("/tmp/sealed"), full_name, expected
                )
        ref.assert_not_called()

    def test_recovery_rejects_wrong_repository_id_without_approval(self) -> None:
        metadata = self.metadata()
        metadata["id"] = int(MODULE.RECOVERABLE_EMPTY_STAGE["repository_id"]) + 1

        with mock.patch.object(MODULE.BASE, "api", return_value=(200, metadata)):
            with self.assertRaisesRegex(RuntimeError, "repository id changed"):
                MODULE.prepare_failed_empty_stage_repository()

        self.assertFalse(MODULE._RECOVERABLE_EMPTY_APPROVED)

    def test_recovery_preserves_exact_main_without_empty_exception(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])

        with (
            mock.patch.object(MODULE.BASE, "api", return_value=(200, self.metadata())),
            mock.patch.object(MODULE, "safe_main_ref", return_value=expected),
        ):
            self.assertEqual(
                MODULE.prepare_failed_empty_stage_repository(),
                "already-exact",
            )

        self.assertFalse(MODULE._RECOVERABLE_EMPTY_APPROVED)

    def test_recovery_preserves_exact_nonempty_main_as_idempotent_replay(self) -> None:
        full_name = str(MODULE.RECOVERABLE_EMPTY_STAGE["full_name"])
        expected = str(MODULE.RECOVERABLE_EMPTY_STAGE["expected_sha"])

        with (
            mock.patch.object(
                MODULE.BASE,
                "api",
                return_value=(200, self.metadata(size=8)),
            ),
            mock.patch.object(MODULE, "safe_main_ref", return_value=expected),
        ):
            self.assertEqual(
                MODULE.prepare_failed_empty_stage_repository(),
                "already-exact",
            )

        self.assertFalse(MODULE._RECOVERABLE_EMPTY_APPROVED)

    def test_recovery_rejects_nonzero_size_without_exact_main(self) -> None:
        with (
            mock.patch.object(
                MODULE.BASE,
                "api",
                return_value=(200, self.metadata(size=8)),
            ),
            mock.patch.object(MODULE, "safe_main_ref", return_value=None),
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                "content without the exact sealed main ref",
            ):
                MODULE.prepare_failed_empty_stage_repository()

        self.assertFalse(MODULE._RECOVERABLE_EMPTY_APPROVED)

    def test_recovery_rejects_any_other_nonempty_main(self) -> None:
        with (
            mock.patch.object(MODULE.BASE, "api", return_value=(200, self.metadata(size=8))),
            mock.patch.object(MODULE, "safe_main_ref", return_value="f" * 40),
        ):
            with self.assertRaisesRegex(RuntimeError, "refusing to initialize non-empty"):
                MODULE.prepare_failed_empty_stage_repository()

        self.assertFalse(MODULE._RECOVERABLE_EMPTY_APPROVED)

    def test_recovery_rejects_invalid_size_metadata(self) -> None:
        metadata = self.metadata()
        metadata["size"] = "8"
        with mock.patch.object(MODULE.BASE, "api", return_value=(200, metadata)):
            with self.assertRaisesRegex(RuntimeError, "size is invalid"):
                MODULE.prepare_failed_empty_stage_repository()

    def test_source_hardcodes_only_test_org_recovery_and_has_no_delete_force_or_public_path(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertIn('"StreemPilot-test/streempilot-compositor.rs"', source)
        self.assertIn("1327442276", source)
        self.assertIn('"2026-08-08T05:25:37Z"', source)
        self.assertIn("POST_PUSH_REF_ATTEMPTS = 6", source)
        self.assertIn('if _CURRENT_TARGET == "stage"', source)
        self.assertIn("BASE.push_exact_main = recovery_push_exact_main", source)
        self.assertNotIn("StreemPilot/streempilot-compositor.rs\"", source)
        self.assertNotIn('BASE.api("DELETE"', source)
        self.assertNotIn("git push --" + "force", source)
        self.assertNotIn("--visibility " + "public", source)
        self.assertNotIn('"private": False', source)
        self.assertNotIn("--organization", source)
        self.assertNotIn("while True", source)


if __name__ == "__main__":
    unittest.main()
