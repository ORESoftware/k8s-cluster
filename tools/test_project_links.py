import copy
import json
import unittest
from datetime import datetime, timezone
from pathlib import Path

from project_links import (
    EXPECTED_PROJECT_COUNT,
    MANAGED_END,
    MANAGED_START,
    compact_marker,
    find_public_boundary_violations,
    merge_compact_marker,
    merge_managed_block,
    scheduled_cron_is_active,
    validate_catalog,
)


class ProjectLinksTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = Path(__file__).resolve().parent.parent
        cls.path = cls.root / "catalog" / "project-links.json"
        cls.catalog = json.loads(cls.path.read_text(encoding="utf-8"))

    def test_committed_catalog_is_valid_and_complete(self):
        self.assertEqual(validate_catalog(copy.deepcopy(self.catalog)), [])
        self.assertEqual(len(self.catalog["projects"]), EXPECTED_PROJECT_COUNT)
        self.assertEqual(
            find_public_boundary_violations(self.path.read_text(encoding="utf-8")),
            [],
        )

    def test_dancing_dragons_is_project_four(self):
        entry = next(
            item for item in self.catalog["projects"] if item["key"] == "dancing-dragons"
        )
        self.assertEqual(entry["github"]["project_number"], 4)

    def test_other_projects_are_project_one(self):
        numbers = {
            item["key"]: item["github"]["project_number"]
            for item in self.catalog["projects"]
            if item["key"] != "dancing-dragons"
        }
        self.assertTrue(numbers)
        self.assertEqual(set(numbers.values()), {1})

    def test_linear_naming_exceptions_are_explicit(self):
        by_key = {item["key"]: item for item in self.catalog["projects"]}
        self.assertEqual(by_key["memebank"]["linear"]["project_name"], "memebank")
        self.assertEqual(
            by_key["meta-agents-demo"]["linear"]["project_name"],
            "meta-agents-demo",
        )
        self.assertEqual(
            by_key["streempilot"]["linear"]["project_name"],
            "github.com/streempilot",
        )

    def test_wrong_slack_channel_is_rejected(self):
        value = copy.deepcopy(self.catalog)
        value["projects"][0]["slack"]["channel_name"] = "#wrong"
        errors = validate_catalog(value)
        self.assertTrue(any("slack.channel_name" in error for error in errors), errors)

    def test_duplicate_key_is_rejected(self):
        value = copy.deepcopy(self.catalog)
        value["projects"][1]["key"] = value["projects"][0]["key"]
        errors = validate_catalog(value)
        self.assertTrue(any("duplicate key" in error for error in errors), errors)

    def test_public_boundary_rejects_tokens_ids_and_uuids(self):
        raw = json.dumps(
            {
                "slack": "C0BLYPGGFH6",
                "linear": "83b03121-db08-4e34-a69f-99fd1c873ced",
                "token": "ghp_abcdefghijklmnopqrstuvwxyz123456",
            }
        )
        violations = find_public_boundary_violations(raw)
        self.assertEqual(len(violations), 3)

    def test_dst_schedule_selects_0800_utc_in_summer(self):
        now = datetime(2026, 8, 4, 14, tzinfo=timezone.utc)
        self.assertTrue(scheduled_cron_is_active(now, "0 8 * * *"))
        self.assertFalse(scheduled_cron_is_active(now, "0 9 * * *"))

    def test_dst_schedule_selects_0900_utc_in_winter(self):
        now = datetime(2026, 1, 4, 14, tzinfo=timezone.utc)
        self.assertFalse(scheduled_cron_is_active(now, "0 8 * * *"))
        self.assertTrue(scheduled_cron_is_active(now, "0 9 * * *"))

    def test_schedule_uses_event_cron_not_runner_start_hour(self):
        delayed = datetime(2026, 8, 4, 18, tzinfo=timezone.utc)
        self.assertTrue(scheduled_cron_is_active(delayed, "0 8 * * *"))

    def test_managed_block_is_idempotent_and_preserves_description(self):
        entry = self.catalog["projects"][0]
        original = "Human-owned project description."
        first = merge_managed_block(original, entry)
        second = merge_managed_block(first, entry)
        self.assertEqual(first, second)
        self.assertIn(original, first)
        self.assertIn(MANAGED_START, first)
        self.assertIn(MANAGED_END, first)

    def test_compact_marker_is_idempotent_and_preserves_topic(self):
        entry = self.catalog["projects"][0]
        marker = compact_marker(entry)
        first = merge_compact_marker("Human topic", entry, 250)
        second = merge_compact_marker(first, entry, 250)
        self.assertEqual(first, second)
        self.assertTrue(first.startswith("Human topic | "))
        self.assertIn(marker, first)


if __name__ == "__main__":
    unittest.main()
