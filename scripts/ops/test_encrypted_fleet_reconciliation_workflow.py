#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = ROOT / ".github/workflows/ops-sync-org-project-docs-once.yml"
WORKFLOW = WORKFLOW_PATH.read_text(encoding="utf-8")


class EncryptedFleetReconciliationWorkflowTests(unittest.TestCase):
    def test_workflow_uses_ephemeral_rsa_oaep_handoff(self) -> None:
        self.assertIn("openssl genpkey", WORKFLOW)
        self.assertIn("rsa_keygen_bits:4096", WORKFLOW)
        self.assertIn("rsa_padding_mode:oaep", WORKFLOW)
        self.assertIn("rsa_oaep_md:sha256", WORKFLOW)
        self.assertIn("rsa_mgf1_md:sha256", WORKFLOW)
        self.assertIn("HANDSHAKE_READY", WORKFLOW)
        self.assertNotIn("PROJECT_SYNC_GITHUB_TOKEN", WORKFLOW)
        self.assertNotIn("workflow_dispatch:\n    inputs:", WORKFLOW)

    def test_credential_material_is_masked_and_deleted_before_evidence_commit(self) -> None:
        mask = WORKFLOW.index("::add-mask::")
        cleanup = WORKFLOW.index("cleanup_credentials\n          trap - EXIT")
        evidence_checkout = WORKFLOW.index("git fetch origin \"$DEFAULT_BRANCH\"")
        self.assertLess(mask, cleanup)
        self.assertLess(cleanup, evidence_checkout)
        self.assertIn("shred -u", WORKFLOW)
        self.assertIn("delete_file_if_present \"$PUBLIC_KEY_PATH\"", WORKFLOW)
        self.assertIn("delete_file_if_present \"$CIPHERTEXT_PATH\"", WORKFLOW)

    def test_contracts_run_before_repository_or_project_mutation(self) -> None:
        validation = WORKFLOW.index(
            "Validate exact trusted source and fail-closed contracts"
        )
        publisher = WORKFLOW.index(
            "python3 scripts/ops/publish_missing_org_repositories_current.py"
        )
        project_sync = WORKFLOW.index(
            "python3 scripts/ops/sync_org_project_docs_rate_aware.py"
        )
        self.assertLess(validation, publisher)
        self.assertLess(publisher, project_sync)
        self.assertIn("test_sync_org_project_docs_rate_aware.py -v", WORKFLOW)
        self.assertIn("test_private_fleet_publisher_contract.py -v", WORKFLOW)

    def test_reconciliation_requires_exact_private_repositories_and_64_orgs(self) -> None:
        for repository in (
            "StreemPilot/streempilot-media-router.rs",
            "hypesiege/hypesiege-scheduler.rs",
            "hypesiege/hypesiege-publishing-worker.rs",
            "hypesiege/hypesiege-analytics.rs",
        ):
            self.assertIn(repository, WORKFLOW)
        self.assertIn('repository.get("private") is not True', WORKFLOW)
        self.assertIn('repository.get("default_branch") != "main"', WORKFLOW)
        self.assertIn("--expected-count 64", WORKFLOW)
        self.assertIn("--validate-only", WORKFLOW)

    def test_workflow_has_no_forceful_product_history_mutation(self) -> None:
        self.assertNotIn("git push --force ", WORKFLOW)
        self.assertNotIn("git push -f ", WORKFLOW)
        self.assertNotIn("gh repo edit", WORKFLOW)
        self.assertNotIn("--visibility", WORKFLOW)
        self.assertIn("--force-with-lease", WORKFLOW)
        self.assertIn("persist-credentials: false", WORKFLOW)
        self.assertIn("cancel-in-progress: false", WORKFLOW)


if __name__ == "__main__":
    unittest.main()
