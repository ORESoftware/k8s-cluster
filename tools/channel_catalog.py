#!/usr/bin/env python3
"""Validate the canonical Slack channel binding registry for DEN-1267."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Mapping

SCHEMA_VERSION = 1
SCHEMA_REFERENCE = "./channels.schema.json"
GOVERNING_ISSUE = "DEN-1267"
OWNERS_CATALOG = "catalog/owners.json"

# This repository is public. Immutable Slack channel IDs and Linear project
# UUIDs belong on DEN-1267, never in a tracked public artifact.
SLACK_CHANNEL_ID = re.compile(r"\bC0[A-Z0-9]{8,}\b")
UUID = re.compile(
    r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
    re.IGNORECASE,
)


def load_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def eponymous_channel(owner: str) -> str:
    """Every project channel is the lowercased GitHub organization login."""
    return "#" + owner.lower()


def validate_catalog(value: Any) -> list[str]:
    errors: list[str] = []
    catalog = _mapping(value)
    if not catalog:
        return ["channel catalog must be a JSON object"]

    if catalog.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"schema_version must be {SCHEMA_VERSION}")
    if catalog.get("$schema") != SCHEMA_REFERENCE:
        errors.append(f"$schema must be {SCHEMA_REFERENCE}")
    if catalog.get("governing_issue") != GOVERNING_ISSUE:
        errors.append(f"governing_issue must be {GOVERNING_ISSUE}")

    channels = catalog.get("channels")
    if not isinstance(channels, list) or not channels:
        errors.append("channels must be a non-empty array")
        return errors

    for index, entry in enumerate(channels):
        errors.extend(_validate_channel(entry, f"channels[{index}]"))

    for field in ("slack_channel", "linear_project", "owner"):
        duplicates = sorted(
            name
            for name, count in Counter(
                _mapping(entry).get(field)
                for entry in channels
                if isinstance(_mapping(entry).get(field), str)
            ).items()
            if count > 1
        )
        for name in duplicates:
            errors.append(f"duplicate {field}: {name}")

    return errors


def _validate_channel(value: Any, identity: str) -> list[str]:
    errors: list[str] = []
    entry = _mapping(value)
    if not entry:
        return [f"{identity} must be a JSON object"]

    owner = entry.get("owner")
    channel = entry.get("slack_channel")
    if not isinstance(owner, str) or not owner:
        errors.append(f"{identity}.owner must be a non-empty string")
        owner = None
    if not isinstance(channel, str) or not channel.startswith("#"):
        errors.append(f"{identity}.slack_channel must start with '#'")
        channel = None

    # The user-facing contract: a project channel is named for its GitHub org.
    if owner and channel and channel != eponymous_channel(owner):
        errors.append(
            f"{identity}: channel {channel} is not eponymous with owner "
            f"{owner} (expected {eponymous_channel(owner)})"
        )

    state = entry.get("binding_state")
    if state not in {"bound", "unbound"}:
        errors.append(f"{identity}.binding_state must be 'bound' or 'unbound'")

    inventoried = entry.get("channel_inventoried")
    if not isinstance(inventoried, bool):
        errors.append(f"{identity}.channel_inventoried must be a boolean")
    elif state == "bound" and not inventoried:
        errors.append(
            f"{identity}: binding_state 'bound' requires channel_inventoried true"
        )

    notifications = entry.get("linear_notifications")
    if not isinstance(notifications, Mapping):
        errors.append(f"{identity}.linear_notifications must be an object")
    else:
        for toggle in ("new_issue", "issue_comments", "issue_statuses"):
            if not isinstance(notifications.get(toggle), bool):
                errors.append(
                    f"{identity}.linear_notifications.{toggle} must be a boolean"
                )

    if not isinstance(entry.get("linear_project"), str) or not entry.get(
        "linear_project"
    ):
        errors.append(f"{identity}.linear_project must be a non-empty string")

    return errors


def find_public_boundary_violations(raw: str) -> list[str]:
    violations: list[str] = []
    for match in sorted(set(SLACK_CHANNEL_ID.findall(raw))):
        violations.append(
            f"Slack channel ID {match} must not appear in a public catalog"
        )
    for match in sorted(set(UUID.findall(raw))):
        violations.append(f"Linear UUID {match} must not appear in a public catalog")
    return violations


def find_unregistered_owners(catalog: Mapping[str, Any], repo_root: Path) -> list[str]:
    owners_path = repo_root / OWNERS_CATALOG
    if not owners_path.exists():
        return [f"{OWNERS_CATALOG} is missing"]
    known = {
        owner.get("owner")
        for owner in _mapping(load_json(owners_path)).get("owners", [])
        if isinstance(owner, Mapping)
    }
    return sorted(
        f"{entry['owner']}: not present in {OWNERS_CATALOG}"
        for entry in (_mapping(item) for item in catalog.get("channels", []))
        if isinstance(entry.get("owner"), str) and entry["owner"] not in known
    )


def render_summary(catalog: Mapping[str, Any]) -> str:
    channels = [_mapping(entry) for entry in catalog.get("channels", [])]
    bound = [entry for entry in channels if entry.get("binding_state") == "bound"]
    unbound = [entry for entry in channels if entry.get("binding_state") == "unbound"]
    lines = [
        f"# Slack channel bindings ({GOVERNING_ISSUE})",
        "",
        f"- tracked channels: {len(channels)}",
        f"- bound: {len(bound)}",
        f"- unbound: {len(unbound)}",
        "",
        "| Slack channel | GitHub owner | Linear project | Binding |",
        "| -- | -- | -- | -- |",
    ]
    for entry in channels:
        lines.append(
            f"| `{entry.get('slack_channel')}` | `{entry.get('owner')}` | "
            f"`{entry.get('linear_project')}` | {entry.get('binding_state')} |"
        )
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate", help="validate a channel registry")
    validate.add_argument("catalog", type=Path)

    check = subparsers.add_parser(
        "check",
        help="validate, enforce the public data boundary, and cross-check owners",
    )
    check.add_argument("catalog", type=Path)
    check.add_argument(
        "--allow-unregistered-owner",
        action="append",
        default=[],
        help="owner known to be absent from catalog/owners.json",
    )

    report = subparsers.add_parser("report", help="render a Markdown summary")
    report.add_argument("catalog", type=Path)
    report.add_argument("--output", type=Path)

    args = parser.parse_args(argv)
    try:
        catalog = load_json(args.catalog)
        errors = validate_catalog(catalog)
        if errors:
            for error in errors:
                print(error, file=sys.stderr)
            return 1

        if args.command == "validate":
            print(f"validated {len(catalog['channels'])} channel bindings")
            return 0

        if args.command == "check":
            raw = args.catalog.read_text(encoding="utf-8")
            violations = find_public_boundary_violations(raw)
            allowed = set(args.allow_unregistered_owner)
            violations.extend(
                problem
                for problem in find_unregistered_owners(catalog, args.repo_root)
                if problem.split(":", 1)[0] not in allowed
            )
            if violations:
                for violation in violations:
                    print(violation, file=sys.stderr)
                return 1
            unbound = sum(
                1
                for entry in catalog["channels"]
                if _mapping(entry).get("binding_state") == "unbound"
            )
            print(
                f"channel catalog is public-safe; {len(catalog['channels'])} "
                f"bindings tracked, {unbound} still unbound"
            )
            return 0

        if args.command == "report":
            text = render_summary(catalog)
            if args.output:
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(text, encoding="utf-8")
            else:
                print(text, end="")
            return 0
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"channel catalog command failed: {exc}", file=sys.stderr)
        return 2
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
