#!/usr/bin/env python3
"""Validate git-submodule portability and Zed dependency boundaries."""

from __future__ import annotations

import configparser
import pathlib
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from urllib.parse import urlparse

ROOT = pathlib.Path(__file__).resolve().parents[1]
GITHUB_SCP = re.compile(r"^git@github\.com:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?$")
RELATIVE_GIT = re.compile(r"^\.\.?/[A-Za-z0-9_.-]+(?:\.git)?$")


@dataclass(frozen=True)
class Submodule:
    name: str
    path: str
    url: str
    branch: str | None
    update: str | None


def load_toml(path: pathlib.Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def parse_gitmodules(path: pathlib.Path) -> list[Submodule]:
    parser = configparser.ConfigParser(interpolation=None, strict=True)
    parser.optionxform = str
    with path.open(encoding="utf-8") as handle:
        parser.read_file(handle)
    records: list[Submodule] = []
    for section in parser.sections():
        if not section.startswith('submodule "') or not section.endswith('"'):
            raise ValueError(f"invalid .gitmodules section: {section}")
        name = section[len('submodule "') : -1]
        records.append(
            Submodule(
                name=name,
                path=parser.get(section, "path"),
                url=parser.get(section, "url"),
                branch=parser.get(section, "branch", fallback=None),
                update=parser.get(section, "update", fallback=None),
            )
        )
    return records


def repository_leaf(url: str) -> str:
    value = url.rstrip("/")
    if value.endswith(".git"):
        value = value[:-4]
    if ":" in value and not value.startswith(("http://", "https://")):
        value = value.rsplit(":", 1)[-1]
    return value.rsplit("/", 1)[-1]


def portable_path(value: str) -> bool:
    path = pathlib.PurePosixPath(value)
    return bool(value) and not path.is_absolute() and ".." not in path.parts and "." not in path.parts


def portable_url(value: str) -> bool:
    if RELATIVE_GIT.fullmatch(value) or GITHUB_SCP.fullmatch(value):
        return True
    parsed = urlparse(value)
    return parsed.scheme == "https" and parsed.netloc == "github.com" and bool(parsed.path.strip("/"))


def classify(leaf: str, policy: dict) -> tuple[str, bool]:
    for rule in policy.get("rule", []):
        suffix = str(rule.get("suffix", ""))
        if suffix and leaf.endswith(suffix):
            return str(rule.get("classification", "source")), bool(rule.get("zed_dependency", True))
    defaults = policy.get("defaults", {})
    return str(defaults.get("classification", "source")), bool(defaults.get("zed_dependency", True))


def main() -> int:
    errors: list[str] = []
    gitmodules_path = ROOT / ".gitmodules"
    if not gitmodules_path.is_file():
        print("error: fiducia-monorepo must keep a checked-in .gitmodules file", file=sys.stderr)
        return 1

    try:
        submodules = parse_gitmodules(gitmodules_path)
    except (OSError, configparser.Error, ValueError) as exc:
        print(f"error: invalid .gitmodules: {exc}", file=sys.stderr)
        return 1
    if not submodules:
        errors.append(".gitmodules contains no submodules")

    manifest = load_toml(ROOT / ".zpkg.toml")
    lock = load_toml(ROOT / ".zpkg.lock")
    policy = load_toml(ROOT / "submodule-policy.toml")
    package = manifest.get("package", {})
    dependencies = manifest.get("dependencies", {})

    if package.get("org") != "fiducia-cloud" or package.get("name") != "fiducia-monorepo":
        errors.append("package identity must be fiducia-cloud/fiducia-monorepo")
    if manifest.get("package", {}).get("repository", {}).get("url") != "https://github.com/fiducia-cloud/fiducia-monorepo":
        errors.append("package.repository.url must match the canonical repository")
    if lock.get("version") != 1:
        errors.append(".zpkg.lock must use version = 1")
    if not isinstance(dependencies, dict):
        errors.append("[dependencies] must be a table")
        dependencies = {}

    names: set[str] = set()
    paths: set[str] = set()
    leaves: set[str] = set()
    for submodule in submodules:
        leaf = repository_leaf(submodule.url)
        classification, should_depend = classify(leaf, policy)
        print(f"{submodule.path}: {leaf} -> {classification}; zed_dependency={str(should_depend).lower()}")

        if submodule.name in names:
            errors.append(f"duplicate submodule name: {submodule.name}")
        names.add(submodule.name)
        if submodule.path in paths:
            errors.append(f"duplicate submodule path: {submodule.path}")
        paths.add(submodule.path)
        if leaf in leaves:
            errors.append(f"duplicate submodule repository: {leaf}")
        leaves.add(leaf)

        if not portable_path(submodule.path):
            errors.append(f"non-portable submodule path: {submodule.path}")
        if "\\" in submodule.path:
            errors.append(f"submodule paths must use POSIX separators: {submodule.path}")
        if not portable_url(submodule.url):
            errors.append(f"submodule URL must be HTTPS, GitHub SSH, or a portable relative URL: {submodule.url}")
        if submodule.branch == ".":
            errors.append(f"submodule {submodule.name} may not inherit a branch with branch = .")
        if submodule.update in {"!", "command"} or (submodule.update and submodule.update.startswith("!")):
            errors.append(f"submodule {submodule.name} may not execute a custom update command")

        coordinate = f"fiducia-cloud/{leaf}"
        if should_depend and leaf in {"fiducia-interfaces", "fiducia-clients"} and coordinate not in dependencies:
            errors.append(f"source submodule {leaf} must be represented in [dependencies]")
        if not should_depend and coordinate in dependencies:
            errors.append(f"{classification} submodule {leaf} must not be a Zed dependency")

    forbidden = [name for name in dependencies if name.rsplit("/", 1)[-1].endswith(("-cli", "-infra"))]
    if forbidden:
        errors.append("monorepo may not import CLI or infra packages: " + ", ".join(sorted(forbidden)))

    if "fiducia-cloud/fiducia-interfaces" not in dependencies:
        errors.append("monorepo must import fiducia-interfaces")
    if "fiducia-cloud/fiducia-clients" not in dependencies:
        errors.append("monorepo must import fiducia-clients")

    try:
        status = subprocess.run(
            ["git", "submodule", "status", "--recursive"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as exc:
        errors.append(f"could not execute git: {exc}")
    else:
        if status.returncode != 0:
            errors.append("git submodule status --recursive failed: " + status.stderr.strip())
        for line in status.stdout.splitlines():
            if line.startswith("-"):
                errors.append("uninitialized recursive submodule: " + line[1:].strip())
            elif line.startswith("U"):
                errors.append("submodule has merge conflicts: " + line[1:].strip())

    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"validated {len(submodules)} portable recursive submodules and the Zed boundary")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
