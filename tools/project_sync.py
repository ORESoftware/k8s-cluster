#!/usr/bin/env python3
"""Reconcile canonical links across GitHub Projects, Linear, ChatGPT, and Slack."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping

from project_links import (
    github_project_url,
    load_json,
    merge_compact_marker,
    merge_managed_block,
    scheduled_cron_is_active,
    validate_catalog,
)

GITHUB_GRAPHQL_URL = "https://api.github.com/graphql"
LINEAR_GRAPHQL_URL = "https://api.linear.app/graphql"
SLACK_API_URL = "https://slack.com/api"
USER_AGENT = "oresoftware-project-link-sync/1"


class ProviderError(RuntimeError):
    """A provider request failed without exposing credentials."""


@dataclass
class Result:
    key: str
    provider: str
    status: str
    changed: bool = False
    detail: str = ""


class JsonHttpClient:
    def __init__(self, token: str, authorization: str) -> None:
        self.token = token
        self.authorization = authorization

    def post_json(
        self,
        url: str,
        payload: Mapping[str, Any],
        extra_headers: Mapping[str, str] | None = None,
    ) -> Mapping[str, Any]:
        headers = {
            "Accept": "application/json",
            "Authorization": self.authorization.format(token=self.token),
            "Content-Type": "application/json",
            "User-Agent": USER_AGENT,
        }
        if extra_headers:
            headers.update(extra_headers)
        request = urllib.request.Request(
            url,
            data=json.dumps(payload).encode("utf-8"),
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                value = json.load(response)
        except urllib.error.HTTPError as exc:
            body = exc.read(1200).decode("utf-8", "replace")
            raise ProviderError(
                f"HTTP {exc.code} from provider: {body[:1200]}"
            ) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise ProviderError(f"provider request failed: {exc}") from exc
        if not isinstance(value, Mapping):
            raise ProviderError("provider returned a non-object JSON response")
        return value


class GitHubClient:
    def __init__(self, token: str) -> None:
        self.http = JsonHttpClient(token, "Bearer {token}")

    def graphql(
        self, query: str, variables: Mapping[str, Any] | None = None
    ) -> Mapping[str, Any]:
        response = self.http.post_json(
            GITHUB_GRAPHQL_URL,
            {"query": query, "variables": variables or {}},
            {"X-Github-Next-Global-ID": "1"},
        )
        errors = response.get("errors")
        if errors:
            raise ProviderError(f"GitHub GraphQL error: {json.dumps(errors)[:1200]}")
        data = response.get("data")
        if not isinstance(data, Mapping):
            raise ProviderError("GitHub response omitted data")
        return data

    def organization_projects(self, login: str) -> tuple[str, list[Mapping[str, Any]]]:
        query = """
        query ProjectLinkSyncOrganization($login: String!) {
          organization(login: $login) {
            id
            projectsV2(first: 100) {
              nodes {
                id
                number
                title
                closed
                url
                shortDescription
              }
            }
          }
        }
        """
        organization = self.graphql(query, {"login": login}).get("organization")
        if not isinstance(organization, Mapping):
            raise ProviderError(f"GitHub organization {login} is not visible")
        owner_id = organization.get("id")
        projects = organization.get("projectsV2")
        nodes = projects.get("nodes") if isinstance(projects, Mapping) else None
        if not isinstance(owner_id, str) or not isinstance(nodes, list):
            raise ProviderError(f"GitHub organization {login} returned incomplete data")
        return owner_id, [node for node in nodes if isinstance(node, Mapping)]

    def create_project(self, owner_id: str, title: str) -> Mapping[str, Any]:
        mutation = """
        mutation ProjectLinkSyncCreate($input: CreateProjectV2Input!) {
          createProjectV2(input: $input) {
            projectV2 { id number title closed url shortDescription }
          }
        }
        """
        value = self.graphql(
            mutation, {"input": {"ownerId": owner_id, "title": title}}
        ).get("createProjectV2")
        project = value.get("projectV2") if isinstance(value, Mapping) else None
        if not isinstance(project, Mapping):
            raise ProviderError("GitHub did not return the created Project")
        return project

    def update_project(
        self,
        project_id: str,
        title: str,
        short_description: str,
        closed: bool,
    ) -> Mapping[str, Any]:
        mutation = """
        mutation ProjectLinkSyncUpdate($input: UpdateProjectV2Input!) {
          updateProjectV2(input: $input) {
            projectV2 { id number title closed url shortDescription }
          }
        }
        """
        value = self.graphql(
            mutation,
            {
                "input": {
                    "projectId": project_id,
                    "title": title,
                    "shortDescription": short_description,
                    "closed": closed,
                }
            },
        ).get("updateProjectV2")
        project = value.get("projectV2") if isinstance(value, Mapping) else None
        if not isinstance(project, Mapping):
            raise ProviderError("GitHub did not return the updated Project")
        return project


class LinearClient:
    def __init__(self, token: str) -> None:
        self.http = JsonHttpClient(token, "{token}")

    def graphql(
        self, query: str, variables: Mapping[str, Any] | None = None
    ) -> Mapping[str, Any]:
        response = self.http.post_json(
            LINEAR_GRAPHQL_URL, {"query": query, "variables": variables or {}}
        )
        errors = response.get("errors")
        if errors:
            raise ProviderError(f"Linear GraphQL error: {json.dumps(errors)[:1200]}")
        data = response.get("data")
        if not isinstance(data, Mapping):
            raise ProviderError("Linear response omitted data")
        return data

    def projects(self) -> list[Mapping[str, Any]]:
        query = """
        query ProjectLinkSyncProjects($after: String) {
          projects(first: 100, after: $after, includeArchived: false) {
            nodes { id name description canceledAt }
            pageInfo { hasNextPage endCursor }
          }
        }
        """
        after: str | None = None
        results: list[Mapping[str, Any]] = []
        while True:
            connection = self.graphql(query, {"after": after}).get("projects")
            if not isinstance(connection, Mapping):
                raise ProviderError("Linear projects query returned incomplete data")
            nodes = connection.get("nodes")
            page_info = connection.get("pageInfo")
            if not isinstance(nodes, list) or not isinstance(page_info, Mapping):
                raise ProviderError("Linear projects query returned invalid pagination")
            results.extend(node for node in nodes if isinstance(node, Mapping))
            if not page_info.get("hasNextPage"):
                break
            after = page_info.get("endCursor")
            if not isinstance(after, str) or not after:
                raise ProviderError("Linear pagination omitted endCursor")
        return results

    def update_description(self, project_id: str, description: str) -> None:
        mutation = """
        mutation ProjectLinkSyncUpdateLinear(
          $id: String!,
          $input: ProjectUpdateInput!
        ) {
          projectUpdate(id: $id, input: $input) { success }
        }
        """
        value = self.graphql(
            mutation, {"id": project_id, "input": {"description": description}}
        ).get("projectUpdate")
        if not isinstance(value, Mapping) or value.get("success") is not True:
            raise ProviderError("Linear projectUpdate did not report success")


class SlackClient:
    def __init__(self, token: str) -> None:
        self.token = token

    def call(self, method: str, payload: Mapping[str, Any]) -> Mapping[str, Any]:
        body = urllib.parse.urlencode(payload).encode("utf-8")
        request = urllib.request.Request(
            f"{SLACK_API_URL}/{method}",
            data=body,
            headers={
                "Accept": "application/json",
                "Authorization": f"Bearer {self.token}",
                "Content-Type": "application/x-www-form-urlencoded",
                "User-Agent": USER_AGENT,
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                value = json.load(response)
        except urllib.error.HTTPError as exc:
            body_text = exc.read(1200).decode("utf-8", "replace")
            raise ProviderError(
                f"Slack HTTP {exc.code}: {body_text[:1200]}"
            ) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise ProviderError(f"Slack request failed: {exc}") from exc
        if not isinstance(value, Mapping):
            raise ProviderError("Slack returned a non-object response")
        if value.get("ok") is not True:
            raise ProviderError(f"Slack {method} failed: {value.get('error', 'unknown')}")
        return value

    def channels(self) -> list[Mapping[str, Any]]:
        cursor = ""
        results: list[Mapping[str, Any]] = []
        while True:
            value = self.call(
                "conversations.list",
                {
                    "types": "public_channel,private_channel",
                    "exclude_archived": "true",
                    "limit": "200",
                    "cursor": cursor,
                },
            )
            channels = value.get("channels")
            if not isinstance(channels, list):
                raise ProviderError("Slack conversations.list omitted channels")
            results.extend(channel for channel in channels if isinstance(channel, Mapping))
            metadata = value.get("response_metadata")
            cursor = metadata.get("next_cursor", "") if isinstance(metadata, Mapping) else ""
            if not cursor:
                return results

    def create_channel(self, name: str) -> Mapping[str, Any]:
        value = self.call("conversations.create", {"name": name})
        channel = value.get("channel")
        if not isinstance(channel, Mapping):
            raise ProviderError("Slack did not return the created channel")
        return channel

    def set_topic(self, channel_id: str, topic: str) -> None:
        self.call("conversations.setTopic", {"channel": channel_id, "topic": topic})

    def post_message(self, channel_id: str, text: str) -> None:
        self.call("chat.postMessage", {"channel": channel_id, "text": text})


class ChatGPTWebhookClient:
    def __init__(self, endpoint: str, token: str | None) -> None:
        self.endpoint = endpoint
        self.token = token or ""

    def reconcile(self, projects: list[Mapping[str, Any]], apply: bool) -> None:
        authorization = "Bearer {token}" if self.token else "{token}"
        client = JsonHttpClient(self.token, authorization)
        value = client.post_json(
            self.endpoint,
            {
                "schema_version": 1,
                "mode": "apply" if apply else "dry-run",
                "projects": projects,
            },
        )
        if value.get("ok") is not True:
            raise ProviderError("ChatGPT project webhook did not report ok=true")


def _github_result(
    client: GitHubClient, entry: Mapping[str, Any], apply: bool
) -> Result:
    key = str(entry["key"])
    github = entry["github"]
    org = str(github["organization"])
    expected_number = int(github["project_number"])
    expected_title = str(github["project_title"])
    owner_id, projects = client.organization_projects(org)
    by_number = [project for project in projects if project.get("number") == expected_number]
    by_title = [project for project in projects if project.get("title") == expected_title]
    if len(by_number) > 1 or len(by_title) > 1:
        raise ProviderError(f"ambiguous GitHub Project match for {org}")
    if by_title and by_title[0].get("number") != expected_number:
        raise ProviderError(
            f"{expected_title} exists as Project #{by_title[0].get('number')}, "
            f"expected #{expected_number}"
        )

    created = False
    project = by_number[0] if by_number else None
    if project is None:
        if not apply:
            return Result(key, "github", "would_create", True, expected_title)
        project = client.create_project(owner_id, expected_title)
        created = True
        if project.get("number") != expected_number:
            raise ProviderError(
                f"created {expected_title} as Project #{project.get('number')}; "
                f"expected #{expected_number}"
            )

    project_id = project.get("id")
    if not isinstance(project_id, str):
        raise ProviderError(f"GitHub Project for {org} omitted id")
    desired_short = merge_compact_marker(
        project.get("shortDescription"), entry, limit=256
    )
    changes = (
        project.get("title") != expected_title
        or project.get("closed") is True
        or project.get("shortDescription") != desired_short
    )
    if changes and apply:
        project = client.update_project(
            project_id, expected_title, desired_short, closed=False
        )
    status = "created" if created else ("updated" if changes else "in_sync")
    if changes and not apply:
        status = "would_update"
    return Result(key, "github", status, created or changes, github_project_url(entry))


def _linear_result(
    projects: list[Mapping[str, Any]],
    client: LinearClient,
    entry: Mapping[str, Any],
    apply: bool,
) -> Result:
    key = str(entry["key"])
    expected_name = str(entry["linear"]["project_name"])
    matches = [
        project
        for project in projects
        if project.get("name") == expected_name and not project.get("canceledAt")
    ]
    if len(matches) != 1:
        raise ProviderError(
            f"expected one active Linear project named {expected_name}; found {len(matches)}"
        )
    project = matches[0]
    project_id = project.get("id")
    if not isinstance(project_id, str):
        raise ProviderError(f"Linear project {expected_name} omitted id")
    desired = merge_managed_block(project.get("description"), entry)
    changed = desired != ((project.get("description") or "").strip() + "\n")
    if changed and apply:
        client.update_description(project_id, desired)
    return Result(
        key,
        "linear",
        "updated" if changed and apply else ("would_update" if changed else "in_sync"),
        changed,
        expected_name,
    )


def _slack_result(
    channels: list[Mapping[str, Any]],
    client: SlackClient,
    entry: Mapping[str, Any],
    apply: bool,
    create_missing: bool,
) -> Result:
    key = str(entry["key"])
    channel_name = str(entry["slack"]["channel_name"])[1:]
    matches = [channel for channel in channels if channel.get("name") == channel_name]
    if len(matches) > 1:
        raise ProviderError(f"ambiguous Slack channel match for #{channel_name}")
    created = False
    if not matches:
        if not create_missing:
            raise ProviderError(f"Slack channel #{channel_name} is missing")
        if not apply:
            return Result(key, "slack", "would_create", True, f"#{channel_name}")
        channel = client.create_channel(channel_name)
        channels.append(channel)
        created = True
    else:
        channel = matches[0]
    channel_id = channel.get("id")
    if not isinstance(channel_id, str):
        raise ProviderError(f"Slack channel #{channel_name} omitted id")
    topic_value = channel.get("topic")
    current_topic = topic_value.get("value") if isinstance(topic_value, Mapping) else ""
    desired_topic = merge_compact_marker(current_topic, entry, limit=250)
    changed = desired_topic != (current_topic or "")
    if changed and apply:
        client.set_topic(channel_id, desired_topic)
    status = "created" if created else ("updated" if changed and apply else "in_sync")
    if changed and not apply:
        status = "would_update"
    return Result(key, "slack", status, created or changed, f"#{channel_name}")


def render_markdown_report(
    mode: str,
    results: list[Result],
    generated_at: str,
    skipped_reason: str = "",
) -> str:
    changed = sum(1 for result in results if result.changed)
    failed = sum(1 for result in results if result.status == "failed")
    lines = [
        "# Daily project-link sync",
        "",
        f"- generated: `{generated_at}`",
        f"- mode: `{mode}`",
        f"- results: `{len(results)}`",
        f"- changed or planned: `{changed}`",
        f"- failed: `{failed}`",
    ]
    if skipped_reason:
        lines.append(f"- skipped: {skipped_reason}")
    lines.extend(
        [
            "",
            "| Key | Provider | Status | Detail |",
            "|---|---|---|---|",
        ]
    )
    for result in results:
        detail = result.detail.replace("|", "\\|").replace("\n", " ")
        lines.append(
            f"| `{result.key}` | {result.provider} | {result.status} | {detail} |"
        )
    return "\n".join(lines) + "\n"


def _write_reports(
    json_output: Path,
    markdown_output: Path,
    mode: str,
    results: list[Result],
    generated_at: str,
    skipped_reason: str = "",
) -> None:
    json_output.parent.mkdir(parents=True, exist_ok=True)
    markdown_output.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "schema_version": 1,
        "generated_at": generated_at,
        "mode": mode,
        "skipped_reason": skipped_reason or None,
        "summary": {
            "results": len(results),
            "changed_or_planned": sum(result.changed for result in results),
            "failed": sum(result.status == "failed" for result in results),
        },
        "results": [asdict(result) for result in results],
    }
    json_output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    markdown_output.write_text(
        render_markdown_report(mode, results, generated_at, skipped_reason),
        encoding="utf-8",
    )


def _parse_now(value: str | None) -> datetime:
    if not value:
        return datetime.now(timezone.utc)
    normalized = value.replace("Z", "+00:00")
    parsed = datetime.fromisoformat(normalized)
    if parsed.tzinfo is None:
        raise ValueError("--at must include a UTC offset")
    return parsed


def run(args: argparse.Namespace) -> int:
    catalog = load_json(args.catalog)
    errors = validate_catalog(catalog)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 2

    now = _parse_now(args.at)
    generated_at = now.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")
    mode = "apply" if args.apply else "dry-run"
    if args.scheduled_cron and not scheduled_cron_is_active(now, args.scheduled_cron):
        reason = (
            f"{args.scheduled_cron} is not the 03:00 America/Chicago lane for "
            f"{generated_at}"
        )
        _write_reports(
            args.json_output, args.markdown_output, mode, [], generated_at, reason
        )
        print(reason)
        return 0

    token_specs = {
        "github": os.getenv("PROJECT_SYNC_GITHUB_TOKEN", "").strip(),
        "linear": os.getenv("LINEAR_API_KEY", "").strip(),
        "slack": os.getenv("SLACK_BOT_TOKEN", "").strip(),
    }
    missing = [name for name, token in token_specs.items() if not token]
    if missing and not args.allow_missing_credentials:
        print(
            "missing required provider credentials: " + ", ".join(sorted(missing)),
            file=sys.stderr,
        )
        return 2

    github_client = GitHubClient(token_specs["github"]) if token_specs["github"] else None
    linear_client = LinearClient(token_specs["linear"]) if token_specs["linear"] else None
    slack_client = SlackClient(token_specs["slack"]) if token_specs["slack"] else None

    linear_projects = linear_client.projects() if linear_client else []
    slack_channels = slack_client.channels() if slack_client else []
    results: list[Result] = []
    projects = catalog["projects"]

    for entry in projects:
        key = str(entry["key"])
        providers = (
            (
                "github",
                lambda: _github_result(github_client, entry, args.apply)
                if github_client
                else Result(key, "github", "skipped_missing_credential"),
            ),
            (
                "linear",
                lambda: _linear_result(
                    linear_projects, linear_client, entry, args.apply
                )
                if linear_client
                else Result(key, "linear", "skipped_missing_credential"),
            ),
            (
                "slack",
                lambda: _slack_result(
                    slack_channels,
                    slack_client,
                    entry,
                    args.apply,
                    args.create_missing_slack,
                )
                if slack_client
                else Result(key, "slack", "skipped_missing_credential"),
            ),
        )
        for provider, operation in providers:
            try:
                results.append(operation())
            except (ProviderError, ValueError, KeyError, TypeError) as exc:
                results.append(Result(key, provider, "failed", False, str(exc)))

        results.append(
            Result(
                key,
                "chatgpt",
                "registry_only",
                False,
                str(entry["chatgpt"]["project_name"]),
            )
        )

    chatgpt_endpoint = os.getenv("CHATGPT_PROJECT_SYNC_ENDPOINT", "").strip()
    if chatgpt_endpoint:
        client = ChatGPTWebhookClient(
            chatgpt_endpoint,
            os.getenv("CHATGPT_PROJECT_SYNC_TOKEN", "").strip() or None,
        )
        try:
            client.reconcile(projects, args.apply)
            for result in results:
                if result.provider == "chatgpt":
                    result.status = "webhook_applied" if args.apply else "webhook_checked"
        except ProviderError as exc:
            for result in results:
                if result.provider == "chatgpt":
                    result.status = "failed"
                    result.detail = str(exc)

    failed = [result for result in results if result.status == "failed"]
    changed = [result for result in results if result.changed]

    if slack_client and args.summary_channel and (failed or changed or args.post_noop_summary):
        summary_matches = [
            channel
            for channel in slack_channels
            if channel.get("name") == args.summary_channel.lstrip("#")
        ]
        if len(summary_matches) == 1 and isinstance(summary_matches[0].get("id"), str):
            try:
                slack_client.post_message(
                    str(summary_matches[0]["id"]),
                    (
                        f"Project-link sync `{mode}` complete: {len(projects)} projects, "
                        f"{len(changed)} changed/planned, {len(failed)} failed. "
                        f"Canonical schedule: 03:00 America/Chicago."
                    ),
                )
            except ProviderError as exc:
                results.append(
                    Result("portfolio", "slack-summary", "failed", False, str(exc))
                )
                failed.append(results[-1])

    _write_reports(
        args.json_output, args.markdown_output, mode, results, generated_at
    )
    print(
        json.dumps(
            {
                "mode": mode,
                "projects": len(projects),
                "results": len(results),
                "changed_or_planned": len(changed),
                "failed": len(failed),
            },
            sort_keys=True,
        )
    )
    return 1 if failed else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--catalog", type=Path, default=Path("catalog/project-links.json")
    )
    parser.add_argument(
        "--json-output",
        type=Path,
        default=Path("artifacts/project-link-sync.json"),
    )
    parser.add_argument(
        "--markdown-output",
        type=Path,
        default=Path("artifacts/project-link-sync.md"),
    )
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--allow-missing-credentials", action="store_true")
    parser.add_argument("--create-missing-slack", action="store_true")
    parser.add_argument("--scheduled-cron")
    parser.add_argument("--at", help="ISO-8601 instant for deterministic tests")
    parser.add_argument("--summary-channel", default="oresoftware")
    parser.add_argument("--post-noop-summary", action="store_true")
    args = parser.parse_args(argv)
    try:
        return run(args)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"project sync failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
