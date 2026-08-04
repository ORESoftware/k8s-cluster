#!/usr/bin/env python3
"""Shared contract helpers for cross-system portfolio project links."""

from __future__ import annotations

import csv
import re
import uuid
from collections import Counter
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Mapping
from urllib.parse import urlparse
from zoneinfo import ZoneInfo

REQUIRED_COLUMNS = (
    "portfolio_key",
    "chatgpt_project_name",
    "github_org",
    "github_project_number",
    "github_project_title",
    "github_project_url",
    "linear_project_id",
    "linear_project_name",
    "linear_project_url",
    "slack_workspace_id",
    "slack_channel_id",
    "slack_channel_name",
    "slack_channel_url",
)

EXPECTED_KEYS = {
    "3fa-app",
    "agent-pontifex",
    "akrion-sim",
    "anticaptrad",
    "athlet-o",
    "benefactor-cc",
    "canonical-cloud",
    "channelsiege",
    "claritas-viz",
    "cliptown",
    "daedalus-fab",
    "dancing-dragons",
    "declarative-migrations",
    "discrete-event-systems",
    "drone-mngr",
    "fanwaave",
    "fiducia-cloud",
    "fifa-math",
    "file-tunnel",
    "gha-indie-worker",
    "hypeblitz",
    "hypesiege",
    "memebank",
    "messaging-intel",
    "meta-agents-demo",
    "networking-components",
    "omniblitz",
    "opto-sync",
    "quaestor-ledger",
    "rust-ssr-demos",
    "sagitta-stack",
    "scintilla-run",
    "shared-auth",
    "sonus-auris",
    "streamkore",
    "streempilot",
    "unreal-unity-poc",
    "usa-acc",
    "voxletra",
    "zed-pkg",
    "zed-pkg-test",
}
EXPECTED_COUNT = len(EXPECTED_KEYS)
EXPECTED_SLACK_WORKSPACE_ID = "T01B3C83PMK"
CENTRAL_TIMEZONE = "America/Chicago"
CENTRAL_LOCAL_HOUR = 3
CENTRAL_LOCAL_TIME = "03:00"
MANAGED_START = "<!-- portfolio-project-sync:start -->"
MANAGED_END = "<!-- portfolio-project-sync:end -->"
REGISTRY_SOURCE_URL = (
    "https://github.com/ORESoftware/k8s-cluster/blob/main/"
    "ops/registries/portfolio-project-links.csv"
)

KEY_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
GITHUB_ORG_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$")
SLACK_CHANNEL_RE = re.compile(r"^C[A-Z0-9]{8,}$")
CREDENTIAL_RE = re.compile(
    r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|"
    r"xox[baprs]-[A-Za-z0-9-]{20,})\b"
)


@dataclass(frozen=True)
class PortfolioLink:
    portfolio_key: str
    chatgpt_project_name: str
    github_org: str
    github_project_number: int
    github_project_title: str
    github_project_url: str
    linear_project_id: str
    linear_project_name: str
    linear_project_url: str
    slack_workspace_id: str
    slack_channel_id: str
    slack_channel_name: str
    slack_channel_url: str

    @classmethod
    def from_row(cls, row: Mapping[str, str]) -> "PortfolioLink":
        return cls(
            portfolio_key=row["portfolio_key"].strip(),
            chatgpt_project_name=row["chatgpt_project_name"].strip(),
            github_org=row["github_org"].strip(),
            github_project_number=int(row["github_project_number"].strip()),
            github_project_title=row["github_project_title"].strip(),
            github_project_url=row["github_project_url"].strip(),
            linear_project_id=row["linear_project_id"].strip(),
            linear_project_name=row["linear_project_name"].strip(),
            linear_project_url=row["linear_project_url"].strip(),
            slack_workspace_id=row["slack_workspace_id"].strip(),
            slack_channel_id=row["slack_channel_id"].strip(),
            slack_channel_name=row["slack_channel_name"].strip(),
            slack_channel_url=row["slack_channel_url"].strip(),
        )

    def routing_payload(self) -> dict[str, Any]:
        """Return the non-secret routing payload used by the ChatGPT bridge."""

        return {
            "portfolio_key": self.portfolio_key,
            "chatgpt_project_name": self.chatgpt_project_name,
            "github": {
                "organization": self.github_org,
                "project_number": self.github_project_number,
                "project_title": self.github_project_title,
                "project_url": self.github_project_url,
            },
            "linear": {
                "project_id": self.linear_project_id,
                "project_name": self.linear_project_name,
                "project_url": self.linear_project_url,
            },
            "slack": {
                "workspace_id": self.slack_workspace_id,
                "channel_id": self.slack_channel_id,
                "channel_name": self.slack_channel_name,
                "channel_url": self.slack_channel_url,
            },
        }


