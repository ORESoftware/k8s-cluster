#!/usr/bin/env python3
from __future__ import annotations

import unittest
from pathlib import Path

ROOT = Path(__file__).parents[2]
SCRIPT = ROOT / "scripts/ops/sync_org_project_docs.sh"


class ReconcilerSourceContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SCRIPT.read_text(encoding="utf-8")

    def test_evidence_paths_are_absolute_before_repository_clones(self) -> None:
        absolute = self.source.index('EVIDENCE_DIR="$(python3 - "$EVIDENCE_DIR"')
        clone = self.source.index('gh repo clone "$repo_full_name"')
        self.assertLess(absolute, clone)

    def test_project_candidates_are_owned_by_the_requested_organization(self) -> None:
        self.assertIn('owner{__typename ... on Organization{login} ... on User{login}}', self.source)
        self.assertIn('.owner.__typename == "Organization"', self.source)
        self.assertIn('(.owner.login | ascii_downcase) == ($org | ascii_downcase)', self.source)
        self.assertIn('[[ "$(jq -r \'.owner.__typename\' <<<"$project_json")" == "Organization" ]]', self.source)

    def test_reconcile_is_not_invoked_as_a_conditional(self) -> None:
        self.assertNotIn('if reconcile_org "$organization" "$linear_url"', self.source)
        self.assertIn('reconcile_org "$organization" "$linear_url"', self.source)
        self.assertIn('reconcile_rc=$?', self.source)
        self.assertIn('if (( reconcile_rc == 0 )); then', self.source)

    def test_success_requires_complete_project_pr_issue_and_item_evidence(self) -> None:
        required = [
            '[[ -n "$project_id" && -n "$project_number" && -n "$project_url" ]]',
            '[[ "$pr_number" =~ ^[1-9][0-9]*$ ]]',
            '[[ "$issue_number" =~ ^[1-9][0-9]*$ ]]',
            '[[ "$project_item_action" == "added" || "$project_item_action" == "existing" ]]',
        ]
        success_record = self.source.index('record_result "ok"')
        for contract in required:
            self.assertIn(contract, self.source)
            self.assertLess(self.source.index(contract), success_record)


if __name__ == "__main__":
    unittest.main()
