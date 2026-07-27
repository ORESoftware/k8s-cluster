#!/usr/bin/env python3
"""Verify that the reviewed source manifest exactly matches recorded gitlinks."""

from __future__ import annotations

import configparser
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST_PATH = ROOT / "release" / "source-manifest.json"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")


def fail(message: str) -> None:
    print(f"source-manifest error: {message}", file=sys.stderr)
    raise SystemExit(1)


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


def normalize_repository_url(url: str) -> str:
    prefix = "https://github.com/"
    if not url.startswith(prefix):
        fail(f"unsupported submodule URL: {url}")
    repository = url[len(prefix) :]
    return repository.removesuffix(".git")


def main() -> None:
    try:
        manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load release/source-manifest.json: {error}")

    if manifest.get("schema_version") != 1:
        fail("schema_version must be 1")
    if manifest.get("gitlinks_status") != "applied":
        fail("gitlinks_status must be applied")

    repositories = manifest.get("repositories")
    if not isinstance(repositories, dict) or not repositories:
        fail("repositories must be a non-empty object")

    parser = configparser.ConfigParser()
    parser.read(ROOT / ".gitmodules", encoding="utf-8")
    expected: dict[str, tuple[str, str]] = {}
    for section in parser.sections():
        path = parser.get(section, "path", fallback="")
        url = parser.get(section, "url", fallback="")
        branch = parser.get(section, "branch", fallback="")
        if not path or not url:
            fail(f"{section}: path and URL are required")
        if branch != "main":
            fail(f"{path}: submodule branch must be main")
        expected[path] = (normalize_repository_url(url), branch)

    if set(repositories) != set(expected):
        missing = sorted(set(expected) - set(repositories))
        extra = sorted(set(repositories) - set(expected))
        fail(f"manifest inventory mismatch; missing={missing}, extra={extra}")

    for path, (expected_repository, expected_branch) in sorted(expected.items()):
        entry = repositories[path]
        if not isinstance(entry, dict):
            fail(f"{path}: manifest entry must be an object")
        repository = entry.get("repository")
        branch = entry.get("branch")
        commit = entry.get("commit")
        if repository != expected_repository:
            fail(f"{path}: repository mismatch: {repository!r} != {expected_repository!r}")
        if branch != expected_branch:
            fail(f"{path}: branch mismatch: {branch!r} != {expected_branch!r}")
        if not isinstance(commit, str) or not SHA_RE.fullmatch(commit):
            fail(f"{path}: commit must be a lowercase 40-character SHA")

        tree_line = run("git", "ls-tree", "HEAD", "--", path)
        fields = tree_line.split()
        if len(fields) < 4 or fields[0] != "160000" or fields[1] != "commit":
            fail(f"{path}: expected a 160000 gitlink, got {tree_line!r}")
        tree_commit = fields[2]
        if tree_commit != commit:
            fail(f"{path}: tree gitlink {tree_commit} != manifest {commit}")

        checkout = ROOT / path
        if not (checkout / ".git").exists() and not (checkout / ".git").is_file():
            fail(f"{path}: recursive checkout is missing")
        checked_out_commit = run("git", "rev-parse", "HEAD", cwd=checkout)
        if checked_out_commit != commit:
            fail(f"{path}: checked-out HEAD {checked_out_commit} != manifest {commit}")

    print(f"source manifest valid: {len(repositories)} exact main-branch gitlinks")


if __name__ == "__main__":
    main()
