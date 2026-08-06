#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = (
    ROOT
    / ".github/workflows/ops-publish-exact-repository-gaps-encrypted-once.yml"
)


class EncryptedRepositoryGapWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW_PATH.read_text(encoding="utf-8")

    def test_exact_trusted_main_and_immutable_actions(self) -> None:
        self.assertIn("github.ref == 'refs/heads/main'", self.workflow)
        self.assertIn("ref: ${{ github.sha }}", self.workflow)
        self.assertIn("persist-credentials: false", self.workflow)
        self.assertIn(
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            self.workflow,
        )
        self.assertIn(
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            self.workflow,
        )
        self.assertNotIn("uses: actions/checkout@v", self.workflow)
        self.assertNotIn("uses: actions/upload-artifact@v", self.workflow)

    def test_token_is_one_time_rsa_oaep_handoff_not_an_input_or_literal(self) -> None:
        self.assertIn("rsa_keygen_bits:4096", self.workflow)
        self.assertIn("rsa_padding_mode:oaep", self.workflow)
        self.assertIn("rsa_oaep_md:sha256", self.workflow)
        self.assertIn("rsa_mgf1_md:sha256", self.workflow)
        self.assertIn("::add-mask::$USER_TOKEN", self.workflow)
        self.assertIn("shred -u", self.workflow)
        self.assertIn("delete_file_if_present \"$PUBLIC_KEY_PATH\"", self.workflow)
        self.assertIn("delete_file_if_present \"$CIPHERTEXT_PATH\"", self.workflow)
        self.assertNotIn("inputs:\n      token", self.workflow)
        self.assertNotIn("ghp_", self.workflow)
        self.assertNotIn("github_pat_", self.workflow)

    def test_rate_limit_recovery_precedes_identity_and_repository_api_use(self) -> None:
        rate_limit = self.workflow.index("['gh', 'api', 'rate_limit']")
        identity = self.workflow.index('gh api user --jq .login')
        publication = self.workflow.index(
            "python3 scripts/ops/publish_missing_org_repositories_current.py"
        )
        self.assertLess(rate_limit, identity)
        self.assertLess(identity, publication)
        self.assertIn("maximum_wait = 10800", self.workflow)
        self.assertIn("minimum = 1800", self.workflow)

    def test_creation_allowlist_is_exact_and_private_postflight_is_required(self) -> None:
        expected = (
            "CREATION_ALLOWLIST: "
            "StreemPilot/streempilot-media-router.rs,"
            "hypesiege/hypesiege-scheduler.rs,"
            "hypesiege/hypesiege-publishing-worker.rs,"
            "hypesiege/hypesiege-analytics.rs"
        )
        self.assertIn(expected, self.workflow)
        self.assertIn("set(missing) - allowed", self.workflow)
        self.assertIn(
            "refusing to create repositories outside exact allowlist",
            self.workflow,
        )
        self.assertIn("repository.get('private') is not True", self.workflow)
        self.assertIn("repository.get('visibility') != 'private'", self.workflow)
        self.assertIn("repository.get('default_branch') != 'main'", self.workflow)
        self.assertIn("len(head) != 40", self.workflow)
        self.assertIn("VERIFIED_REQUESTED_GAPS {len(evidence)}/4", self.workflow)

    def test_bounded_publisher_and_preservation_contracts_run_before_live_use(self) -> None:
        contract = self.workflow.index(
            "python3 scripts/ops/test_private_fleet_publisher_contract.py -v"
        )
        preflight = self.workflow.index("VERIFIED_PREFLIGHT")
        publication = self.workflow.index(
            "python3 scripts/ops/publish_missing_org_repositories_current.py"
        )
        postflight = self.workflow.index("VERIFIED_REQUESTED_GAPS")
        self.assertLess(contract, preflight)
        self.assertLess(preflight, publication)
        self.assertLess(publication, postflight)
        self.assertIn(
            "VERIFIED private canonical fleet remote state",
            self.workflow,
        )
        self.assertNotIn("git push --force", self.workflow)
        self.assertNotIn("gh repo edit", self.workflow)
        self.assertNotIn("--visibility public", self.workflow)

    def test_evidence_is_retained_in_a_pull_request_and_artifact(self) -> None:
        self.assertIn(
            "agent/DEN-2328-evidence-${GITHUB_RUN_ID}",
            self.workflow,
        )
        self.assertIn("gh pr create", self.workflow)
        self.assertIn(
            "ops(DEN-2328): record encrypted repository-gap evidence",
            self.workflow,
        )
        self.assertIn(
            "encrypted-exact-repository-gaps-${{ github.run_id }}",
            self.workflow,
        )
        self.assertIn("retention-days: 30", self.workflow)
        self.assertNotIn("gh pr merge", self.workflow)
        self.assertNotIn("--admin", self.workflow)


if __name__ == "__main__":
    unittest.main()
