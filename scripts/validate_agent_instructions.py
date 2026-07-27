#!/usr/bin/env python3
"""Validate canonical hierarchical agent instructions."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
POINTERS = {
    ROOT / "AGENTS.md": "# Canonical agent instructions\n\nRead and apply [`agents.md`](agents.md). This compatibility file must not duplicate repository instructions.\n",
    ROOT / ".claude/CLAUDE.md": "# Canonical agent instructions\n\nRead and apply [`../agents.md`](../agents.md). Do not duplicate repository instructions here.\n",
    ROOT / ".gemini/GEMINI.md": "# Canonical agent instructions\n\nRead and apply [`../agents.md`](../agents.md). Do not duplicate repository instructions here.\n",
    ROOT / ".openai/AGENTS.md": "# Canonical agent instructions\n\nRead and apply [`../agents.md`](../agents.md). Do not duplicate repository instructions here.\n",
}
REQUIRED_PHRASES = (
    "github.com/sonus-auris",
    "Linear project: `github.com/sonus-auris`",
    "Resolve conflicts semantically",
    "git fetch --all --prune",
    "<<<<<<<",
    "gitlink",
)


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def discover(start: Path) -> list[Path]:
    directory = start.resolve(strict=True)
    if directory.is_file():
        directory = directory.parent
    ancestors = list(directory.parents)
    ancestors.reverse()
    ancestors.append(directory)
    seen: set[Path] = set()
    found: list[Path] = []
    for ancestor in ancestors:
        candidate = ancestor / "agents.md"
        if not candidate.exists():
            continue
        try:
            resolved = candidate.resolve(strict=True)
            candidate.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            fail(f"cannot read {candidate}: {error}")
        if resolved in seen:
            continue
        seen.add(resolved)
        found.append(resolved)
    return found


def main() -> None:
    canonical = ROOT / "agents.md"
    if not canonical.is_file():
        fail("missing lowercase agents.md")
    text = canonical.read_text(encoding="utf-8")
    for phrase in REQUIRED_PHRASES:
        if phrase not in text:
            fail(f"canonical agents.md is missing required phrase: {phrase}")
    for path, expected in POINTERS.items():
        if not path.is_file():
            fail(f"missing pointer {path.relative_to(ROOT)}")
        if path.read_text(encoding="utf-8") != expected:
            fail(f"pointer duplicates or diverges from canonical instructions: {path.relative_to(ROOT)}")

    start = ROOT / "apps"
    if not start.is_dir():
        fail("missing apps directory used for hierarchy validation")
    chain = discover(start)
    expected = [canonical.resolve()]
    if chain != expected:
        fail(f"wrong root-to-leaf instruction chain: expected {expected}, got {chain}")
    print("agents.md chain for apps/:")
    for path in chain:
        print(f"  - {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
