#!/usr/bin/env python3
"""CLI for the bounded five-repository MCP publication contract."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from requested_mcp_publisher import check, publish, validate_specs


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--execute", action="store_true")
    parser.add_argument("--report", type=Path, default=Path("/tmp/requested-mcp-publication.json"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    specs = validate_specs()
    if args.check:
        print(json.dumps(check(specs), indent=2, sort_keys=True))
        return 0
    report = publish(specs, args.report)
    if len(report["repositories"]) != len(specs):
        raise RuntimeError("publication report is incomplete")
    print(f"PASS published and verified {len(specs)} requested MCP repositories")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
