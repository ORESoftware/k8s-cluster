#!/usr/bin/env python3
"""Print applicable lowercase agents.md files from filesystem root to $PWD."""

from __future__ import annotations

import os
from pathlib import Path
import sys


def main() -> int:
    start = Path(sys.argv[1] if len(sys.argv) > 1 else os.getcwd())
    try:
        current = start.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        print(f"agent instructions: cannot resolve start directory: {error}", file=sys.stderr)
        return 2
    if not current.is_dir():
        current = current.parent

    ancestors: list[Path] = []
    while True:
        ancestors.append(current)
        if current.parent == current:
            break
        current = current.parent

    seen: set[Path] = set()
    had_error = False
    for directory in reversed(ancestors):
        candidate = directory / "agents.md"
        if not os.path.lexists(candidate):
            continue
        try:
            resolved = candidate.resolve(strict=True)
        except (OSError, RuntimeError) as error:
            print(f"agent instructions: cannot resolve {candidate}: {error}", file=sys.stderr)
            had_error = True
            continue
        if resolved in seen:
            continue
        seen.add(resolved)
        if not resolved.is_file():
            print(f"agent instructions: not a file: {candidate}", file=sys.stderr)
            had_error = True
            continue
        if not os.access(resolved, os.R_OK):
            print(f"agent instructions: unreadable: {candidate}", file=sys.stderr)
            had_error = True
            continue
        print(resolved)

    return 1 if had_error else 0


if __name__ == "__main__":
    raise SystemExit(main())
