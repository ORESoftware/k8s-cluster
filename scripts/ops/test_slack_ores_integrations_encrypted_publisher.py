#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-publish-slack-ores-integrations-encrypted-owner.yml"
CONTRACT = ROOT / ".github/workflows/ops-publish-slack-ores-integrations-encrypted-owner-contract.yml"


class SlackOresEncryptedPublisherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.contract = CONTRACT.read_text(encoding="utf-8")

    def test_carrier_is_metadata_only_and_same_repository(self) -> None:
        for snippet in (
            "pull_request_target:",
            "branches: [main]",
            "- .github/slack-ores-integrations-publish-trigger",
            "github.event.pull_request.draft == true",
            "github.event.pull_request.user.login == 'ORESoftware'",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "startsWith(github.event.pull_request.head.ref, 'agent/slack-ores-integrations-publish-')",
            "DO NOT MERGE: publish the canonical slack-ores-integrations repository",
            ".commits == 1",
            ".changed_files == 1",
            ".additions == 6",
            ".deletions == 0",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_marker_pins_exact_source_and_target(self) -> None:
        for snippet in (
            'test "$marker_target" = ORESoftware/slack-ores-integrations',
            'test "$marker_source" = ORESoftware/devops-slack',
            'test "$marker_source_sha" = 7d7b6f2a1018204eba8b727ffa069e6bef31b6a7',
            'test "$marker_default" = main',
            'test "$marker_protocol" = rsa-oaep-sha256-v1',
            '(.status == "ahead" or .status == "identical") and .behind_by == 0',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_permissions_and_checkout_are_bounded(self) -> None:
        permission_match = re.search(
            r"(?ms)^permissions:\n(?P<body>(?:  [^\n]+\n)+)", self.workflow
        )
        self.assertIsNotNone(permission_match)
        assert permission_match is not None
        self.assertEqual(
            {line.strip() for line in permission_match.group("body").splitlines()},
            {
                "contents: read",
                "issues: write",
                "pull-requests: write",
                "statuses: write",
            },
        )
        self.assertIn("ref: main", self.workflow)
        self.assertIn("persist-credentials: false", self.workflow)
        self.assertNotIn("id-token:", self.workflow)

    def test_source_is_exact_and_tested_before_credential_use(self) -> None:
        validate = self.workflow.index("stage=validate-source")
        challenge = self.workflow.index("stage=challenge-bootstrap")
        self.assertLess(validate, challenge)
        for snippet in (
            "SOURCE_SHA: 7d7b6f2a1018204eba8b727ffa069e6bef31b6a7",
            '"https://github.com/${SOURCE_REPOSITORY}.git"',
            'test "$(git -C "$source_root" rev-parse HEAD)" = "$SOURCE_SHA"',
            'npm --prefix "$source_root" install --ignore-scripts --package-lock=false',
            'npm --prefix "$source_root" run dependencies:check',
            'npm --prefix "$source_root" run manifest:check',
            'npm --prefix "$source_root" run check',
            'npm --prefix "$source_root" test',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_challenge_and_response_are_single_use(self) -> None:
        for snippet in (
            "rsa_keygen_bits:3072",
            "openssl rand -hex 24",
            "slack-ores-credential-challenge:",
            "slack-ores-credential-response:",
            "rsa_padding_mode:oaep",
            "rsa_oaep_md:sha256",
            "rsa_mgf1_md:sha256",
            'select(.id > $challenge_id)',
            'select(.user.login == "ORESoftware")',
            'test "${#ciphertext}" -le 8192',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_plaintext_token_is_memory_only_and_erased(self) -> None:
        for snippet in (
            'echo "::add-mask::$owner_token"',
            'export GH_TOKEN="$owner_token"',
            "unset owner_token actor GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN",
        ):
            self.assertIn(snippet, self.workflow)
        self.assertNotIn("upload-artifact", self.workflow)
        self.assertNotIn("actions/cache", self.workflow)
        self.assertNotIn("owner_token >>", self.workflow)
        self.assertNotRegex(self.workflow, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(self.workflow, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_target_creation_and_push_are_idempotent_and_no_force(self) -> None:
        for snippet in (
            "repos/${TARGET_REPOSITORY}",
            "gh api --method POST user/repos",
            'git -C "$SOURCE_ROOT" push target "$SOURCE_SHA:refs/heads/main"',
            "gh auth setup-git --hostname github.com --force",
            'test "$remote_main" = "$SOURCE_SHA"',
            'gh api --method PATCH "repos/${TARGET_REPOSITORY}" -f default_branch=main',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)
        self.assertNotIn("git push --force", self.workflow)
        self.assertNotIn("git push -f", self.workflow)

    def test_feature_branch_adds_runtime_ownership_contract(self) -> None:
        for snippet in (
            "agent/den-1602-canonicalize-repository",
            "config/runtime-integration.json",
            "docs/runtime-ownership.md",
            "test/runtime-ownership.test.js",
            "03f38e876daec61eba587f6cb87393d3cc2a7dac",
            "/slack/commands/ores-claude",
            "/slack/commands/ores-chatgpt",
            'npm --prefix "$SOURCE_ROOT" run manifest:check',
            'npm --prefix "$SOURCE_ROOT" run check',
            'npm --prefix "$SOURCE_ROOT" test',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_live_metadata_refs_pr_and_carrier_cleanup_are_required(self) -> None:
        for snippet in (
            '.owner.login == "ORESoftware"',
            '.name == "slack-ores-integrations"',
            '.default_branch == "main"',
            'test "$main_sha" = "$SOURCE_SHA"',
            'test "$feature_sha" = "$canonical_feature_sha"',
            "ops/slack-ores-integrations-publication",
            'gh api --method PATCH "repos/${REPOSITORY}/pulls/${PR_NUMBER}" -f state=closed',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_contract_is_read_only_and_runs_actionlint_and_tests(self) -> None:
        self.assertIn("permissions:\n  contents: read", self.contract)
        self.assertIn("rhysd/actionlint@sha256:", self.contract)
        self.assertIn("persist-credentials: false", self.contract)
        self.assertIn(
            "python3 -m unittest -v scripts/ops/test_slack_ores_integrations_encrypted_publisher.py",
            self.contract,
        )


if __name__ == "__main__":
    unittest.main()
