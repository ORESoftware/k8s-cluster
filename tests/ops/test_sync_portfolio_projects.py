#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import os
import sys
from pathlib import Path
import tempfile
import unittest

ROOT = Path(os.environ.get("ROOT_OVERRIDE", Path(__file__).resolve().parents[2]))
SCRIPT = ROOT / "scripts/ops/sync_portfolio_projects.py"
REGISTRY = ROOT / "config/portfolio-projects.json"
WORKFLOW = ROOT / ".github/workflows/portfolio-project-sync.yml"

spec = importlib.util.spec_from_file_location("portfolio_sync", SCRIPT)
if spec is None or spec.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
portfolio_sync = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = portfolio_sync
spec.loader.exec_module(portfolio_sync)


class RecordingHttp:
    def __init__(self) -> None:
        self.calls: list[dict[str, object]] = []

    def request_json(self, method, url, *, headers=None, payload=None):
        self.calls.append(
            {
                "method": method,
                "url": url,
                "headers": dict(headers or {}),
                "payload": payload,
            }
        )
        return {"data": {"viewer": {"id": "viewer"}}}


class RegistryContractTests(unittest.TestCase):
    def test_registry_is_exact_canonical_fleet(self) -> None:
        registry = portfolio_sync.Registry.load(REGISTRY)
        self.assertEqual(41, len(registry.entries))
        self.assertEqual("0 3 * * *", registry.cron)
        self.assertEqual("America/Chicago", registry.timezone_name)
        self.assertEqual("T01B3C83PMK", registry.slack_workspace_id)

        by_key = {entry.key: entry for entry in registry.entries}
        self.assertEqual(41, len(by_key))
        self.assertEqual(4, by_key["dancing-dragons"].github_project_number)
        self.assertTrue(
            all(
                entry.github_project_number == 1
                for key, entry in by_key.items()
                if key != "dancing-dragons"
            )
        )
        self.assertEqual("3FA-app", by_key["3fa-app"].github_owner)
        self.assertEqual("OmniBlitz", by_key["omniblitz"].github_owner)
        self.assertEqual("StreemPilot", by_key["streempilot"].github_owner)

    def test_registry_keys_are_shared_by_chatgpt_and_slack(self) -> None:
        registry = portfolio_sync.Registry.load(REGISTRY)
        for entry in registry.entries:
            with self.subTest(key=entry.key):
                self.assertEqual(entry.key, entry.chatgpt_name)
                self.assertEqual(entry.key, entry.slack_channel_name)
                self.assertTrue(entry.issue_sync_enabled)
                self.assertEqual(
                    f"{entry.github_owner}-project", entry.github_project_title
                )

    def test_registry_rejects_an_extra_project(self) -> None:
        raw = json.loads(REGISTRY.read_text(encoding="utf-8"))
        raw["entries"].append(dict(raw["entries"][0]))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "registry.json"
            path.write_text(json.dumps(raw), encoding="utf-8")
            with self.assertRaisesRegex(
                portfolio_sync.SyncError, "exactly 41 entries"
            ):
                portfolio_sync.Registry.load(path)

    def test_registry_rejects_a_second_nonstandard_project_number(self) -> None:
        raw = json.loads(REGISTRY.read_text(encoding="utf-8"))
        for entry in raw["entries"]:
            if entry["key"] == "zed-pkg":
                entry["github"]["project_number"] = 2
                entry["github"]["project_url"] = (
                    "https://github.com/orgs/zed-pkg/projects/2"
                )
                break
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "registry.json"
            path.write_text(json.dumps(raw), encoding="utf-8")
            with self.assertRaisesRegex(
                portfolio_sync.SyncError,
                "exactly one GitHub Project may use a number other than 1",
            ):
                portfolio_sync.Registry.load(path)


class AuthenticationTests(unittest.TestCase):
    QUERY = "query Viewer { viewer { id } }"

    def test_github_uses_bearer_authorization(self) -> None:
        http = RecordingHttp()
        client = portfolio_sync.GraphQLClient(
            portfolio_sync.GITHUB_GRAPHQL_URL,
            "github-token",
            http,
            bearer=True,
        )
        client.execute(self.QUERY, {})
        self.assertEqual(
            "Bearer github-token", http.calls[0]["headers"]["Authorization"]
        )

    def test_linear_personal_api_key_is_not_prefixed_with_bearer(self) -> None:
        http = RecordingHttp()
        client = portfolio_sync.GraphQLClient(
            portfolio_sync.LINEAR_GRAPHQL_URL,
            "linear-api-key",
            http,
            bearer=False,
        )
        client.execute(self.QUERY, {})
        self.assertEqual(
            "linear-api-key", http.calls[0]["headers"]["Authorization"]
        )


