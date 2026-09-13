"""Dependency graph scanners and remote stable-version resolution."""

from __future__ import annotations

import argparse
import base64
import configparser
import csv
import dataclasses
import datetime as dt
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict

from .models import *
from .runtime import *

def scan_zpkg_manifest(path: Path, root: Path, repository: str) -> list[DependencyRef]:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise StewardError(f"cannot parse {relpath(path, root)}: {exc}") from exc
    dependencies = data.get("dependencies") or {}
    if not isinstance(dependencies, dict):
        return []
    found: list[DependencyRef] = []
    for name, spec in dependencies.items():
        version_expr: str | None = None
        source: str | None = None
        if isinstance(spec, str):
            version_expr = spec
        elif isinstance(spec, dict):
            raw = spec.get("version")
            version_expr = raw if isinstance(raw, str) else None
            for key in ("git", "url", "repository"):
                raw_source = spec.get(key)
                if isinstance(raw_source, str):
                    source = raw_source
                    break
        if not version_expr:
            continue
        coordinate = str(name)
        source = source or (
            github_url(coordinate) if GITHUB_REPO_RE.fullmatch(coordinate) else coordinate
        )
        found.append(
            DependencyRef(
                repository=repository,
                kind="zpkg",
                key=f"zpkg:{coordinate}",
                name=coordinate,
                source_url=source,
                manifest_path=relpath(path, root),
                current_ref=version_expr,
                current_version=SemVer.parse(version_expr),
                locator={"dependency": coordinate, "version_expr": version_expr},
            )
        )
    return found


def _walk_lock_values(value: Any) -> Iterable[dict[str, Any]]:
    if isinstance(value, dict):
        if isinstance(value.get("version"), str) and any(
            isinstance(value.get(key), str)
            for key in ("name", "package", "coordinate", "url", "repository")
        ):
            yield value
        for child in value.values():
            yield from _walk_lock_values(child)
    elif isinstance(value, list):
        for child in value:
            yield from _walk_lock_values(child)


def scan_zpkg_lock(path: Path, root: Path, repository: str) -> list[DependencyRef]:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise StewardError(f"cannot parse {relpath(path, root)}: {exc}") from exc
    found: list[DependencyRef] = []
    for entry in _walk_lock_values(data):
        name = next(
            (
                str(entry[key])
                for key in ("name", "package", "coordinate")
                if isinstance(entry.get(key), str)
            ),
            "unknown-zpkg-lock-entry",
        )
        source = next(
            (
                str(entry[key])
                for key in ("url", "repository")
                if isinstance(entry.get(key), str)
            ),
            github_url(name) if GITHUB_REPO_RE.fullmatch(name) else name,
        )
        version = str(entry.get("version"))
        found.append(
            DependencyRef(
                repository=repository,
                kind="zpkg-lock",
                key=f"zpkg:{name}",
                name=name,
                source_url=source,
                manifest_path=relpath(path, root),
                current_ref=str(entry.get("rev") or version),
                current_version=SemVer.parse(version),
                mutable=False,
                note="lock evidence; updated through its .zpkg.toml resolver run",
            )
        )
    return found


def scan_flake_lock(path: Path, root: Path, repository: str) -> list[DependencyRef]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise StewardError(f"cannot parse {relpath(path, root)}: {exc}") from exc
    nodes = data.get("nodes") or {}
    if not isinstance(nodes, dict):
        return []
    found: list[DependencyRef] = []
    for input_name, node in nodes.items():
        if not isinstance(node, dict):
            continue
        locked = node.get("locked") or {}
        if not isinstance(locked, dict) or locked.get("type") != "github":
            continue
        owner, repo = locked.get("owner"), locked.get("repo")
        rev = locked.get("rev")
        if not all(isinstance(item, str) for item in (owner, repo, rev)):
            continue
        coordinate = f"{owner}/{repo}"
        found.append(
            DependencyRef(
                repository=repository,
                kind="nix-flake",
                key=f"nix:{relpath(path, root)}:{input_name}:{coordinate}",
                name=coordinate,
                source_url=github_url(coordinate),
                manifest_path=relpath(path, root),
                current_ref=rev,
                locator={"input": str(input_name)},
            )
        )
    return found


FETCH_GITHUB_RE = re.compile(
    r"fetchFromGitHub\s*\{(?P<body>.{0,3000}?)\}", re.DOTALL
)
NIX_STRING_RE = re.compile(r"\b(owner|repo|rev)\s*=\s*\"([^\"]+)\"\s*;")
FLAKE_URL_RE = re.compile(
    r"github:(?P<owner>[A-Za-z0-9_.-]+)/(?P<repo>[A-Za-z0-9_.-]+)"
    r"(?:/(?P<ref>[^\s\"';)]+))?"
)


