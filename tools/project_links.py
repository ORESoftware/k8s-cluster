#!/usr/bin/env python3
"""Validate and render the fleet-wide project-link registry."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path
from typing import Any, Mapping
from zoneinfo import ZoneInfo

SCHEMA_VERSION = 1
SCHEMA_REFERENCE = "./project-links.schema.json"
EXPECTED_PROJECT_COUNT = 41
CENTRAL_TIMEZONE = "America/Chicago"
CENTRAL_LOCAL_TIME = "03:00"
MANAGED_START = "<!-- project-link-sync:start -->"
MANAGED_END = "<!-- project-link-sync:end -->"

LINEAR_NAME_EXCEPTIONS = {
    "streempilot": "github.com/streempilot",
    "memebank": "memebank",
    "meta-agents-demo": "meta-agents-demo",
}

SLACK_CHANNEL_ID = re.compile(r"\bC[A-Z0-9]{8,}\b")
UUID = re.compile(
    r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
    re.IGNORECASE,
)
SECRET = re.compile(
    r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|xox[baprs]-[A-Za-z0-9-]{20,})\b"
)


def load_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def canonical_key(value: str) -> str:
    return value.lower()


def github_project_url(entry: Mapping[str, Any]) -> str:
    github = _mapping(entry.get("github"))
    return (
        f"https://github.com/orgs/{github.get('organization')}/projects/"
        f"{github.get('project_number')}"
    )


def expected_linear_name(org: str) -> str:
    key = canonical_key(org)
    return LINEAR_NAME_EXCEPTIONS.get(key, f"github.com/{org}")


def validate_catalog(value: Any) -> list[str]:
    errors: list[str] = []
    catalog = _mapping(value)
    if not catalog:
        return ["project-link catalog must be a JSON object"]

    if catalog.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"schema_version must be {SCHEMA_VERSION}")
    if catalog.get("$schema") != SCHEMA_REFERENCE:
        errors.append(f"$schema must be {SCHEMA_REFERENCE}")

    schedule = _mapping(catalog.get("schedule"))
    if schedule.get("timezone") != CENTRAL_TIMEZONE:
        errors.append(f"schedule.timezone must be {CENTRAL_TIMEZONE}")
    if schedule.get("local_time") != CENTRAL_LOCAL_TIME:
        errors.append(f"schedule.local_time must be {CENTRAL_LOCAL_TIME}")

    projects = catalog.get("projects")
    if not isinstance(projects, list) or not projects:
        errors.append("projects must be a non-empty array")
        return errors
    if len(projects) != EXPECTED_PROJECT_COUNT:
        errors.append(
            f"projects must contain the verified {EXPECTED_PROJECT_COUNT}-organization fleet; "
            f"found {len(projects)}"
        )

    for index, entry in enumerate(projects):
        errors.extend(_validate_entry(entry, f"projects[{index}]"))

    duplicate_fields = {
        "key": lambda entry: _mapping(entry).get("key"),
        "github.organization": lambda entry: _mapping(
            _mapping(entry).get("github")
        ).get("organization", "").lower(),
        "github.project_title": lambda entry: _mapping(
            _mapping(entry).get("github")
        ).get("project_title", "").lower(),
        "linear.project_name": lambda entry: _mapping(
            _mapping(entry).get("linear")
        ).get("project_name", "").lower(),
        "chatgpt.project_name": lambda entry: _mapping(
            _mapping(entry).get("chatgpt")
        ).get("project_name", "").lower(),
        "slack.channel_name": lambda entry: _mapping(
            _mapping(entry).get("slack")
        ).get("channel_name", "").lower(),
    }
    for field, extractor in duplicate_fields.items():
        values = [extractor(entry) for entry in projects]
        for duplicate, count in Counter(value for value in values if value).items():
            if count > 1:
                errors.append(f"duplicate {field}: {duplicate}")

    sorted_keys = sorted(
        _mapping(entry).get("key")
        for entry in projects
        if isinstance(_mapping(entry).get("key"), str)
    )
    actual_keys = [
        _mapping(entry).get("key")
        for entry in projects
        if isinstance(_mapping(entry).get("key"), str)
    ]
    if actual_keys != sorted_keys:
        errors.append("projects must be sorted by canonical key")

    return errors


def _validate_entry(value: Any, identity: str) -> list[str]:
    errors: list[str] = []
    entry = _mapping(value)
    if not entry:
        return [f"{identity} must be a JSON object"]

    key = entry.get("key")
    if not isinstance(key, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]*", key):
        errors.append(f"{identity}.key must be a lowercase slug")
        return errors

    github = _mapping(entry.get("github"))
    org = github.get("organization")
    title = github.get("project_title")
    number = github.get("project_number")
    if not isinstance(org, str) or not re.fullmatch(
        r"[A-Za-z0-9][A-Za-z0-9-]*", org
    ):
        errors.append(f"{identity}.github.organization is invalid")
        org = None
    if org and key != canonical_key(org):
        errors.append(
            f"{identity}.key must be the lowercase GitHub organization login "
            f"({canonical_key(org)})"
        )
    if org and title != f"{org}-project":
        errors.append(
            f"{identity}.github.project_title must be exactly {org}-project"
        )
    if not isinstance(number, int) or isinstance(number, bool) or number < 1:
        errors.append(f"{identity}.github.project_number must be a positive integer")
    elif org:
        expected_number = 4 if canonical_key(org) == "dancing-dragons" else 1
        if number != expected_number:
            errors.append(
                f"{identity}.github.project_number must be {expected_number} for {org}"
            )

    linear = _mapping(entry.get("linear"))
    linear_name = linear.get("project_name")
    if not isinstance(linear_name, str) or not linear_name:
        errors.append(f"{identity}.linear.project_name must be non-empty")
    elif org and linear_name != expected_linear_name(org):
        errors.append(
            f"{identity}.linear.project_name must be {expected_linear_name(org)}"
        )

    chatgpt = _mapping(entry.get("chatgpt"))
    chatgpt_name = chatgpt.get("project_name")
    if chatgpt_name != key:
        errors.append(f"{identity}.chatgpt.project_name must be {key}")

    slack = _mapping(entry.get("slack"))
    slack_name = slack.get("channel_name")
    if slack_name != f"#{key}":
        errors.append(f"{identity}.slack.channel_name must be #{key}")

    return errors


def find_public_boundary_violations(raw: str) -> list[str]:
    violations: list[str] = []
    for match in sorted(set(SLACK_CHANNEL_ID.findall(raw))):
        violations.append(
            f"immutable Slack channel ID {match} must not appear in a public catalog"
        )
    for match in sorted(set(UUID.findall(raw))):
        violations.append(f"provider UUID {match} must not appear in a public catalog")
    for match in sorted(set(SECRET.findall(raw))):
        violations.append(
            f"credential beginning {match[:8]}… must not appear in a public catalog"
        )
    return violations


def scheduled_cron_is_active(
    now: datetime,
    cron_expression: str,
    timezone: str = CENTRAL_TIMEZONE,
    local_hour: int = 3,
) -> bool:
    """Return whether a dual-UTC cron expression is today's Central 03:00 lane.

    GitHub Actions cron is UTC. The workflow schedules both 08:00 and 09:00 UTC;
    this guard selects the one corresponding to 03:00 in America/Chicago for the
    current date, including daylight-saving transitions. It keys off the event's
    scheduled expression, so ordinary runner delay cannot cause the job to skip.
    """

    fields = cron_expression.split()
    if len(fields) != 5 or fields[0] != "0":
        raise ValueError(f"unsupported scheduled cron expression: {cron_expression}")
    try:
        scheduled_utc_hour = int(fields[1])
    except ValueError as exc:
        raise ValueError(
            f"unsupported scheduled cron expression: {cron_expression}"
        ) from exc

    central = now.astimezone(ZoneInfo(timezone))
    offset = central.utcoffset()
    if offset is None:
        raise ValueError(f"timezone {timezone} has no UTC offset")
    offset_hours = int(offset.total_seconds() // 3600)
    expected_utc_hour = (local_hour - offset_hours) % 24
    return scheduled_utc_hour == expected_utc_hour


def managed_block(entry: Mapping[str, Any]) -> str:
    github = _mapping(entry.get("github"))
    linear = _mapping(entry.get("linear"))
    chatgpt = _mapping(entry.get("chatgpt"))
    slack = _mapping(entry.get("slack"))
    return "\n".join(
        [
            MANAGED_START,
            "## Canonical project links",
            "",
            f"- Canonical key: `{entry.get('key')}`",
            (
                "- GitHub organization: "
                f"[github.com/{github.get('organization')}]"
                f"(https://github.com/{github.get('organization')})"
            ),
            (
                f"- GitHub Project: [{github.get('project_title')}]"
                f"({github_project_url(entry)})"
            ),
            f"- Linear project: `{linear.get('project_name')}`",
            f"- ChatGPT project: `{chatgpt.get('project_name')}`",
            f"- Slack channel: `{slack.get('channel_name')}`",
            MANAGED_END,
        ]
    )


def merge_managed_block(description: str | None, entry: Mapping[str, Any]) -> str:
    current = (description or "").strip()
    replacement = managed_block(entry)
    pattern = re.compile(
        re.escape(MANAGED_START) + r".*?" + re.escape(MANAGED_END), re.DOTALL
    )
    if pattern.search(current):
        merged = pattern.sub(replacement, current, count=1)
    elif current:
        merged = current + "\n\n" + replacement
    else:
        merged = replacement
    return merged.strip() + "\n"


def compact_marker(entry: Mapping[str, Any]) -> str:
    github = _mapping(entry.get("github"))
    linear = _mapping(entry.get("linear"))
    chatgpt = _mapping(entry.get("chatgpt"))
    slack = _mapping(entry.get("slack"))
    return (
        f"[sync:{entry.get('key')}] GitHub {github.get('project_title')} | "
        f"Linear {linear.get('project_name')} | ChatGPT {chatgpt.get('project_name')} | "
        f"Slack {slack.get('channel_name')}"
    )


def merge_compact_marker(current: str | None, entry: Mapping[str, Any], limit: int) -> str:
    marker = compact_marker(entry)
    key = re.escape(str(entry.get("key")))
    pattern = re.compile(rf"(?:\s*\|\s*)?\[sync:{key}\][^\[]*$")
    base = pattern.sub("", (current or "").strip()).strip(" |")
    merged = f"{base} | {marker}" if base else marker
    if len(merged) > limit:
        if base:
            raise ValueError(
                f"existing provider description leaves no room for managed marker "
                f"({len(merged)} > {limit})"
            )
        raise ValueError(f"managed marker exceeds provider limit ({len(merged)} > {limit})")
    return merged


def render_summary(catalog: Mapping[str, Any]) -> str:
    projects = [_mapping(entry) for entry in catalog.get("projects", [])]
    lines = [
        "# Canonical project links",
        "",
        f"- projects: {len(projects)}",
        f"- schedule: `{CENTRAL_LOCAL_TIME} {CENTRAL_TIMEZONE}`",
        "",
        "| Key | GitHub Project | Linear project | ChatGPT project | Slack channel |",
        "|---|---|---|---|---|",
    ]
    for entry in projects:
        github = _mapping(entry.get("github"))
        linear = _mapping(entry.get("linear"))
        chatgpt = _mapping(entry.get("chatgpt"))
        slack = _mapping(entry.get("slack"))
        lines.append(
            f"| `{entry.get('key')}` | "
            f"[{github.get('project_title')}]({github_project_url(entry)}) | "
            f"`{linear.get('project_name')}` | "
            f"`{chatgpt.get('project_name')}` | "
            f"`{slack.get('channel_name')}` |"
        )
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate", help="validate the registry")
    validate.add_argument("catalog", type=Path)

    report = subparsers.add_parser("report", help="render a Markdown routing table")
    report.add_argument("catalog", type=Path)
    report.add_argument("--output", type=Path)

    args = parser.parse_args(argv)
    try:
        catalog = load_json(args.catalog)
        errors = validate_catalog(catalog)
        errors.extend(
            find_public_boundary_violations(args.catalog.read_text(encoding="utf-8"))
        )
        if errors:
            for error in errors:
                print(error, file=sys.stderr)
            return 1

        if args.command == "validate":
            print(f"validated {len(catalog['projects'])} canonical project links")
            return 0

        text = render_summary(catalog)
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(text, encoding="utf-8")
        else:
            print(text, end="")
        return 0
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"project-link command failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
