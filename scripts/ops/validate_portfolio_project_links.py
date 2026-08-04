#!/usr/bin/env python3
"""Validate the canonical cross-system portfolio-link registry."""

from __future__ import annotations

import argparse
import csv
import re
import sys
import uuid
from collections import Counter
from pathlib import Path
from urllib.parse import urlparse

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

UNIQUE_COLUMNS = {
    "portfolio_key",
    "chatgpt_project_name",
    "github_project_url",
    "linear_project_id",
    "linear_project_url",
    "slack_channel_id",
    "slack_channel_name",
    "slack_channel_url",
}

KEY_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
SLACK_WORKSPACE_RE = re.compile(r"^T[A-Z0-9]+$")
SLACK_CHANNEL_RE = re.compile(r"^[CG][A-Z0-9]+$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "registry",
        nargs="?",
        type=Path,
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    parser.add_argument(
        "--expected-minimum",
        type=int,
        default=41,
        help="Fail when fewer than this many canonical mappings exist (default: 41).",
    )
    return parser.parse_args()


def duplicate_values(rows: list[dict[str, str]], column: str) -> list[str]:
    counts = Counter(row[column] for row in rows)
    return sorted(value for value, count in counts.items() if count > 1)


def validate(registry: Path, expected_minimum: int) -> list[str]:
    errors: list[str] = []

    if not registry.is_file():
        return [f"registry does not exist: {registry}"]

    with registry.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        actual_columns = tuple(reader.fieldnames or ())
        if actual_columns != REQUIRED_COLUMNS:
            errors.append(
                "CSV columns must exactly match the canonical order:\n"
                f"  expected: {REQUIRED_COLUMNS}\n"
                f"  actual:   {actual_columns}"
            )
        rows = list(reader)

    if len(rows) < expected_minimum:
        errors.append(
            f"registry contains {len(rows)} mappings; expected at least {expected_minimum}"
        )

    for column in UNIQUE_COLUMNS:
        if column not in actual_columns:
            continue
        duplicates = duplicate_values(rows, column)
        if duplicates:
            errors.append(f"duplicate {column} values: {', '.join(duplicates)}")

    for line_number, row in enumerate(rows, start=2):
        missing = [column for column in REQUIRED_COLUMNS if not row.get(column, "").strip()]
        if missing:
            errors.append(f"line {line_number}: blank required fields: {', '.join(missing)}")
            continue

        key = row["portfolio_key"]
        github_org = row["github_org"]

        if not KEY_RE.fullmatch(key):
            errors.append(f"line {line_number}: invalid portfolio_key {key!r}")
        if row["chatgpt_project_name"] != key:
            errors.append(
                f"line {line_number}: chatgpt_project_name must equal portfolio_key"
            )
        if row["slack_channel_name"] != key:
            errors.append(
                f"line {line_number}: slack_channel_name must equal portfolio_key"
            )
        if github_org.lower() != key:
            errors.append(
                f"line {line_number}: lowercased github_org {github_org.lower()!r} "
                f"does not equal portfolio_key {key!r}"
            )

        expected_title = f"{github_org}-project"
        if row["github_project_title"] != expected_title:
            errors.append(
                f"line {line_number}: github_project_title must be {expected_title!r}"
            )

        project_number: int | None = None
        try:
            project_number = int(row["github_project_number"])
            if project_number < 1:
                raise ValueError
        except ValueError:
            errors.append(
                f"line {line_number}: github_project_number must be a positive integer"
            )

        if project_number is not None:
            expected_github_url = (
                f"https://github.com/orgs/{github_org}/projects/{project_number}"
            )
            if row["github_project_url"] != expected_github_url:
                errors.append(
                    f"line {line_number}: github_project_url must be "
                    f"{expected_github_url!r}"
                )

        try:
            uuid.UUID(row["linear_project_id"])
        except ValueError:
            errors.append(f"line {line_number}: invalid Linear UUID")

        linear_url = urlparse(row["linear_project_url"])
        if (
            linear_url.scheme != "https"
            or linear_url.netloc != "linear.app"
            or not linear_url.path.startswith("/denman/project/")
        ):
            errors.append(f"line {line_number}: invalid Linear project URL")

        if not SLACK_WORKSPACE_RE.fullmatch(row["slack_workspace_id"]):
            errors.append(f"line {line_number}: invalid Slack workspace ID")
        if not SLACK_CHANNEL_RE.fullmatch(row["slack_channel_id"]):
            errors.append(f"line {line_number}: invalid Slack channel ID")

        expected_slack_url = (
            "https://oresoftware-workspace.slack.com/archives/"
            f"{row['slack_channel_id']}"
        )
        if row["slack_channel_url"] != expected_slack_url:
            errors.append(
                f"line {line_number}: slack_channel_url must be {expected_slack_url!r}"
            )

    keys = [row["portfolio_key"] for row in rows]
    if keys != sorted(keys):
        errors.append("registry rows must be sorted by portfolio_key")

    return errors


def main() -> int:
    args = parse_args()
    errors = validate(args.registry, args.expected_minimum)
    if errors:
        print("portfolio project-link registry validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    with args.registry.open(newline="", encoding="utf-8") as handle:
        count = sum(1 for _ in csv.DictReader(handle))
    print(f"validated {count} canonical portfolio mappings in {args.registry}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
