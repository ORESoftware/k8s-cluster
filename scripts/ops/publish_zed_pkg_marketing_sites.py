#!/usr/bin/env python3
"""Integrity-checked loader for the reviewable publisher and compressed site data."""
from pathlib import Path
import base64
import bz2
import hashlib

_ROOT = Path(__file__).resolve().parent
_PARTS = sorted(_ROOT.glob(Path(__file__).name + ".part*"))
if not _PARTS:
    raise RuntimeError("publisher source parts are missing")
_SOURCE = "".join(part.read_text(encoding="utf-8") for part in _PARTS)
_EXPECTED_SHA256 = "7ea3026cc918f4fb96580483c9c694b9dc827c93fdfb87d2110bd32f298f609f"
if hashlib.sha256(_SOURCE.encode("utf-8")).hexdigest() != _EXPECTED_SHA256:
    raise RuntimeError("publisher source integrity check failed")

_SPEC_PARTS = sorted(_ROOT.glob("zed_pkg_marketing_sites.json.bz2.b64.part*"))
if not _SPEC_PARTS:
    raise RuntimeError("compressed site specification parts are missing")
_SPEC_ENCODED = "".join(part.read_text(encoding="ascii").strip() for part in _SPEC_PARTS)
try:
    _SPEC_BYTES = bz2.decompress(base64.b64decode(_SPEC_ENCODED, validate=True))
except (ValueError, OSError) as exc:
    raise RuntimeError("compressed site specification is malformed") from exc
_EXPECTED_SPEC_SHA256 = "337a54d4a6335979728ed1fd36e6422557060fa028221f87d56b4ed85a2f8b0d"
if hashlib.sha256(_SPEC_BYTES).hexdigest() != _EXPECTED_SPEC_SHA256:
    raise RuntimeError("site specification integrity check failed")
_RUNTIME_SPEC = _ROOT / "zed_pkg_marketing_sites.json.part00.runtime"
_RUNTIME_SPEC.write_bytes(_SPEC_BYTES)

exec(compile(_SOURCE, __file__, "exec"), globals(), globals())
