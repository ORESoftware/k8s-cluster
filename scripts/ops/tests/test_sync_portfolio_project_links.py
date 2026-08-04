from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

OPS_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(OPS_DIR))

from portfolio_project_links import load_links  # noqa: E402
from sync_portfolio_project_links import (  # noqa: E402
    Result,
    render_markdown_report,
    run,
    sync_github,
    sync_linear,
    sync_slack,
)


class FakeGitHub:
    def __init__(self, project: dict[str, object]) -> None:
        self.project = dict(project)
        self.updated: dict[str, object] | None = None

    def organization_projects(self, login: str) -> list[dict[str, object]]:
        return [self.project]

    def update_project(
        self,
        project_id: str,
        title: str,
        short_description: str,
        readme: str,
        closed: bool,
    ) -> dict[str, object]:
        self.updated = {
            "id": project_id,
            "title": title,
            "shortDescription": short_description,
            "readme": readme,
            "closed": closed,
        }
        self.project.update(self.updated)
        return self.project


class FakeLinear:
    def __init__(self, project: dict[str, object]) -> None:
        self.value = dict(project)
        self.updated: tuple[str, str] | None = None

    def project(self, project_id: str) -> dict[str, object]:
        return self.value

    def update_description(self, project_id: str, description: str) -> None:
        self.updated = (project_id, description)
        self.value["description"] = description


class FakeSlack:
    def __init__(self, channel: dict[str, object]) -> None:
        self.value = dict(channel)
        self.updated: tuple[str, str] | None = None

    def channel(self, channel_id: str) -> dict[str, object]:
        return self.value

    def set_topic(self, channel_id: str, topic: str) -> None:
        self.updated = (channel_id, topic)
        self.value["topic"] = {"value": topic}


class PortfolioProjectSyncTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.registry = REPO_ROOT / "ops/registries/portfolio-project-links.csv"
        cls.link = load_links(cls.registry)[0]

    def test_github_apply_updates_and_then_is_idempotent(self) -> None:
        client = FakeGitHub(
            {
                "id": "project-id",
                "number": self.link.github_project_number,
                "title": self.link.github_project_title,
                "closed": True,
                "url": self.link.github_project_url,
                "shortDescription": "",
                "readme": "",
            }
        )
        first = sync_github(client, self.link, apply=True)
        self.assertEqual(first.status, "updated")
        self.assertIsNotNone(client.updated)
        self.assertFalse(client.updated["closed"])
        second = sync_github(client, self.link, apply=True)
        self.assertEqual(second.status, "in_sync")

    def test_github_fails_closed_on_wrong_project_number(self) -> None:
        client = FakeGitHub(
            {
                "id": "project-id",
                "number": 99,
                "title": self.link.github_project_title,
                "closed": False,
                "url": self.link.github_project_url,
                "shortDescription": "",
                "readme": "",
            }
        )
        with self.assertRaisesRegex(RuntimeError, "expected one GitHub Project"):
            sync_github(client, self.link, apply=False)

    def test_linear_apply_preserves_human_description(self) -> None:
        client = FakeLinear(
            {
                "id": self.link.linear_project_id,
                "name": self.link.linear_project_name,
                "description": "Human description",
                "url": self.link.linear_project_url,
                "canceledAt": None,
            }
        )
        result = sync_linear(client, self.link, apply=True)
        self.assertEqual(result.status, "updated")
        self.assertIn("Human description", client.updated[1])
        self.assertIn("Canonical portfolio links", client.updated[1])
        self.assertEqual(sync_linear(client, self.link, apply=True).status, "in_sync")

    def test_linear_fails_closed_on_name_drift(self) -> None:
        client = FakeLinear(
            {
                "id": self.link.linear_project_id,
                "name": "wrong-name",
                "description": "",
                "url": self.link.linear_project_url,
                "canceledAt": None,
            }
        )
        with self.assertRaisesRegex(RuntimeError, "Linear name drift"):
            sync_linear(client, self.link, apply=False)

    def test_slack_apply_updates_topic_and_then_is_idempotent(self) -> None:
        client = FakeSlack(
            {
                "id": self.link.slack_channel_id,
                "name": self.link.slack_channel_name,
                "is_archived": False,
                "topic": {"value": "Human topic"},
            }
        )
        first = sync_slack(
            client,
            self.link.slack_workspace_id,
            self.link,
            apply=True,
        )
        self.assertEqual(first.status, "updated")
        self.assertIn(
            f"portfolio-link-registry:v1:{self.link.portfolio_key}",
            client.updated[1],
        )
        second = sync_slack(
            client,
            self.link.slack_workspace_id,
            self.link,
            apply=True,
        )
        self.assertEqual(second.status, "in_sync")

    def test_slack_fails_closed_on_wrong_workspace(self) -> None:
        client = FakeSlack(
            {
                "id": self.link.slack_channel_id,
                "name": self.link.slack_channel_name,
                "is_archived": False,
                "topic": {"value": ""},
            }
        )
        with self.assertRaisesRegex(RuntimeError, "Slack workspace drift"):
            sync_slack(client, "TWRONG", self.link, apply=False)

    def test_report_escapes_pipe_in_details(self) -> None:
        text = render_markdown_report(
            "dry-run",
            [Result("x", "github", "failed", False, "a|b")],
            "now",
        )
        self.assertIn("a\\|b", text)

    def test_inactive_scheduled_lane_exits_before_credentials(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(
                registry=self.registry,
                json_output=root / "report.json",
                markdown_output=root / "report.md",
                apply=True,
                allow_missing_credentials=False,
                scheduled_cron="0 9 * * *",
                at="2026-08-04T14:00:00Z",
                summary_channel="oresoftware",
                post_noop_summary=False,
            )
            with patch.dict(os.environ, {}, clear=True):
                self.assertEqual(run(args), 0)
            report = json.loads(args.json_output.read_text(encoding="utf-8"))
            self.assertIsNotNone(report["skipped_reason"])

    def test_active_apply_without_credentials_writes_failure_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(
                registry=self.registry,
                json_output=root / "report.json",
                markdown_output=root / "report.md",
                apply=True,
                allow_missing_credentials=False,
                scheduled_cron=None,
                at="2026-08-04T14:00:00Z",
                summary_channel="oresoftware",
                post_noop_summary=False,
            )
            with patch.dict(os.environ, {}, clear=True):
                self.assertEqual(run(args), 2)
            report = json.loads(args.json_output.read_text(encoding="utf-8"))
            self.assertEqual(report["summary"]["failed"], 3)

    def test_credential_free_dry_run_covers_all_provider_lanes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(
                registry=self.registry,
                json_output=root / "report.json",
                markdown_output=root / "report.md",
                apply=False,
                allow_missing_credentials=True,
                scheduled_cron=None,
                at="2026-08-04T14:00:00Z",
                summary_channel="oresoftware",
                post_noop_summary=False,
            )
            with patch.dict(os.environ, {}, clear=True):
                self.assertEqual(run(args), 0)
            report = json.loads(args.json_output.read_text(encoding="utf-8"))
            self.assertEqual(report["summary"]["results"], 41 * 4)
            self.assertEqual(report["summary"]["failed"], 0)


if __name__ == "__main__":
    unittest.main()
