#!/usr/bin/env python3

from __future__ import annotations

import copy
import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "validate_org_project_docs_evidence.py"
SPEC = importlib.util.spec_from_file_location("validator", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


def valid_row(org: str = "example-org") -> dict:
    return {
        "status": "ok",
        "requested_org": org,
        "canonical_org": org,
        "linear_url": "https://linear.app/denman/project/example",
        "project_title": f"{org}-project",
        "project_number": "1",
        "project_url": f"https://github.com/orgs/{org}/projects/1",
        "project_action": "existing",
        "repository_action": "existing",
        "documentation_action": "updated",
        "pull_request": {
            "number": "2",
            "url": f"https://github.com/{org}/.github/pull/2",
            "state": "merged-squash",
        },
        "governance_issue": {
            "number": "1",
            "url": f"https://github.com/{org}/.github/issues/1",
            "project_item_action": "existing",
        },
        "error": "",
        "run_stamp": "test-run",
    }


class EvidenceValidatorTests(unittest.TestCase):
    def test_valid_row_passes(self) -> None:
        self.assertEqual(VALIDATOR.validate_results([valid_row()], 1), [])

    def test_unchanged_documentation_without_pr_passes(self) -> None:
        row = valid_row()
        row["documentation_action"] = "unchanged"
        row["pull_request"] = {"number": "", "url": "", "state": "not-needed"}
        self.assertEqual(VALIDATOR.validate_results([row], 1), [])

    def test_rate_limit_payload_fails_closed(self) -> None:
        row = valid_row()
        row["canonical_org"] = '{"message":"API rate limit exceeded","status":"403"}'
        row["project_title"] = f"{row['canonical_org']}-project"
        errors = VALIDATOR.validate_results([row], 1)
        self.assertTrue(any("rate-limit/error payload" in error for error in errors))
        self.assertTrue(any("invalid canonical_org" in error for error in errors))

    def test_blank_landing_evidence_cannot_be_ok(self) -> None:
        row = valid_row()
        row["project_number"] = ""
        row["project_url"] = ""
        row["pull_request"] = {"number": "", "url": "", "state": "open"}
        row["governance_issue"] = {
            "number": "",
            "url": "",
            "project_item_action": "not-attempted",
        }
        errors = VALIDATOR.validate_results([row], 1)
        self.assertTrue(any("invalid project_number" in error for error in errors))
        self.assertTrue(any("numeric PR number" in error for error in errors))
        self.assertTrue(any("governance issue number" in error for error in errors))

    def test_duplicate_organization_fails(self) -> None:
        first = valid_row()
        second = copy.deepcopy(first)
        errors = VALIDATOR.validate_results([first, second], 2)
        self.assertTrue(any("duplicate requested_org" in error for error in errors))
        self.assertTrue(any("duplicate canonical_org" in error for error in errors))

    def test_requested_and_canonical_must_match_case_insensitively(self) -> None:
        row = valid_row("StreemPilot")
        row["canonical_org"] = "streamkore"
        row["project_title"] = "streamkore-project"
        row["project_url"] = "https://github.com/orgs/streamkore/projects/1"
        row["pull_request"]["url"] = "https://github.com/streamkore/.github/pull/2"
        row["governance_issue"]["url"] = "https://github.com/streamkore/.github/issues/1"
        errors = VALIDATOR.validate_results([row], 1)
        self.assertTrue(any("does not resolve" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
