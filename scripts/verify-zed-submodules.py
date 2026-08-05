#!/usr/bin/env python3
"""Enforce non-overlapping Zed-package and Git-submodule ownership."""

from __future__ import annotations

import configparser
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_DEPENDENCIES = {
    "shared-auth/shared-auth-interfaces",
    "shared-auth/shared-auth-lib",
    "shared-auth/shared-auth-clients",
}
REQUIRED_PACKAGE_SUBMODULES = {
    "apps/shared-auth-interfaces",
    "apps/shared-auth-lib",
    "apps/shared-auth-clients",
}
FORBIDDEN_REPOSITORIES = {"shared-auth-infra", "shared-auth-cli"}


def read_gitlinks() -> set[str]:
    result = subprocess.run(
        ["git", "ls-files", "--stage", "apps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    gitlinks: set[str] = set()
    for line in result.stdout.splitlines():
        metadata, path = line.split("\t", 1)
        mode = metadata.split(" ", 1)[0]
        if mode == "160000":
            gitlinks.add(path)
    return gitlinks


def main() -> int:
    errors: list[str] = []
    manifest = tomllib.loads((ROOT / ".zpkg.toml").read_text(encoding="utf-8"))
    dependencies = set(manifest.get("dependencies", {}))
    if dependencies != EXPECTED_DEPENDENCIES:
        errors.append(
            "Zed dependencies must be exactly: "
            + ", ".join(sorted(EXPECTED_DEPENDENCIES))
        )

    install_dir = manifest.get("install", {}).get("dir")
    if install_dir != ".vendor/.zed":
        errors.append("install.dir must be .vendor/.zed")

    parser = configparser.ConfigParser(interpolation=None)
    parser.read(ROOT / ".gitmodules", encoding="utf-8")
    declared_paths: set[str] = set()
    for section in parser.sections():
        path = parser.get(section, "path", fallback="")
        url = parser.get(section, "url", fallback="")
        declared_paths.add(path)
        if not path.startswith("apps/"):
            errors.append(f"submodule path must be below apps/: {path}")
        repository = path.removeprefix("apps/")
        if repository in FORBIDDEN_REPOSITORIES:
            errors.append(f"forbidden monorepo import: {repository}")
        expected_url = f"git@github.com:shared-auth/{repository}.git"
        if url != expected_url:
            errors.append(f"unexpected URL for {repository}: {url}")
        if path.startswith(".vendor/.zed"):
            errors.append(f"submodule overlaps Zed install path: {path}")

    gitlinks = read_gitlinks()
    if declared_paths != gitlinks:
        missing_gitlinks = declared_paths - gitlinks
        undeclared_gitlinks = gitlinks - declared_paths
        if missing_gitlinks:
            errors.append("declared without gitlink: " + ", ".join(sorted(missing_gitlinks)))
        if undeclared_gitlinks:
            errors.append("gitlink without declaration: " + ", ".join(sorted(undeclared_gitlinks)))

    missing_package_submodules = REQUIRED_PACKAGE_SUBMODULES - declared_paths
    if missing_package_submodules:
        errors.append(
            "missing package source submodules: "
            + ", ".join(sorted(missing_package_submodules))
        )

    for path in declared_paths | gitlinks:
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
        f"{len(gitlinks)} non-overlapping gitlinks"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
