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
        self.assertFalse(result["credential_free_contract_only"])

    def test_governance_only_pull_request_skips_private_gitlinks(self):
        result = MODULE.classify(
            "pull_request",
            [
                "scripts/ops/build_org_project_docs_retry_registry.py",
                "ops/evidence/org-project-docs/audit.json",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])

    def test_current_relationship_publisher_is_governance_only(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-current-org-dotgithub-relationships-ephemeral-publish.yml",
                "docs/operations/org-dotgithub-relationship-publication.md",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/org_repository_relationships_graph.py",
                "scripts/ops/org_repository_relationships_model.py",
                "scripts/ops/org_repository_relationships_render.py",
                "scripts/ops/org_repository_relationships_roles.py",
                "scripts/ops/publish_current_org_repository_relationships.py",
                "scripts/ops/publish_org_repository_relationships.py",
                "tests/ops/test_publish_current_org_repository_relationships.py",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "governance_only_no_private_gitlinks",
            result["reason"],
        )

    def test_den_3286_sealed_publisher_is_governance_only(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-publish-test-org-expansion-20260808.yml",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/publish_test_org_expansion_20260808.py.gz.b64",
                "scripts/ops/test-org-expansion-20260808.json.gz.b64",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "governance_only_no_private_gitlinks",
            result["reason"],
        )

    def test_den_3286_encrypted_recovery_is_governance_only(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-provision-den-3286-pat-recipient-20260809.yml",
                ".github/workflows/ops-publish-den-3286-encrypted-pat-20260809.yml",
                "ops/requests/den-3286-encrypted-pat-20260809.json",
                "ops/requests/den-3286-pat-recipient-20260809.json",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/provision_den_3286_pat_recipient_20260809.sh",
                "scripts/ops/publish_den_3286_with_encrypted_pat_20260809.sh",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "governance_only_no_private_gitlinks",
            result["reason"],
        )

    def test_unreviewed_den_3286_encrypted_recovery_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-provision-den-3286-pat-recipient-20260810.yml",
                "ops/requests/den-3286-pat-recipient-20260810.json",
                "scripts/ops/provision_den_3286_pat_recipient_20260810.sh",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_unreviewed_publisher_path_still_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-publish-test-org-expansion-20260809.yml",
                "scripts/ops/publish_test_org_expansion_20260809.py.gz.b64",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_den_319_rename_alias_contract_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/den-319-private-fleet-contracts.yml",
                ".github/workflows/repo-check-scope-contract.yml",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/repository_rename_alias_guard.py",
                "scripts/ops/test_repository_rename_alias_guard.py",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertTrue(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "credential_free_contract_only_no_private_gitlinks",
            result["reason"],
        )

    def test_credential_free_contract_mixed_with_unknown_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/den-319-private-fleet-contracts.yml",
                "scripts/ops/test_repository_rename_alias_guard.py",
                "README.md",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_scope_control_change_requires_a_payload(self):
        result = MODULE.classify(
            "pull_request",
            [".github/workflows/repo-checks.yml"],
        )
        self.assertTrue(result["private_contracts_required"])
        self.assertFalse(result["credential_free_contract_only"])

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
        self.assertFalse(result["credential_free_contract_only"])

    def test_remote_or_unknown_change_runs_private_contracts(self):
        for path in [
            "remote/tests/general/example.test.ts",
            "scripts/ci/init-submodules-with-report.sh",
            "README.md",
        ]:
            with self.subTest(path=path):
                result = MODULE.classify("pull_request", [path])
                self.assertTrue(result["private_contracts_required"])
                self.assertFalse(result["credential_free_contract_only"])

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
