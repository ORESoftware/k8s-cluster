#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).parents[2] / "scripts/ops/audit_org_project_docs_evidence.py"
SPEC = importlib.util.spec_from_file_location("org_project_audit", MODULE_PATH)
assert SPEC and SPEC.loader
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)


def valid_record(org: str = "Example-Org") -> dict:
    return {
        "status": "ok",
        "requested_org": org,
        "canonical_org": org,
        "linear_url": "https://linear.app/denman/project/example-123",
        "project_title": f"{org}-project",
        "project_number": "1",
        "project_url": f"https://github.com/orgs/{org}/projects/1",
        "project_action": "existing",
        "repository_action": "existing",
        "documentation_action": "updated",
        "pull_request": {
            "number": "4",
            "url": f"https://github.com/{org}/.github/pull/4",
            "state": "merged-squash",
        },
        "governance_issue": {
            "number": "1",
            "url": f"https://github.com/{org}/.github/issues/1",
            "project_item_action": "added",
        },
        "error": "",
        "run_stamp": "test",
    }


class EvidenceAuditTests(unittest.TestCase):
    def test_accepts_complete_valid_evidence(self) -> None:
        audit = AUDIT.build_audit(["Example-Org"], [valid_record()])
        self.assertTrue(audit["is_valid"])
        self.assertEqual(audit["valid_records"], 1)
        self.assertEqual(audit["invalid_records"], 0)

    def test_rejects_rate_limit_json_as_canonical_org(self) -> None:
        row = valid_record("example-org")
        row["canonical_org"] = '{"message":"API rate limit exceeded","status":"403"}'
        row["project_title"] = f"{row['canonical_org']}-project"
        row["project_number"] = ""
        row["project_url"] = ""
        row["pull_request"] = {"number": "", "url": "", "state": "open"}
        row["governance_issue"] = {
            "number": "",
            "url": "",
            "project_item_action": "not-attempted",
        }
        audit = AUDIT.build_audit(["example-org"], [row])
        self.assertFalse(audit["is_valid"])
        self.assertIn("invalid-canonical-org", audit["invalid"][0]["reasons"])

    def test_rejects_missing_registry_rows(self) -> None:
        audit = AUDIT.build_audit(["Example-Org", "Second-Org"], [valid_record()])
        self.assertFalse(audit["is_valid"])
        self.assertEqual(audit["missing_requested_orgs"], ["Second-Org"])

    def test_rejects_duplicate_requested_organization(self) -> None:
        audit = AUDIT.build_audit(["Example-Org"], [valid_record(), valid_record()])
        self.assertFalse(audit["is_valid"])
        self.assertEqual(audit["observed_records"], 2)
        self.assertEqual(audit["invalid_records"], 2)


if __name__ == "__main__":
    unittest.main()
