#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "ops-probe-org-dotgithub-app-capabilities.yml"


class OrgDotgithubAppCapabilityWorkflowTests(unittest.TestCase):
    def setUp(self) -> None:
        self.text = WORKFLOW.read_text(encoding="utf-8")

    def test_exact_bounded_trigger(self) -> None:
        for phrase in (
            "github.event.issue.number == 615",
            "github.actor == 'ORESoftware'",
            "github.event.comment.body == 'ops-probe-org-dotgithub-app:615:20260803-v1'",
            "ref: main",
            "persist-credentials: false",
        ):
            self.assertIn(phrase, self.text)

    def test_fixed_36_organization_registry(self) -> None:
        block = self.text.split("organizations=(", 1)[1].split(")", 1)[0]
        organizations = re.findall(r"[A-Za-z0-9][A-Za-z0-9-]*", block)
        self.assertEqual(36, len(organizations))
        self.assertEqual(36, len({item.lower() for item in organizations}))
        for required in ("fiducia-cloud", "sonus-auris", "shared-auth", "StreemPilot"):
            self.assertIn(required, organizations)

    def test_requires_exact_app_capability(self) -> None:
        for phrase in (
            '"administration":"write"',
            '"contents":"write"',
            '"metadata":"read"',
            'repository_selection" == \'"all"\'',
            '"$administration" == \'"write"\'',
            '"$contents" == \'"write"\'',
            'test "${{ steps.probe.outputs.capable_count }}" = 36',
        ):
            self.assertIn(phrase, self.text)

    def test_does_not_publish_sensitive_fields(self) -> None:
        report = self.text.split("echo '# GitHub App capability", 1)[1]
        for forbidden in (
            "installation_id",
            "installation_token",
            "app_jwt",
            "K8S_SUBMODULE_APP_PRIVATE_KEY",
            "token_json",
        ):
            self.assertNotIn(forbidden, report)
        self.assertIn("No installation ID", report)

    def test_no_new_destructive_or_history_rewriting_commands(self) -> None:
        for phrase in (
            "git stash",
            "git reset",
            "git clean",
            "git filter-repo",
            "git filter-branch",
            "git push --force",
            "rm -rf",
            "find -delete",
            "terraform destroy",
            "kubectl delete",
            "--no-verify",
        ):
            self.assertNotIn(phrase, self.text)

    def test_private_key_is_not_written_to_a_file(self) -> None:
        self.assertIn("openssl dgst -sha256 -sign /dev/fd/3", self.text)
        self.assertIn('3<<<"$K8S_SUBMODULE_APP_PRIVATE_KEY"', self.text)
        self.assertNotIn("app-private-key.pem", self.text)


if __name__ == "__main__":
    unittest.main()
