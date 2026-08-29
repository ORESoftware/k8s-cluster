#!/usr/bin/env python3
"""Validate the ClipTown secondary-checkout contract without touching cluster state."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONTRACT_PATH = ROOT / "remote/deployments/cliptown-monorepo.pointer.json"
GITMODULES_PATH = ROOT / ".gitmodules"
ALLOWED_CHANGED_PATHS = {
    ".gitmodules",
    ".github/workflows/cliptown-monorepo-pointer.yml",
    "remote/deployments/cliptown-monorepo",
    "remote/deployments/cliptown-monorepo.pointer.json",
    "scripts/validate-cliptown-monorepo-pointer.py",
}


def run(*args: str, cwd: Path = ROOT) -> str:
    result = subprocess.run(
        args,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return result.stdout.strip()


def fail(message: str) -> "NoReturn":
    raise SystemExit(message)


def main() -> None:
    contract = json.loads(CONTRACT_PATH.read_text(encoding="utf-8"))
    if contract.get("schema_version") != 1:
        fail("unsupported ClipTown pointer schema")
    if contract.get("deployment_authorized") is not False:
        fail("pointer maintenance must not authorize a deployment")

    path = str(contract["path"])
    url = str(contract["url"])
    branch = str(contract["branch"])
    expected = str(contract["commit"])
    section = f'submodule.{path}'

    configured_path = run(
        "git", "config", "--file", str(GITMODULES_PATH), "--get", f"{section}.path"
    )
    configured_url = run(
        "git", "config", "--file", str(GITMODULES_PATH), "--get", f"{section}.url"
    )
    configured_branch = run(
        "git", "config", "--file", str(GITMODULES_PATH), "--get", f"{section}.branch"
    )
    if (configured_path, configured_url, configured_branch) != (path, url, branch):
        fail(".gitmodules does not match the reviewed ClipTown pointer contract")

    tree_line = run("git", "ls-tree", "HEAD", "--", path)
    fields = tree_line.split(maxsplit=3)
    if len(fields) != 4 or fields[0] != "160000" or fields[1] != "commit":
        fail(f"{path} is not a gitlink")
    if fields[2] != expected:
        fail(f"ClipTown gitlink mismatch: expected {expected}, found {fields[2]}")

    remote_line = run("git", "ls-remote", url, f"refs/heads/{branch}")
    remote_fields = remote_line.split()
    if len(remote_fields) != 2 or remote_fields[0] != expected:
        fail(
            f"ClipTown {branch} moved: expected {expected}, "
            f"found {remote_fields[0] if remote_fields else 'no ref'}"
        )

    initialized_git = ROOT / path / ".git"
    if initialized_git.exists():
        actual = run("git", "rev-parse", "HEAD", cwd=ROOT / path)
        if actual != expected:
            fail(f"initialized ClipTown checkout mismatch: expected {expected}, found {actual}")

    base_ref = None
    try:
        base_ref = run("git", "merge-base", "HEAD", "origin/main")
    except subprocess.CalledProcessError:
        base_ref = None
    if base_ref:
        changed = {
            item
            for item in run("git", "diff", "--name-only", f"{base_ref}...HEAD").splitlines()
            if item
        }
        unexpected = sorted(changed - ALLOWED_CHANGED_PATHS)
        if unexpected:
            fail(
                "ClipTown pointer PR contains out-of-scope files: " + ", ".join(unexpected)
            )

    print(f"ClipTown pointer validated at {expected}; deployment_authorized=false")


if __name__ == "__main__":
    main()
