#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts/ops/run_exact_private_repository_gaps_with_retry.py"
SPEC = importlib.util.spec_from_file_location("exact_gap_retry_runner", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def repository(full_name: str, disposition: str = "created") -> dict[str, object]:
    digest = {
        "StreemPilot/streempilot-media-router.rs": "1" * 40,
        "hypesiege/hypesiege-analytics.rs": "2" * 40,
        "hypesiege/hypesiege-publishing-worker.rs": "3" * 40,
        "hypesiege/hypesiege-scheduler.rs": "4" * 40,
    }[full_name]
    return {
        "full_name": full_name,
        "repository_id": 100 + int(digest[0]),
        "visibility": "private",
        "default_branch": "main",
        "main_sha": digest,
        "expected_sealed_sha": digest,
        "disposition": disposition,
        "html_url": f"https://github.com/{full_name}",
    }


def document(organization: str, names: tuple[str, ...]) -> dict[str, object]:
    return {
        "schema_version": 1,
        "organization": organization,
        "sealed_source_repository": "ORESoftware/ai-agent-coordinator.rs",
        "sealed_source_sha": "5d9a0c2cb44dff607bc3953954ce4b9af08e5789",
        "repositories": [repository(name) for name in names],
    }


class ExactGapRetryRunnerTests(unittest.TestCase):
    def evidence(self) -> list[dict[str, object]]:
        return [
            document(
                "hypesiege",
                (
                    "hypesiege/hypesiege-analytics.rs",
                    "hypesiege/hypesiege-publishing-worker.rs",
                    "hypesiege/hypesiege-scheduler.rs",
                ),
            ),
            document(
                "StreemPilot",
                ("StreemPilot/streempilot-media-router.rs",),
            ),
        ]

    def test_transient_classifier_is_bounded_to_rate_and_service_failures(self) -> None:
        self.assertTrue(MODULE.is_transient_failure("secondary rate limit exceeded", 403))
        self.assertTrue(MODULE.is_transient_failure("HTTP 503 service unavailable", 503))
        self.assertTrue(MODULE.is_transient_failure("connection reset by peer"))
        self.assertFalse(MODULE.is_transient_failure("Bad credentials", 401))
        self.assertFalse(MODULE.is_transient_failure("Repository visibility mismatch", 422))

    def test_retry_delay_is_bounded(self) -> None:
        self.assertEqual(MODULE.retry_delay(1, {"Retry-After": "2"}), 5)
        self.assertEqual(MODULE.retry_delay(8, {"Retry-After": "1000"}), 180)
        self.assertLessEqual(MODULE.retry_delay(8), 180)

    def test_combined_evidence_requires_exact_four_private_sealed_heads(self) -> None:
        combined = MODULE.combine_evidence(
            self.evidence(),
            authenticated_login="ORESoftware",
        )
        self.assertEqual(combined["expected_repository_count"], 4)
        self.assertEqual(combined["summary"]["created"], 4)
        self.assertEqual(combined["summary"]["failures"], 0)
        self.assertEqual(
            {item["full_name"] for item in combined["repositories"]},
            MODULE.EXPECTED_REPOSITORIES,
        )

    def test_combined_evidence_rejects_nonsealed_or_expanded_state(self) -> None:
        evidence = self.evidence()
        evidence[0]["repositories"][0]["main_sha"] = "f" * 40
        with self.assertRaisesRegex(RuntimeError, "does not match sealed history"):
            MODULE.combine_evidence(evidence, authenticated_login="ORESoftware")

        evidence = self.evidence()
        evidence[0]["repositories"].append(
            {
                **repository("hypesiege/hypesiege-analytics.rs"),
                "full_name": "hypesiege/unreviewed.rs",
            }
        )
        with self.assertRaisesRegex(RuntimeError, "exact repository evidence mismatch"):
            MODULE.combine_evidence(evidence, authenticated_login="ORESoftware")

    def test_source_has_no_token_literal_public_creation_or_force_update(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertNotIn("gh" + "p_", source)
        self.assertNotIn("github_" + "pat_", source)
        self.assertNotIn('"private": False', source)
        self.assertNotIn("--visibility " + "public", source)
        self.assertNotIn("git push --" + "force", source)
        self.assertIn("--token-file", source)
        self.assertIn("VERIFIED_ENCRYPTED_EXACT_PRIVATE_GAPS", source)


if __name__ == "__main__":
    unittest.main()
