#!/usr/bin/env python3
"""Prepare the shell reconciler to use the tested Markdown block helper."""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RECONCILER = ROOT / "scripts/ops/sync_org_project_docs.sh"

REPLACEMENT = r'''upsert_managed_block() {
  local path="$1"
  local marker="$2"
  local block_file="$3"
  local helper_dir

  helper_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
  python3 "$helper_dir/upsert_managed_markdown_block.py" \
    "$path" \
    "$marker" \
    "$block_file"
}

record_result()'''


def main() -> int:
    text = RECONCILER.read_text(encoding="utf-8")

    if "upsert_managed_markdown_block.py" in text:
        return 0

    pattern = re.compile(
        r"upsert_managed_block\(\) \{\n.*?\n\}\n\nrecord_result\(\)",
        re.DOTALL,
    )
    updated, count = pattern.subn(REPLACEMENT, text, count=1)
    if count != 1:
        raise SystemExit(
            "could not identify exactly one upsert_managed_block function"
        )

    RECONCILER.write_text(updated, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
