#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-meta-agent-ephemeral-owner-broker.yml"
DOC = ROOT / "docs/operations/meta-agent-ephemeral-credential-publication.md"


class MetaAgentEphemeralOwnerBrokerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.doc = DOC.read_text(encoding="utf-8")

    def test_trigger_and_permissions_are_metadata_only_and_bounded(self) -> None:
        self.assertIn("pull_request_target:", self.workflow)
        self.assertIn("branches: [main]", self.workflow)
        self.assertIn("- .github/meta-agent-ephemeral-publish-trigger", self.workflow)
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
        self.assertNotIn("id-token:", self.workflow)
        self.assertNotIn("actions/checkout@", self.workflow)

    def test_carrier_identity_and_shape_are_exact(self) -> None:
        required = (
            "github.event.pull_request.draft == true",
            "github.event.pull_request.user.login == 'ORESoftware'",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "startsWith(github.event.pull_request.head.ref, 'agent/meta-agent-ephemeral-publish-')",
            "DO NOT MERGE: publish exact Meta Agent repository with encrypted credential",
            ".commits == 1",
            ".changed_files == 1",
            ".additions == 4",
            ".deletions == 0",
            '.[0].filename == $path',
            '.[0].status == "added"',
            '.[0].changes == 4',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_carrier_is_ancestor_safe_and_pins_reviewed_bundle(self) -> None:
        required = (
            'test "$marker_main" = "$parent_sha"',
            '(.status == "ahead" or .status == "identical") and .behind_by == 0',
            "SOURCE_SHA: 55ee15c190b7cfa4e075f6984c7cb551acd4b9d3",
            "BUNDLE_SHA256: 1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031",
            "PUBLISHER_SHA256: e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278",
            "EXPECTED_MAIN: 4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1",
            "EXPECTED_FEATURE: 789d48039da232faed985d4f8de176959f117e08",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_challenge_uses_ephemeral_rsa_oaep_sha256(self) -> None:
        required = (
            "rsa_keygen_bits:3072",
            "openssl rand -hex 24",
            "meta-agent-credential-challenge:",
            "meta-agent-credential-response:",
            "rsa_padding_mode:oaep",
            "rsa_oaep_md:sha256",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_response_is_owner_authored_newer_than_challenge_and_bounded(self) -> None:
        required = (
            'select(.id > $challenge_id)',
            'select(.user.login == "ORESoftware")',
            'select(.body | startswith($marker + "\\n"))',
            "test \"$(grep -c '^ciphertext-base64=' <<<\"$response_body\")\" -eq 1",
            'test "${#ciphertext}" -le 8192',
            "for _ in $(seq 1 180); do",
            "sleep 5",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_plaintext_credential_is_memory_only_and_masked(self) -> None:
        self.assertIn('echo "::add-mask::$owner_token"', self.workflow)
        self.assertIn('export GH_TOKEN="$owner_token"', self.workflow)
        self.assertIn('unset owner_token GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN', self.workflow)
        self.assertNotIn("GITHUB_ENV", self.workflow)
        self.assertNotIn("upload-artifact", self.workflow)
        self.assertNotRegex(self.workflow, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(self.workflow, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_owner_identity_and_org_admin_are_verified_before_mutation(self) -> None:
        identity = self.workflow.index("gh api user --jq '.login'")
        membership = self.workflow.index("/user/memberships/orgs/meta-agents-demo")
        publication = self.workflow.index("python3 \"$publisher\" \"$bundle\"")
        self.assertLess(identity, publication)
        self.assertLess(membership, publication)
        self.assertIn('test "$membership" = admin:active', self.workflow)

    def test_exact_bundle_and_live_refs_are_verified(self) -> None:
        required = (
            'sha256sum --check --strict',
            'git bundle verify "$bundle"',
            'test "$observed_heads" = "$expected_heads"',
            'test "$main_sha" = "$EXPECTED_MAIN"',
            'test "$feature_sha" = "$EXPECTED_FEATURE"',
            '.visibility == "public"',
            '.private == false',
            '.default_branch == "main"',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_no_force_or_generated_replacement_history_path_exists(self) -> None:
        self.assertNotIn("git push --force", self.workflow)
        self.assertNotIn("git push -f", self.workflow)
        self.assertNotIn('"--force"', self.workflow)
        self.assertIn("publish_meta_control_plane.py", self.workflow)

    def test_review_pr_and_carrier_cleanup_are_required(self) -> None:
        self.assertIn('gh api --method POST "repos/${TARGET_REPOSITORY}/pulls"', self.workflow)
        self.assertIn("Review PR: ${pr_url}", self.workflow)
        self.assertIn('gh api --method PATCH "repos/${REPOSITORY}/pulls/${PR_NUMBER}" -f state=closed', self.workflow)

    def test_documentation_names_rotation_and_non_persistence_boundary(self) -> None:
        for phrase in (
            "never committed",
            "ephemeral RSA",
            "rotate the credential",
            "exact recovered Git history",
            "Linear",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, self.doc)


if __name__ == "__main__":
    unittest.main()
