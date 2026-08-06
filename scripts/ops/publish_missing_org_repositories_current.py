#!/usr/bin/env python3
"""Run the fixed allowlist publisher against either sealed publisher transport form."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("publish_missing_org_repositories.py")
SPEC = importlib.util.spec_from_file_location("bounded_missing_repo_publisher", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def repair_or_validate_publisher(path: Path) -> None:
    """Repair the one known legacy escaping defect, or validate current source."""

    text = path.read_text(encoding="utf-8")
    broken = """        askpass.write_text('#!/bin/sh
case \"$1\" in *Username*) echo x-access-token;; *) echo \"$GITHUB_REPOSITORY_ADMIN_TOKEN\";; esac
')
"""
    fixed = """        askpass.write_text(
            '#!/bin/sh\\ncase \"$1\" in *Username*) echo x-access-token;; *) echo \"$GITHUB_REPOSITORY_ADMIN_TOKEN\";; esac\\n'
        )
"""

    if broken in text:
        if text.count(broken) != 1:
            MODULE.fail("publisher contains an unexpected number of legacy transport defects")
        path.write_text(text.replace(broken, fixed, 1), encoding="utf-8")
        text = path.read_text(encoding="utf-8")
    elif "askpass.write_text(" not in text:
        MODULE.fail("publisher lacks the bounded non-interactive Git credential transport")

    required = (
        "GITHUB_REPOSITORY_ADMIN_TOKEN",
        "GIT_ASKPASS",
        "GIT_TERMINAL_PROMPT",
        "x-access-token",
    )
    missing = [snippet for snippet in required if snippet not in text]
    if missing:
        MODULE.fail(f"publisher credential contract is incomplete: {missing}")

    subprocess.run([sys.executable, "-m", "py_compile", str(path)], check=True)


MODULE.repair_publisher = repair_or_validate_publisher
raise SystemExit(MODULE.main())