class ManagedMetadataTests(unittest.TestCase):
    def test_marked_block_preserves_human_text(self) -> None:
        first = portfolio_sync.replace_managed_block("Human introduction.\n", "v1")
        second = portfolio_sync.replace_managed_block(first, "v2")
        self.assertIn("Human introduction.", second)
        self.assertIn("v2", second)
        self.assertNotIn("v1", second)
        self.assertEqual(1, second.count(portfolio_sync.MANAGED_BEGIN))
        self.assertEqual(1, second.count(portfolio_sync.MANAGED_END))

    def test_marked_block_fails_closed_on_broken_boundary(self) -> None:
        with self.assertRaisesRegex(portfolio_sync.SyncError, "one boundary"):
            portfolio_sync.replace_managed_block(
                f"text\n{portfolio_sync.MANAGED_BEGIN}\n", "managed"
            )

    def test_slack_purpose_replaces_only_managed_suffix(self) -> None:
        old = (
            "Human purpose | [portfolio-sync key=zed-pkg] GitHub=old | "
            "Linear=old | ChatGPT=zed-pkg"
        )
        managed = (
            "[portfolio-sync key=zed-pkg] GitHub=new | Linear=new | "
            "ChatGPT=zed-pkg"
        )
        merged = portfolio_sync.merge_slack_managed_purpose(old, managed)
        self.assertEqual(f"Human purpose | {managed}", merged)

    def test_slack_purpose_refuses_to_truncate_human_text(self) -> None:
        human = "h" * 240
        with self.assertRaisesRegex(portfolio_sync.SyncError, "leaves no room"):
            portfolio_sync.merge_slack_managed_purpose(
                human, "[portfolio-sync key=x] GitHub=x | Linear=x | ChatGPT=x"
            )

    def test_slack_purpose_fails_closed_when_human_text_follows_marker(self) -> None:
        malformed = (
            "[portfolio-sync key=zed-pkg] GitHub=old | Linear=old | "
            "ChatGPT=zed-pkg | Human suffix"
        )
        with self.assertRaisesRegex(portfolio_sync.SyncError, "not the final segment"):
            portfolio_sync.merge_slack_managed_purpose(
                malformed,
                "[portfolio-sync key=zed-pkg] GitHub=new | Linear=new | "
                "ChatGPT=zed-pkg",
            )

    def test_drift_status_overrides_warning_for_dry_run_change(self) -> None:
        report = portfolio_sync.EntryReport("zed-pkg")
        report.add_warning("optional snapshot absent")
        report.add_change("github", "update-project", "title", False)
        self.assertEqual("drift", report.status)
        self.assertEqual(["optional snapshot absent"], report.warnings)


