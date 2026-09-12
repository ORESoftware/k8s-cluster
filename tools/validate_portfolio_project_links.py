#!/usr/bin/env python3
"""Validate the canonical ChatGPT/GitHub/Linear/Slack portfolio registry."""

from __future__ import annotations

import argparse
import csv
import re
import sys
import uuid
from collections import Counter
from pathlib import Path
from typing import Iterable

EXPECTED_HEADERS = (
    "portfolio_key",
    "chatgpt_project_name",
    "github_org",
    "github_project_number",
    "github_project_title",
    "linear_project_id",
    "linear_project_name",
    "slack_workspace_id",
    "slack_channel_id",
    "slack_channel_name",
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
KEY = re.compile(r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
SLACK_WORKSPACE_ID = "T01B3C83PMK"
SLACK_CHANNEL_ID = re.compile(r"^C[A-Z0-9]{8,}$")
GITHUB_CREDENTIAL = re.compile(r"(?:ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})")


def duplicates(values: Iterable[str]) -> list[str]:
    return sorted(value for value, count in Counter(values).items() if count > 1)


def validate_row(row: dict[str, str], line_number: int) -> list[str]:
    errors: list[str] = []
    identity = f"line {line_number}"
    missing = [header for header in EXPECTED_HEADERS if not row.get(header)]
    if missing:
        errors.append(f"{identity}: blank field(s): {', '.join(missing)}")
        return errors

    key = row["portfolio_key"]
    owner = row["github_org"]
    if not KEY.fullmatch(key):
        errors.append(f"{identity}: invalid portfolio_key {key!r}")
    if owner.lower() != key:
        errors.append(
            f"{identity}: portfolio_key {key!r} must equal lowercased GitHub org {owner.lower()!r}"
        )
    if row["chatgpt_project_name"] != key:
        errors.append(f"{identity}: ChatGPT project name must equal portfolio_key")
    if row["slack_channel_name"] != key:
        errors.append(f"{identity}: Slack channel name must equal portfolio_key")

    expected_title = f"{owner}-project"
    if row["github_project_title"] != expected_title:
        errors.append(
            f"{identity}: GitHub Project title must be {expected_title!r}"
        )

    try:
        project_number = int(row["github_project_number"])
    except ValueError:
        errors.append(f"{identity}: GitHub Project number must be an integer")
    else:
        expected_number = 4 if key == "dancing-dragons" else 1
        if project_number != expected_number:
            errors.append(
                f"{identity}: GitHub Project number must be {expected_number}"
            )

    try:
        uuid.UUID(row["linear_project_id"])
    except ValueError:
        errors.append(f"{identity}: invalid Linear project UUID")

    allowed_linear_names = {
        key,
        f"github.com/{key}",
        f"github.com/{owner}",
    }
    if row["linear_project_name"] not in allowed_linear_names:
        errors.append(
            f"{identity}: Linear project name {row['linear_project_name']!r} is not a canonical alias"
        )

    if row["slack_workspace_id"] != SLACK_WORKSPACE_ID:
        errors.append(
            f"{identity}: Slack workspace must be {SLACK_WORKSPACE_ID}"
        )
    if not SLACK_CHANNEL_ID.fullmatch(row["slack_channel_id"]):
        errors.append(f"{identity}: invalid Slack channel ID")

    return errors


def validate(path: Path) -> list[str]:
    errors: list[str] = []
    raw = path.read_text(encoding="utf-8")
    if GITHUB_CREDENTIAL.search(raw):
        errors.append("registry contains a GitHub credential-like value")

    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        headers = tuple(reader.fieldnames or ())
        if headers != EXPECTED_HEADERS:
            errors.append(
                "header mismatch: expected "
                + ",".join(EXPECTED_HEADERS)
                + "; got "
                + ",".join(headers)
            )
            return errors
        rows = list(reader)

    if len(rows) != len(EXPECTED_KEYS):
        errors.append(
            f"expected {len(EXPECTED_KEYS)} rows, found {len(rows)}"
        )

    for line_number, row in enumerate(rows, start=2):
        errors.extend(validate_row(row, line_number))

    keys = [row["portfolio_key"] for row in rows]
    actual_keys = set(keys)
    missing = sorted(EXPECTED_KEYS - actual_keys)
    unexpected = sorted(actual_keys - EXPECTED_KEYS)
    if missing:
        errors.append("missing portfolio key(s): " + ", ".join(missing))
    if unexpected:
        errors.append("unexpected portfolio key(s): " + ", ".join(unexpected))
    if keys != sorted(keys):
        errors.append("rows must be sorted by portfolio_key")

    unique_columns = (
        "portfolio_key",
        "linear_project_id",
        "slack_channel_id",
        "github_project_title",
    )
    for column in unique_columns:
        for value in duplicates(row[column] for row in rows):
            errors.append(f"duplicate {column}: {value}")

    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "registry",
        type=Path,
        nargs="?",
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    args = parser.parse_args(argv)

    try:
        errors = validate(args.registry)
    except (OSError, csv.Error) as exc:
        print(f"portfolio registry validation failed: {exc}", file=sys.stderr)
        return 2

    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    print(f"validated {len(EXPECTED_KEYS)} canonical portfolio project links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
