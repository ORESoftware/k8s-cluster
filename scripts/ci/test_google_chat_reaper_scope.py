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
    "tools/google-chat-space-export/REAPER_HARDENING.md",
    "tools/google-chat-space-export/reaper-core.mjs",
    "tools/google-chat-space-export/test/reaper-review-disposition.test.mjs",
]


class GoogleChatReaperScopeTests(unittest.TestCase):
    def test_exact_reaper_surface_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/repo-check-scope-contract.yml",
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
            [REAPER_FILES[1], "README.md"],
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


if __name__ == "__main__":
    unittest.main()
