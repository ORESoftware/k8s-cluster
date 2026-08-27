#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-create-slack-ores-integrations-encrypted-owner.yml"
CONTRACT = ROOT / ".github/workflows/ops-create-slack-ores-integrations-encrypted-owner-contract.yml"

CANONICAL_TARGET = "TARGET_REPOSITORY: ORESoftware/slack-ores-integrations"
HONEYPOT_TARGET = "TARGET_REPOSITORY: ORESoftware/honeypot.rs"


class EncryptedOwnerRepositoryCreatorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.contract = CONTRACT.read_text(encoding="utf-8")
        variants = {
            "canonical-slack": CANONICAL_TARGET in cls.workflow,
            "temporary-den-3873-honeypot": HONEYPOT_TARGET in cls.workflow,
        }
        selected = [name for name, enabled in variants.items() if enabled]
        if len(selected) != 1:
            raise AssertionError(
                "the registered encrypted-owner broker must match exactly one reviewed variant"
            )
        cls.variant = selected[0]

    def assert_snippets(self, snippets: tuple[str, ...]) -> None:
        for snippet in snippets:
            with self.subTest(variant=self.variant, snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_only_reviewed_variants_are_accepted(self) -> None:
        self.assertIn(
            self.variant,
            {"canonical-slack", "temporary-den-3873-honeypot"},
        )
        self.assertNotEqual(
            CANONICAL_TARGET in self.workflow,
            HONEYPOT_TARGET in self.workflow,
        )

    def test_carrier_is_same_repo_metadata_only_and_exact(self) -> None:
        self.assert_snippets(
            (
                "pull_request_target:",
                "types: [opened, reopened, synchronize, ready_for_review]",
                "branches: [main]",
                "github.event.pull_request.draft == true",
                "github.event.pull_request.user.login == 'ORESoftware'",
                "github.event.pull_request.head.repo.full_name == github.repository",
                ".commits == 1",
                ".changed_files == 1",
                ".deletions == 0",
                '.[0].filename == $path',
                '.[0].status == "added"',
                '(.status == "ahead" or .status == "identical") and .behind_by == 0',
            )
        )
        self.assertNotIn("actions/checkout", self.workflow)
        self.assertNotIn("github.event.pull_request.head.sha }}", self.workflow)

        if self.variant == "canonical-slack":
            self.assert_snippets(
                (
                    "- .github/slack-ores-repository-create-trigger",
                    "startsWith(github.event.pull_request.head.ref, 'agent/slack-ores-repository-create-')",
                    "DO NOT MERGE: create the empty canonical slack-ores-integrations repository",
                    ".additions == 5",
                    '.[0].changes == 5',
                )
            )
        else:
            self.assert_snippets(
                (
                    "- .github/honeypot-rs-bootstrap-trigger",
                    "github.event.pull_request.head.ref == 'agent/den-3873-honeypot-rs-bootstrap-carrier'",
                    "DO NOT MERGE: bootstrap ORESoftware/honeypot.rs with encrypted credential",
                    ".additions == 4",
                    '.[0].changes == 4',
                )
            )

    def test_marker_and_ancestor_checks_are_fail_closed(self) -> None:
        if self.variant == "canonical-slack":
            self.assert_snippets(
                (
                    'test "$marker_target" = ORESoftware/slack-ores-integrations',
                    'test "$marker_visibility" = public',
                    'test "$marker_source_sha" = 7d7b6f2a1018204eba8b727ffa069e6bef31b6a7',
                    'test "$marker_protocol" = rsa-oaep-sha256-v1',
                    'test "$marker_main" = "$parent_sha"',
                )
            )
        else:
            self.assert_snippets(
                (
                    "'target=ORESoftware/honeypot.rs'",
                    "'linear=DEN-3873'",
                    "'protocol=rsa-oaep-sha256-v1'",
                    "'action=create-public-repository'",
                    'test "$marker" = "$expected"',
                )
            )

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
        for forbidden in (
            "id-token:",
            "actions: write",
            "contents: write",
            "administration: write",
            "packages: write",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, self.workflow)

    def test_challenge_is_one_time_rsa_oaep_and_response_is_bounded(self) -> None:
        self.assert_snippets(
            (
                "rsa_keygen_bits:3072",
                "openssl rand -hex 24",
                "rsa_padding_mode:oaep",
                "rsa_oaep_md:sha256",
                "rsa_mgf1_md:sha256",
                'select(.id > $challenge_id)',
                'select(.user.login == "ORESoftware")',
                'test "${#ciphertext}" -le 8192',
                "for _ in $(seq 1 180); do",
                "sleep 5",
            )
        )
        if self.variant == "canonical-slack":
            self.assert_snippets(
                (
                    "slack-ores-create-credential-challenge:",
                    "slack-ores-create-credential-response:",
                )
            )
        else:
            self.assert_snippets(
                (
                    "den-3873-honeypot-create-challenge:",
                    "den-3873-honeypot-create-response:",
                    'test "${#owner_token}" -ge 20',
                    'test "${#owner_token}" -le 318',
                )
            )

    def test_plaintext_token_is_masked_memory_only_and_erased(self) -> None:
        self.assert_snippets(
            (
                'echo "::add-mask::$owner_token"',
                'export GH_TOKEN="$owner_token"',
                'export GITHUB_REPOSITORY_ADMIN_TOKEN="$owner_token"',
            )
        )
        self.assertRegex(
            self.workflow,
            r"unset[^\n]*owner_token[^\n]*GH_TOKEN[^\n]*GITHUB_TOKEN[^\n]*GITHUB_REPOSITORY_ADMIN_TOKEN",
        )
        for forbidden in (
            "upload-artifact",
            "actions/cache",
            "GITHUB_ENV",
            "GITHUB_OUTPUT <<",
            "owner_token >>",
            "git remote add",
            "git push",
            "workflow_dispatch:",
            "${{ secrets.",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, self.workflow)
        self.assertNotRegex(self.workflow, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(self.workflow, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_actor_validation_precedes_repository_mutation(self) -> None:
        identity = self.workflow.index("stage=validate-owner-identity")
        create_stage = (
            "stage=create-or-validate-empty-repository"
            if self.variant == "canonical-slack"
            else "stage=create-or-validate-repository"
        )
        create = self.workflow.index(create_stage)
        self.assertLess(identity, create)
        if self.variant == "canonical-slack":
            self.assertIn('test "$actor" = ORESoftware', self.workflow)
        else:
            self.assertIn(
                'test "$(gh api user --jq \'.login\')" = ORESoftware',
                self.workflow,
            )

    def test_creation_is_idempotent_and_exact_target(self) -> None:
        self.assert_snippets(
            (
                'gh api "repos/${TARGET_REPOSITORY}"',
                "--method POST user/repos",
                "-F private=false",
                "-F has_issues=true",
                "-F has_projects=false",
                "-F has_wiki=false",
            )
        )
        if self.variant == "canonical-slack":
            self.assert_snippets(
                (
                    "gh_api_content_retry()",
                    "local max_attempts=8",
                    "secondary rate limit|abuse detection|HTTP 429|status 429",
                    "gh_api_content_retry create-target-repository --method POST user/repos",
                    "-f name=slack-ores-integrations",
                    '.name == "slack-ores-integrations"',
                    'test "$branch_count" -eq 0 || test "$branch_state" = "main:7d7b6f2a1018204eba8b727ffa069e6bef31b6a7"',
                    'test "$verified_branch_count" -eq 0',
                )
            )
            self.assertNotIn("while true", self.workflow)
            self.assertNotIn("until false", self.workflow)
        else:
            self.assert_snippets(
                (
                    "-f name=honeypot.rs",
                    '.full_name == "ORESoftware/honeypot.rs"',
                    '.owner.login == "ORESoftware"',
                    '.visibility == "public"',
                    '.private == false',
                    '.archived == false',
                    '.disabled == false',
                    "-f ref=refs/heads/main",
                    "-f default_branch=main",
                    '.default_branch == "main"',
                )
            )

    def test_mutation_surface_is_bounded_and_carrier_closes_without_merge(self) -> None:
        self.assertNotIn("/merge", self.workflow)
        self.assertNotIn("--force", self.workflow)
        self.assertNotIn("DELETE repos/${TARGET_REPOSITORY}", self.workflow)
        self.assertNotIn("repos/${TARGET_REPOSITORY}/actions/secrets", self.workflow)
        self.assertNotIn("repos/${TARGET_REPOSITORY}/hooks", self.workflow)
        self.assertNotIn("orgs/ORESoftware", self.workflow)
        self.assertIn(
            'gh api --method PATCH "repos/${REPOSITORY}/pulls/${PR_NUMBER}"',
            self.workflow,
        )
        if self.variant == "canonical-slack":
            self.assertIn(
                "ops/slack-ores-empty-repository-creation",
                self.workflow,
            )
            self.assertNotIn("refs/heads/", self.workflow)
            self.assertNotIn("repos/${TARGET_REPOSITORY}/pulls", self.workflow)
        else:
            self.assert_snippets(
                (
                    "ops/den-3873-honeypot-repository-creation",
                    'gh api --method PATCH "repos/${TARGET_REPOSITORY}"',
                    'gh api --method POST "repos/${TARGET_REPOSITORY}/git/refs"',
                    'GH_TOKEN="$workflow_token" gh api --method DELETE',
                )
            )

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