def scan_nix_manifest(path: Path, root: Path, repository: str) -> list[DependencyRef]:
    text = path.read_text(encoding="utf-8", errors="replace")
    found: list[DependencyRef] = []
    relative = relpath(path, root)
    for index, match in enumerate(FETCH_GITHUB_RE.finditer(text)):
        values = {key: value for key, value in NIX_STRING_RE.findall(match.group("body"))}
        if not all(key in values for key in ("owner", "repo", "rev")):
            continue
        coordinate = f"{values['owner']}/{values['repo']}"
        found.append(
            DependencyRef(
                repository=repository,
                kind="nix-expression",
                key=f"nix:{relative}:fetchFromGitHub:{index}:{coordinate}",
                name=coordinate,
                source_url=github_url(coordinate),
                manifest_path=relative,
                current_ref=values["rev"],
                current_version=SemVer.parse(values["rev"]),
                mutable=False,
                note=(
                    "generic Nix fetchers are graph-only unless the repository "
                    "provides a dependency_steward update command"
                ),
            )
        )
    for index, match in enumerate(FLAKE_URL_RE.finditer(text)):
        coordinate = f"{match.group('owner')}/{match.group('repo')}"
        found.append(
            DependencyRef(
                repository=repository,
                kind="nix-expression",
                key=f"nix:{relative}:github-url:{index}:{coordinate}",
                name=coordinate,
                source_url=github_url(coordinate),
                manifest_path=relative,
                current_ref=match.group("ref"),
                current_version=SemVer.parse(match.group("ref")),
                mutable=False,
                note="graph-only URL; prefer flake.lock for automated changes",
            )
        )
    return found


def scan_gitmodules(
    path: Path, root: Path, repository: str
) -> list[DependencyRef]:
    parser = configparser.ConfigParser(interpolation=None)
    parser.read(path, encoding="utf-8")
    found: list[DependencyRef] = []
    prefix = path.parent.relative_to(root)
    for section in parser.sections():
        if not section.startswith('submodule "'):
            continue
        module_path = parser.get(section, "path", fallback="").strip()
        url = parser.get(section, "url", fallback="").strip()
        if not module_path or not url:
            continue
        full_path = (prefix / module_path).as_posix()
        listing = run_process(
            ["git", "ls-tree", "HEAD", "--", full_path], cwd=root, check=False
        ).stdout.strip()
        sha = None
        if listing:
            match = re.match(r"160000\s+commit\s+([0-9a-f]{40})\t", listing)
            if match:
                sha = match.group(1)
        source = resolve_submodule_url(repository, url)
        coordinate = canonical_github_repo(source) or source
        found.append(
            DependencyRef(
                repository=repository,
                kind="git-submodule",
                key=f"submodule:{full_path}:{coordinate}",
                name=coordinate,
                source_url=source,
                manifest_path=relpath(path, root),
                current_ref=sha,
                locator={"path": full_path},
                mutable=sha is not None,
                note=None if sha else "gitlink SHA could not be resolved",
            )
        )
    return found


def scan_repository(root: Path, repository: str) -> list[DependencyRef]:
    edges: list[DependencyRef] = []
    for path in iter_manifest_paths(root):
        if path.name == ".zpkg.toml":
            edges.extend(scan_zpkg_manifest(path, root, repository))
        elif path.name == ".zpkg.lock":
            edges.extend(scan_zpkg_lock(path, root, repository))
        elif path.name == "flake.lock":
            edges.extend(scan_flake_lock(path, root, repository))
        elif path.name == ".gitmodules":
            edges.extend(scan_gitmodules(path, root, repository))
        elif path.suffix == ".nix":
            edges.extend(scan_nix_manifest(path, root, repository))
    # Lock entries duplicate manifest edges by design. Keep them in the graph but
    # never attempt a second update for the same lock authority.
    return edges


def remote_versions(
    source_url: str, *, token: str, timeout: int = 180
) -> list[RemoteVersion]:
    command = ["git", *git_auth_config(token), "ls-remote", "--tags", source_url]
    result = run_process(command, timeout=timeout, check=False)
    if result.returncode:
        return []
    raw: dict[str, dict[str, str]] = defaultdict(dict)
    for line in result.stdout.splitlines():
        fields = line.split("\t", 1)
        if len(fields) != 2 or not SHA_RE.fullmatch(fields[0]):
            continue
        ref = fields[1]
        if not ref.startswith("refs/tags/"):
            continue
        tag = ref.removeprefix("refs/tags/")
        if tag.endswith("^{}"):
            raw[tag[:-3]]["peeled"] = fields[0]
        else:
            raw[tag]["object"] = fields[0]
    by_version: dict[SemVer, RemoteVersion] = {}
    for tag, shas in raw.items():
        version = SemVer.parse(tag)
        # SemVer.parse searches substrings; require the whole conventional tag.
        if version is None or tag not in {str(version), f"v{version}"}:
            continue
        sha = shas.get("peeled") or shas.get("object")
        if not sha:
            continue
        item = RemoteVersion(version, tag, sha)
        existing = by_version.get(version)
        if existing is None or (tag.startswith("v") and not existing.tag.startswith("v")):
            by_version[version] = item
    return sorted(by_version.values(), key=lambda item: item.version)


def resolve_current_version(
    dep: DependencyRef, versions: Sequence[RemoteVersion]
) -> SemVer | None:
    if dep.current_version:
        return dep.current_version
    if dep.current_ref:
        parsed = SemVer.parse(dep.current_ref)
        if parsed:
            return parsed
        matches = [item.version for item in versions if item.sha == dep.current_ref]
        if matches:
            return max(matches)
    return None
