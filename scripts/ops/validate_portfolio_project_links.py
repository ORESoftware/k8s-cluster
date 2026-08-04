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

EXPECTED_KEYS = frozenset(
    {
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
)

UNIQUE_COLUMNS = (
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

EXPECTED_SLACK_WORKSPACE_ID = "T01B3C83PMK"
KEY_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
SLACK_CHANNEL_RE = re.compile(r"^C[A-Z0-9]{8,}$")
LINEAR_SLUG_RE = re.compile(
    r"^(?P<name>[a-z0-9-]+)-(?P<identifier>[0-9a-f]{12})$"
)
GITHUB_CREDENTIAL_RE = re.compile(
    r"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})"
)


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
        default=len(EXPECTED_KEYS),
        help=(
            "Additional mapping-count floor; the canonical 41-key inventory is "
            "always enforced."
        ),
    )
    return parser.parse_args()


def duplicate_values(rows: list[dict[str, str]], column: str) -> list[str]:
    counts = Counter(row.get(column, "") for row in rows)
    return sorted(
        value for value, count in counts.items() if value and count > 1
    )


def canonical_linear_slug_name(linear_project_name: str) -> str:
    return re.sub(r"[^a-z0-9-]+", "", linear_project_name.lower())


def validate(registry: Path, expected_minimum: int) -> list[str]:
    errors: list[str] = []

    if not registry.is_file():
        return [f"registry does not exist: {registry}"]
    if expected_minimum < 0:
        return ["--expected-minimum must be non-negative"]

    try:
        raw_registry = registry.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"cannot read registry {registry}: {exc}"]

    if GITHUB_CREDENTIAL_RE.search(raw_registry):
        errors.append("registry contains a GitHub credential-like value")

    try:
        with registry.open(newline="", encoding="utf-8") as handle:
            reader = csv.DictReader(handle)
            actual_columns = tuple(reader.fieldnames or ())
            rows = [
                {
                    column: (row.get(column) or "").strip()
                    for column in REQUIRED_COLUMNS
                }
                for row in reader
            ]
    except (OSError, csv.Error) as exc:
        return errors + [f"cannot parse registry {registry}: {exc}"]

    if actual_columns != REQUIRED_COLUMNS:
        errors.append(
            "CSV columns must exactly match the canonical order:\n"
            f"  expected: {REQUIRED_COLUMNS}\n"
            f"  actual:   {actual_columns}"
        )

    expected_count = len(EXPECTED_KEYS)
    if len(rows) != expected_count:
        errors.append(
            f"registry contains {len(rows)} mappings; expected exactly "
            f"{expected_count}"
        )
    if len(rows) < expected_minimum:
        errors.append(
            f"registry contains {len(rows)} mappings; expected at least "
            f"{expected_minimum}"
        )

    for column in UNIQUE_COLUMNS:
        duplicates = duplicate_values(rows, column)
        if duplicates:
            errors.append(f"duplicate {column} values: {', '.join(duplicates)}")

    for line_number, row in enumerate(rows, start=2):
        missing = [column for column in REQUIRED_COLUMNS if not row[column]]
        if missing:
            errors.append(
                f"line {line_number}: blank required fields: "
                f"{', '.join(missing)}"
            )
            continue

        key = row["portfolio_key"]
        github_org = row["github_org"]

        if not KEY_RE.fullmatch(key):
            errors.append(f"line {line_number}: invalid portfolio_key {key!r}")
        if row["chatgpt_project_name"] != key:
            errors.append(
                f"line {line_number}: chatgpt_project_name must equal "
                "portfolio_key"
            )
        if row["slack_channel_name"] != key:
            errors.append(
                f"line {line_number}: slack_channel_name must equal "
                "portfolio_key"
            )
        if github_org.lower() != key:
            errors.append(
                f"line {line_number}: lowercased github_org "
                f"{github_org.lower()!r} does not equal portfolio_key {key!r}"
            )

        expected_title = f"{github_org}-project"
        if row["github_project_title"] != expected_title:
            errors.append(
                f"line {line_number}: github_project_title must be "
                f"{expected_title!r}"
            )

        expected_project_number = 4 if key == "dancing-dragons" else 1
        try:
            project_number = int(row["github_project_number"])
        except ValueError:
            errors.append(
                f"line {line_number}: github_project_number must be an integer"
            )
        else:
            if project_number != expected_project_number:
                errors.append(
                    f"line {line_number}: github_project_number must be "
                    f"{expected_project_number} for {key!r}"
                )

            expected_github_url = (
                f"https://github.com/orgs/{github_org}/projects/"
                f"{expected_project_number}"
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

        allowed_linear_names = {
            key,
            f"github.com/{key}",
            f"github.com/{github_org}",
        }
        if row["linear_project_name"] not in allowed_linear_names:
            errors.append(
                f"line {line_number}: linear_project_name "
                f"{row['linear_project_name']!r} is not a canonical alias"
            )

        linear_url = urlparse(row["linear_project_url"])
        linear_slug = linear_url.path.removeprefix("/denman/project/")
        linear_slug_match = LINEAR_SLUG_RE.fullmatch(linear_slug)
        if (
            linear_url.scheme != "https"
            or linear_url.netloc != "linear.app"
            or not linear_url.path.startswith("/denman/project/")
            or linear_url.params
            or linear_url.query
            or linear_url.fragment
            or linear_slug_match is None
        ):
            errors.append(f"line {line_number}: invalid Linear project URL")
        elif linear_slug_match.group("name") != canonical_linear_slug_name(
            row["linear_project_name"]
        ):
            errors.append(
                f"line {line_number}: Linear URL slug does not match "
                "linear_project_name"
            )

        if row["slack_workspace_id"] != EXPECTED_SLACK_WORKSPACE_ID:
            errors.append(
                f"line {line_number}: slack_workspace_id must be "
                f"{EXPECTED_SLACK_WORKSPACE_ID}"
            )
        if not SLACK_CHANNEL_RE.fullmatch(row["slack_channel_id"]):
            errors.append(f"line {line_number}: invalid Slack channel ID")

        expected_slack_url = (
            "https://oresoftware-workspace.slack.com/archives/"
            f"{row['slack_channel_id']}"
        )
        if row["slack_channel_url"] != expected_slack_url:
            errors.append(
                f"line {line_number}: slack_channel_url must be "
                f"{expected_slack_url!r}"
            )

    keys = [row["portfolio_key"] for row in rows]
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


def main() -> int:
    args = parse_args()
    errors = validate(args.registry, args.expected_minimum)
    if errors:
        print("portfolio project-link registry validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(
        f"validated {len(EXPECTED_KEYS)} canonical portfolio mappings in "
        f"{args.registry}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
