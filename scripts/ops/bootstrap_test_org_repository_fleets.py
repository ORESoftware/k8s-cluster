#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path

_PARTS = Path(__file__).with_name("test_fleet_publisher")
_SOURCE = "".join(path.read_text(encoding="utf-8") for path in sorted(_PARTS.glob("part-*.pyfrag")))
if not _SOURCE:
    raise RuntimeError(f"no publisher source fragments found under {_PARTS}")
exec(compile(_SOURCE, str(_PARTS / "assembled_bootstrap_test_org_repository_fleets.py"), "exec"), globals(), globals())
