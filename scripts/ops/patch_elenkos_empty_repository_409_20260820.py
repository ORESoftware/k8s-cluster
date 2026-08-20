#!/usr/bin/env python3
"""Patch the materialized DEN-3786 publisher for GitHub's empty-repository ref response.

GitHub returns HTTP 409 (rather than 404) when reading a ref from a newly
created repository that has no commits. The Elenkos publisher intentionally
creates repositories with auto_init=false, so the first main-ref lookup must
treat that single 409 state as "ref absent" and continue to the atomic initial
commit. All repository marker, visibility, inventory, and drift checks remain
unchanged.
"""
from __future__ import annotations

import sys
from pathlib import Path

RELATIVE = Path("scripts/ops/publish_elenkos_fleet_20260819.py")
OLD = 'status, document = api.get(f"/repos/{spec.full_name}/git/ref/heads/main", allow=(404,))'
NEW = 'status, document = api.get(f"/repos/{spec.full_name}/git/ref/heads/main", allow=(404, 409))'


def main(argv: list[str]) -> int:
    if len(argv) > 2:
        raise SystemExit("usage: patch_elenkos_empty_repository_409_20260820.py [root]")
    root = Path(argv[1] if len(argv) == 2 else ".").resolve()
    target = root / RELATIVE
    if not target.is_file():
        raise RuntimeError(f"missing materialized publisher: {RELATIVE}")

    source = target.read_text(encoding="utf-8")
    old_count = source.count(OLD)
    new_count = source.count(NEW)
    if old_count == 0 and new_count == 1:
        print("ELENKOS_EMPTY_REPOSITORY_409_PATCH already-applied")
        return 0
    if old_count != 1 or new_count != 0:
        raise RuntimeError(
            f"refusing unexpected publisher source: old_matches={old_count} new_matches={new_count}"
        )

    target.write_text(source.replace(OLD, NEW, 1), encoding="utf-8")
    print("ELENKOS_EMPTY_REPOSITORY_409_PATCH applied allow_statuses=404,409")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
