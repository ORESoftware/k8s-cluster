#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
DISPATCHER = ROOT / ".github/workflows/ops-dispatch-private-fleet-gaps-comment.yml"
OBSERVER = ROOT / ".github/workflows/observe-private-fleet-publisher.yml"
PUBLISHER = ROOT / ".github/workflows/ops-publish-missing-org-repositories-gh-profile.yml"


class PrivateFleetGapDispatchContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.dispatcher = DISPATCHER.read_text(encoding="utf-8")
        cls.observer = OBSERVER.read_text(encoding="utf-8")
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

    def test_observer_is_bound_to_the_exact_protected_workflow(self) -> None:
        self.assertIn(
            "- Publish missing organization repositories from protected gh profile",
            self.observer,
        )
        self.assertIn("types:\n      - requested\n      - completed", self.observer)
        self.assertIn("RUN_ID: ${{ github.event.workflow_run.id }}", self.observer)
        self.assertIn("RUN_HEAD_SHA: ${{ github.event.workflow_run.head_sha }}", self.observer)
        self.assertIn('test "$RUN_HEAD_BRANCH" = main', self.observer)
        self.assertIn("push|workflow_dispatch", self.observer)

    def test_observer_reports_bounded_metadata_without_raw_logs_or_credentials(self) -> None:
        self.assertIn("actions: read", self.observer)
        self.assertIn("issues: write", self.observer)
        self.assertIn("failed_jobs", self.observer)
        self.assertIn("actions/runs/${RUN_ID}/jobs", self.observer)
        self.assertNotIn("gh run view --log", self.observer)
        self.assertNotIn("fetch_workflow_job_logs", self.observer)
        for marker in (
            "GITHUB_REPOSITORY_ADMIN_TOKEN",
            "REMOTE_DEV_GH_PAT",
            "AWS_SSM_INSTANCE_ID",
            "GIT_ASKPASS",
            "Authorization: Bearer",
        ):
            self.assertNotIn(marker, self.observer)
        self.assertIn("gh pr comment 595", self.observer)
        self.assertIn("gh pr comment 291", self.observer)


if __name__ == "__main__":
    unittest.main()
