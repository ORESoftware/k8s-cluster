#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "ops-bootstrap-org-dotgithub-fleet.yml"
RUNNER = ROOT / "scripts" / "ops" / "run_protected_org_dotgithub_publisher.sh"


class ProtectedOrgDotgithubPublisherTests(unittest.TestCase):
    def test_runner_is_valid_bash(self) -> None:
        subprocess.run(["bash", "-n", str(RUNNER)], check=True)

    def test_runner_uses_only_fixed_protected_credential_sources(self) -> None:
        text = RUNNER.read_text(encoding="utf-8")
        for phrase in (
            "dd/remote-dev/agent-secrets",
            "dd-agent-secrets",
            "jsonpath='{.data.GH_PAT}'",
            "gh auth token --hostname github.com",
            "/root/.config/gh/hosts.yml",
            "/home/ec2-user/.config/gh/hosts.yml",
            "credential never crosses the",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)
        self.assertNotIn("${3", text)
        self.assertNotIn('GH_TOKEN="${3', text)
        self.assertNotIn('GH_TOKEN="$3', text)

    def test_runner_executes_exact_hardened_fleet_and_certifies_36(self) -> None:
        text = RUNNER.read_text(encoding="utf-8")
        for phrase in (
            "bootstrap_org_dotgithub_repositories_hardened.py",
            "--execute",
            "len(organizations) != 36",
            'repository != f"{organization}/.github"',
            'item.get("verified") is not True',
            "org-dotgithub-governance-report-complete",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)

    def test_runner_does_not_add_blacklisted_commands(self) -> None:
        text = RUNNER.read_text(encoding="utf-8")
        for phrase in (
            "git checkout",
            "git reset",
            "git stash",
            "git clean",
            "git filter-repo",
            "git filter-branch",
            "git push --force",
        ):
            with self.subTest(phrase=phrase):
                self.assertNotIn(phrase, text)
        self.assertIsNone(re.search(r"(?:^|[;&|()]|\s)rm(?:\s|$)", text))
        self.assertIsNone(re.search(r"(?:^|[;&|()]|\s)sed(?:\s|$)", text))
        self.assertNotIn("set -x", text)
        self.assertNotIn('echo "$GH_TOKEN"', text)
        self.assertNotIn('printf \'%s\\n\' "$GH_TOKEN"', text)

    def test_workflow_routes_secretless_request_to_protected_ssm_host(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for phrase in (
            "AWS_SSM_INSTANCE_ID",
            "aws ssm send-command",
            "AWS-RunShellScript",
            "run_protected_org_dotgithub_publisher.sh",
            "git -C \"$work/repository\" archive FETCH_HEAD",
            "org-dotgithub-governance-report-complete",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)
        self.assertNotIn("aws secretsmanager get-secret-value", text)
        self.assertNotIn("GH_PAT", text)
        self.assertNotIn("git checkout --detach", text)
        self.assertNotIn("git clean", text)
        self.assertNotIn("git reset", text)
        self.assertNotIn("git stash", text)
        self.assertNotIn("git filter-repo", text)

    def test_workflow_remains_exactly_bounded_to_issue_and_actor(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        for phrase in (
            "github.event.issue.number == 615",
            "github.actor == 'ORESoftware'",
            "github.event.comment.body == 'ops-bootstrap-org-dotgithub:615:ad6e27250729ed647513a302d02fd767'",
            "ref: main",
            "persist-credentials: false",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, text)

    def test_ssm_arguments_contain_no_credential_variable(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        command_start = text.index('remote_command="')
        command_end = text.index('"\n          parameters=', command_start)
        command = text[command_start:command_end]
        for forbidden in ("GH_TOKEN", "GITHUB_TOKEN", "GH_PAT", "oauth_token", "Authorization"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, command)


if __name__ == "__main__":
    unittest.main()
