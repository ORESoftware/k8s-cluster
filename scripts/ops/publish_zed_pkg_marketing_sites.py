#!/usr/bin/env python3
"""Integrity-checked loader for the reviewable publisher source parts."""
from pathlib import Path
import hashlib

_PARTS = sorted(Path(__file__).resolve().parent.glob(Path(__file__).name + ".part*"))
if not _PARTS:
    raise RuntimeError("publisher source parts are missing")
_SOURCE = "".join(part.read_text(encoding="utf-8") for part in _PARTS)
_EXPECTED_SHA256 = "7ea3026cc918f4fb96580483c9c694b9dc827c93fdfb87d2110bd32f298f609f"
if hashlib.sha256(_SOURCE.encode("utf-8")).hexdigest() != _EXPECTED_SHA256:
    raise RuntimeError("publisher source integrity check failed")
exec(compile(_SOURCE, __file__, "exec"), globals(), globals())
