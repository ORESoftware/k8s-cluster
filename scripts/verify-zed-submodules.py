#!/usr/bin/env python3
"""Enforce the ClipTown Zed-package and Git-submodule ownership boundary."""

from __future__ import annotations

import configparser
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_DEPENDENCIES = {
    "cliptown/cliptown-interfaces",
    "cliptown/cliptown-clients",
}
FORBIDDEN_REPOSITORIES = {"cliptown-infra", "cliptown-cli"}


def gitlinks() -> set[str]:
    result = subprocess.run(
        ["git", "ls-files", "--stage", "apps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    paths: set[str] = set()
    for line in result.stdout.splitlines():
        metadata, path = line.split("\t", 1)
        if metadata.split(" ", 1)[0] == "160000":
            paths.add(path)
    return paths


def main() -> int:
    errors: list[str] = []
    manifest = tomllib.loads((ROOT / ".zpkg.toml").read_text(encoding="utf-8"))
    dependencies = set(manifest.get("dependencies", {}))
    if dependencies != EXPECTED_DEPENDENCIES:
        errors.append(
            "Zed dependencies must be exactly: "
            + ", ".join(sorted(EXPECTED_DEPENDENCIES))
        )
    if manifest.get("install", {}).get("dir") != ".vendor/.zed":
        errors.append("install.dir must be .vendor/.zed")

    parser = configparser.ConfigParser(interpolation=None)
    parser.read(ROOT / ".gitmodules", encoding="utf-8")
    declared: set[str] = set()
    for section in parser.sections():
        path = parser.get(section, "path", fallback="")
        url = parser.get(section, "url", fallback="")
        declared.add(path)
        if not path.startswith("apps/"):
            errors.append(f"submodule path must be below apps/: {path}")
        repository = path.removeprefix("apps/")
        if repository in FORBIDDEN_REPOSITORIES:
            errors.append(f"forbidden monorepo import: {repository}")
        expected_url = f"https://github.com/cliptown/{repository}.git"
        if url != expected_url:
            errors.append(f"unexpected URL for {repository}: {url}")
        if path.startswith(".vendor/.zed"):
            errors.append(f"submodule overlaps Zed install path: {path}")

    indexed = gitlinks()
    if declared != indexed:
        missing = declared - indexed
        undeclared = indexed - declared
        if missing:
            errors.append("declared without gitlink: " + ", ".join(sorted(missing)))
        if undeclared:
            errors.append("gitlink without declaration: " + ", ".join(sorted(undeclared)))

    for path in declared | indexed:
        repository = path.removeprefix("apps/")
        if repository in FORBIDDEN_REPOSITORIES:
            errors.append(f"forbidden gitlink: {path}")

    if errors:
        print("Zed/submodule topology validation failed:", file=sys.stderr)
        for error in errors:
            print(f" - {error}", file=sys.stderr)
        return 1

    print(
        f"validated {len(dependencies)} Zed dependencies and "
        f"{len(indexed)} non-overlapping source gitlinks"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
