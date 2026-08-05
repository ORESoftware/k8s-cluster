#!/usr/bin/env python3
"""Validate the canonical ChatGPT/GitHub/Linear/Slack portfolio registry."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from portfolio_project_links import EXPECTED_COUNT, validate_registry


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "registry",
        nargs="?",
        type=Path,
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        errors = validate_registry(args.registry)
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
