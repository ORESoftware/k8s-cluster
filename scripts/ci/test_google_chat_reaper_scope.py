#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("classify_repo_check_scope.py")
SPEC = importlib.util.spec_from_file_location("repo_check_scope", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

REAPER_FILES = [
    ".github/workflows/google-chat-daily-reconciliation.yml",
    "tools/google-chat-space-export/REAPER_HARDENING.md",
    "tools/google-chat-space-export/reaper-contracts.mjs",
    "tools/google-chat-space-export/reaper-core.mjs",
    "tools/google-chat-space-export/reconciliation-receipt.mjs",
    "tools/google-chat-space-export/test/reaper-contracts.test.mjs",
    "tools/google-chat-space-export/test/reaper-review-disposition.test.mjs",
    "tools/google-chat-space-export/test/reconciliation-review-reasons.test.mjs",
    "tools/google-chat-space-export/contracts/main.tsp",
    "tools/google-chat-space-export/contracts/authored.schema.json",
    "tools/google-chat-space-export/contracts/instances/CoverageEvidence/valid/minimal.json",
    "tools/google-chat-space-export/contracts/instances/CoverageEvidence/invalid/extra-field.json",
    "tools/google-chat-space-export/contracts/instances/ReaperSummary/valid/minimal.json",
    "tools/google-chat-space-export/contracts/instances/ReaperSummary/invalid/negative-count.json",
    "tools/google-chat-space-export/contracts/instances/ReconciliationReceipt/valid/minimal.json",
    "tools/google-chat-space-export/contracts/instances/ReconciliationReceipt/invalid/missing-receipt-id.json",
]


class GoogleChatReaperScopeTests(unittest.TestCase):
    def test_exact_reaper_surface_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/namespace-migration-contract.yml",
                ".github/workflows/repo-check-scope-contract.yml",
                "catalog/namespaces/migration-manifest.json",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_google_chat_reaper_scope.py",
                *REAPER_FILES,
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertTrue(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "credential_free_contract_only_no_private_gitlinks",
            result["reason"],
        )

    def test_reaper_surface_mixed_with_unknown_file_fails_closed(self):
        result = MODULE.classify(
            "pull_request",
            [REAPER_FILES[3], "README.md"],
        )
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_neighboring_chat_export_files_are_not_implicitly_allowlisted(self):
        result = MODULE.classify(
            "pull_request",
            ["tools/google-chat-space-export/fetch-bridge-pages.mjs"],
        )
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_contract_prefix_is_exact_and_does_not_match_siblings(self):
        self.assertTrue(
            MODULE.is_credential_free_contract_path(
                "tools/google-chat-space-export/contracts/main.tsp"
            )
        )
        self.assertFalse(
            MODULE.is_credential_free_contract_path(
                "tools/google-chat-space-export/contracts-extra/main.tsp"
            )
        )

    def test_non_pull_request_runs_still_require_full_fleet_checks(self):
        result = MODULE.classify("push", REAPER_FILES)
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])
        self.assertEqual(
            "non_pull_request_runs_are_full_fleet_checks",
            result["reason"],
        )


if __name__ == "__main__":
    unittest.main()
