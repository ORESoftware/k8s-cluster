#!/usr/bin/env python3
from __future__ import annotations

import os
from pathlib import Path
import re
import unittest

ROOT = Path(os.environ.get("ROOT_OVERRIDE", Path(__file__).resolve().parents[2]))
WORKFLOW = ROOT / ".github/workflows/ops-owner-device-publish-org-dotgithub-fleet.yml"


class OwnerDeviceOrgDotgithubFleetTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.text = WORKFLOW.read_text(encoding="utf-8")

    def test_pull_request_validation_is_credential_free_and_checkout_free(self) -> None:
        self.assertIn("pull_request:", self.text)
        validate = self.text.split("  validate-pr:", 1)[1].split("\n  publish:", 1)[0]
        self.assertIn("HEAD_SHA: ${{ github.event.pull_request.head.sha }}", validate)
        self.assertIn("ROOT_OVERRIDE=", validate)
        self.assertNotIn("OAUTH_CLIENT_ID", validate)
        self.assertNotIn("login/device", validate)
        self.assertNotIn("actions/checkout@", self.text)

    def test_exact_owner_issue_trigger_is_fixed(self) -> None:
        for phrase in (
            "github.event.issue.number == 615",
            "github.actor == 'ORESoftware'",
            "github.event.comment.body == 'ops-owner-device-org-dotgithub:615:20260803-v1'",
        ):
            self.assertIn(phrase, self.text)

    def test_publisher_source_and_blobs_are_immutable(self) -> None:
        expected = {
            "SOURCE_SHA": "412f03155ba108890735414d6fbf5a1a72d9c554",
            "BASE_PUBLISHER_BLOB": "2028311196f066d4e73473562e9dea33bd9d5c10",
            "HARDENED_PUBLISHER_BLOB": "0960606f1f0c8136b3f9ea42f8f41aae8b450906",
            "BASE_TEST_BLOB": "1a5e83d49d0b2f6acfdca657f4f71cb4f86a6b2d",
            "HARDENED_TEST_BLOB": "a358aabc444f90f02c0ff7f7514eb94dcb4c9fcf",
            "FLEET_WORKFLOW_BLOB": "7e2aea773c6e17ff2f021604907bcdec42387fbd",
        }
        for name, value in expected.items():
            self.assertIn(f"{name}: {value}", self.text)
        self.assertIn("test \"$(jq -er '.sha'", self.text)
        self.assertIn("python3 -m unittest discover", self.text)

    def test_device_flow_uses_minimum_public_repo_scopes_and_correct_polling(self) -> None:
        for phrase in (
            "scope=public_repo read:org",
            "https://github.com/login/device/code",
            "https://github.com/login/oauth/access_token",
            "grant_type=urn:ietf:params:oauth:grant-type:device_code",
            "authorization_pending",
            "slow_down",
            "elapsed < expires_in",
            "sleep \"$interval\"",
        ):
            self.assertIn(phrase, self.text)
        self.assertNotIn("scope=repo read:org", self.text)

    def test_token_is_memory_only_masked_and_not_persisted(self) -> None:
        publish = self.text.split("  publish:", 1)[1]
        for phrase in (
            'echo "::add-mask::$access_token"',
            'export GH_TOKEN="$access_token"',
            'export GITHUB_REPOSITORY_ADMIN_TOKEN="$access_token"',
            "clear_sensitive_state",
            "Revoke the authorized OAuth app",
        ):
            self.assertIn(phrase, publish)
        self.assertNotIn('>> "$GITHUB_ENV"', publish)
        self.assertNotIn('>> "${GITHUB_ENV}"', publish)
        self.assertNotIn("upload-artifact", publish)
        self.assertNotRegex(publish, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(publish, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_all_36_memberships_are_preflighted_before_execute(self) -> None:
        block = self.text.split("organizations=(", 1)[1].split(")", 1)[0]
        organizations = re.findall(r"[A-Za-z0-9][A-Za-z0-9-]*", block)
        self.assertEqual(36, len(organizations))
        self.assertEqual(36, len({item.casefold() for item in organizations}))
        membership = self.text.index("stage=preflight-all-36-owner-memberships")
        execute = self.text.index("stage=execute-hardened-fleet-publisher")
        self.assertLess(membership, execute)
        self.assertIn("test \"$(jq -er '.role'", self.text)
        self.assertIn("= admin", self.text)
        self.assertIn("test \"$(jq -er '.state'", self.text)
        self.assertIn("= active", self.text)

    def test_publisher_is_execute_mode_and_report_certifies_exact_fleet(self) -> None:
        for phrase in (
            "bootstrap_org_dotgithub_repositories_hardened.py",
            "--execute",
            'payload.get("mode") == "execute"',
            "len(organizations) == 36",
            'item["repository"] == f"{item[\'organization\']}/.github"',
            'item["verified"] is True',
            "org-dotgithub-owner-device-report-complete",
        ):
            self.assertIn(phrase, self.text)

    def test_no_destructive_or_history_rewriting_commands_are_added(self) -> None:
        forbidden = (
            "git stash",
            "git reset",
            "git clean",
            "git filter-repo",
            "git filter-branch",
            "git checkout",
            "git rebase",
            "git commit --amend",
            "git push --force",
            "rm -rf",
            "find -delete",
            "terraform destroy",
            "kubectl delete",
            "--no-verify",
        )
        for phrase in forbidden:
            with self.subTest(phrase=phrase):
                self.assertNotIn(phrase, self.text)
        self.assertIsNone(re.search(r"(?:^|[;&|()]|\s)sed(?:\s|$)", self.text))
        self.assertIsNone(re.search(r"(?:^|[;&|()]|\s)rm(?:\s|$)", self.text))

    def test_authorization_code_is_bounded_and_only_user_action_required(self) -> None:
        self.assertIn("expires_in", self.text)
        self.assertIn(". <= 900", self.text)
        self.assertIn("Open ${verification_uri} and enter", self.text)
        self.assertIn("EXPECTED_LOGIN: ORESoftware", self.text)
        self.assertIn("Mutation starts only after", self.text)


if __name__ == "__main__":
    unittest.main()
