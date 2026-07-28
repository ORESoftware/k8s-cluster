#!/usr/bin/env python3

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER = ROOT / "remote/deployments/browser-test-server/src/server.ts"

source = SERVER.read_text(encoding="utf-8")
repairs = {
    "const metrics = {const metrics = {": "const metrics = {",
    "export type { RunRequest, RunResult, Step };\nexport type { RunRequest, RunResult, Step };\n": "export type { RunRequest, RunResult, Step };\n",
}

for old, new in repairs.items():
    count = source.count(old)
    if count > 1:
        raise SystemExit(f"ambiguous migration repair for {old!r}: {count} matches")
    if count == 1:
        source = source.replace(old, new, 1)

for forbidden in (
    "const metrics = {const metrics = {",
    "export type { RunRequest, RunResult, Step };\nexport type { RunRequest, RunResult, Step };",
):
    if forbidden in source:
        raise SystemExit(f"migration repair did not remove {forbidden!r}")

SERVER.write_text(source, encoding="utf-8")
print("repaired browser-test migration marker boundaries")
