#!/usr/bin/env python3
from __future__ import annotations

import unittest
from pathlib import Path

WORKFLOW = Path(".github/workflows/ops-sync-org-project-docs-rate-aware-once.yml")


class ProjectReconciliationWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.text = WORKFLOW.read_text(encoding="utf-8")

    def test_exact_project_reconciliation_precedes_private_fleet_audit(self) -> None:
        reconcile = self.text.index("- name: Reconcile all registered organization Projects and docs")
        validate = self.text.index("- name: Revalidate exact 64-organization Project evidence")
        private_audit = self.text.index(
            "- name: Audit sealed private repository gaps without blocking Project reconciliation"
        )
        self.assertLess(reconcile, validate)
        self.assertLess(validate, private_audit)

    def test_private_fleet_audit_is_explicitly_advisory(self) -> None:
        start = self.text.index(
            "- name: Audit sealed private repository gaps without blocking Project reconciliation"
        )
        end = self.text.index("- name: Record advisory private repository audit result")
        block = self.text[start:end]
        self.assertIn("continue-on-error: true", block)
        self.assertIn("id: private_repository_audit", block)
        self.assertIn('"blocking": False', self.text)
        self.assertIn('"project_reconciliation_independent": True', self.text)

    def test_exact_64_organization_validation_remains_blocking(self) -> None:
        start = self.text.index("- name: Revalidate exact 64-organization Project evidence")
        end = self.text.index(
            "- name: Audit sealed private repository gaps without blocking Project reconciliation"
        )
        block = self.text[start:end]
        self.assertIn("--expected-count 64", block)
        self.assertIn("--validate-only", block)
        self.assertNotIn("continue-on-error", block)

    def test_credential_handoff_is_unique_per_run_attempt(self) -> None:
        self.assertIn(
            "HANDSHAKE_NONCE: fleet-reconcile-${{ github.run_id }}-${{ github.run_attempt }}",
            self.text,
        )
        self.assertIn(
            "PUBLIC_KEY_PATH: .github/tmp/fleet-reconcile-${{ github.run_id }}-${{ github.run_attempt }}.pub.pem",
            self.text,
        )
        self.assertIn(
            "CIPHERTEXT_PATH: .github/tmp/fleet-reconcile-${{ github.run_id }}-${{ github.run_attempt }}.enc.b64",
            self.text,
        )
        self.assertIn("IDENTITY_MAX_WAIT_SECONDS: 1800", self.text)
        self.assertIn("IDENTITY_RETRY_INITIAL_SECONDS: 60", self.text)
        self.assertIn("IDENTITY_RETRY_MAX_SECONDS: 300", self.text)

    def test_fifth_gap_and_alias_guard_are_explicitly_covered(self) -> None:
        self.assertIn(
            "- scripts/ops/repository_rename_alias_guard.py", self.text
        )
        self.assertIn(
            "- scripts/ops/test_project_reconciliation_workflow_contract.py",
            self.text,
        )
        self.assertIn(
            "scripts/ops/repository_rename_alias_guard.py", self.text
        )
        self.assertIn(
            "python3 scripts/ops/test_project_reconciliation_workflow_contract.py -v",
            self.text,
        )
        self.assertIn(
            '"StreemPilot/streempilot-flutter-app"', self.text
        )
        self.assertIn(
            'VERIFIED_REQUESTED_GAPS {len(evidence)}/5', self.text
        )
        self.assertNotIn(
            'VERIFIED_REQUESTED_GAPS {len(evidence)}/4', self.text
        )


if __name__ == "__main__":
    unittest.main()
