#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
DISPATCHER = ROOT / ".github/workflows/ops-dispatch-private-fleet-gaps-comment.yml"
PUBLISHER = ROOT / ".github/workflows/ops-publish-missing-org-repositories-gh-profile.yml"


class PrivateFleetGapDispatchContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.dispatcher = DISPATCHER.read_text(encoding="utf-8")
        cls.publisher = PUBLISHER.read_text(encoding="utf-8")

    def test_dispatch_is_bound_to_exact_owner_comment_and_merged_pr(self) -> None:
        self.assertIn("github.event.issue.number == 595", self.dispatcher)
        self.assertIn("github.actor == 'ORESoftware'", self.dispatcher)
        self.assertIn(
            "ops-dispatch-private-fleet-gaps:595:0cdb72ec06cff74a4cb5d6062777b619703be12b",
            self.dispatcher,
        )
        self.assertIn("EXPECTED_HEAD_SHA: '04352cf5e8d87532ffc65afa9480a7ccdc3407b8'", self.dispatcher)
        self.assertIn("EXPECTED_MERGE_SHA: '0cdb72ec06cff74a4cb5d6062777b619703be12b'", self.dispatcher)
        self.assertIn("test \"$(jq -r '.merged'", self.dispatcher)
        self.assertIn("compare/${EXPECTED_MERGE_SHA}...main", self.dispatcher)

    def test_dispatch_uses_trusted_default_branch_workflow_only(self) -> None:
        self.assertIn(
            "PUBLISH_WORKFLOW: ops-publish-missing-org-repositories-gh-profile.yml",
            self.dispatcher,
        )
        self.assertIn('gh workflow run "$PUBLISH_WORKFLOW"', self.dispatcher)
        self.assertIn('--ref main', self.dispatcher)
        self.assertNotIn("actions/checkout", self.dispatcher)
        self.assertNotIn("github.event.pull_request.head", self.dispatcher)

    def test_dispatch_tracks_exact_child_run_to_completion(self) -> None:
        self.assertIn("gh run list", self.dispatcher)
        self.assertIn("candidate", self.dispatcher)
        self.assertIn("gh run watch \"$RUN_ID\"", self.dispatcher)
        self.assertIn("--exit-status", self.dispatcher)
        self.assertIn("test \"$JOB_RESULT\" = success", self.dispatcher)
        self.assertIn("TRACKING_PR: '595'", self.dispatcher)
        self.assertIn("PORTFOLIO_PR: '291'", self.dispatcher)

    def test_dispatcher_cannot_receive_or_forward_repository_credentials(self) -> None:
        forbidden = (
            "GITHUB_REPOSITORY_ADMIN_TOKEN",
            "REMOTE_DEV_GH_PAT",
            "HYPESIEGE_REPOSITORY_ADMIN_TOKEN",
            "STREEMPILOT_REPOSITORY_ADMIN_TOKEN",
            "AWS_SSM_INSTANCE_ID",
            "GIT_ASKPASS",
        )
        for marker in forbidden:
            self.assertNotIn(marker, self.dispatcher)
        self.assertIn("permissions:\n  actions: write\n  contents: read", self.dispatcher)
        self.assertIn("GH_TOKEN: ${{ github.token }}", self.dispatcher)

    def test_protected_publisher_remains_idempotent_and_gap_aware(self) -> None:
        self.assertIn("stage=publisher-static-validation", self.publisher)
        self.assertIn("test_repository_fleet_remote_state.py", self.publisher)
        self.assertIn("stage=bounded-repository-publication", self.publisher)
        self.assertIn("created-exact-preserved-unchanged", self.publisher)
        self.assertNotIn("finalize_missing_org_repositories.py", self.publisher)


if __name__ == "__main__":
    unittest.main()
