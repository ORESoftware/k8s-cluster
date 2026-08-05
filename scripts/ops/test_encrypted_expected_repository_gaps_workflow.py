#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = (
    ROOT
    / ".github/workflows/ops-publish-expected-repository-gaps-encrypted-once.yml"
)
VERIFIER_PATH = ROOT / "scripts/ops/verify_expected_repository_gaps.py"
WORKFLOW = WORKFLOW_PATH.read_text(encoding="utf-8")
VERIFIER = VERIFIER_PATH.read_text(encoding="utf-8")


class EncryptedExpectedRepositoryGapsWorkflowTests(unittest.TestCase):
    def test_workflow_is_manual_trusted_main_only_without_token_inputs(self) -> None:
        self.assertIn("on:\n  workflow_dispatch:\n", WORKFLOW)
        self.assertNotIn("workflow_dispatch:\n    inputs:", WORKFLOW)
        self.assertIn("github.ref == 'refs/heads/main'", WORKFLOW)
        self.assertIn("github.actor == 'ORESoftware'", WORKFLOW)
        self.assertNotIn("PROJECT_SYNC_GITHUB_TOKEN", WORKFLOW)
        self.assertNotIn("GH_PAT", WORKFLOW)

    def test_ephemeral_handoff_uses_rsa_oaep_sha256(self) -> None:
        for snippet in (
            "openssl genpkey",
            "rsa_keygen_bits:4096",
            "rsa_padding_mode:oaep",
            "rsa_oaep_md:sha256",
            "rsa_mgf1_md:sha256",
            "HANDSHAKE_READY",
        ):
            self.assertIn(snippet, WORKFLOW)

    def test_credential_material_is_removed_before_evidence_git_auth(self) -> None:
        mask = WORKFLOW.index("::add-mask::")
        cleanup = WORKFLOW.index("cleanup_credentials\n          trap - EXIT")
        repository_token = WORKFLOW.index('export GH_TOKEN="$GITHUB_TOKEN_VALUE"')
        evidence_checkout = WORKFLOW.index('git fetch origin "$DEFAULT_BRANCH"')
        self.assertLess(mask, cleanup)
        self.assertLess(cleanup, repository_token)
        self.assertLess(repository_token, evidence_checkout)
        self.assertIn("shred -u", WORKFLOW)
        self.assertIn('unset USER_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN', WORKFLOW)
        self.assertIn('delete_file_if_present "$PUBLIC_KEY_PATH"', WORKFLOW)
        self.assertIn('delete_file_if_present "$CIPHERTEXT_PATH"', WORKFLOW)

    def test_exact_read_only_preflight_precedes_live_publication(self) -> None:
        preflight = WORKFLOW.index(
            "verify_expected_repository_gaps.py preflight"
        )
        publisher = WORKFLOW.index(
            "publish_missing_org_repositories_current.py"
        )
        postflight = WORKFLOW.index(
            "verify_expected_repository_gaps.py postflight"
        )
        self.assertLess(preflight, publisher)
        self.assertLess(publisher, postflight)
        self.assertIn("assert_expected_missing", VERIFIER)
        self.assertIn("observed_missing", VERIFIER)
        self.assertIn("repository ID changed", VERIFIER)
        self.assertIn("main changed during publication", VERIFIER)

    def test_exact_four_target_repositories_are_bound_in_verifier(self) -> None:
        for repository in (
            "StreemPilot/streempilot-media-router.rs",
            "hypesiege/hypesiege-scheduler.rs",
            "hypesiege/hypesiege-publishing-worker.rs",
            "hypesiege/hypesiege-analytics.rs",
        ):
            self.assertIn(repository, VERIFIER)
        self.assertIn("repository_count\") != 32", VERIFIER)
        self.assertIn("resolved_gaps", VERIFIER)
        self.assertIn("preserved_count", VERIFIER)

    def test_workflow_does_not_patch_visibility_or_force_product_history(self) -> None:
        self.assertNotIn("gh repo edit", WORKFLOW)
        self.assertNotIn("--visibility", WORKFLOW)
        self.assertNotIn("git push --force ", WORKFLOW)
        self.assertNotIn("git push -f ", WORKFLOW)
        self.assertIn("git push --force-with-lease", WORKFLOW)
        self.assertNotIn("--admin", WORKFLOW)
        self.assertNotIn("gh pr merge", WORKFLOW)

    def test_contracts_run_before_encrypted_mutation_step(self) -> None:
        validation = WORKFLOW.index(
            "Validate trusted source and fail-closed contracts"
        )
        encrypted_step = WORKFLOW.index(
            "Publish exact reviewed gaps and prepare evidence"
        )
        self.assertLess(validation, encrypted_step)
        for test in (
            "test_verify_expected_repository_gaps.py -v",
            "test_repository_fleet_visibility.py -v",
            "test_repository_fleet_remote_state.py -v",
            "test_private_fleet_publisher_contract.py -v",
            "test_encrypted_expected_repository_gaps_workflow.py -v",
        ):
            self.assertIn(test, WORKFLOW)

    def test_actions_are_immutable_and_concurrency_matches_other_publishers(self) -> None:
        self.assertIn(
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            WORKFLOW,
        )
        self.assertIn(
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            WORKFLOW,
        )
        self.assertIn(
            "group: ops-publish-missing-organization-repositories",
            WORKFLOW,
        )
        self.assertIn("cancel-in-progress: false", WORKFLOW)


if __name__ == "__main__":
    unittest.main()
