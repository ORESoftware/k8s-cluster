#!/usr/bin/env python3
"""Fail closed if the MemeBank/ClipTown boundary drifts back to phone coupling."""

from __future__ import annotations

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
README = ROOT / "README.md"
CONTRACT = ROOT / "docs" / "memebank-integration.md"


def fail(message: str) -> None:
    print(f"MemeBank boundary check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def read(path: pathlib.Path) -> str:
    try:
        return path.read_text(encoding="utf-8").lower()
    except OSError as error:
        fail(f"cannot read {path.relative_to(ROOT)}: {error}")
        raise AssertionError("unreachable")


readme = read(README)
contract = read(CONTRACT)
combined = f"{readme}\n{contract}"

required_phrases = (
    "api- and sdk-only",
    "official sdk",
    "shared-auth is the sole cross-product authentication and assurance boundary",
    "audience `cliptown-api`",
    "authorized party `memebank-api`",
    "cliptown:memebank:read",
    "cliptown:memebank:write",
    "cliptown:memebank:delete",
    "never requires both mobile apps to be installed",
    "native clipboard export remains a separate user feature",
    "ciphertext-only payload boundary",
    "does not discover a local application",
)

for phrase in required_phrases:
    if phrase not in combined:
        fail(f"required invariant is missing: {phrase!r}")

forbidden_phrases = (
    "future direct local bridge",
    "local user-mediated handoff",
    "integrate through explicit clipboard writes",
    "integration transport is the clipboard",
    "memebank://",
    "cliptown://",
    "canopenurl",
    "canlaunchurl",
)

for phrase in forbidden_phrases:
    if phrase in combined:
        fail(f"forbidden co-installation or local-transport wording remains: {phrase!r}")

contract_only_requirements = (
    "must not",
    "probe whether memebank or cliptown is installed",
    "use local ipc, loopback ports",
    "fall back to clipboard handoff",
    "memeBank never calls a 3fa backend".lower(),
    "write and delete operations",
    "idempotency-key/body mismatch",
    "no mobile applications installed",
    "contract preview rather than a deployable integration",
)

for phrase in contract_only_requirements:
    if phrase not in contract:
        fail(f"contract-specific invariant is missing: {phrase!r}")

print("MemeBank API/SDK-only boundary contract passed")
