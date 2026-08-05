#!/usr/bin/env python3
"""Classify an expected OCI release entry against GitHub issue comments.

Exit codes:
  0  exact entry already exists; publication is idempotent
  10 marker is absent; caller may append the expected entry
  65 malformed input or conflicting entry for the same source/target marker
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

MARKER_RE = re.compile(
    r"^<!-- gha-continuity-oci-release:([0-9a-f]{40}):(clone-server|executor-router) -->$"
)


def fail(message: str) -> "NoReturn":
    print(message, file=sys.stderr)
    raise SystemExit(65)


def flatten_comments(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        fail("comments payload must be a JSON array")
    flattened: list[dict[str, Any]] = []
    for item in value:
        if isinstance(item, list):
            flattened.extend(flatten_comments(item))
        elif isinstance(item, dict):
            flattened.append(item)
        else:
            fail("comments payload contains a non-object entry")
    return flattened


def main() -> int:
    if len(sys.argv) != 3:
        fail(f"usage: {sys.argv[0]} EXPECTED_ENTRY COMMENTS_JSON")

    expected = Path(sys.argv[1]).read_text(encoding="utf-8")
    comments_value = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
    first_line = expected.splitlines()[0] if expected.splitlines() else ""
    if not MARKER_RE.fullmatch(first_line):
        fail("expected entry has an invalid release marker")
    if not expected.endswith("\n"):
        fail("expected entry must end with a newline")

    matching: list[str] = []
    for comment in flatten_comments(comments_value):
        body = comment.get("body")
        if not isinstance(body, str):
            continue
        if first_line in body:
            matching.append(body)

    if not matching:
        print("missing")
        return 10
    if all(body.rstrip("\n") == expected.rstrip("\n") for body in matching):
        print("present")
        return 0

    fail("release marker already exists with conflicting metadata")


if __name__ == "__main__":
    raise SystemExit(main())
