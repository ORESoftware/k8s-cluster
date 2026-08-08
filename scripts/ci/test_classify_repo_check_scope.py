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


class RepoCheckScopeTests(unittest.TestCase):
    def test_non_pull_request_always_runs_private_contracts(self):
        result = MODULE.classify("push", ["docs/readme.md"])
        self.assertTrue(result["private_contracts_required"])

    def test_governance_only_pull_request_skips_private_gitlinks(self):
        result = MODULE.classify(
            "pull_request",
            [
                "scripts/ops/build_org_project_docs_retry_registry.py",
                "ops/evidence/org-project-docs/audit.json",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["private_contracts_required"])

    def test_scope_control_change_requires_a_governance_payload(self):
        result = MODULE.classify(
            "pull_request",
            [".github/workflows/repo-checks.yml"],
        )
        self.assertTrue(result["private_contracts_required"])

    def test_scope_control_plus_governance_payload_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/repo-checks.yml",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/test_build_org_project_docs_retry_registry.py",
            ],
        )
        self.assertFalse(result["private_contracts_required"])

    def test_remote_or_unknown_change_runs_private_contracts(self):
        for path in [
            "remote/tests/general/example.test.ts",
            "scripts/ci/init-submodules-with-report.sh",
            "README.md",
        ]:
            with self.subTest(path=path):
                result = MODULE.classify("pull_request", [path])
                self.assertTrue(result["private_contracts_required"])

    def test_empty_pull_request_fails_closed(self):
        with self.assertRaises(MODULE.ScopeError):
            MODULE.classify("pull_request", [])

    def test_unsafe_paths_fail_closed(self):
        for path in ["../escape", "/absolute", "windows\\path"]:
            with self.subTest(path=path):
                with self.assertRaises(MODULE.ScopeError):
                    MODULE.classify("pull_request", [path])


if __name__ == "__main__":
    unittest.main()
