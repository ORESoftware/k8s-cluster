#!/usr/bin/env python3
"""Fail when customer CI and production image builds use different sibling commits."""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PINS = json.loads((ROOT / "dependency-pins.json").read_text(encoding="utf-8"))
HEX_SHA = re.compile(r"^[0-9a-f]{40}$")

required = {
    "fiducia-interfaces",
    "fiducia-marketing.web",
    "fiducia-payments.rs",
    "fiducia-test-config",
}
if set(PINS) != required:
    raise SystemExit(
        "dependency-pins.json keys must be exactly: " + ", ".join(sorted(required))
    )
for repository, sha in PINS.items():
    if not isinstance(sha, str) or not HEX_SHA.fullmatch(sha):
        raise SystemExit(f"{repository}: expected a full lowercase commit SHA")

interfaces = PINS["fiducia-interfaces"]
marketing = PINS["fiducia-marketing.web"]
payments = PINS["fiducia-payments.rs"]
test_config = PINS["fiducia-test-config"]

contracts = {
    ".github/workflows/ci.yml": [
        rf"repository:\s*fiducia-cloud/fiducia-interfaces\s+ref:\s*{interfaces}\b",
        rf"repository:\s*fiducia-cloud/fiducia-payments\.rs\s+ref:\s*{payments}\b",
    ],
    ".github/workflows/docker.yml": [
        rf"INTERFACES_SHA={interfaces}\b",
        rf"MARKETING_REF={marketing}\b",
        rf"PAYMENTS_REF={payments}\b",
        rf"TEST_CONFIG_REF={test_config}\b",
    ],
    "Dockerfile": [
        rf"ARG INTERFACES_SHA={interfaces}\b",
        rf"ARG MARKETING_REF={marketing}\b",
        rf"ARG PAYMENTS_REF={payments}\b",
        rf"ARG TEST_CONFIG_REF={test_config}\b",
    ],
}

failures: list[str] = []
for relative_path, patterns in contracts.items():
    text = (ROOT / relative_path).read_text(encoding="utf-8")
    for pattern in patterns:
        if re.search(pattern, text, flags=re.MULTILINE) is None:
            failures.append(f"{relative_path}: missing synchronized pin matching {pattern}")

if failures:
    raise SystemExit("sibling dependency pin drift:\n- " + "\n- ".join(failures))

print(
    "customer sibling dependency pins synchronized: "
    + ", ".join(f"{name}={sha}" for name, sha in sorted(PINS.items()))
)
