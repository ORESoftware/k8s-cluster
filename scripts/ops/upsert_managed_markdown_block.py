#!/usr/bin/env python3
"""Upsert a marked Markdown block without changing unrelated prose."""

from __future__ import annotations

import argparse
from pathlib import Path


class ManagedBlockError(ValueError):
    pass


def render_managed_block(original: str, marker: str, content: str) -> str:
    start = f"<!-- {marker}:start -->"
    end = f"<!-- {marker}:end -->"
    managed = f"{start}\n{content.strip()}\n{end}"

    start_count = original.count(start)
    end_count = original.count(end)
    if start_count != end_count or start_count > 1:
        raise ManagedBlockError(
            f"expected zero or one balanced managed block for marker {marker!r}"
        )

    if start_count == 1:
        before, remainder = original.split(start, 1)
        _, after = remainder.split(end, 1)
        prefix = before.rstrip()
        suffix = after.lstrip("\r\n")

        updated = f"{prefix}\n\n{managed}" if prefix else managed
        if suffix:
            updated += f"\n\n{suffix}"
        else:
            updated += "\n"
        return updated

    prefix = original.rstrip()
    return f"{prefix}\n\n{managed}\n" if prefix else f"{managed}\n"


def upsert_managed_block(path: Path, marker: str, block_file: Path) -> bool:
    original = path.read_text(encoding="utf-8") if path.exists() else ""
    content = block_file.read_text(encoding="utf-8")
    updated = render_managed_block(original, marker, content)

    if updated == original:
        return False

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(updated, encoding="utf-8")
    return True


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("path", type=Path)
    parser.add_argument("marker")
    parser.add_argument("block_file", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    upsert_managed_block(args.path, args.marker, args.block_file)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