def read_registry(path: Path) -> tuple[tuple[str, ...], list[dict[str, str]]]:
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        columns = tuple(reader.fieldnames or ())
        rows = [dict(row) for row in reader]
    return columns, rows


def duplicate_values(rows: list[dict[str, str]], column: str) -> list[str]:
    counts = Counter((row.get(column) or "").strip() for row in rows)
    return sorted(value for value, count in counts.items() if value and count > 1)


def _valid_https_url(value: str, host: str, path_prefix: str) -> bool:
    parsed = urlparse(value)
    return (
        parsed.scheme == "https"
        and parsed.netloc == host
        and parsed.path.startswith(path_prefix)
        and not parsed.params
        and not parsed.query
        and not parsed.fragment
    )


def _validate_row(row: dict[str, str], line_number: int) -> list[str]:
    errors: list[str] = []
    missing = [
        column for column in REQUIRED_COLUMNS if not (row.get(column) or "").strip()
    ]
    if missing:
        return [f"line {line_number}: blank required fields: {', '.join(missing)}"]

    key = row["portfolio_key"].strip()
    github_org = row["github_org"].strip()
    project_number_raw = row["github_project_number"].strip()

    if not KEY_RE.fullmatch(key):
        errors.append(f"line {line_number}: invalid portfolio_key {key!r}")
    if row["chatgpt_project_name"].strip() != key:
        errors.append(f"line {line_number}: chatgpt_project_name must equal portfolio_key")
    if row["slack_channel_name"].strip() != key:
        errors.append(f"line {line_number}: slack_channel_name must equal portfolio_key")
    if not GITHUB_ORG_RE.fullmatch(github_org):
        errors.append(f"line {line_number}: invalid github_org {github_org!r}")
    if github_org.lower() != key:
        errors.append(
            f"line {line_number}: lowercased github_org {github_org.lower()!r} "
            f"does not equal portfolio_key {key!r}"
        )

    expected_title = f"{github_org}-project"
    if row["github_project_title"].strip() != expected_title:
        errors.append(
            f"line {line_number}: github_project_title must be {expected_title!r}"
        )

    project_number: int | None = None
    try:
        project_number = int(project_number_raw)
        if project_number < 1:
            raise ValueError
    except ValueError:
        errors.append(
            f"line {line_number}: github_project_number must be a positive integer"
        )

    if project_number is not None:
        expected_number = 4 if key == "dancing-dragons" else 1
        if project_number != expected_number:
            errors.append(
                f"line {line_number}: github_project_number must be {expected_number} "
                f"for {key}"
            )
        expected_github_url = (
            f"https://github.com/orgs/{github_org}/projects/{project_number}"
        )
        if row["github_project_url"].strip() != expected_github_url:
            errors.append(
                f"line {line_number}: github_project_url must be "
                f"{expected_github_url!r}"
            )

    allowed_linear_names = {
        key,
        f"github.com/{key}",
        f"github.com/{github_org}",
    }
    linear_name = row["linear_project_name"].strip()
    if linear_name not in allowed_linear_names:
        errors.append(
            f"line {line_number}: linear_project_name {linear_name!r} "
            "is not an accepted canonical alias"
        )

    try:
        uuid.UUID(row["linear_project_id"].strip())
    except ValueError:
        errors.append(f"line {line_number}: invalid Linear UUID")

    if not _valid_https_url(
        row["linear_project_url"].strip(), "linear.app", "/denman/project/"
    ):
        errors.append(f"line {line_number}: invalid Linear project URL")

    workspace_id = row["slack_workspace_id"].strip()
    channel_id = row["slack_channel_id"].strip()
    if workspace_id != EXPECTED_SLACK_WORKSPACE_ID:
        errors.append(
            f"line {line_number}: slack_workspace_id must be "
            f"{EXPECTED_SLACK_WORKSPACE_ID}"
        )
    if not SLACK_CHANNEL_RE.fullmatch(channel_id):
        errors.append(f"line {line_number}: invalid public Slack channel ID")

    expected_slack_url = (
        f"https://oresoftware-workspace.slack.com/archives/{channel_id}"
    )
    if row["slack_channel_url"].strip() != expected_slack_url:
        errors.append(
            f"line {line_number}: slack_channel_url must be {expected_slack_url!r}"
        )

    return errors


