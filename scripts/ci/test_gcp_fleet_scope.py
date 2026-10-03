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


class GcpFleetScopeTests(unittest.TestCase):
    def test_exact_gcp_fleet_schema_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            ["remote/gcp/fleet-rust-service-target.schema.json"],
        )
        self.assertTrue(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "credential_free_contract_only_no_private_gitlinks",
            result["reason"],
        )

    def test_gcp_contract_bundle_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/gcp-fleet-target-contract.yml",
                "catalog/namespaces/migration-manifest.json",
                "remote/gcp/fleet-rust-service-target.schema.json",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_gcp_fleet_scope.py",
                "tests/gcp_fleet_target_contract_test.py",
            ],
        )
        self.assertTrue(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])

    def test_neighboring_unknown_gcp_file_still_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            ["remote/gcp/unreviewed-deployment-target.schema.json"],
        )
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_exact_schema_mixed_with_unknown_gcp_file_fails_closed(self):
        result = MODULE.classify(
            "pull_request",
            [
                "remote/gcp/fleet-rust-service-target.schema.json",
                "remote/gcp/unreviewed-deployment-target.schema.json",
            ],
        )
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])


if __name__ == "__main__":
    unittest.main()
