#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("sync_org_project_docs_rate_aware.py")
SPEC = importlib.util.spec_from_file_location("rate_aware_sync", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def valid_result(
    org: str = "athlet-o",
    linear_url: str = "https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb",
):
    return {
        "status": "ok",
        "requested_org": org,
        "canonical_org": org,
        "linear_url": linear_url,
        "project_title": f"{org}-project",
        "project_number": "1",
        "project_url": f"https://github.com/orgs/{org}/projects/1",
        "project_action": "existing",
        "repository_action": "existing",
        "documentation_action": "updated",
        "pull_request": {
            "number": "7",
            "url": f"https://github.com/{org}/.github/pull/7",
            "state": "merged-squash",
        },
        "governance_issue": {
            "number": "1",
            "url": f"https://github.com/{org}/.github/issues/1",
            "project_item_action": "existing",
        },
        "error": "",
        "run_stamp": "20260805T120000Z",
    }


class RateAwareSyncContractTests(unittest.TestCase):
    def test_valid_result_is_accepted(self):
        row = MODULE.RegistryRow(
            "athlet-o",
            "https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb",
        )
        self.assertEqual(MODULE.validate_result(valid_result(), row)["status"], "ok")

    def test_rate_limit_json_cannot_be_used_as_an_org_login(self):
        row = MODULE.RegistryRow(
            "athlet-o",
            "https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb",
        )
        result = valid_result()
        result["canonical_org"] = (
            '{"message":"API rate limit exceeded for user ID 1",'
            '"documentation_url":"https://docs.github.com/rest/using-the-rest-api/rate-limits"}'
        )
        with self.assertRaisesRegex(
            MODULE.ReconcileError, "canonical organization is invalid"
        ):
            MODULE.validate_result(result, row)

    def test_updated_documentation_must_be_merged(self):
        row = MODULE.RegistryRow(
            "athlet-o",
            "https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb",
        )
        result = valid_result()
        result["pull_request"]["state"] = "auto-merge-enabled"
        with self.assertRaisesRegex(
            MODULE.ReconcileError, "documentation PR is not merged"
        ):
            MODULE.validate_result(result, row)

    def test_aggregate_requires_exact_unique_registry_coverage(self):
        rows = [
            MODULE.RegistryRow(
                "athlet-o",
                "https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb",
            ),
            MODULE.RegistryRow(
                "cliptown",
                "https://linear.app/denman/project/githubcomcliptown-123456789abc",
            ),
        ]
        with self.assertRaisesRegex(MODULE.ReconcileError, "incomplete"):
            MODULE.validate_results([valid_result()], rows)

        duplicate = [valid_result(), valid_result()]
        with self.assertRaisesRegex(MODULE.ReconcileError, "duplicate"):
            MODULE.validate_results(duplicate, [rows[0]])

    def test_registry_rejects_duplicates_and_wrong_count(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "registry.tsv"
            path.write_text(
                "organization\tlinear_url\n"
                "athlet-o\thttps://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb\n"
                "ATHLET-O\thttps://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(MODULE.ReconcileError, "duplicate"):
                MODULE.load_registry(path)
            with self.assertRaisesRegex(MODULE.ReconcileError, "expected 64"):
                MODULE.load_registry(path, expected_count=64)

    def test_failure_evidence_cannot_validate_as_completion(self):
        rows = [
            MODULE.RegistryRow(
                "athlet-o",
                "https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb",
            )
        ]
        failed = {
            "status": "failed",
            "requested_org": "athlet-o",
            "canonical_org": "",
            "linear_url": rows[0].linear_url,
            "error": "rate limit",
        }
        with self.assertRaisesRegex(MODULE.ReconcileError, "status is not ok"):
            MODULE.validate_results([failed], rows)


if __name__ == "__main__":
    unittest.main()
