#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "ops-bootstrap-org-dotgithub-direct.yml"
RUNNER = ROOT / "scripts" / "ops" / "run_protected_org_dotgithub_publisher.sh"


class DirectOrgDotgithubPublisherTests(unittest.TestCase):
    def test_runner_is_valid_bash(self) -> None:
        subprocess.run(["bash", "-n", str(RUNNER)], check=True)

    def test_pull_request_validation_is_secretless_and_exact_head_bound(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for phrase in (
            "pull_request:",
            "github.event_name == 'pull_request'",
            "github.event.pull_request.head.sha",
            'test "$(git rev-parse HEAD)" = "$EXPECTED_HEAD_SHA"',
            "persist-credentials: false",
            "test_bootstrap_org_dotgithub*.py",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)

    def test_publication_is_exact_main_and_aws_oidc_bound(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for phrase in (
            "push:",
            "workflow_dispatch:",
            "github.ref == 'refs/heads/main'",
            "ref: ${{ github.sha }}",
            'test "$GITHUB_REF" = refs/heads/main',
            'test "$(git rev-parse HEAD)" = "$GITHUB_SHA"',
            "id-token: write",
            "aws-actions/configure-aws-credentials@e6de054238d6b7531b4efff3b6587d9aade6a06c",
            "run_protected_org_dotgithub_publisher.sh",
            '"$GITHUB_SHA"',
            '"$GITHUB_WORKSPACE"',
            "org-dotgithub-governance-report-complete",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)

    def test_workflow_does_not_transport_an_organization_admin_token(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for forbidden in (
            "secrets.GH_PAT",
            "secrets.GITHUB_REPOSITORY_ADMIN_TOKEN",
            "GH_PAT:",
            "GITHUB_REPOSITORY_ADMIN_TOKEN:",
            "Authorization: Bearer ${{",
            "aws secretsmanager get-secret-value",
            "AWS_SSM_INSTANCE_ID",
            "aws ssm send-command",
            "github.com/login/device",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)
        self.assertIn("GH_TOKEN: ${{ github.token }}", text)
        self.assertIn("issues: write", text)

    def test_publication_reports_sanitized_bounded_results(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for phrase in (
            "continue-on-error: true",
            "PUBLISH_OUTCOME: ${{ steps.publish.outcome }}",
            "org-dotgithub-redacted.log",
            "github_pat_***",
            "gh*_***",
            "gh issue comment 615",
            'test "$PUBLISH_OUTCOME" == success',
            "The fixed 36-organization fleet was verified",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)
        self.assertNotIn("gh issue close 615", text)

    def test_workflow_adds_no_destructive_recovery_commands(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for phrase in (
            "git checkout --",
            "git reset",
            "git stash",
            "git clean",
            "git filter-repo",
            "git filter-branch",
            "git push --force",
            "git push --force-with-lease",
            "set -x",
        ):
            with self.subTest(phrase=phrase):
                self.assertNotIn(phrase, text)
        self.assertIsNone(re.search(r"(?:^|[;&|()]|\s)rm(?:\s|$)", text))


if __name__ == "__main__":
    unittest.main()
