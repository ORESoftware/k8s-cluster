#!/usr/bin/env python3
"""Align the 3FA real-process test with the server's allowlist status contract."""

from __future__ import annotations

import subprocess
from pathlib import Path

PATH = Path("remote/deployments/gha-clone-server-rs/tests/threefa_interfaces.rs")
EXPECTED_BLOB = "a39061d98b5fa9e43a200bd39ca928a966e22de6"


def main() -> None:
    source = PATH.read_text(encoding="utf-8")
    new = "        assert_eq!(response.status(), StatusCode::FORBIDDEN);\n"
    if new in source:
        print("3FA allowlist status assertion is already current")
        return

    observed = subprocess.check_output(
        ["git", "hash-object", str(PATH)], text=True
    ).strip()
    if observed != EXPECTED_BLOB:
        raise SystemExit(
            f"refusing drifted {PATH}: expected {EXPECTED_BLOB}, observed {observed}"
        )

    old = "        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);\n"
    if source.count(old) != 1:
        raise SystemExit("expected one final allowlist-status assertion")
    PATH.write_text(source.replace(old, new, 1), encoding="utf-8")
    print("3FA repository/workflow allowlist assertions now require HTTP 403")


if __name__ == "__main__":
    main()
