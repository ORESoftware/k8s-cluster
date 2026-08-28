#!/usr/bin/env python3

from __future__ import annotations

import json
import subprocess
import unittest
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
REGISTRY_VALIDATOR = REPOSITORY_ROOT / "scripts/ci/check-github-linear-project-registry.mjs"
LEDGER_PATH = REPOSITORY_ROOT / "docs/org-project-delivery-ledger-2026-08-05.md"


class OrganizationProjectDeliveryLedgerTests(unittest.TestCase):
    def test_ledger_uses_the_canonical_exact_64_registry_contract(self) -> None:
        completed = subprocess.run(
            ["node", str(REGISTRY_VALIDATOR)],
            cwd=REPOSITORY_ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        report = json.loads(completed.stdout)
        self.assertEqual(report["organizationCount"], 64)

        ledger = LEDGER_PATH.read_text(encoding="utf-8")
        self.assertIn(
            "[`ops/portfolio/github-linear-project-registry.tsv`](../ops/portfolio/github-linear-project-registry.tsv)",
            ledger,
        )
        self.assertIn("current 64-organization registry", ledger)
        self.assertNotIn("41-row registry", ledger)
        self.assertNotIn("Daily 41-organization", ledger)

    def test_ledger_keeps_cancelled_run_out_of_completion_evidence(self) -> None:
        ledger = LEDGER_PATH.read_text(encoding="utf-8")

        self.assertIn("31037622675", ledger)
        self.assertIn("was cancelled", ledger)
        self.assertIn("It is not accepted as completion evidence", ledger)
        self.assertIn("Issue #831 and DEN-2242 remain open", ledger)
        self.assertIn("zero rate-limit/error payloads", ledger)


if __name__ == "__main__":
    unittest.main()
