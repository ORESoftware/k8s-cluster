from __future__ import annotations

import copy
import sys
import unittest
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

    def test_status_aliases_and_idempotency_keys(self) -> None:
        field = {"options": [{"id": "a", "name": "Todo"}, {"id": "b", "name": "In Progress"}]}
        self.assertEqual(sync.status_option(field, "Todo"), "a")
        self.assertEqual(sync.status_option(field, "In Progress"), "b")
        issue = {"identifier": "DEN-1", "url": "https://linear.app/denman/issue/DEN-1"}
        items = [{"id": "x", "content": {"body": "<!-- linear-sync-key:DEN-1 -->"}}]
        self.assertEqual([item["id"] for item in sync.matches(items, issue)], ["x"])


if __name__ == "__main__":
    unittest.main()
