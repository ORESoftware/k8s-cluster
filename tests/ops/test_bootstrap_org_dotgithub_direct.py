#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "ops-bootstrap-org-dotgithub-direct.yml"
DIRECT_RUNNER = ROOT / "scripts" / "ops" / "run_direct_org_dotgithub_publisher.sh"
LEGACY_PROTECTED_RUNNER = ROOT / "scripts" / "ops" / "run_protected_org_dotgithub_publisher.sh"
ALL_PROTECTED_RUNNER = ROOT / "scripts" / "ops" / "run_protected_org_dotgithub_all_publisher.sh"


class DirectOrgDotgithubPublisherTests(unittest.TestCase):
    def test_runners_are_valid_bash(self) -> None:
        for runner in (DIRECT_RUNNER, LEGACY_PROTECTED_RUNNER, ALL_PROTECTED_RUNNER):
            with self.subTest(runner=runner.name):
                subprocess.run(["bash", "-n", str(runner)], check=True)

    def test_pull_request_validation_is_secretless_and_exact_head_bound(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for phrase in (
            "pull_request:",
            "github.event_name == 'pull_request'",
            "github.event.pull_request.head.sha",
            'test "$(git rev-parse HEAD)" = "$EXPECTED_HEAD_SHA"',
            "persist-credentials: false",
            "run_direct_org_dotgithub_publisher.sh",
            "run_protected_org_dotgithub_all_publisher.sh",
            "bootstrap_org_dotgithub_repositories_all.py",
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
            "run_direct_org_dotgithub_publisher.sh",
            '"$GITHUB_SHA"',
            '"$GITHUB_WORKSPACE"',
            "org-dotgithub-governance-report-complete",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)

    def test_direct_runner_normalizes_bootstrap_and_delegates_fail_closed(self) -> None:
        text = DIRECT_RUNNER.read_text(encoding="utf-8")
        for phrase in (
            "stage=direct-bootstrap",
            "invalid-trusted-sha",
            "source-root-not-absolute",
            "source-root-missing",
            "tr -d '[:space:]'",
            "invalid-aws-region",
            "export AWS_REGION",
            "run_protected_org_dotgithub_all_publisher.sh",
            "bash -n \"$protected_runner\"",
            "stage=direct-delegate",
            "status=failed rc=%s",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)
        for forbidden in (
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_PAT",
            "oauth_token",
            "Authorization",
            "secretsmanager",
            "kubectl",
            "gh auth",
            "old_target_count",
            "text.replace",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)

    def test_all_protected_launcher_is_fixed_bounded_and_secret_safe(self) -> None:
        text = ALL_PROTECTED_RUNNER.read_text(encoding="utf-8")
        for phrase in (
            "dd/remote-dev/agent-secrets",
            'payload.get("GH_PAT")',
            "bootstrap_org_dotgithub_repositories_all.py",
            "len(organizations) != 61",
            "publisher.TARGET_ORGANIZATIONS",
            "publisher.EXCLUDED_ORGANIZATIONS",
            "organizations=61",
            "org-dotgithub-governance-report-complete",
            "source=aws-secrets-manager",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)
        self.assertNotIn("${3", text)
        self.assertNotIn("set -x", text)
        self.assertNotIn('echo "$GH_TOKEN"', text)
        self.assertNotIn("Authorization: Bearer", text)

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
            "The fixed 61-organization fleet was verified",
            "Publish and verify the fixed 61-organization fleet",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)
        self.assertNotIn("gh issue close 615", text)

    def test_direct_workflow_and_runners_add_no_destructive_recovery_commands(self) -> None:
        text = "\n".join(
            path.read_text(encoding="utf-8")
            for path in (WORKFLOW, DIRECT_RUNNER, ALL_PROTECTED_RUNNER)
        )
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
        self.assertIsNone(re.search(r"(?:^|[;&|()]|\s)sed(?:\s|$)", text))


if __name__ == "__main__":
    unittest.main()
