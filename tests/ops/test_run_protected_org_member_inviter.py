#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "scripts" / "ops" / "run_protected_org_member_inviter.sh"


class ProtectedOrgMemberInviterTests(unittest.TestCase):
    def test_runner_is_valid_bash(self):
        subprocess.run(["bash", "-n", str(RUNNER)], check=True)

    def test_runner_uses_protected_secret_without_printing_it(self):
        text = RUNNER.read_text(encoding="utf-8")
        for phrase in (
            "dd/remote-dev/agent-secrets",
            'payload.get("GH_PAT")',
            "invite_org_member_all.py",
            "--execute",
            "--expected-authenticated-login ORESoftware",
            "protected-verification",
        ):
            self.assertIn(phrase, text)
        for forbidden in (
            "set -x",
            'echo "$GH_TOKEN"',
            'printf "%s" "$GH_TOKEN"',
            "Authorization: Bearer",
            "GITHUB_REPOSITORY_ADMIN_TOKEN",
        ):
            self.assertNotIn(forbidden, text)

    def test_runner_has_no_destructive_membership_commands(self):
        text = RUNNER.read_text(encoding="utf-8")
        for phrase in ("DELETE /orgs", "remove-membership", "cancel-invitation", "role\":\"admin"):
            self.assertNotIn(phrase, text)
        self.assertIsNone(re.search(r"(?:^|[;&|()]|\s)rm(?:\s|$)", text))
        self.assertIsNone(re.search(r"git push --force", text))


if __name__ == "__main__":
    unittest.main()
