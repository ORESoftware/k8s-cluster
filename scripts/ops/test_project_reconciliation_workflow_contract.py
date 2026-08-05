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


if __name__ == "__main__":
    unittest.main()
