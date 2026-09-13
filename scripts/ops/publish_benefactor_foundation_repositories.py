#!/usr/bin/env python3
"""Verify and run the reviewed Benefactor foundation publisher payload."""

from __future__ import annotations

import base64
import gzip
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

CARRIER_DIR = Path(__file__).with_name("benefactor-foundation")
PUBLISHER_CARRIER = CARRIER_DIR / "publisher.py.gz.b64"
ENCODED_SHA256 = "a0d98208ec28d64f4f2f3c6d010e60567da1e1e1fad6207613ffcb2619ef6ba3"
GZIP_SHA256 = "d94531a975086014893876542e28d84ea3af7626b2971dad851fc66da760ef9e"
SOURCE_SHA256 = "761b92380afe981e71d14d63c562b823c2913ee3b9a11aec62015452faccc7ac"
SOURCE_SIZE = 23236


def sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def fail(message: str) -> int:
    print(f"benefactor-foundation-loader: {message}", file=sys.stderr)
    return 1


def main() -> int:
    os.umask(0o077)
    if not PUBLISHER_CARRIER.is_file():
        return fail(f"missing reviewed publisher carrier: {PUBLISHER_CARRIER}")

    encoded = b"".join(PUBLISHER_CARRIER.read_bytes().split())
    if sha256(encoded) != ENCODED_SHA256:
        return fail("encoded publisher checksum mismatch")

    try:
        compressed = base64.b64decode(encoded, validate=True)
    except ValueError:
        return fail("publisher carrier is not valid base64")
    if sha256(compressed) != GZIP_SHA256:
        return fail("compressed publisher checksum mismatch")

    try:
        source = gzip.decompress(compressed)
    except (OSError, EOFError):
        return fail("publisher carrier is not valid deterministic gzip")
    if len(source) != SOURCE_SIZE or sha256(source) != SOURCE_SHA256:
        return fail("decoded publisher checksum mismatch")

    work = Path(tempfile.mkdtemp(prefix="benefactor-foundation-publisher-"))
    try:
        publisher = work / "publisher.py"
        publisher.write_bytes(source)
        publisher.chmod(0o700)
        environment = os.environ.copy()
        environment["BENEFACTOR_FOUNDATION_CARRIER_DIR"] = str(CARRIER_DIR)
        completed = subprocess.run(
            [sys.executable, str(publisher), *sys.argv[1:]],
            env=environment,
            check=False,
        )
        return completed.returncode
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
