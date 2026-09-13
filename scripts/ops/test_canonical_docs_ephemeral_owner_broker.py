#!/usr/bin/env python3
"""Static safety contracts for the Canonical Docs ephemeral credential broker."""

from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-canonical-docs-ephemeral-owner-broker.yml"
CONTRACT = ROOT / ".github/workflows/ops-canonical-docs-ephemeral-owner-broker-contract.yml"


class CanonicalDocsBrokerContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.contract = CONTRACT.read_text(encoding="utf-8")

    def test_trigger_is_exact_draft_same_repository_carrier(self) -> None:
        for text in (
            "pull_request_target:",
            ".github/canonical-docs-ephemeral-publish-trigger",
            "github.event.pull_request.draft == true",
            "github.event.pull_request.user.login == 'ORESoftware'",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "agent/canonical-docs-ephemeral-publish-",
            "DO NOT MERGE: publish canonical-docs with encrypted credential",
            "commits == 1",
            "changed_files == 1",
            "additions == 4",
            "deletions == 0",
        ):
            self.assertIn(text, self.workflow)

    def test_credential_is_one_time_encrypted_and_memory_bounded(self) -> None:
        for text in (
            "openssl genpkey",
            "rsa_keygen_bits:3072",
            "rsa_padding_mode:oaep",
            "rsa_oaep_md:sha256",
            "rsa_mgf1_md:sha256",
            "::add-mask::",
        ):
            self.assertIn(text, self.workflow)
        self.assertNotIn("meta-agent-credential", self.workflow)
        self.assertIn("canonical-docs-credential-challenge", self.workflow)
        self.assertIn("canonical-docs-credential-response", self.workflow)
        self.assertIn('test "${#ciphertext}" -le 8192', self.workflow)
        self.assertNotIn("secrets.", self.workflow)
        self.assertNotIn("GITHUB_ENV", self.workflow)
        self.assertNotIn("upload-artifact", self.workflow)

    def test_identity_source_and_target_contracts_are_pinned(self) -> None:
        for text in (
            "TARGET_REPOSITORY: canonical-cloud/canonical-docs",
            "EXPECTED_MAIN: 1848835599049ca41f68a079b5ac04f7d360fe87",
            "EXPECTED_FEATURE: 54aa2efcbcfd21020614cbecccea5a907ead813f",
            "BUNDLE_SHA256: 3169c190a11f8889ca0a29d5db58acabae1e3b887cc302407ccc350d3a461828",
            "VERIFY_SHA256: 8ae154019b70d2f1c117b3ed4882405042b0da6c7792709df65bd37e88746e9c",
            "PUBLISHER_SHA256: 0dbbff9b1859fcea7cdfed7b97df404ae06bb3f4cf8f414ec956cd63be2f4b15",
            "/user/memberships/orgs/canonical-cloud",
            'test "$owner_login" = ORESoftware',
            'test "$membership" = admin:active',
        ):
            self.assertIn(text, self.workflow)

    def test_workflow_never_checks_out_or_executes_carrier_code(self) -> None:
        self.assertNotRegex(self.workflow, r"uses:\s*actions/checkout@")
        self.assertNotIn("CARRIER_HEAD_SHA", self.workflow)
        self.assertIn("TRUSTED_SHA", self.workflow)
        self.assertIn(
            "repos/${REPOSITORY}/git/trees/${source_tree_sha}?recursive=1",
            self.workflow,
        )

    def test_cleanup_and_carrier_disposition_are_non_destructive(self) -> None:
        self.assertIn("shutil.rmtree", self.workflow)
        self.assertNotRegex(self.workflow, r"(^|[;&|]\s*)rm\s", re.MULTILINE)
        self.assertIn(
            '--method PATCH "repos/${REPOSITORY}/pulls/${PR_NUMBER}"',
            self.workflow,
        )
        self.assertIn("-f state=closed", self.workflow)
        self.assertNotIn("gh pr merge", self.workflow)

    def test_contract_runs_actionlint_unit_tests_and_live_source_verification(self) -> None:
        for text in (
            "docker://rhysd/actionlint@sha256:",
            "python3 -m py_compile",
            "python3 -m unittest -v",
            "verify_canonical_docs_bundle.py",
            "test_publish_canonical_docs.py",
            "test_canonical_docs_ephemeral_owner_broker.py",
            "git diff --check",
        ):
            self.assertIn(text, self.contract)
        self.assertIn("permissions:\n  contents: read", self.contract)


if __name__ == "__main__":
    unittest.main()
