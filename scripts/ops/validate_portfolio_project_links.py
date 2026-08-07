#!/usr/bin/env python3
"""Validate the canonical ChatGPT/GitHub/Linear/Slack portfolio registry."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from urllib.parse import urlparse

from portfolio_project_links import (
    EXPECTED_COUNT,
    REQUIRED_COLUMNS,
    duplicate_values,
    read_registry,
    validate_registry,
)

LINEAR_SLUG_RE = re.compile(
    r"^(?P<name>[a-z0-9-]+)-(?P<identifier>[0-9a-f]{12})$"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "registry",
        nargs="?",
        type=Path,
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    return parser.parse_args()


def canonical_linear_slug_name(linear_project_name: str) -> str:
    """Normalize the canonical Linear alias to its URL-slug name component."""

    return re.sub(r"[^a-z0-9-]+", "", linear_project_name.lower())


def validate_linear_url_identities(path: Path) -> list[str]:
    """Apply URL/name and alias-uniqueness checks to an otherwise valid registry."""

    columns, rows = read_registry(path)
    if columns != REQUIRED_COLUMNS:
        return []

    errors: list[str] = []
    duplicate_names = duplicate_values(rows, "linear_project_name")
    if duplicate_names:
        errors.append(
            "duplicate linear_project_name values: " + ", ".join(duplicate_names)
        )

    linear_prefix = "/denman/project/"
    for line_number, row in enumerate(rows, start=2):
        linear_url = urlparse(row["linear_project_url"].strip())
        linear_slug = linear_url.path.removeprefix(linear_prefix)
        linear_slug_match = LINEAR_SLUG_RE.fullmatch(linear_slug)
        if (
            linear_url.scheme != "https"
            or linear_url.netloc != "linear.app"
            or not linear_url.path.startswith(linear_prefix)
            or linear_url.params
            or linear_url.query
            or linear_url.fragment
            or linear_slug_match is None
        ):
            errors.append(f"line {line_number}: invalid Linear project URL")
            continue

        expected_name = canonical_linear_slug_name(row["linear_project_name"].strip())
        if linear_slug_match.group("name") != expected_name:
            errors.append(
                f"line {line_number}: Linear URL slug does not match "
                "linear_project_name"
            )

    return errors


def main() -> int:
    args = parse_args()
    try:
        errors = validate_registry(args.registry)
        if not errors:
            errors.extend(validate_linear_url_identities(args.registry))
    except OSError as exc:
        print(
            f"portfolio project-link registry validation failed: {exc}",
            file=sys.stderr,
        )
        return 2

    if errors:
        print("portfolio project-link registry validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(f"validated {EXPECTED_COUNT} canonical portfolio mappings in {args.registry}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
