import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from project_sync import (
    Result,
    _github_result,
    _linear_result,
    _slack_result,
    render_markdown_report,
    run,
)


class FakeGitHub:
    def __init__(self, project):
        self.project = dict(project)
        self.updated = None

    def organization_projects(self, login):
        return "owner-id", [self.project]

    def create_project(self, owner_id, title):
        raise AssertionError("not expected")

    def update_project(self, project_id, title, short_description, closed):
        self.updated = {
            "project_id": project_id,
            "title": title,
            "shortDescription": short_description,
            "closed": closed,
        }
        self.project.update(self.updated)
        return self.project


class FakeLinear:
    def __init__(self):
        self.updated = None

    def update_description(self, project_id, description):
        self.updated = (project_id, description)


class FakeSlack:
    def __init__(self):
        self.topic = None
        self.created = None

    def create_channel(self, name):
        self.created = name
        return {"id": "channel-id", "name": name, "topic": {"value": ""}}

    def set_topic(self, channel_id, topic):
        self.topic = (channel_id, topic)


class ProjectSyncTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        root = Path(__file__).resolve().parent.parent
        cls.catalog_path = root / "catalog" / "project-links.json"
        cls.catalog = json.loads(cls.catalog_path.read_text(encoding="utf-8"))
        cls.entry = cls.catalog["projects"][0]

    def test_github_dry_run_plans_managed_description(self):
        github = self.entry["github"]
        client = FakeGitHub(
            {
                "id": "project-id",
                "number": github["project_number"],
                "title": github["project_title"],
                "closed": False,
                "shortDescription": "",
            }
        )
        result = _github_result(client, self.entry, apply=False)
        self.assertEqual(result.status, "would_update")
        self.assertTrue(result.changed)
        self.assertIsNone(client.updated)

    def test_github_apply_updates_idempotently(self):
        github = self.entry["github"]
        client = FakeGitHub(
            {
                "id": "project-id",
                "number": github["project_number"],
                "title": github["project_title"],
                "closed": True,
                "shortDescription": "",
            }
        )
        result = _github_result(client, self.entry, apply=True)
        self.assertEqual(result.status, "updated")
        self.assertFalse(client.updated["closed"])
        second = _github_result(client, self.entry, apply=True)
        self.assertEqual(second.status, "in_sync")

    def test_linear_apply_preserves_human_description(self):
        client = FakeLinear()
        projects = [
            {
                "id": "linear-id",
                "name": self.entry["linear"]["project_name"],
                "description": "Human description",
                "canceledAt": None,
            }
        ]
        result = _linear_result(projects, client, self.entry, apply=True)
        self.assertEqual(result.status, "updated")
        self.assertIn("Human description", client.updated[1])
        self.assertIn("Canonical project links", client.updated[1])

    def test_linear_fails_closed_on_ambiguous_name(self):
        client = FakeLinear()
        project = {
            "id": "linear-id",
            "name": self.entry["linear"]["project_name"],
            "description": "",
            "canceledAt": None,
        }
        with self.assertRaisesRegex(RuntimeError, "expected one active"):
            _linear_result([project, dict(project)], client, self.entry, apply=False)

    def test_slack_apply_updates_existing_channel_topic(self):
        client = FakeSlack()
        channel_name = self.entry["slack"]["channel_name"][1:]
        channels = [{"id": "channel-id", "name": channel_name, "topic": {"value": ""}}]
        result = _slack_result(
            channels, client, self.entry, apply=True, create_missing=False
        )
        self.assertEqual(result.status, "updated")
        self.assertEqual(client.topic[0], "channel-id")
        self.assertIn(f"[sync:{self.entry['key']}]", client.topic[1])

    def test_slack_can_create_missing_channel(self):
        client = FakeSlack()
        result = _slack_result([], client, self.entry, apply=True, create_missing=True)
        self.assertEqual(result.status, "created")
        self.assertEqual(client.created, self.entry["key"])

    def test_report_escapes_pipe_in_details(self):
        text = render_markdown_report(
            "dry-run", [Result("x", "github", "failed", False, "a|b")], "now"
        )
        self.assertIn("a\\|b", text)

    def test_inactive_scheduled_lane_exits_without_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            args = SimpleNamespace(
                catalog=self.catalog_path,
                json_output=root / "report.json",
                markdown_output=root / "report.md",
                apply=True,
                allow_missing_credentials=False,
                create_missing_slack=True,
                scheduled_cron="0 9 * * *",
                at="2026-08-04T14:00:00Z",
                summary_channel="oresoftware",
                post_noop_summary=False,
            )
            self.assertEqual(run(args), 0)
            report = json.loads(args.json_output.read_text(encoding="utf-8"))
            self.assertIsNotNone(report["skipped_reason"])


if __name__ == "__main__":
    unittest.main()
