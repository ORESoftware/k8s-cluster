#!/usr/bin/env python3
"""Compatibility entrypoint for the canonical empty-repository main-ref patch.

The reviewed fleet materializer already invokes
`patch_elenkos_empty_repository_main_ref_20260820.py`. This wrapper remains for
older one-shot workflows and delegates to that fail-closed implementation, so
reapplying it after materialization is idempotent rather than a second,
divergent source rewrite.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

CANONICAL = Path("scripts/ops/patch_elenkos_empty_repository_main_ref_20260820.py")
PUBLISHER = Path("scripts/ops/publish_elenkos_fleet_20260819.py")


def main(argv: list[str]) -> int:
    if len(argv) > 2:
        raise SystemExit("usage: patch_elenkos_empty_repository_409_20260820.py [root]")
    root = Path(argv[1] if len(argv) == 2 else ".").resolve()
    canonical = root / CANONICAL
    publisher = root / PUBLISHER
    if not canonical.is_file():
        raise RuntimeError(f"missing canonical patcher: {CANONICAL}")
    if not publisher.is_file():
        raise RuntimeError(f"missing materialized publisher: {PUBLISHER}")

    subprocess.run(
        [sys.executable, str(canonical), "--publisher", str(publisher)],
        cwd=root,
        check=True,
    )
    print("ELENKOS_EMPTY_REPOSITORY_409_COMPAT verified canonical_patch=true")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
