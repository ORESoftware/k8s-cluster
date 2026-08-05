#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-create-slack-ores-integrations-encrypted-owner.yml"
CONTRACT = ROOT / ".github/workflows/ops-create-slack-ores-integrations-encrypted-owner-contract.yml"


class SlackOresRepositoryCreatorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.contract = CONTRACT.read_text(encoding="utf-8")

    def test_carrier_is_same_repo_metadata_only_and_exact(self) -> None:
        for snippet in (
            "pull_request_target:",
            "branches: [main]",
            "- .github/slack-ores-repository-create-trigger",
            "github.event.pull_request.draft == true",
            "github.event.pull_request.user.login == 'ORESoftware'",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "startsWith(github.event.pull_request.head.ref, 'agent/slack-ores-repository-create-')",
            "DO NOT MERGE: create the empty canonical slack-ores-integrations repository",
            ".commits == 1",
            ".changed_files == 1",
            ".additions == 5",
            ".deletions == 0",
            '.[0].filename == $path',
            '.[0].status == "added"',
            '.[0].changes == 5',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_marker_and_ancestor_checks_are_fail_closed(self) -> None:
        for snippet in (
            'test "$marker_target" = ORESoftware/slack-ores-integrations',
            'test "$marker_visibility" = public',
            'test "$marker_source_sha" = 7d7b6f2a1018204eba8b727ffa069e6bef31b6a7',
            'test "$marker_protocol" = rsa-oaep-sha256-v1',
            'test "$marker_main" = "$parent_sha"',
            '(.status == "ahead" or .status == "identical") and .behind_by == 0',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_permissions_are_bounded(self) -> None:
        match = re.search(r"(?ms)^permissions:\n(?P<body>(?:  [^\n]+\n)+)", self.workflow)
        self.assertIsNotNone(match)
        assert match is not None
        self.assertEqual(
            {line.strip() for line in match.group("body").splitlines()},
            {
                "contents: read",
                "issues: write",
                "pull-requests: write",
                "statuses: write",
            },
        )
        self.assertNotIn("id-token:", self.workflow)
        self.assertNotIn("actions: write", self.workflow)

    def test_challenge_is_one_time_rsa_oaep_and_response_is_bounded(self) -> None:
        for snippet in (
            "rsa_keygen_bits:3072",
            "openssl rand -hex 24",
            "slack-ores-create-credential-challenge:",
            "slack-ores-create-credential-response:",
            "rsa_padding_mode:oaep",
            "rsa_oaep_md:sha256",
            "rsa_mgf1_md:sha256",
            'select(.id > $challenge_id)',
            'select(.user.login == "ORESoftware")',
            'test "${#ciphertext}" -le 8192',
            "for _ in $(seq 1 180); do",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_plaintext_token_is_masked_memory_only_and_erased(self) -> None:
        for snippet in (
            'echo "::add-mask::$owner_token"',
            'export GH_TOKEN="$owner_token"',
            "unset owner_token actor GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN",
        ):
            self.assertIn(snippet, self.workflow)
        for forbidden in (
            "upload-artifact",
            "actions/cache",
            "GITHUB_ENV",
            "owner_token >>",
            "git remote add",
            "git push",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, self.workflow)
        self.assertNotRegex(self.workflow, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(self.workflow, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_actor_validation_precedes_repository_mutation(self) -> None:
        identity = self.workflow.index("stage=validate-owner-identity")
        create = self.workflow.index("stage=create-or-validate-empty-repository")
        self.assertLess(identity, create)
        self.assertIn('test "$actor" = ORESoftware', self.workflow)

    def test_content_creation_retry_is_bounded_and_error_specific(self) -> None:
        retry = self.workflow[
            self.workflow.index("gh_api_content_retry()") :
            self.workflow.index('private_key="$work/private.pem"')
        ]
        for snippet in (
            "local max_attempts=8",
            "local delay_seconds=15",
            "while (( attempt <= max_attempts ))",
            "secondary rate limit|abuse detection|HTTP 429|status 429",
            "if (( attempt >= max_attempts ))",
            "if (( delay_seconds > 120 ))",
            "delay_seconds=120",
            "non-retryable response",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, retry)
        self.assertNotIn("while true", retry)
        self.assertNotIn("until false", retry)
        self.assertIn(
            "gh_api_content_retry create-target-repository --method POST user/repos",
            self.workflow,
        )
        self.assertNotIn("gh_api_content_retry validate-owner-identity", self.workflow)

    def test_existing_repository_must_be_empty_or_exactly_initialized(self) -> None:
        for snippet in (
            '.owner.login == "ORESoftware"',
            '.name == "slack-ores-integrations"',
            '.visibility == "public"',
            '.private == false',
            'test "$branch_count" -eq 0 || test "$branch_state" = "main:7d7b6f2a1018204eba8b727ffa069e6bef31b6a7"',
            'test "$verified_branch_count" -eq 0',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_creation_is_only_mutation_and_carrier_closes_without_merge(self) -> None:
        self.assertIn(
            "gh_api_content_retry create-target-repository --method POST user/repos",
            self.workflow,
        )
        self.assertNotIn("repos/${TARGET_REPOSITORY}/pulls", self.workflow)
        self.assertNotIn("refs/heads/", self.workflow)
        for snippet in (
            "ops/slack-ores-empty-repository-creation",
            'gh api --method PATCH "repos/${REPOSITORY}/pulls/${PR_NUMBER}" -f state=closed',
        ):
            self.assertIn(snippet, self.workflow)

    def test_contract_is_read_only_and_runs_actionlint_and_tests(self) -> None:
        self.assertIn("permissions:\n  contents: read", self.contract)
        self.assertIn("rhysd/actionlint@sha256:", self.contract)
        self.assertIn("persist-credentials: false", self.contract)
        self.assertIn(
            "python3 -m unittest -v scripts/ops/test_slack_ores_repository_creator.py",
            self.contract,
        )


if __name__ == "__main__":
    unittest.main()