def validate_registry(path: Path) -> list[str]:
    errors: list[str] = []
    if not path.is_file():
        return [f"registry does not exist: {path}"]

    raw = path.read_text(encoding="utf-8")
    credential = CREDENTIAL_RE.search(raw)
    if credential:
        errors.append(
            f"credential-shaped value beginning {credential.group(0)[:8]}… "
            "must not appear in the registry"
        )

    try:
        columns, rows = read_registry(path)
    except (OSError, csv.Error) as exc:
        return [f"cannot read registry {path}: {exc}"]

    if columns != REQUIRED_COLUMNS:
        errors.append(
            "CSV columns must exactly match the canonical order:\n"
            f"  expected: {REQUIRED_COLUMNS}\n"
            f"  actual:   {columns}"
        )

    if len(rows) != EXPECTED_COUNT:
        errors.append(
            f"registry contains {len(rows)} mappings; expected exactly "
            f"{EXPECTED_COUNT}"
        )

    if columns == REQUIRED_COLUMNS:
        unique_columns = (
            "portfolio_key",
            "chatgpt_project_name",
            "github_org",
            "github_project_title",
            "github_project_url",
            "linear_project_id",
            "linear_project_url",
            "slack_channel_id",
            "slack_channel_name",
            "slack_channel_url",
        )
        for column in unique_columns:
            duplicates = duplicate_values(rows, column)
            if duplicates:
                errors.append(f"duplicate {column} values: {', '.join(duplicates)}")

        for line_number, row in enumerate(rows, start=2):
            errors.extend(_validate_row(row, line_number))

        keys = [row["portfolio_key"].strip() for row in rows]
        actual_keys = set(keys)
        missing_keys = sorted(EXPECTED_KEYS - actual_keys)
        unexpected_keys = sorted(actual_keys - EXPECTED_KEYS)
        if missing_keys:
            errors.append("missing portfolio keys: " + ", ".join(missing_keys))
        if unexpected_keys:
            errors.append("unexpected portfolio keys: " + ", ".join(unexpected_keys))
        if keys != sorted(keys):
            errors.append("registry rows must be sorted by portfolio_key")

    return errors


def load_links(path: Path) -> list[PortfolioLink]:
    errors = validate_registry(path)
    if errors:
        raise ValueError("; ".join(errors))
    _, rows = read_registry(path)
    return [PortfolioLink.from_row(row) for row in rows]


