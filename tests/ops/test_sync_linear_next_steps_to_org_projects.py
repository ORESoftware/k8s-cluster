from __future__ import annotations

import copy
import sys
import unittest
from unittest import mock
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts/ci"))
import sync_linear_next_steps_to_org_projects as sync  # noqa: E402

MANIFEST = REPO / "ops/linear-github-project-sync/2026-08-04"


class SyncContractTests(unittest.TestCase):
    def test_manifest_counts_and_empty_projects(self) -> None:
        entries = sync.load_manifest(MANIFEST)
        self.assertEqual(len(entries), 40)
        self.assertEqual(sum(len(entry["issues"]) for entry in entries), 85)
        self.assertEqual(sum(not entry["issues"] for entry in entries), 9)

    def test_duplicate_issue_is_rejected(self) -> None:
        entries = copy.deepcopy(sync.load_manifest(MANIFEST))
        populated = [entry for entry in entries if entry["issues"]]
        populated[1]["issues"][0]["identifier"] = populated[0]["issues"][0]["identifier"]
        import tempfile, json
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "00.json").write_text(json.dumps(entries))
            with self.assertRaises(sync.ApiError):
                sync.load_manifest(Path(directory))

    def test_direct_token_mode_skips_app_token_minting(self) -> None:
        entry = {
            "organization": "example-org",
            "project_title": "example-org-project",
            "project_url": "https://github.com/orgs/example-org/projects/1",
            "issues": [],
        }
        project = {
            "closed": False,
            "title": entry["project_title"],
            "url": entry["project_url"],
            "all_items": [],
        }
        with mock.patch.object(sync, "mint_org_token", side_effect=AssertionError("must not mint")), \
             mock.patch.object(sync, "load_project", return_value=project) as load_project:
            result = sync.sync_one("direct-token", entry, dry_run=True, direct_token=True)
        self.assertEqual(result["outcome"], "empty")
        load_project.assert_called_once_with("direct-token", "example-org", 1)

    def test_status_aliases_and_idempotency_keys(self) -> None:
        field = {"options": [{"id": "a", "name": "Todo"}, {"id": "b", "name": "In Progress"}]}
        self.assertEqual(sync.status_option(field, "Todo"), "a")
        self.assertEqual(sync.status_option(field, "In Progress"), "b")
        issue = {"identifier": "DEN-1", "url": "https://linear.app/denman/issue/DEN-1"}
        items = [{"id": "x", "content": {"body": "<!-- linear-sync-key:DEN-1 -->"}}]
        self.assertEqual([item["id"] for item in sync.matches(items, issue)], ["x"])


if __name__ == "__main__":
    unittest.main()
