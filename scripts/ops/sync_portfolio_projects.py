#!/usr/bin/env python3
"""Reconcile the ORESoftware portfolio identity graph and active work mirror.

The checked-in registry is the source of truth for the canonical project key and
its GitHub Projects v2, Linear, ChatGPT, and Slack identities. The scheduled
workflow runs this module at 03:00 America/Chicago.

Mutations are deliberately bounded:

* GitHub Project title, open state, short description, and one marked README
  block are reconciled.
* Linear project name and one marked description block are reconciled.
* Slack public-channel name and one bounded managed purpose segment are
  reconciled; existing human text is preserved when it fits.
* Active Linear issues are mirrored to managed GitHub Project draft items.
  Completed, canceled, removed, or moved Linear issues archive only the draft
  items created by this tool. Existing GitHub issues and unmarked draft items
  are never changed.
* ChatGPT Project names are always part of the identity graph. Live ChatGPT
  verification is read-only and optional through a snapshot supplied by a
  supported bridge because the consumer ChatGPT Projects surface does not
  expose a general write API suitable for this workflow.

Secrets are read from environment variables and are never written to manifests,
URLs, reports, or logs.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import sys
import time
from typing import Any, Iterable, Mapping, Sequence
import urllib.error
import urllib.parse
import urllib.request


GITHUB_GRAPHQL_URL = "https://api.github.com/graphql"
LINEAR_GRAPHQL_URL = "https://api.linear.app/graphql"
SLACK_API_URL = "https://slack.com/api"
USER_AGENT = "oresoftware-portfolio-project-sync/1"

MANAGED_BEGIN = "<!-- portfolio-sync:begin -->"
MANAGED_END = "<!-- portfolio-sync:end -->"
LINEAR_ISSUE_MARKER_RE = re.compile(
    r"<!--\s*portfolio-sync:linear-issue-id:([0-9a-fA-F-]{36})\s*-->"
)
SLACK_MANAGED_PREFIX = "[portfolio-sync key="
SLACK_MANAGED_RE = re.compile(
    r"\[portfolio-sync key=[a-z0-9-]+\]\s+GitHub=[^|]+\s*\|\s*"
    r"Linear=[^|]+\s*\|\s*ChatGPT=[^|]+\s*$"
)
KEY_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,77}[a-z0-9])?$")
UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-"
    r"[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)

ACTIVE_LINEAR_STATE_TYPES = frozenset({"triage", "backlog", "unstarted", "started"})
TERMINAL_LINEAR_STATE_TYPES = frozenset({"completed", "canceled"})
MAX_SLACK_METADATA = 250
MAX_GITHUB_SHORT_DESCRIPTION = 256
EXPECTED_PORTFOLIO_SIZE = 41


class SyncError(RuntimeError):
    """Expected reconciliation error safe to include in a redacted report."""


@dataclass(frozen=True)
class ProjectMapping:
    key: str
    chatgpt_name: str
    github_owner: str
    github_project_number: int
    github_project_title: str
    github_project_url: str
    linear_project_id: str
    linear_project_name: str
    slack_workspace_id: str
    slack_channel_id: str
    slack_channel_name: str
    issue_sync_enabled: bool

    @classmethod
    def from_json(cls, value: Mapping[str, Any]) -> "ProjectMapping":
        try:
            chatgpt = require_mapping(value, "chatgpt")
            github = require_mapping(value, "github")
            linear = require_mapping(value, "linear")
            slack = require_mapping(value, "slack")
            issue_sync = require_mapping(value, "issue_sync")
            return cls(
                key=require_string(value, "key"),
                chatgpt_name=require_string(chatgpt, "name"),
                github_owner=require_string(github, "owner"),
                github_project_number=require_int(github, "project_number"),
                github_project_title=require_string(github, "project_title"),
                github_project_url=require_string(github, "project_url"),
                linear_project_id=require_string(linear, "project_id"),
                linear_project_name=require_string(linear, "project_name"),
                slack_workspace_id=require_string(slack, "workspace_id"),
                slack_channel_id=require_string(slack, "channel_id"),
                slack_channel_name=require_string(slack, "channel_name"),
                issue_sync_enabled=bool(issue_sync.get("enabled", False)),
            )
        except (KeyError, TypeError, ValueError) as error:
            raise SyncError(f"invalid registry entry: {error}") from error


@dataclass(frozen=True)
class Registry:
    schema_version: int
    name: str
    cron: str
    timezone_name: str
    slack_workspace_id: str
    slack_workspace_name: str
    entries: tuple[ProjectMapping, ...]

    @classmethod
    def load(cls, path: Path) -> "Registry":
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError as error:
            raise SyncError(f"registry not found: {path}") from error
        except json.JSONDecodeError as error:
            raise SyncError(f"registry is not valid JSON: {error}") from error
        if not isinstance(raw, dict):
            raise SyncError("registry root must be an object")
        schedule = require_mapping(raw, "schedule")
        slack_workspace = require_mapping(raw, "slack_workspace")
        raw_entries = raw.get("entries")
        if not isinstance(raw_entries, list):
            raise SyncError("registry entries must be a list")
        registry = cls(
            schema_version=require_int(raw, "schema_version"),
            name=require_string(raw, "name"),
            cron=require_string(schedule, "cron"),
            timezone_name=require_string(schedule, "timezone"),
            slack_workspace_id=require_string(slack_workspace, "id"),
            slack_workspace_name=require_string(slack_workspace, "name"),
            entries=tuple(ProjectMapping.from_json(item) for item in raw_entries),
        )
        registry.validate()
        return registry

    def validate(self) -> None:
        errors: list[str] = []
        if self.schema_version != 1:
            errors.append(f"unsupported schema_version {self.schema_version}")
        if self.cron != "0 3 * * *":
            errors.append(f"schedule must be exactly '0 3 * * *', got {self.cron!r}")
        if self.timezone_name != "America/Chicago":
            errors.append(
                f"schedule timezone must be America/Chicago, got {self.timezone_name!r}"
            )
        if len(self.entries) != EXPECTED_PORTFOLIO_SIZE:
            errors.append(
                f"registry must contain exactly {EXPECTED_PORTFOLIO_SIZE} entries, "
                f"got {len(self.entries)}"
            )

        unique_fields: dict[str, set[str]] = {
            "key": set(),
            "github owner": set(),
            "linear project ID": set(),
            "slack channel ID": set(),
            "slack channel name": set(),
            "chatgpt project name": set(),
        }
        for entry in self.entries:
            if not KEY_RE.fullmatch(entry.key):
                errors.append(f"invalid canonical key: {entry.key!r}")
            if entry.chatgpt_name != entry.key:
                errors.append(
                    f"{entry.key}: ChatGPT project name must equal canonical key"
                )
            if entry.slack_channel_name != entry.key:
                errors.append(f"{entry.key}: Slack channel name must equal canonical key")
            if entry.slack_workspace_id != self.slack_workspace_id:
                errors.append(f"{entry.key}: Slack workspace ID does not match registry")
            if entry.github_project_number < 1:
                errors.append(f"{entry.key}: GitHub project number must be positive")
            expected_title = f"{entry.github_owner}-project"
            if entry.github_project_title != expected_title:
                errors.append(
                    f"{entry.key}: GitHub project title must be {expected_title!r}"
                )
            expected_url = (
                f"https://github.com/orgs/{entry.github_owner}/projects/"
                f"{entry.github_project_number}"
            )
            if entry.github_project_url != expected_url:
                errors.append(f"{entry.key}: GitHub project URL must be {expected_url}")
            if not UUID_RE.fullmatch(entry.linear_project_id):
                errors.append(f"{entry.key}: invalid Linear project UUID")
            if not re.fullmatch(r"C[A-Z0-9]{8,}", entry.slack_channel_id):
                errors.append(f"{entry.key}: invalid Slack channel ID")

            values = {
                "key": entry.key.casefold(),
                "github owner": entry.github_owner.casefold(),
                "linear project ID": entry.linear_project_id.casefold(),
                "slack channel ID": entry.slack_channel_id.casefold(),
                "slack channel name": entry.slack_channel_name.casefold(),
                "chatgpt project name": entry.chatgpt_name.casefold(),
            }
            for label, normalized in values.items():
                if normalized in unique_fields[label]:
                    errors.append(f"duplicate {label}: {normalized}")
                unique_fields[label].add(normalized)

        nonstandard_numbers = [
            entry
            for entry in self.entries
            if entry.github_project_number != 1
        ]
        if len(nonstandard_numbers) != 1:
            errors.append(
                "exactly one GitHub Project may use a number other than 1"
            )
        elif (
            nonstandard_numbers[0].key != "dancing-dragons"
            or nonstandard_numbers[0].github_project_number != 4
        ):
            errors.append(
                "the only GitHub Project number exception must be "
                "dancing-dragons#4"
            )

        if errors:
            raise SyncError("registry validation failed:\n- " + "\n- ".join(errors))


@dataclass
class Change:
    system: str
    action: str
    detail: str
    applied: bool = False

    def to_json(self) -> dict[str, Any]:
        return {
            "system": self.system,
            "action": self.action,
            "detail": self.detail,
            "applied": self.applied,
        }


@dataclass
class EntryReport:
    key: str
    status: str = "ok"
    changes: list[Change] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    stats: dict[str, int] = field(default_factory=dict)
    chatgpt_verification: str = "registry-only"

    def add_error(self, message: str) -> None:
        self.errors.append(message)
        self.status = "error"

    def add_warning(self, message: str) -> None:
        self.warnings.append(message)
        if self.status == "ok":
            self.status = "warning"

    def add_change(self, system: str, action: str, detail: str, applied: bool) -> None:
        self.changes.append(Change(system, action, detail, applied))
        if not applied and self.status != "error":
            self.status = "drift"

    def to_json(self) -> dict[str, Any]:
        return {
            "key": self.key,
            "status": self.status,
            "chatgpt_verification": self.chatgpt_verification,
            "changes": [item.to_json() for item in self.changes],
            "warnings": self.warnings,
            "errors": self.errors,
            "stats": self.stats,
        }


@dataclass(frozen=True)
class DraftIssuePlan:
    creates: tuple[dict[str, Any], ...]
    updates: tuple[tuple[dict[str, Any], dict[str, Any]], ...]
    archives: tuple[dict[str, Any], ...]


class JsonHttpClient:
    def __init__(self, *, attempts: int = 5, timeout_seconds: int = 40) -> None:
        self.attempts = attempts
        self.timeout_seconds = timeout_seconds

    def request_json(
        self,
        method: str,
        url: str,
        *,
        headers: Mapping[str, str] | None = None,
        payload: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        body = None if payload is None else json.dumps(payload).encode("utf-8")
        request_headers = {"User-Agent": USER_AGENT, "Accept": "application/json"}
        if headers:
            request_headers.update(headers)
        if body is not None:
            request_headers.setdefault("Content-Type", "application/json; charset=utf-8")

        for attempt in range(1, self.attempts + 1):
            request = urllib.request.Request(
                url,
                data=body,
                headers=request_headers,
                method=method,
            )
            try:
                with urllib.request.urlopen(
                    request, timeout=self.timeout_seconds
                ) as response:
                    raw = response.read()
                    if not raw:
                        return {}
                    decoded = json.loads(raw)
                    if not isinstance(decoded, dict):
                        raise SyncError(f"{url} returned a non-object JSON response")
                    return decoded
            except urllib.error.HTTPError as error:
                retry_after = error.headers.get("Retry-After")
                retryable = error.code == 429 or 500 <= error.code <= 599
                if retryable and attempt < self.attempts:
                    delay = parse_retry_after(retry_after, fallback=min(2**attempt, 20))
                    time.sleep(delay)
                    continue
                raw = error.read(4096).decode("utf-8", errors="replace")
                message = safe_api_message(raw)
                raise SyncError(
                    f"HTTP {error.code} from {urllib.parse.urlsplit(url).netloc}: {message}"
                ) from error
            except urllib.error.URLError as error:
                if attempt < self.attempts:
                    time.sleep(min(2**attempt, 20))
                    continue
                raise SyncError(
                    f"network error contacting {urllib.parse.urlsplit(url).netloc}: "
                    f"{type(error.reason).__name__}"
                ) from error
            except TimeoutError as error:
                if attempt < self.attempts:
                    time.sleep(min(2**attempt, 20))
                    continue
                raise SyncError(
                    f"timeout contacting {urllib.parse.urlsplit(url).netloc}"
                ) from error
        raise AssertionError("unreachable")


class GraphQLClient:
    def __init__(
        self,
        endpoint: str,
        token: str,
        http: JsonHttpClient,
        *,
        bearer: bool,
    ) -> None:
        if not token:
            raise SyncError(f"missing token for {urllib.parse.urlsplit(endpoint).netloc}")
        self.endpoint = endpoint
        self.authorization = f"Bearer {token}" if bearer else token
        self.http = http

    def execute(self, query: str, variables: Mapping[str, Any]) -> dict[str, Any]:
        payload = self.http.request_json(
            "POST",
            self.endpoint,
            headers={"Authorization": self.authorization},
            payload={"query": query, "variables": dict(variables)},
        )
        errors = payload.get("errors")
        if errors:
            safe_errors: list[str] = []
            if isinstance(errors, list):
                for item in errors[:5]:
                    if isinstance(item, dict):
                        safe_errors.append(str(item.get("message", "GraphQL error")))
                    else:
                        safe_errors.append("GraphQL error")
            raise SyncError("GraphQL request failed: " + "; ".join(safe_errors))
        data = payload.get("data")
        if not isinstance(data, dict):
            raise SyncError("GraphQL response did not contain an object data field")
        return data


class GitHubClient:
    PROJECT_QUERY = """
      query PortfolioProject($owner: String!, $number: Int!, $after: String) {
        organization(login: $owner) {
          projectV2(number: $number) {
            id
            number
            title
            url
            readme
            shortDescription
            closed
            items(first: 100, after: $after) {
              nodes {
                id
                content {
                  __typename
                  ... on DraftIssue {
                    id
                    title
                    body
                  }
                }
              }
              pageInfo { hasNextPage endCursor }
            }
          }
        }
      }
    """
    UPDATE_PROJECT_MUTATION = """
      mutation UpdatePortfolioProject($input: UpdateProjectV2Input!) {
        updateProjectV2(input: $input) {
          projectV2 { id title readme shortDescription closed }
        }
      }
    """
    CREATE_DRAFT_MUTATION = """
      mutation AddLinearDraft($input: AddProjectV2DraftIssueInput!) {
        addProjectV2DraftIssue(input: $input) {
          projectItem {
            id
            content {
              ... on DraftIssue { id title body }
            }
          }
        }
      }
    """
    UPDATE_DRAFT_MUTATION = """
      mutation UpdateLinearDraft($input: UpdateProjectV2DraftIssueInput!) {
        updateProjectV2DraftIssue(input: $input) {
          draftIssue { id title body }
        }
      }
    """
    ARCHIVE_ITEM_MUTATION = """
      mutation ArchiveLinearDraft($input: ArchiveProjectV2ItemInput!) {
        archiveProjectV2Item(input: $input) {
          item { id }
        }
      }
    """

    def __init__(self, token: str, http: JsonHttpClient) -> None:
        self.graphql = GraphQLClient(
            GITHUB_GRAPHQL_URL, token, http, bearer=True
        )

    def get_project(self, owner: str, number: int) -> dict[str, Any]:
        after: str | None = None
        project: dict[str, Any] | None = None
        items: list[dict[str, Any]] = []
        while True:
            data = self.graphql.execute(
                self.PROJECT_QUERY,
                {"owner": owner, "number": number, "after": after},
            )
            organization = data.get("organization")
            if not isinstance(organization, dict):
                raise SyncError(f"GitHub organization not found or inaccessible: {owner}")
            current = organization.get("projectV2")
            if not isinstance(current, dict):
                raise SyncError(f"GitHub Project not found: {owner}#{number}")
            if project is None:
                project = dict(current)
            connection = current.get("items")
            if not isinstance(connection, dict):
                raise SyncError(f"GitHub Project items missing: {owner}#{number}")
            nodes = connection.get("nodes")
            if not isinstance(nodes, list):
                raise SyncError(f"GitHub Project items invalid: {owner}#{number}")
            items.extend(item for item in nodes if isinstance(item, dict))
            page_info = connection.get("pageInfo")
            if not isinstance(page_info, dict) or not page_info.get("hasNextPage"):
                break
            end_cursor = page_info.get("endCursor")
            if not isinstance(end_cursor, str) or not end_cursor:
                raise SyncError(f"GitHub pagination cursor missing: {owner}#{number}")
            after = end_cursor
        assert project is not None
        project["items"] = items
        return project

    def update_project(self, project_id: str, **fields: Any) -> None:
        input_value: dict[str, Any] = {"projectId": project_id}
        input_value.update(fields)
        self.graphql.execute(self.UPDATE_PROJECT_MUTATION, {"input": input_value})

    def create_draft_issue(
        self, project_id: str, title: str, body: str
    ) -> dict[str, Any]:
        data = self.graphql.execute(
            self.CREATE_DRAFT_MUTATION,
            {"input": {"projectId": project_id, "title": title, "body": body}},
        )
        payload = data.get("addProjectV2DraftIssue")
        if not isinstance(payload, dict) or not isinstance(payload.get("projectItem"), dict):
            raise SyncError("GitHub did not return the created draft project item")
        return payload["projectItem"]

    def update_draft_issue(self, draft_id: str, title: str, body: str) -> None:
        self.graphql.execute(
            self.UPDATE_DRAFT_MUTATION,
            {"input": {"draftIssueId": draft_id, "title": title, "body": body}},
        )

    def archive_project_item(self, project_id: str, item_id: str) -> None:
        self.graphql.execute(
            self.ARCHIVE_ITEM_MUTATION,
            {"input": {"projectId": project_id, "itemId": item_id}},
        )


class LinearClient:
    PROJECT_QUERY = """
      query PortfolioLinearProject($id: String!, $after: String) {
        project(id: $id) {
          id
          name
          description
          url
          issues(first: 100, after: $after) {
            nodes {
              id
              identifier
              title
              url
              priority
              updatedAt
              state { name type }
            }
            pageInfo { hasNextPage endCursor }
          }
        }
      }
    """
    UPDATE_PROJECT_MUTATION = """
      mutation UpdatePortfolioLinearProject($id: String!, $input: ProjectUpdateInput!) {
        projectUpdate(id: $id, input: $input) {
          success
          project { id name description url }
        }
      }
    """

    def __init__(self, token: str, http: JsonHttpClient) -> None:
        self.graphql = GraphQLClient(
            LINEAR_GRAPHQL_URL, token, http, bearer=False
        )

    def get_project(self, project_id: str, *, max_issues: int) -> dict[str, Any]:
        after: str | None = None
        project: dict[str, Any] | None = None
        issues: list[dict[str, Any]] = []
        while True:
            data = self.graphql.execute(
                self.PROJECT_QUERY, {"id": project_id, "after": after}
            )
            current = data.get("project")
            if not isinstance(current, dict):
                raise SyncError(f"Linear project not found: {project_id}")
            if project is None:
                project = dict(current)
            connection = current.get("issues")
            if not isinstance(connection, dict):
                raise SyncError(f"Linear project issues missing: {project_id}")
            nodes = connection.get("nodes")
            if not isinstance(nodes, list):
                raise SyncError(f"Linear project issue nodes invalid: {project_id}")
            issues.extend(item for item in nodes if isinstance(item, dict))
            if len(issues) > max_issues:
                raise SyncError(
                    f"Linear project {project_id} exceeds max issue bound {max_issues}"
                )
            page_info = connection.get("pageInfo")
            if not isinstance(page_info, dict) or not page_info.get("hasNextPage"):
                break
            end_cursor = page_info.get("endCursor")
            if not isinstance(end_cursor, str) or not end_cursor:
                raise SyncError(f"Linear pagination cursor missing: {project_id}")
            after = end_cursor
        assert project is not None
        project["issues"] = issues
        return project

    def update_project(self, project_id: str, **fields: Any) -> None:
        data = self.graphql.execute(
            self.UPDATE_PROJECT_MUTATION,
            {"id": project_id, "input": fields},
        )
        payload = data.get("projectUpdate")
        if not isinstance(payload, dict) or payload.get("success") is not True:
            raise SyncError(f"Linear project update was not successful: {project_id}")


class SlackClient:
    def __init__(self, token: str, http: JsonHttpClient, *, write_delay: float) -> None:
        if not token:
            raise SyncError("missing Slack token")
        self.token = token
        self.http = http
        self.write_delay = write_delay

    def call(self, method: str, payload: Mapping[str, Any]) -> dict[str, Any]:
        response = self.http.request_json(
            "POST",
            f"{SLACK_API_URL}/{method}",
            headers={"Authorization": f"Bearer {self.token}"},
            payload=payload,
        )
        if response.get("ok") is not True:
            error = response.get("error", "unknown_error")
            raise SyncError(f"Slack {method} failed: {error}")
        return response

    def list_public_channels(self) -> dict[str, dict[str, Any]]:
        cursor = ""
        channels: dict[str, dict[str, Any]] = {}
        while True:
            payload: dict[str, Any] = {
                "exclude_archived": False,
                "limit": 200,
                "types": "public_channel",
            }
            if cursor:
                payload["cursor"] = cursor
            response = self.call("conversations.list", payload)
            raw_channels = response.get("channels")
            if not isinstance(raw_channels, list):
                raise SyncError("Slack conversations.list returned invalid channels")
            for channel in raw_channels:
                if isinstance(channel, dict) and isinstance(channel.get("id"), str):
                    channels[str(channel["id"])] = channel
            metadata = response.get("response_metadata")
            next_cursor = metadata.get("next_cursor", "") if isinstance(metadata, dict) else ""
            if not isinstance(next_cursor, str) or not next_cursor:
                return channels
            cursor = next_cursor

    def _write(self, method: str, payload: Mapping[str, Any]) -> dict[str, Any]:
        try:
            result = self.call(method, payload)
        except SyncError as error:
            if "not_in_channel" not in str(error):
                raise
            self.call("conversations.join", {"channel": payload["channel"]})
            result = self.call(method, payload)
        if self.write_delay > 0:
            time.sleep(self.write_delay)
        return result

    def rename_channel(self, channel_id: str, name: str) -> None:
        self._write("conversations.rename", {"channel": channel_id, "name": name})

    def set_purpose(self, channel_id: str, purpose: str) -> None:
        if len(purpose) > MAX_SLACK_METADATA:
            raise SyncError(f"refusing Slack purpose longer than {MAX_SLACK_METADATA}")
        self._write(
            "conversations.setPurpose", {"channel": channel_id, "purpose": purpose}
        )


@dataclass(frozen=True)
class ChatGPTSnapshot:
    names: frozenset[str]

    @classmethod
    def load(cls, path: Path | None) -> "ChatGPTSnapshot | None":
        if path is None:
            return None
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError as error:
            raise SyncError(f"ChatGPT snapshot not found: {path}") from error
        except json.JSONDecodeError as error:
            raise SyncError(f"ChatGPT snapshot is invalid JSON: {error}") from error
        projects = raw.get("projects") if isinstance(raw, dict) else raw
        if not isinstance(projects, list):
            raise SyncError("ChatGPT snapshot must be a list or an object with projects")
        names: set[str] = set()
        for item in projects:
            if isinstance(item, str):
                name = item
            elif isinstance(item, dict) and isinstance(item.get("name"), str):
                name = item["name"]
            else:
                raise SyncError("ChatGPT snapshot project entries need a string name")
            normalized = name.casefold()
            if normalized in names:
                raise SyncError(f"duplicate ChatGPT project in snapshot: {name}")
            names.add(normalized)
        return cls(frozenset(names))


class PortfolioSynchronizer:
    def __init__(
        self,
        registry: Registry,
        github: GitHubClient,
        linear: LinearClient,
        slack: SlackClient,
        chatgpt_snapshot: ChatGPTSnapshot | None,
        *,
        apply: bool,
        sync_issues: bool,
        max_issues_per_project: int,
    ) -> None:
        self.registry = registry
        self.github = github
        self.linear = linear
        self.slack = slack
        self.chatgpt_snapshot = chatgpt_snapshot
        self.apply = apply
        self.sync_issues = sync_issues
        self.max_issues_per_project = max_issues_per_project

    def run(self) -> list[EntryReport]:
        channels = self.slack.list_public_channels()
        reports: list[EntryReport] = []
        for entry in self.registry.entries:
            report = EntryReport(entry.key)
            reports.append(report)
            try:
                self._sync_entry(entry, channels, report)
            except SyncError as error:
                report.add_error(str(error))
            except Exception as error:  # defensive boundary; no secrets in type name
                report.add_error(f"unexpected {type(error).__name__}")
        return reports

    def _sync_entry(
        self,
        entry: ProjectMapping,
        channels: Mapping[str, dict[str, Any]],
        report: EntryReport,
    ) -> None:
        linear_project = self.linear.get_project(
            entry.linear_project_id, max_issues=self.max_issues_per_project
        )
        github_project = self.github.get_project(
            entry.github_owner, entry.github_project_number
        )
        channel = channels.get(entry.slack_channel_id)
        if not isinstance(channel, dict):
            raise SyncError(
                f"Slack channel not found by canonical ID: {entry.slack_channel_id}"
            )

        linear_url = value_string(linear_project, "url")
        github_url = value_string(github_project, "url")
        if github_url.rstrip("/") != entry.github_project_url.rstrip("/"):
            report.add_warning(
                f"GitHub API URL {github_url} differs from registry URL "
                f"{entry.github_project_url}"
            )

        self._verify_chatgpt(entry, report)
        self._sync_github_project(entry, github_project, linear_url, report)
        self._sync_linear_project(entry, linear_project, report)
        self._sync_slack_channel(entry, channel, linear_url, report)
        if self.sync_issues and entry.issue_sync_enabled:
            self._sync_linear_issues(entry, linear_project, github_project, report)

    def _verify_chatgpt(self, entry: ProjectMapping, report: EntryReport) -> None:
        if self.chatgpt_snapshot is None:
            report.chatgpt_verification = "registry-only"
            report.add_warning(
                "ChatGPT Project is keyed in the registry but no live snapshot was supplied"
            )
            return
        if entry.chatgpt_name.casefold() not in self.chatgpt_snapshot.names:
            raise SyncError(
                f"ChatGPT snapshot is missing project {entry.chatgpt_name!r}"
            )
        report.chatgpt_verification = "snapshot-verified"

    def _sync_github_project(
        self,
        entry: ProjectMapping,
        project: Mapping[str, Any],
        linear_url: str,
        report: EntryReport,
    ) -> None:
        project_id = value_string(project, "id")
        desired_block = identity_markdown(entry, linear_url)
        current_readme = nullable_string(project.get("readme"))
        desired_readme = replace_managed_block(current_readme, desired_block)
        desired_short = github_short_description(entry)
        update: dict[str, Any] = {}
        details: list[str] = []
        if project.get("title") != entry.github_project_title:
            update["title"] = entry.github_project_title
            details.append("title")
        if project.get("closed") is True:
            update["closed"] = False
            details.append("open state")
        if nullable_string(project.get("shortDescription")) != desired_short:
            update["shortDescription"] = desired_short
            details.append("short description")
        if current_readme != desired_readme:
            update["readme"] = desired_readme
            details.append("README identity block")
        if not update:
            return
        applied = False
        if self.apply:
            self.github.update_project(project_id, **update)
            applied = True
        report.add_change(
            "github",
            "update-project",
            ", ".join(details),
            applied,
        )

    def _sync_linear_project(
        self,
        entry: ProjectMapping,
        project: Mapping[str, Any],
        report: EntryReport,
    ) -> None:
        current_description = nullable_string(project.get("description"))
        desired_description = replace_managed_block(
            current_description,
            identity_markdown(entry, value_string(project, "url")),
        )
        update: dict[str, Any] = {}
        details: list[str] = []
        if project.get("name") != entry.linear_project_name:
            update["name"] = entry.linear_project_name
            details.append("name")
        if current_description != desired_description:
            update["description"] = desired_description
            details.append("description identity block")
        if not update:
            return
        applied = False
        if self.apply:
            self.linear.update_project(entry.linear_project_id, **update)
            applied = True
        report.add_change(
            "linear", "update-project", ", ".join(details), applied
        )

    def _sync_slack_channel(
        self,
        entry: ProjectMapping,
        channel: Mapping[str, Any],
        linear_url: str,
        report: EntryReport,
    ) -> None:
        current_name = value_string(channel, "name")
        if current_name != entry.slack_channel_name:
            applied = False
            if self.apply:
                self.slack.rename_channel(
                    entry.slack_channel_id, entry.slack_channel_name
                )
                applied = True
            report.add_change(
                "slack",
                "rename-channel",
                f"{current_name} -> {entry.slack_channel_name}",
                applied,
            )

        purpose_value = channel.get("purpose")
        current_purpose = ""
        if isinstance(purpose_value, dict):
            current_purpose = nullable_string(purpose_value.get("value"))
        managed = slack_managed_purpose(entry, linear_url)
        desired_purpose = merge_slack_managed_purpose(current_purpose, managed)
        if current_purpose == desired_purpose:
            return
        applied = False
        if self.apply:
            self.slack.set_purpose(entry.slack_channel_id, desired_purpose)
            applied = True
        report.add_change(
            "slack", "set-purpose", "managed identity segment", applied
        )

    def _sync_linear_issues(
        self,
        entry: ProjectMapping,
        linear_project: Mapping[str, Any],
        github_project: Mapping[str, Any],
        report: EntryReport,
    ) -> None:
        raw_issues = linear_project.get("issues")
        raw_items = github_project.get("items")
        if not isinstance(raw_issues, list) or not isinstance(raw_items, list):
            raise SyncError(f"{entry.key}: issue collections are invalid")
        issues = [item for item in raw_issues if isinstance(item, dict)]
        items = [item for item in raw_items if isinstance(item, dict)]
        plan = plan_draft_issue_changes(issues, items, entry.key)
        project_id = value_string(github_project, "id")
        for issue in plan.creates:
            title, body = desired_draft_issue(issue, entry.key)
            applied = False
            if self.apply:
                self.github.create_draft_issue(project_id, title, body)
                applied = True
            report.add_change(
                "github",
                "create-linear-draft",
                value_string(issue, "identifier"),
                applied,
            )
        for issue, item in plan.updates:
            content = require_mapping(item, "content")
            draft_id = value_string(content, "id")
            title, body = desired_draft_issue(issue, entry.key)
            applied = False
            if self.apply:
                self.github.update_draft_issue(draft_id, title, body)
                applied = True
            report.add_change(
                "github",
                "update-linear-draft",
                value_string(issue, "identifier"),
                applied,
            )
        for item in plan.archives:
            content = require_mapping(item, "content")
            marker = extract_linear_issue_id(nullable_string(content.get("body")))
            applied = False
            if self.apply:
                self.github.archive_project_item(project_id, value_string(item, "id"))
                applied = True
            report.add_change(
                "github",
                "archive-linear-draft",
                marker or value_string(item, "id"),
                applied,
            )
        report.stats.update(
            {
                "linear_issues_total": len(issues),
                "draft_creates": len(plan.creates),
                "draft_updates": len(plan.updates),
                "draft_archives": len(plan.archives),
            }
        )


def require_mapping(value: Mapping[str, Any], key: str) -> Mapping[str, Any]:
    item = value[key]
    if not isinstance(item, dict):
        raise TypeError(f"{key} must be an object")
    return item


def require_string(value: Mapping[str, Any], key: str) -> str:
    item = value[key]
    if not isinstance(item, str) or not item:
        raise TypeError(f"{key} must be a non-empty string")
    return item


def require_int(value: Mapping[str, Any], key: str) -> int:
    item = value[key]
    if isinstance(item, bool) or not isinstance(item, int):
        raise TypeError(f"{key} must be an integer")
    return item


def value_string(value: Mapping[str, Any], key: str) -> str:
    item = value.get(key)
    if not isinstance(item, str) or not item:
        raise SyncError(f"required string missing: {key}")
    return item


def nullable_string(value: Any) -> str:
    return value if isinstance(value, str) else ""


def parse_retry_after(value: str | None, *, fallback: int) -> float:
    if value is None:
        return float(fallback)
    try:
        return max(0.0, min(float(value), 60.0))
    except ValueError:
        return float(fallback)


def safe_api_message(raw: str) -> str:
    try:
        decoded = json.loads(raw)
    except json.JSONDecodeError:
        return "request failed"
    if isinstance(decoded, dict):
        for key in ("message", "error", "error_description"):
            value = decoded.get(key)
            if isinstance(value, str):
                return value[:500]
    return "request failed"


def replace_managed_block(existing: str, body: str) -> str:
    block = f"{MANAGED_BEGIN}\n{body.strip()}\n{MANAGED_END}"
    has_begin = MANAGED_BEGIN in existing
    has_end = MANAGED_END in existing
    if has_begin != has_end:
        raise SyncError("managed identity block has only one boundary marker")
    if not has_begin:
        stripped = existing.rstrip()
        return f"{stripped}\n\n{block}\n" if stripped else f"{block}\n"
    start = existing.index(MANAGED_BEGIN)
    end = existing.index(MANAGED_END, start) + len(MANAGED_END)
    if existing.find(MANAGED_BEGIN, start + len(MANAGED_BEGIN)) != -1:
        raise SyncError("multiple managed identity blocks found")
    return (existing[:start] + block + existing[end:]).rstrip() + "\n"


def identity_markdown(entry: ProjectMapping, linear_url: str) -> str:
    return "\n".join(
        [
            "### Portfolio identity",
            "",
            f"- Canonical key: `{entry.key}`",
            f"- ChatGPT Project: `{entry.chatgpt_name}`",
            f"- GitHub Project: [{entry.github_project_title}]({entry.github_project_url})",
            f"- Linear Project: [{entry.linear_project_name}]({linear_url})",
            f"- Slack: `#{entry.slack_channel_name}` (`{entry.slack_channel_id}`)",
            "",
            "Managed by `ORESoftware/k8s-cluster` portfolio sync. Text outside "
            "this marked block is preserved.",
        ]
    )


def github_short_description(entry: ProjectMapping) -> str:
    value = (
        f"key={entry.key} | Linear={entry.linear_project_id} | "
        f"Slack=#{entry.slack_channel_name} | ChatGPT={entry.chatgpt_name}"
    )
    if len(value) > MAX_GITHUB_SHORT_DESCRIPTION:
        raise SyncError("generated GitHub short description exceeds platform limit")
    return value


def slack_managed_purpose(entry: ProjectMapping, linear_url: str) -> str:
    full = (
        f"[portfolio-sync key={entry.key}] GitHub={entry.github_project_url} | "
        f"Linear={linear_url} | ChatGPT={entry.chatgpt_name}"
    )
    if len(full) <= MAX_SLACK_METADATA:
        return full
    compact = (
        f"[portfolio-sync key={entry.key}] GitHub={entry.github_owner}#"
        f"{entry.github_project_number} | Linear={entry.linear_project_id} | "
        f"ChatGPT={entry.chatgpt_name}"
    )
    if len(compact) > MAX_SLACK_METADATA:
        raise SyncError("generated Slack managed purpose exceeds platform limit")
    return compact


def merge_slack_managed_purpose(existing: str, managed: str) -> str:
    marker_count = existing.count(SLACK_MANAGED_PREFIX)
    if marker_count > 1:
        raise SyncError("multiple managed Slack purpose segments found")
    matches = list(SLACK_MANAGED_RE.finditer(existing))
    if marker_count == 1 and not matches:
        raise SyncError(
            "managed Slack purpose segment is malformed or is not the final segment"
        )
    if matches:
        match = matches[0]
        result = (existing[: match.start()] + managed + existing[match.end() :]).strip()
    elif existing.strip():
        result = f"{existing.strip()} | {managed}"
    else:
        result = managed
    if len(result) > MAX_SLACK_METADATA:
        raise SyncError(
            "Slack purpose has human text that leaves no room for the managed identity segment"
        )
    return result


def extract_linear_issue_id(body: str) -> str | None:
    match = LINEAR_ISSUE_MARKER_RE.search(body)
    return match.group(1).lower() if match else None


def linear_state_type(issue: Mapping[str, Any]) -> str:
    state = issue.get("state")
    if not isinstance(state, dict):
        raise SyncError(f"Linear issue {issue.get('id', '<unknown>')} has no state")
    state_type = state.get("type")
    if not isinstance(state_type, str) or not state_type:
        raise SyncError(f"Linear issue {issue.get('id', '<unknown>')} has invalid state")
    return state_type.casefold()


def is_linear_issue_active(issue: Mapping[str, Any]) -> bool:
    state_type = linear_state_type(issue)
    if state_type in ACTIVE_LINEAR_STATE_TYPES:
        return True
    if state_type in TERMINAL_LINEAR_STATE_TYPES:
        return False
    raise SyncError(f"unsupported Linear workflow state type: {state_type}")


def desired_draft_issue(issue: Mapping[str, Any], project_key: str) -> tuple[str, str]:
    issue_id = value_string(issue, "id").lower()
    identifier = value_string(issue, "identifier")
    title = value_string(issue, "title")
    issue_url = value_string(issue, "url")
    state = require_mapping(issue, "state")
    state_name = require_string(state, "name")
    updated_at = value_string(issue, "updatedAt")
    priority = issue.get("priority")
    priority_value = str(priority) if isinstance(priority, int) else "unset"
    draft_title = f"[{identifier}] {title}"
    draft_body = "\n".join(
        [
            f"<!-- portfolio-sync:linear-issue-id:{issue_id} -->",
            f"Canonical project key: `{project_key}`",
            f"Linear issue: [{identifier}]({issue_url})",
            f"Linear state: `{state_name}`",
            f"Linear priority: `{priority_value}`",
            f"Linear updated at: `{updated_at}`",
            "",
            "This draft item mirrors Linear metadata and is managed by "
            "`ORESoftware/k8s-cluster` portfolio sync. Edit the Linear issue, not "
            "this draft item.",
        ]
    )
    return draft_title, draft_body


def managed_github_drafts(
    project_items: Iterable[Mapping[str, Any]],
) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for item in project_items:
        content = item.get("content")
        if not isinstance(content, dict) or content.get("__typename") != "DraftIssue":
            continue
        marker = extract_linear_issue_id(nullable_string(content.get("body")))
        if marker is None:
            continue
        if marker in result:
            raise SyncError(f"duplicate GitHub draft mirror for Linear issue {marker}")
        result[marker] = dict(item)
    return result


def plan_draft_issue_changes(
    linear_issues: Sequence[Mapping[str, Any]],
    github_items: Sequence[Mapping[str, Any]],
    project_key: str,
) -> DraftIssuePlan:
    issues_by_id: dict[str, Mapping[str, Any]] = {}
    active_by_id: dict[str, Mapping[str, Any]] = {}
    for issue in linear_issues:
        issue_id = value_string(issue, "id").lower()
        if issue_id in issues_by_id:
            raise SyncError(f"duplicate Linear issue ID in project: {issue_id}")
        issues_by_id[issue_id] = issue
        if is_linear_issue_active(issue):
            active_by_id[issue_id] = issue

    drafts = managed_github_drafts(github_items)
    creates: list[dict[str, Any]] = []
    updates: list[tuple[dict[str, Any], dict[str, Any]]] = []
    archives: list[dict[str, Any]] = []

    for issue_id, issue in active_by_id.items():
        item = drafts.get(issue_id)
        if item is None:
            creates.append(dict(issue))
            continue
        content = require_mapping(item, "content")
        desired_title, desired_body = desired_draft_issue(issue, project_key)
        if content.get("title") != desired_title or nullable_string(
            content.get("body")
        ) != desired_body:
            updates.append((dict(issue), dict(item)))

    for issue_id, item in drafts.items():
        if issue_id not in active_by_id:
            archives.append(item)

    sort_key = lambda issue: value_string(issue, "identifier")
    creates.sort(key=sort_key)
    updates.sort(key=lambda pair: sort_key(pair[0]))
    archives.sort(
        key=lambda item: extract_linear_issue_id(
            nullable_string(require_mapping(item, "content").get("body"))
        )
        or ""
    )
    return DraftIssuePlan(tuple(creates), tuple(updates), tuple(archives))


def build_report(
    registry: Registry,
    entries: Sequence[EntryReport],
    *,
    apply: bool,
    sync_issues: bool,
    started_at: datetime,
    finished_at: datetime,
) -> dict[str, Any]:
    changes = sum(len(item.changes) for item in entries)
    applied_changes = sum(
        1 for item in entries for change in item.changes if change.applied
    )
    errors = sum(len(item.errors) for item in entries)
    warnings = sum(len(item.warnings) for item in entries)
    drifted = sum(1 for item in entries if item.status == "drift")
    return {
        "schema_version": 1,
        "registry": registry.name,
        "mode": "apply" if apply else "dry-run",
        "issue_sync": sync_issues,
        "started_at": started_at.isoformat(),
        "finished_at": finished_at.isoformat(),
        "summary": {
            "projects": len(entries),
            "changes": changes,
            "applied_changes": applied_changes,
            "errors": errors,
            "warnings": warnings,
            "drifted_projects": drifted,
        },
        "entries": [item.to_json() for item in entries],
    }


def render_markdown_report(report: Mapping[str, Any]) -> str:
    summary = require_mapping(report, "summary")
    lines = [
        "## Portfolio project sync",
        "",
        f"- Mode: `{report.get('mode')}`",
        f"- Projects checked: `{summary.get('projects')}`",
        f"- Changes: `{summary.get('changes')}`",
        f"- Applied changes: `{summary.get('applied_changes')}`",
        f"- Errors: `{summary.get('errors')}`",
        f"- Warnings: `{summary.get('warnings')}`",
        f"- Linear issue mirror enabled: `{report.get('issue_sync')}`",
        "",
        "| Key | Status | Changes | Errors | ChatGPT |",
        "|---|---:|---:|---:|---|",
    ]
    entries = report.get("entries")
    if isinstance(entries, list):
        for item in entries:
            if not isinstance(item, dict):
                continue
            lines.append(
                "| {key} | {status} | {changes} | {errors} | {chatgpt} |".format(
                    key=item.get("key", ""),
                    status=item.get("status", ""),
                    changes=len(item.get("changes", []))
                    if isinstance(item.get("changes"), list)
                    else 0,
                    errors=len(item.get("errors", []))
                    if isinstance(item.get("errors"), list)
                    else 0,
                    chatgpt=item.get("chatgpt_verification", ""),
                )
            )
    return "\n".join(lines) + "\n"


def resolve_snapshot_path(argument: str | None) -> Path | None:
    candidate = argument or os.environ.get("CHATGPT_PROJECTS_SNAPSHOT", "")
    return Path(candidate) if candidate else None


def write_outputs(
    report: Mapping[str, Any], report_path: Path, summary_path: Path | None
) -> None:
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    if summary_path is not None:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        with summary_path.open("a", encoding="utf-8") as handle:
            handle.write(render_markdown_report(report))


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path("config/portfolio-projects.json"),
    )
    parser.add_argument(
        "--report",
        type=Path,
        default=Path("artifacts/portfolio-project-sync-report.json"),
    )
    parser.add_argument("--summary", type=Path)
    parser.add_argument("--chatgpt-snapshot")
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--sync-issues", action="store_true")
    parser.add_argument("--require-chatgpt-snapshot", action="store_true")
    parser.add_argument("--fail-on-drift", action="store_true")
    parser.add_argument("--max-issues-per-project", type=int, default=5000)
    parser.add_argument("--slack-write-delay", type=float, default=3.1)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    started_at = datetime.now(timezone.utc)
    registry: Registry | None = None
    entry_reports: list[EntryReport] = []
    fatal_error: str | None = None

    try:
        if args.max_issues_per_project < 1:
            raise SyncError("--max-issues-per-project must be positive")
        if args.slack_write_delay < 0:
            raise SyncError("--slack-write-delay cannot be negative")
        registry = Registry.load(args.registry)
        snapshot = ChatGPTSnapshot.load(resolve_snapshot_path(args.chatgpt_snapshot))
        if args.require_chatgpt_snapshot and snapshot is None:
            raise SyncError("a ChatGPT Projects snapshot is required")

        if args.validate_only:
            entry_reports = [EntryReport(entry.key) for entry in registry.entries]
            for item in entry_reports:
                if snapshot is None:
                    item.chatgpt_verification = "registry-only"
                elif item.key.casefold() in snapshot.names:
                    item.chatgpt_verification = "snapshot-verified"
                else:
                    item.add_error(f"ChatGPT snapshot is missing project {item.key!r}")
        else:
            http = JsonHttpClient()
            github = GitHubClient(
                os.environ.get("PORTFOLIO_GITHUB_TOKEN", ""), http
            )
            linear = LinearClient(os.environ.get("LINEAR_API_KEY", ""), http)
            slack = SlackClient(
                os.environ.get("SLACK_BOT_TOKEN", ""),
                http,
                write_delay=args.slack_write_delay,
            )
            synchronizer = PortfolioSynchronizer(
                registry,
                github,
                linear,
                slack,
                snapshot,
                apply=args.apply,
                sync_issues=args.sync_issues,
                max_issues_per_project=args.max_issues_per_project,
            )
            entry_reports = synchronizer.run()
    except SyncError as error:
        fatal_error = str(error)

    finished_at = datetime.now(timezone.utc)
    if registry is None:
        report: dict[str, Any] = {
            "schema_version": 1,
            "mode": "apply" if args.apply else "dry-run",
            "issue_sync": args.sync_issues,
            "started_at": started_at.isoformat(),
            "finished_at": finished_at.isoformat(),
            "summary": {
                "projects": 0,
                "changes": 0,
                "applied_changes": 0,
                "errors": 1,
                "warnings": 0,
                "drifted_projects": 0,
            },
            "entries": [],
            "fatal_error": fatal_error or "registry unavailable",
        }
    else:
        report = build_report(
            registry,
            entry_reports,
            apply=args.apply,
            sync_issues=args.sync_issues,
            started_at=started_at,
            finished_at=finished_at,
        )
        if fatal_error:
            report["fatal_error"] = fatal_error
            summary = require_mapping(report, "summary")
            summary["errors"] = int(summary.get("errors", 0)) + 1

    write_outputs(report, args.report, args.summary)
    print(render_markdown_report(report), end="")

    summary = require_mapping(report, "summary")
    errors = int(summary.get("errors", 0))
    drifted = int(summary.get("drifted_projects", 0))
    if errors:
        return 1
    if args.fail_on_drift and drifted:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