def scheduled_cron_is_active(
    now: datetime,
    cron_expression: str,
    timezone: str = CENTRAL_TIMEZONE,
    local_hour: int = CENTRAL_LOCAL_HOUR,
) -> bool:
    """Return whether a dual-UTC cron expression is today's Central 03:00 lane."""

    fields = cron_expression.split()
    if len(fields) != 5 or fields[0] != "0":
        raise ValueError(f"unsupported scheduled cron expression: {cron_expression}")
    try:
        scheduled_utc_hour = int(fields[1])
    except ValueError as exc:
        raise ValueError(
            f"unsupported scheduled cron expression: {cron_expression}"
        ) from exc

    local = now.astimezone(ZoneInfo(timezone))
    offset = local.utcoffset()
    if offset is None:
        raise ValueError(f"timezone {timezone} has no UTC offset")
    offset_hours = int(offset.total_seconds() // 3600)
    expected_utc_hour = (local_hour - offset_hours) % 24
    return scheduled_utc_hour == expected_utc_hour


def linear_managed_block(link: PortfolioLink) -> str:
    return "\n".join(
        (
            MANAGED_START,
            "## Canonical portfolio links",
            "",
            f"- Portfolio key: `{link.portfolio_key}`",
            (
                f"- GitHub Project: [{link.github_project_title}]"
                f"({link.github_project_url})"
            ),
            f"- ChatGPT project: `{link.chatgpt_project_name}`",
            (
                f"- Slack channel: [#{link.slack_channel_name}]"
                f"({link.slack_channel_url})"
            ),
            f"- Marker: `portfolio-link-registry:v1:{link.portfolio_key}`",
            MANAGED_END,
        )
    )


def merge_linear_description(current: str | None, link: PortfolioLink) -> str:
    existing = (current or "").strip()
    replacement = linear_managed_block(link)
    pattern = re.compile(
        re.escape(MANAGED_START) + r".*?" + re.escape(MANAGED_END), re.DOTALL
    )
    if pattern.search(existing):
        merged = pattern.sub(replacement, existing, count=1)
    elif existing:
        merged = existing + "\n\n" + replacement
    else:
        merged = replacement
    return merged.strip() + "\n"


def github_short_description(link: PortfolioLink) -> str:
    return (
        f"key={link.portfolio_key} · "
        f"Linear {link.linear_project_name} · "
        f"Slack #{link.slack_channel_name}"
    )


def github_project_readme(link: PortfolioLink) -> str:
    rows = (
        f"# {link.github_project_title}",
        "",
        f"Canonical cross-system project linkage for `{link.portfolio_key}`.",
        "",
        "| System | Canonical reference |",
        "| --- | --- |",
        f"| Portfolio key | `portfolio_key={link.portfolio_key}` |",
        f"| ChatGPT Project | `{link.chatgpt_project_name}` |",
        (
            f"| GitHub organization | [{link.github_org}]"
            f"(https://github.com/{link.github_org}) |"
        ),
        (
            f"| GitHub Project | [{link.github_project_title}]"
            f"({link.github_project_url}) |"
        ),
        (
            f"| Linear Project | [{link.linear_project_name}]"
            f"({link.linear_project_url}) (`{link.linear_project_id}`) |"
        ),
        (
            f"| Slack channel | [#{link.slack_channel_name}]"
            f"({link.slack_channel_url}) (`{link.slack_channel_id}`) |"
        ),
        "",
        (
            "Source of truth: [portfolio-project-links.csv]"
            f"({REGISTRY_SOURCE_URL})"
        ),
        "",
        f"Marker: `portfolio-link-registry:v1:{link.portfolio_key}`",
    )
    return "\n".join(rows) + "\n"


def slack_marker(link: PortfolioLink) -> str:
    return (
        f"portfolio-link-registry:v1:{link.portfolio_key} · "
        f"GitHub {link.github_project_title} · "
        f"Linear {link.linear_project_name} · "
        f"ChatGPT {link.chatgpt_project_name}"
    )


def merge_compact_marker(
    current: str | None,
    marker: str,
    portfolio_key: str,
    limit: int,
) -> str:
    key = re.escape(portfolio_key)
    pattern = re.compile(
        rf"(?:\s*(?:\||·)\s*)?"
        rf"(?:\[portfolio:{key}\]|portfolio-link-registry:v1:{key}).*$"
    )
    human = pattern.sub("", (current or "").strip()).strip(" |·")
    merged = f"{human} · {marker}" if human else marker
    if len(merged) > limit:
        raise ValueError(
            f"managed marker exceeds provider limit ({len(merged)} > {limit}); "
            "shorten the existing human-authored text"
        )
    return merged
