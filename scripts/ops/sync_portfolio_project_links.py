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
from typing import Any, Callable, Mapping

from portfolio_project_links import (
    PortfolioLink,
    github_project_readme,
    github_short_description,
    load_links,
    merge_compact_marker,
    merge_linear_description,
    scheduled_cron_is_active,
    slack_marker,
)

GITHUB_GRAPHQL_URL = "https://api.github.com/graphql"
LINEAR_GRAPHQL_URL = "https://api.linear.app/graphql"
SLACK_API_URL = "https://slack.com/api"
USER_AGENT = "oresoftware-portfolio-project-sync/1"


class ProviderError(RuntimeError):
    """A provider operation failed without exposing credentials."""


@dataclass
class Result:
    portfolio_key: str
    provider: str
    status: str
    changed: bool = False
    detail: str = ""


class JsonHttpClient:
    def __init__(self, token: str, authorization_template: str) -> None:
        self.token = token
        self.authorization_template = authorization_template

    def post_json(
        self,
        url: str,
        payload: Mapping[str, Any],
        extra_headers: Mapping[str, str] | None = None,
    ) -> Mapping[str, Any]:
        headers = {
            "Accept": "application/json",
            "Authorization": self.authorization_template.format(token=self.token),
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
        self,
        query: str,
        variables: Mapping[str, Any] | None = None,
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

    def organization_projects(self, login: str) -> list[Mapping[str, Any]]:
        query = """
        query PortfolioProjectSyncOrganization($login: String!) {
          organization(login: $login) {
            projectsV2(first: 100) {
              nodes {
                id
                number
                title
                closed
                url
                shortDescription
                readme
              }
            }
          }
        }
        """
        organization = self.graphql(query, {"login": login}).get("organization")
        if not isinstance(organization, Mapping):
            raise ProviderError(f"GitHub organization {login} is not visible")
        projects = organization.get("projectsV2")
        nodes = projects.get("nodes") if isinstance(projects, Mapping) else None
        if not isinstance(nodes, list):
            raise ProviderError(f"GitHub organization {login} returned incomplete data")
        return [node for node in nodes if isinstance(node, Mapping)]

    def update_project(
        self,
        project_id: str,
        title: str,
        short_description: str,
        readme: str,
        closed: bool,
    ) -> Mapping[str, Any]:
        mutation = """
        mutation PortfolioProjectSyncUpdate($input: UpdateProjectV2Input!) {
          updateProjectV2(input: $input) {
            projectV2 {
              id
              number
              title
              closed
              url
              shortDescription
              readme
            }
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
                    "readme": readme,
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
        self,
        query: str,
        variables: Mapping[str, Any] | None = None,
    ) -> Mapping[str, Any]:
        response = self.http.post_json(
            LINEAR_GRAPHQL_URL,
            {"query": query, "variables": variables or {}},
        )
        errors = response.get("errors")
        if errors:
            raise ProviderError(f"Linear GraphQL error: {json.dumps(errors)[:1200]}")
        data = response.get("data")
        if not isinstance(data, Mapping):
            raise ProviderError("Linear response omitted data")
        return data

    def project(self, project_id: str) -> Mapping[str, Any]:
        query = """
        query PortfolioProjectSyncLinear($id: String!) {
          project(id: $id) {
            id
            name
            description
            url
            canceledAt
          }
        }
        """
        project = self.graphql(query, {"id": project_id}).get("project")
        if not isinstance(project, Mapping):
            raise ProviderError(f"Linear project {project_id} is not visible")
        return project

    def update_description(self, project_id: str, description: str) -> None:
        mutation = """
        mutation PortfolioProjectSyncUpdateLinear(
          $id: String!,
          $input: ProjectUpdateInput!
        ) {
          projectUpdate(id: $id, input: $input) {
            success
          }
        }
        """
        value = self.graphql(
            mutation,
            {"id": project_id, "input": {"description": description}},
        ).get("projectUpdate")
        if not isinstance(value, Mapping) or value.get("success") is not True:
            raise ProviderError("Linear projectUpdate did not report success")


class SlackClient:
    def __init__(self, token: str) -> None:
        self.token = token

    def call(self, method: str, payload: Mapping[str, Any]) -> Mapping[str, Any]:
        request = urllib.request.Request(
            f"{SLACK_API_URL}/{method}",
            data=urllib.parse.urlencode(payload).encode("utf-8"),
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
            body = exc.read(1200).decode("utf-8", "replace")
            raise ProviderError(f"Slack HTTP {exc.code}: {body[:1200]}") from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise ProviderError(f"Slack request failed: {exc}") from exc
        if not isinstance(value, Mapping):
            raise ProviderError("Slack returned a non-object JSON response")
        if value.get("ok") is not True:
            raise ProviderError(f"Slack {method} failed: {value.get('error', 'unknown')}")
        return value

    def workspace_id(self) -> str:
        value = self.call("auth.test", {})
        team_id = value.get("team_id")
        if not isinstance(team_id, str) or not team_id:
            raise ProviderError("Slack auth.test omitted team_id")
        return team_id

    def channel(self, channel_id: str) -> Mapping[str, Any]:
        value = self.call("conversations.info", {"channel": channel_id})
        channel = value.get("channel")
        if not isinstance(channel, Mapping):
            raise ProviderError(f"Slack channel {channel_id} is not visible")
        return channel

    def set_topic(self, channel_id: str, topic: str) -> None:
        self.call("conversations.setTopic", {"channel": channel_id, "topic": topic})

    def find_channel_id(self, name: str) -> str | None:
        cursor = ""
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
            for channel in channels:
                if isinstance(channel, Mapping) and channel.get("name") == name:
                    channel_id = channel.get("id")
                    if isinstance(channel_id, str):
                        return channel_id
            metadata = value.get("response_metadata")
            cursor = (
                metadata.get("next_cursor", "")
                if isinstance(metadata, Mapping)
                else ""
            )
            if not cursor:
                return None

    def post_message(self, channel_id: str, text: str) -> None:
        self.call("chat.postMessage", {"channel": channel_id, "text": text})


class ChatGPTWebhookClient:
    def __init__(self, endpoint: str, token: str | None) -> None:
        authorization = "Bearer {token}" if token else "{token}"
        self.endpoint = endpoint
        self.http = JsonHttpClient(token or "", authorization)

    def reconcile(self, links: list[PortfolioLink], apply: bool) -> None:
        value = self.http.post_json(
            self.endpoint,
            {
                "schema_version": 1,
                "mode": "apply" if apply else "dry-run",
                "projects": [link.routing_payload() for link in links],
            },
        )
        if value.get("ok") is not True:
            raise ProviderError("ChatGPT project webhook did not report ok=true")


def sync_github(
    client: GitHubClient,
    link: PortfolioLink,
    apply: bool,
) -> Result:
    projects = client.organization_projects(link.github_org)
    number_matches = [
        project
        for project in projects
        if project.get("number") == link.github_project_number
    ]
    title_matches = [
        project for project in projects if project.get("title") == link.github_project_title
    ]
    if len(number_matches) != 1:
        raise ProviderError(
            f"expected one GitHub Project #{link.github_project_number} in "
            f"{link.github_org}; found {len(number_matches)}"
        )
    if len(title_matches) > 1:
        raise ProviderError(
            f"ambiguous GitHub Project title {link.github_project_title!r}"
        )
    if title_matches and title_matches[0].get("number") != link.github_project_number:
        raise ProviderError(
            f"{link.github_project_title} exists as Project "
            f"#{title_matches[0].get('number')}; expected "
            f"#{link.github_project_number}"
        )

    project = number_matches[0]
    project_id = project.get("id")
    if not isinstance(project_id, str):
        raise ProviderError(f"GitHub Project {link.github_project_url} omitted id")
    if project.get("url") != link.github_project_url:
        raise ProviderError(
            f"GitHub Project URL drift: expected {link.github_project_url}, "
            f"got {project.get('url')}"
        )

    desired_short_description = github_short_description(link)
    desired_readme = github_project_readme(link)
    changed = (
        project.get("title") != link.github_project_title
        or project.get("closed") is True
        or project.get("shortDescription") != desired_short_description
        or project.get("readme") != desired_readme
    )
    if changed and apply:
        updated = client.update_project(
            project_id,
            link.github_project_title,
            desired_short_description,
            desired_readme,
            closed=False,
        )
        if (
            updated.get("title") != link.github_project_title
            or updated.get("closed") is True
            or updated.get("shortDescription") != desired_short_description
            or updated.get("readme") != desired_readme
        ):
            raise ProviderError(
                f"GitHub Project metadata did not verify for {link.portfolio_key}"
            )
    status = (
        "updated"
        if changed and apply
        else ("would_update" if changed else "in_sync")
    )
    return Result(
        link.portfolio_key,
        "github",
        status,
        changed,
        link.github_project_url,
    )


def sync_linear(
    client: LinearClient,
    link: PortfolioLink,
    apply: bool,
) -> Result:
    project = client.project(link.linear_project_id)
    if project.get("id") != link.linear_project_id:
        raise ProviderError(f"Linear returned the wrong project for {link.portfolio_key}")
    if project.get("canceledAt"):
        raise ProviderError(f"Linear project {link.linear_project_name} is canceled")
    if project.get("name") != link.linear_project_name:
        raise ProviderError(
            f"Linear name drift: expected {link.linear_project_name!r}, "
            f"got {project.get('name')!r}"
        )
    if str(project.get("url", "")).rstrip("/") != link.linear_project_url.rstrip("/"):
        raise ProviderError(
            f"Linear URL drift: expected {link.linear_project_url}, "
            f"got {project.get('url')}"
        )

    current = project.get("description")
    current_text = current if isinstance(current, str) else ""
    desired = merge_linear_description(current_text, link)
    normalized_current = current_text.strip() + "\n" if current_text else ""
    changed = desired != normalized_current
    if changed and apply:
        client.update_description(link.linear_project_id, desired)
    status = "updated" if changed and apply else ("would_update" if changed else "in_sync")
    return Result(
        link.portfolio_key,
        "linear",
        status,
        changed,
        link.linear_project_url,
    )


def sync_slack(
    client: SlackClient,
    workspace_id: str,
    link: PortfolioLink,
    apply: bool,
) -> Result:
    if workspace_id != link.slack_workspace_id:
        raise ProviderError(
            f"Slack workspace drift: expected {link.slack_workspace_id}, "
            f"authenticated to {workspace_id}"
        )
    channel = client.channel(link.slack_channel_id)
    if channel.get("id") != link.slack_channel_id:
        raise ProviderError(f"Slack returned the wrong channel for {link.portfolio_key}")
    if channel.get("is_archived") is True:
        raise ProviderError(f"Slack channel #{link.slack_channel_name} is archived")
    if channel.get("name") != link.slack_channel_name:
        raise ProviderError(
            f"Slack name drift: expected #{link.slack_channel_name}, "
            f"got #{channel.get('name')}"
        )

    topic_value = channel.get("topic")
    current_topic = (
        topic_value.get("value", "") if isinstance(topic_value, Mapping) else ""
    )
    desired_topic = merge_compact_marker(
        current_topic,
        slack_marker(link),
        link.portfolio_key,
        limit=250,
    )
    changed = desired_topic != current_topic
    if changed and apply:
        client.set_topic(link.slack_channel_id, desired_topic)
    status = "updated" if changed and apply else ("would_update" if changed else "in_sync")
    return Result(
        link.portfolio_key,
        "slack",
        status,
        changed,
        link.slack_channel_url,
    )


def render_markdown_report(
    mode: str,
    results: list[Result],
    generated_at: str,
    skipped_reason: str = "",
) -> str:
    changed = sum(1 for result in results if result.changed)
    failed = sum(1 for result in results if result.status == "failed")
    lines = [
        "# Portfolio project-link sync",
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
        (
            "",
            "| Portfolio key | Provider | Status | Detail |",
            "|---|---|---|---|",
        )
    )
    for result in results:
        detail = result.detail.replace("|", "\\|").replace("\n", " ")
        lines.append(
            f"| `{result.portfolio_key}` | {result.provider} | "
            f"{result.status} | {detail} |"
        )
    return "\n".join(lines) + "\n"


def write_reports(
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


def parse_now(value: str | None) -> datetime:
    if not value:
        return datetime.now(timezone.utc)
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("--at must include a UTC offset")
    return parsed


def capture_result(
    results: list[Result],
    link: PortfolioLink,
    provider: str,
    operation: Callable[[], Result],
) -> None:
    try:
        results.append(operation())
    except (ProviderError, ValueError, KeyError, TypeError) as exc:
        results.append(
            Result(link.portfolio_key, provider, "failed", False, str(exc))
        )


def run(args: argparse.Namespace) -> int:
    links = load_links(args.registry)
    now = parse_now(args.at)
    generated_at = now.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")
    mode = "apply" if args.apply else "dry-run"

    if args.scheduled_cron and not scheduled_cron_is_active(now, args.scheduled_cron):
        reason = (
            f"{args.scheduled_cron} is not the 03:00 America/Chicago lane for "
            f"{generated_at}"
        )
        write_reports(
            args.json_output,
            args.markdown_output,
            mode,
            [],
            generated_at,
            reason,
        )
        print(reason)
        return 0

    credentials = {
        "github": os.getenv("PROJECT_SYNC_GITHUB_TOKEN", "").strip(),
        "linear": os.getenv("LINEAR_API_KEY", "").strip(),
        "slack": os.getenv("SLACK_BOT_TOKEN", "").strip(),
    }
    missing = sorted(name for name, token in credentials.items() if not token)
    if missing and not args.allow_missing_credentials:
        results = [
            Result(
                "portfolio",
                provider,
                "failed",
                False,
                "missing required protected credential",
            )
            for provider in missing
        ]
        write_reports(
            args.json_output,
            args.markdown_output,
            mode,
            results,
            generated_at,
        )
        print(
            "missing required provider credentials: " + ", ".join(missing),
            file=sys.stderr,
        )
        return 2

    github_client = GitHubClient(credentials["github"]) if credentials["github"] else None
    linear_client = LinearClient(credentials["linear"]) if credentials["linear"] else None
    slack_client = SlackClient(credentials["slack"]) if credentials["slack"] else None
    slack_workspace_id = slack_client.workspace_id() if slack_client else ""

    results: list[Result] = []
    for link in links:
        if github_client:
            capture_result(
                results,
                link,
                "github",
                lambda link=link: sync_github(github_client, link, args.apply),
            )
        else:
            results.append(
                Result(
                    link.portfolio_key,
                    "github",
                    "skipped_missing_credential",
                )
            )

        if linear_client:
            capture_result(
                results,
                link,
                "linear",
                lambda link=link: sync_linear(linear_client, link, args.apply),
            )
        else:
            results.append(
                Result(
                    link.portfolio_key,
                    "linear",
                    "skipped_missing_credential",
                )
            )

        if slack_client:
            capture_result(
                results,
                link,
                "slack",
                lambda link=link: sync_slack(
                    slack_client,
                    slack_workspace_id,
                    link,
                    args.apply,
                ),
            )
        else:
            results.append(
                Result(
                    link.portfolio_key,
                    "slack",
                    "skipped_missing_credential",
                )
            )

        results.append(
            Result(
                link.portfolio_key,
                "chatgpt",
                "registry_only",
                False,
                link.chatgpt_project_name,
            )
        )

    chatgpt_endpoint = os.getenv("CHATGPT_PROJECT_SYNC_ENDPOINT", "").strip()
    if chatgpt_endpoint:
        chatgpt_client = ChatGPTWebhookClient(
            chatgpt_endpoint,
            os.getenv("CHATGPT_PROJECT_SYNC_TOKEN", "").strip() or None,
        )
        try:
            chatgpt_client.reconcile(links, args.apply)
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

    if slack_client and args.summary_channel and (
        failed or changed or args.post_noop_summary
    ):
        try:
            summary_channel_id = slack_client.find_channel_id(
                args.summary_channel.lstrip("#")
            )
            if not summary_channel_id:
                raise ProviderError(
                    f"Slack summary channel #{args.summary_channel.lstrip('#')} is missing"
                )
            slack_client.post_message(
                summary_channel_id,
                (
                    f"Portfolio project-link sync `{mode}` complete: {len(links)} projects, "
                    f"{len(changed)} changed/planned, {len(failed)} failed. "
                    "Canonical schedule: 03:00 America/Chicago."
                ),
            )
        except ProviderError as exc:
            summary_failure = Result(
                "portfolio",
                "slack-summary",
                "failed",
                False,
                str(exc),
            )
            results.append(summary_failure)
            failed.append(summary_failure)

    write_reports(
        args.json_output,
        args.markdown_output,
        mode,
        results,
        generated_at,
    )
    print(
        json.dumps(
            {
                "mode": mode,
                "projects": len(links),
                "results": len(results),
                "changed_or_planned": len(changed),
                "failed": len(failed),
            },
            sort_keys=True,
        )
    )
    return 1 if failed else 0


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    parser.add_argument(
        "--json-output",
        type=Path,
        default=Path("artifacts/portfolio-project-sync.json"),
    )
    parser.add_argument(
        "--markdown-output",
        type=Path,
        default=Path("artifacts/portfolio-project-sync.md"),
    )
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--allow-missing-credentials", action="store_true")
    parser.add_argument("--scheduled-cron")
    parser.add_argument("--at", help="ISO-8601 instant for deterministic tests")
    parser.add_argument("--summary-channel", default="oresoftware")
    parser.add_argument("--post-noop-summary", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    try:
        return run(parse_args(argv))
    except (OSError, ProviderError, ValueError, json.JSONDecodeError) as exc:
        print(f"portfolio project sync failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
