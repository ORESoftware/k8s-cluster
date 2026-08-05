#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-publish-four-org-fleet-encrypted-owner.yml"
CONTRACT = ROOT / ".github/workflows/ops-publish-four-org-fleet-encrypted-owner-contract.yml"


class FourOrgEncryptedPublisherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.contract = CONTRACT.read_text(encoding="utf-8")

    def test_trigger_is_metadata_only_and_same_repository(self) -> None:
        for snippet in (
            "pull_request_target:",
            "branches: [main]",
            "- .github/four-org-encrypted-publish-trigger",
            "github.event.pull_request.draft == true",
            "github.event.pull_request.user.login == 'ORESoftware'",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "startsWith(github.event.pull_request.head.ref, 'agent/four-org-encrypted-publish-')",
            "DO NOT MERGE: publish the reviewed four-organization fleet",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_carrier_shape_and_ancestor_checks_are_exact(self) -> None:
        for snippet in (
            ".commits == 1",
            ".changed_files == 1",
            ".additions == 5",
            ".deletions == 0",
            '.[0].filename == $path',
            '.[0].status == "added"',
            '.[0].changes == 5',
            'test "$marker_target" = four-org-48-repository-fleet',
            'test "$marker_protocol" = rsa-oaep-sha256-v1',
            'test "$marker_repositories" = 48',
            'test "$marker_pull_requests" = 20',
            '(.status == "ahead" or .status == "identical") and .behind_by == 0',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_workflow_permissions_are_bounded(self) -> None:
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
        self.assertNotIn("actions: write", self.workflow)

    def test_fleet_is_reconstructed_and_tested_from_trusted_main(self) -> None:
        for snippet in (
            "ref: main",
            "persist-credentials: false",
            "bootstrap_four_org_fleets.py.xz.b64.part*",
            "repair_four_org_generator.py",
            "four-org-additions-overlay-2026-08-04.tar.xz.b64.part*",
            "5787afe68091b62d6cbde1d9e094f663807742de742d3567e781365ba01779a7",
            "prepare_four_org_fleet.sh",
            'printf \'FLEET_ROOT=%s\\n\' "$fleet_root" >> "$GITHUB_ENV"',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_challenge_is_ephemeral_rsa_oaep_sha256(self) -> None:
        for snippet in (
            "rsa_keygen_bits:3072",
            "openssl rand -hex 24",
            "four-org-credential-challenge:",
            "four-org-credential-response:",
            "rsa_padding_mode:oaep",
            "rsa_oaep_md:sha256",
            "rsa_mgf1_md:sha256",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_response_is_newer_owner_authored_and_bounded(self) -> None:
        for snippet in (
            'select(.id > $challenge_id)',
            'select(.user.login == "ORESoftware")',
            'select(.body | startswith($marker + "\\n"))',
            "test \"$(grep -c '^ciphertext-base64=' <<<\"$response_body\")\" -eq 1",
            'test "${#ciphertext}" -le 8192',
            "for _ in $(seq 1 180); do",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_plaintext_token_is_masked_memory_only_and_erased(self) -> None:
        for snippet in (
            'echo "::add-mask::$owner_token"',
            'export GH_TOKEN="$owner_token"',
            "unset owner_token actor membership GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN",
        ):
            self.assertIn(snippet, self.workflow)
        self.assertNotIn("upload-artifact", self.workflow)
        self.assertNotIn("actions/cache", self.workflow)
        self.assertNotIn("owner_token >>", self.workflow)
        self.assertNotRegex(self.workflow, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(self.workflow, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_owner_and_four_org_memberships_precede_publication(self) -> None:
        identity = self.workflow.index("stage=validate-owner-identity")
        memberships = self.workflow.index("stage=validate-owner-memberships")
        publish = self.workflow.index('"$FLEET_ROOT/scripts/publish-all.sh" "$FLEET_ROOT"')
        self.assertLess(identity, memberships)
        self.assertLess(memberships, publish)
        self.assertIn('test "$actor" = ORESoftware', self.workflow)
        for org in (
            "apostille-me",
            "evento-globolo",
            "hacker-house-medellin",
            "embedded-alerts",
        ):
            self.assertIn(org, self.workflow)
        self.assertIn('test "$membership" = active:admin', self.workflow)

    def test_remote_result_counts_and_carrier_cleanup_are_required(self) -> None:
        for snippet in (
            'test "$repository_count" = 48',
            'test "$pull_request_count" = 20',
            "FOUR_ORG_PUBLICATION_COMPLETE",
            "ops/four-org-encrypted-publication",
            'gh api --method PATCH "repos/${REPOSITORY}/pulls/${PR_NUMBER}" -f state=closed',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_contract_workflow_is_read_only_and_runs_actionlint_and_tests(self) -> None:
        self.assertIn("permissions:\n  contents: read", self.contract)
        self.assertIn("docker://rhysd/actionlint@sha256:", self.contract)
        self.assertIn(
            "python3 -m unittest -v scripts/ops/test_four_org_encrypted_publisher.py",
            self.contract,
        )
        self.assertIn("persist-credentials: false", self.contract)


if __name__ == "__main__":
    unittest.main()