class LinearIssueMirrorTests(unittest.TestCase):
    @staticmethod
    def issue(
        issue_id: str,
        identifier: str,
        title: str,
        state_type: str = "started",
        state_name: str = "In Progress",
    ) -> dict[str, object]:
        return {
            "id": issue_id,
            "identifier": identifier,
            "title": title,
            "url": f"https://linear.app/example/issue/{identifier}",
            "priority": 2,
            "updatedAt": "2026-08-04T18:00:00.000Z",
            "state": {"name": state_name, "type": state_type},
        }

    @staticmethod
    def draft_item(
        item_id: str,
        draft_id: str,
        issue: dict[str, object],
        project_key: str,
        *,
        stale: bool = False,
    ) -> dict[str, object]:
        title, body = portfolio_sync.desired_draft_issue(issue, project_key)
        if stale:
            title += " stale"
        return {
            "id": item_id,
            "content": {
                "__typename": "DraftIssue",
                "id": draft_id,
                "title": title,
                "body": body,
            },
        }

    def test_plan_creates_updates_and_archives_only_managed_drafts(self) -> None:
        active_new = self.issue(
            "11111111-1111-1111-1111-111111111111", "DEN-1", "Create"
        )
        active_stale = self.issue(
            "22222222-2222-2222-2222-222222222222", "DEN-2", "Update"
        )
        completed = self.issue(
            "33333333-3333-3333-3333-333333333333",
            "DEN-3",
            "Archive",
            state_type="completed",
            state_name="Done",
        )
        github_items = [
            self.draft_item("item-2", "draft-2", active_stale, "zed-pkg", stale=True),
            self.draft_item("item-3", "draft-3", completed, "zed-pkg"),
            {
                "id": "human-item",
                "content": {
                    "__typename": "DraftIssue",
                    "id": "human-draft",
                    "title": "Human draft",
                    "body": "No portfolio marker",
                },
            },
            {
                "id": "issue-item",
                "content": {"__typename": "Issue", "id": "real-issue"},
            },
        ]
        plan = portfolio_sync.plan_draft_issue_changes(
            [active_new, active_stale, completed], github_items, "zed-pkg"
        )
        self.assertEqual(["DEN-1"], [item["identifier"] for item in plan.creates])
        self.assertEqual(
            ["DEN-2"], [pair[0]["identifier"] for pair in plan.updates]
        )
        self.assertEqual(["item-3"], [item["id"] for item in plan.archives])

    def test_triage_issue_is_mirrored_as_pending_work(self) -> None:
        issue = self.issue(
            "55555555-5555-5555-5555-555555555555",
            "DEN-5",
            "Triage",
            state_type="triage",
            state_name="Triage",
        )
        plan = portfolio_sync.plan_draft_issue_changes([issue], [], "zed-pkg")
        self.assertEqual(["DEN-5"], [item["identifier"] for item in plan.creates])

    def test_duplicate_managed_draft_fails_closed(self) -> None:
        issue = self.issue(
            "44444444-4444-4444-4444-444444444444", "DEN-4", "Duplicate"
        )
        items = [
            self.draft_item("one", "draft-one", issue, "zed-pkg"),
            self.draft_item("two", "draft-two", issue, "zed-pkg"),
        ]
        with self.assertRaisesRegex(portfolio_sync.SyncError, "duplicate GitHub"):
            portfolio_sync.plan_draft_issue_changes([issue], items, "zed-pkg")


class WorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_schedule_is_exactly_three_am_chicago(self) -> None:
        self.assertIn('cron: "0 3 * * *"', self.workflow)
        self.assertIn('timezone: "America/Chicago"', self.workflow)
        self.assertIn("cancel-in-progress: false", self.workflow)

    def test_pull_request_validation_is_credential_free(self) -> None:
        validate = self.workflow.split("  validate:", 1)[1].split("\n  sync:", 1)[0]
        self.assertNotIn("secrets.", validate)
        self.assertIn("--validate-only", validate)
        self.assertIn("persist-credentials: false", validate)
        self.assertNotIn("pull_request_target:", self.workflow)

    def test_scheduled_lane_applies_and_syncs_issues(self) -> None:
        for phrase in (
            "PORTFOLIO_GITHUB_TOKEN",
            "LINEAR_API_KEY",
            "SLACK_BOT_TOKEN",
            "--apply",
            "--sync-issues",
            "github.event_name == 'schedule'",
        ):
            self.assertIn(phrase, self.workflow)

    def test_actions_are_pinned_and_no_token_literal_exists(self) -> None:
        self.assertIn(
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            self.workflow,
        )
        self.assertIn(
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            self.workflow,
        )
        combined = "\n".join(
            [
                self.workflow,
                SCRIPT.read_text(encoding="utf-8"),
                REGISTRY.read_text(encoding="utf-8"),
            ]
        )
        self.assertNotRegex(combined, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(combined, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_forbidden_git_and_destructive_commands_are_not_added(self) -> None:
        combined = self.workflow + "\n" + SCRIPT.read_text(encoding="utf-8")
        for phrase in (
            "git checkout",
            "git reset",
            "git stash",
            "git clean",
            "git rebase",
            "git push --force",
            "rm -rf",
            "kubectl delete",
            "terraform destroy",
        ):
            with self.subTest(phrase=phrase):
                self.assertNotIn(phrase, combined)


class CliValidationTests(unittest.TestCase):
    def test_validate_only_writes_complete_report(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.json"
            exit_code = portfolio_sync.main(
                [
                    "--validate-only",
                    "--registry",
                    str(REGISTRY),
                    "--report",
                    str(report),
                ]
            )
            self.assertEqual(0, exit_code)
            payload = json.loads(report.read_text(encoding="utf-8"))
            self.assertEqual(41, payload["summary"]["projects"])
            self.assertEqual(0, payload["summary"]["errors"])
            self.assertEqual(41, len(payload["entries"]))
            self.assertTrue(
                all(
                    entry["chatgpt_verification"] == "registry-only"
                    for entry in payload["entries"]
                )
            )


if __name__ == "__main__":
    unittest.main()
