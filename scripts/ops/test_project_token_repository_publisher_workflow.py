#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = (
    ROOT
    / ".github/workflows/ops-publish-missing-org-repositories-project-token-once.yml"
).read_text(encoding="utf-8")


class ProjectTokenRepositoryPublisherWorkflowTests(unittest.TestCase):
    def test_credential_is_protected_and_not_user_supplied(self) -> None:
        self.assertIn("environment: portfolio-project-sync", WORKFLOW)
        self.assertIn("secrets.PROJECT_SYNC_GITHUB_TOKEN", WORKFLOW)
        self.assertNotIn("workflow_dispatch:\n    inputs:", WORKFLOW)
        self.assertNotIn("ghp_", WORKFLOW)
        self.assertNotIn("github_pat_", WORKFLOW)

    def test_execution_is_bound_to_exact_trusted_main(self) -> None:
        self.assertIn("ref: ${{ github.sha }}", WORKFLOW)
        self.assertIn('test "$GITHUB_REF" = refs/heads/main', WORKFLOW)
        self.assertIn('test "$(git rev-parse HEAD)" = "$GITHUB_SHA"', WORKFLOW)
        self.assertIn("persist-credentials: false", WORKFLOW)
        self.assertIn("cancel-in-progress: false", WORKFLOW)

    def test_contracts_precede_the_only_live_publisher(self) -> None:
        contracts = WORKFLOW.index("Run publisher contract suites before mutation")
        publication = WORKFLOW.index(
            "Publish only sealed missing repositories and verify preserved histories"
        )
        self.assertLess(contracts, publication)
        self.assertEqual(
            WORKFLOW.count("python3 scripts/ops/publish_missing_org_repositories_current.py"),
            1,
        )
        self.assertNotIn("--force", WORKFLOW)
        self.assertNotIn("git push -f", WORKFLOW)
        self.assertNotIn("git push --force", WORKFLOW)
        self.assertNotIn("gh repo edit", WORKFLOW)

    def test_requested_gaps_are_verified_private_at_main(self) -> None:
        for full_name in (
            "StreemPilot/streempilot-media-router.rs",
            "hypesiege/hypesiege-scheduler.rs",
            "hypesiege/hypesiege-publishing-worker.rs",
            "hypesiege/hypesiege-analytics.rs",
        ):
            self.assertIn(full_name, WORKFLOW)
        self.assertIn('repo.get("visibility") != "private"', WORKFLOW)
        self.assertIn('repo.get("private") is not True', WORKFLOW)
        self.assertIn('repo.get("default_branch") != "main"', WORKFLOW)
        self.assertIn("VERIFIED_REQUESTED_GAPS {len(evidence)}/4", WORKFLOW)

    def test_evidence_is_retained(self) -> None:
        self.assertIn("publisher.log", WORKFLOW)
        self.assertIn("requested-gaps.json", WORKFLOW)
        self.assertIn("actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a", WORKFLOW)
        self.assertIn("if-no-files-found: error", WORKFLOW)
        self.assertIn("retention-days: 30", WORKFLOW)


if __name__ == "__main__":
    unittest.main()
