#!/usr/bin/env python3
"""Run the bounded publisher with its current transport and visibility contract."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path
from typing import Any

MODULE_PATH = Path(__file__).with_name("publish_missing_org_repositories.py")
SPEC = importlib.util.spec_from_file_location("bounded_missing_repo_publisher", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def fail(message: str) -> None:
    raise RuntimeError(message)


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
            fail("publisher contains an unexpected number of legacy transport defects")
        path.write_text(text.replace(broken, fixed, 1), encoding="utf-8")
        text = path.read_text(encoding="utf-8")
    elif "askpass.write_text(" not in text:
        fail("publisher lacks the bounded non-interactive Git credential transport")

    required = (
        "GITHUB_REPOSITORY_ADMIN_TOKEN",
        "GIT_ASKPASS",
        "GIT_TERMINAL_PROMPT",
        "x-access-token",
    )
    missing = [snippet for snippet in required if snippet not in text]
    if missing:
        fail(f"publisher credential contract is incomplete: {missing}")

    subprocess.run([sys.executable, "-m", "py_compile", str(path)], check=True)


def ensure_private_repository(owner: str, name: str, description: str) -> dict[str, Any]:
    """Create extracted repositories privately and reject visibility drift.

    The fleet finalizer treats these repositories as private product source.  Do
    not silently publish them publicly merely because an older transport helper
    used a public default.
    """

    status, current = MODULE.api("GET", f"/repos/{owner}/{name}")
    if status == 404:
        status, current = MODULE.api(
            "POST",
            f"/orgs/{owner}/repos",
            {
                "name": name,
                "description": description,
                "private": True,
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "auto_init": False,
                "allow_squash_merge": True,
                "allow_merge_commit": True,
                "allow_rebase_merge": False,
                "delete_branch_on_merge": True,
            },
        )
        if status != 201 or not isinstance(current, dict):
            fail(f"failed to create {owner}/{name}: HTTP {status}")
        print(f"CREATED {owner}/{name}")

    if not isinstance(current, dict):
        fail(f"invalid repository response for {owner}/{name}")
    if current.get("private") is not True or current.get("visibility") != "private":
        fail(
            f"visibility mismatch for {owner}/{name}: "
            f"private={current.get('private')!r}, visibility={current.get('visibility')!r}"
        )
    return current


MODULE.repair_publisher = repair_or_validate_publisher
MODULE.ensure_repository = ensure_private_repository
raise SystemExit(MODULE.main())
