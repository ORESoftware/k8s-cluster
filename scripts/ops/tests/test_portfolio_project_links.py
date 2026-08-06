from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

OPS_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(OPS_DIR))

from portfolio_project_links import (  # noqa: E402
    EXPECTED_COUNT,
    MANAGED_END,
    MANAGED_START,
    REQUIRED_COLUMNS,
    github_short_description,
    load_links,
    merge_compact_marker,
    merge_linear_description,
    scheduled_cron_is_active,
    slack_marker,
    validate_registry,
)


class PortfolioProjectLinksTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.registry = REPO_ROOT / "ops/registries/portfolio-project-links.csv"
        cls.links = load_links(cls.registry)

    def test_committed_registry_is_valid_and_complete(self) -> None:
        self.assertEqual(validate_registry(self.registry), [])
        self.assertEqual(len(self.links), EXPECTED_COUNT)

    def test_verified_project_number_exception(self) -> None:
        by_key = {link.portfolio_key: link for link in self.links}
        self.assertEqual(by_key["dancing-dragons"].github_project_number, 4)
        self.assertEqual(
            {
                link.github_project_number
                for link in self.links
                if link.portfolio_key != "dancing-dragons"
            },
            {1},
        )

    def test_direct_urls_match_provider_ids(self) -> None:
        for link in self.links:
            self.assertEqual(
                link.github_project_url,
                (
                    f"https://github.com/orgs/{link.github_org}/projects/"
                    f"{link.github_project_number}"
                ),
            )
            self.assertTrue(link.linear_project_url.startswith("https://linear.app/"))
            self.assertEqual(
                link.slack_channel_url,
                (
                    "https://oresoftware-workspace.slack.com/archives/"
                    f"{link.slack_channel_id}"
                ),
            )

    def test_url_column_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "registry.csv"
            rows = []
            with self.registry.open(newline="", encoding="utf-8") as handle:
                rows = list(csv.DictReader(handle))
            rows[0]["github_project_url"] = "https://example.invalid/project"
            with path.open("w", newline="", encoding="utf-8") as handle:
                writer = csv.DictWriter(handle, fieldnames=REQUIRED_COLUMNS)
                writer.writeheader()
                writer.writerows(rows)
            errors = validate_registry(path)
            self.assertTrue(any("github_project_url" in error for error in errors), errors)

    def test_credential_shaped_value_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "registry.csv"
            raw = self.registry.read_text(encoding="utf-8")
            token_fixture = "ghp_" + "a" * 30
            path.write_text(raw + f"\n# {token_fixture}\n", encoding="utf-8")
            errors = validate_registry(path)
            self.assertTrue(any("credential-shaped" in error for error in errors), errors)

    def test_dst_schedule_selects_0800_utc_in_summer(self) -> None:
        now = datetime(2026, 8, 4, 14, tzinfo=timezone.utc)
        self.assertTrue(scheduled_cron_is_active(now, "0 8 * * *"))
        self.assertFalse(scheduled_cron_is_active(now, "0 9 * * *"))

    def test_dst_schedule_selects_0900_utc_in_winter(self) -> None:
        now = datetime(2026, 1, 4, 14, tzinfo=timezone.utc)
        self.assertFalse(scheduled_cron_is_active(now, "0 8 * * *"))
        self.assertTrue(scheduled_cron_is_active(now, "0 9 * * *"))

    def test_schedule_uses_event_cron_not_runner_start_hour(self) -> None:
        delayed = datetime(2026, 8, 4, 18, tzinfo=timezone.utc)
        self.assertTrue(scheduled_cron_is_active(delayed, "0 8 * * *"))

    def test_linear_managed_block_is_idempotent(self) -> None:
        link = self.links[0]
        first = merge_linear_description("Human-owned description.", link)
        second = merge_linear_description(first, link)
        self.assertEqual(first, second)
        self.assertIn("Human-owned description.", first)
        self.assertIn(MANAGED_START, first)
        self.assertIn(MANAGED_END, first)

    def test_compact_markers_are_idempotent_and_fit_provider_limits(self) -> None:
        for link in self.links:
            self.assertLessEqual(len(github_short_description(link)), 256)

            slack_value = merge_compact_marker(
                "Human topic",
                slack_marker(link),
                link.portfolio_key,
                250,
            )
            self.assertLessEqual(len(slack_value), 250)
            self.assertEqual(
                merge_compact_marker(
                    slack_value,
                    slack_marker(link),
                    link.portfolio_key,
                    250,
                ),
                slack_value,
            )


if __name__ == "__main__":
    unittest.main()
