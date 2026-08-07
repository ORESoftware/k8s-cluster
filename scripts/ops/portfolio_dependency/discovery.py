"""Dependency graph discovery for submodules, Zed packages, and Nix flakes."""

from __future__ import annotations

import configparser
import dataclasses
import json
import tomllib
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterator, Mapping, Sequence

from .model import (
    SHA_RE,
    BotError,
    DependencyEdge,
    Repository,
    normalize_github_repo_url,
    run_command,
    strip_version_operator,
)

def gitlink_sha(worktree: Path, path: str) -> str | None:
    completed = run_command(
        ("git", "ls-files", "--stage", "--", path), cwd=worktree, check=False
    )
    for line in completed.stdout.splitlines():
        parts = line.split()
        if len(parts) >= 3 and parts[0] == "160000" and SHA_RE.fullmatch(parts[1]):
            return parts[1]
    return None


def discover_gitmodules(repository: Repository, worktree: Path) -> list[DependencyEdge]:
    path = worktree / ".gitmodules"
    if not path.is_file():
        return []
    parser = configparser.ConfigParser(interpolation=None, strict=False)
    parser.read_string(path.read_text(encoding="utf-8"))
    edges: list[DependencyEdge] = []
    for section in parser.sections():
        if not section.startswith("submodule "):
            continue
        sub_path = parser.get(section, "path", fallback="").strip()
        url = parser.get(section, "url", fallback="").strip()
        branch = parser.get(section, "branch", fallback="").strip() or None
        if not sub_path or not url:
            continue
        target_repo = normalize_github_repo_url(url, repository.owner)
        edges.append(
            DependencyEdge(
                source_repo=repository.full_name,
                source_path=".gitmodules",
                kind="git-submodule",
                dependency_key=sub_path,
                target_repo=target_repo,
                target_url=url,
                current_sha=gitlink_sha(worktree, sub_path),
                tracked_branch=branch,
                metadata={"gitlinkPath": sub_path},
            )
        )
    return edges


def _zpkg_dependency_target(key: str, value: Any, owner: str) -> tuple[str | None, str | None]:
    if "/" in key and len(key.split("/", 1)) == 2:
        return key, f"https://github.com/{key}.git"
    if isinstance(value, Mapping):
        raw = value.get("git") or value.get("url") or value.get("repository")
        if isinstance(raw, str):
            return normalize_github_repo_url(raw, owner), raw
    return None, None


def discover_zpkg(repository: Repository, worktree: Path) -> list[DependencyEdge]:
    manifest = worktree / ".zpkg.toml"
    if not manifest.is_file():
        return []
    try:
        value = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise BotError(f"cannot parse {repository.full_name}/.zpkg.toml: {exc}") from exc
    dependencies = value.get("dependencies")
    if not isinstance(dependencies, Mapping):
        return []
    edges: list[DependencyEdge] = []
    for key, spec in dependencies.items():
        target_repo, target_url = _zpkg_dependency_target(str(key), spec, repository.owner)
        current_version: str | None = None
        current_sha: str | None = None
        branch: str | None = None
        pin_field: str | None = None
        if isinstance(spec, str):
            current_version = strip_version_operator(spec)
        elif isinstance(spec, Mapping):
            raw_version = spec.get("version")
            raw_sha = None
            for candidate_field in ("rev", "sha", "commit"):
                candidate_value = spec.get(candidate_field)
                if isinstance(candidate_value, str):
                    raw_sha = candidate_value
                    pin_field = candidate_field
                    break
            raw_branch = spec.get("branch") or spec.get("ref")
            if isinstance(raw_version, str):
                current_version = strip_version_operator(raw_version)
            if isinstance(raw_sha, str):
                current_sha = raw_sha
            if isinstance(raw_branch, str):
                branch = raw_branch
        edges.append(
            DependencyEdge(
                source_repo=repository.full_name,
                source_path=".zpkg.toml",
                kind="zed-package",
                dependency_key=str(key),
                target_repo=target_repo,
                target_url=target_url,
                current_version=current_version,
                current_sha=current_sha,
                tracked_branch=branch,
                metadata={
                    "lockfilePresent": (worktree / ".zpkg.lock").is_file(),
                    "manifestForm": "scalar" if isinstance(spec, str) else "mapping",
                    "pinField": pin_field,
                },
            )
        )
    return edges


def _flake_node_reference(value: Any) -> str | None:
    """Return a concrete flake.lock node key, excluding `follows` paths."""

    return value if isinstance(value, str) and value else None


def discover_flake_lock(repository: Repository, worktree: Path) -> list[DependencyEdge]:
    lockfile = worktree / "flake.lock"
    if not lockfile.is_file():
        return []
    try:
        value = json.loads(lockfile.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise BotError(f"cannot parse {repository.full_name}/flake.lock: {exc}") from exc
    nodes = value.get("nodes") if isinstance(value, Mapping) else None
    if not isinstance(nodes, Mapping):
        return []

    root_key = value.get("root") if isinstance(value.get("root"), str) else "root"
    root_node = nodes.get(root_key)
    root_inputs = root_node.get("inputs") if isinstance(root_node, Mapping) else None
    direct_names_by_node: dict[str, list[str]] = defaultdict(list)
    if isinstance(root_inputs, Mapping):
        for input_name, reference in root_inputs.items():
            node_key = _flake_node_reference(reference)
            if node_key is not None:
                direct_names_by_node[node_key].append(str(input_name))

    edges: list[DependencyEdge] = []
    for node_key, node in nodes.items():
        if node_key == root_key or not isinstance(node, Mapping):
            continue
        locked = node.get("locked")
        original = node.get("original")
        if not isinstance(locked, Mapping):
            continue
        owner = locked.get("owner")
        repo = locked.get("repo")
        rev = locked.get("rev")
        target_repo = (
            f"{owner}/{repo}"
            if isinstance(owner, str) and isinstance(repo, str)
            else None
        )
        target_url: str | None = None
        if target_repo:
            target_url = f"https://github.com/{target_repo}.git"
        elif isinstance(locked.get("url"), str):
            target_url = str(locked["url"])
            target_repo = normalize_github_repo_url(target_url, repository.owner)
        branch = original.get("ref") if isinstance(original, Mapping) else None
        if branch is not None and not isinstance(branch, str):
            branch = None
        if not (target_repo or target_url):
            continue

        direct_names = direct_names_by_node.get(str(node_key), [])
        if direct_names:
            for input_name in direct_names:
                edges.append(
                    DependencyEdge(
                        source_repo=repository.full_name,
                        source_path="flake.lock",
                        kind="nix-flake",
                        dependency_key=input_name,
                        target_repo=target_repo,
                        target_url=target_url,
                        current_sha=str(rev) if isinstance(rev, str) else None,
                        tracked_branch=branch,
                        input_name=input_name,
                        metadata={
                            "lockedType": locked.get("type"),
                            "lockNode": str(node_key),
                            "directInput": True,
                        },
                    )
                )
        else:
            edges.append(
                DependencyEdge(
                    source_repo=repository.full_name,
                    source_path="flake.lock",
                    kind="nix-flake-transitive",
                    dependency_key=str(node_key),
                    target_repo=target_repo,
                    target_url=target_url,
                    current_sha=str(rev) if isinstance(rev, str) else None,
                    tracked_branch=branch,
                    metadata={
                        "lockedType": locked.get("type"),
                        "lockNode": str(node_key),
                        "directInput": False,
                        "graphOnly": True,
                    },
                )
            )
    return edges


def _walk_lock_entries(value: Any, trail: tuple[str, ...] = ()) -> Iterator[tuple[tuple[str, ...], Mapping[str, Any]]]:
    if isinstance(value, Mapping):
        lowered = {str(key).lower() for key in value}
        if lowered & {"version", "rev", "sha", "commit"} and lowered & {
            "name",
            "package",
            "dependency",
            "repository",
            "url",
            "git",
        }:
            yield trail, value
        for key, child in value.items():
            yield from _walk_lock_entries(child, trail + (str(key),))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from _walk_lock_entries(child, trail + (str(index),))


def _read_zpkg_lock(worktree: Path) -> Any | None:
    lockfile = worktree / ".zpkg.lock"
    if not lockfile.is_file():
        return None
    raw = lockfile.read_text(encoding="utf-8")
    try:
        return tomllib.loads(raw)
    except tomllib.TOMLDecodeError:
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return None


def _zpkg_lock_name(trail: tuple[str, ...], item: Mapping[str, Any]) -> str:
    raw = item.get("name") or item.get("package") or item.get("dependency")
    if isinstance(raw, str) and raw:
        return raw
    return "/".join(trail[-2:])


def enrich_zpkg_from_lock(
    repository: Repository,
    worktree: Path,
    existing: Sequence[DependencyEdge],
) -> list[DependencyEdge]:
    """Fill a direct Git dependency's current pin from `.zpkg.lock` when possible."""

    parsed = _read_zpkg_lock(worktree)
    if parsed is None:
        return list(existing)
    lock_entries: list[tuple[tuple[str, ...], Mapping[str, Any]]] = list(
        _walk_lock_entries(parsed)
    )
    enriched: list[DependencyEdge] = []
    for edge in existing:
        if edge.current_sha or edge.current_version:
            enriched.append(edge)
            continue
        match: tuple[tuple[str, ...], Mapping[str, Any]] | None = None
        for trail, item in lock_entries:
            name = _zpkg_lock_name(trail, item)
            raw_url = item.get("git") or item.get("url") or item.get("repository")
            target_repo = (
                normalize_github_repo_url(str(raw_url), repository.owner)
                if isinstance(raw_url, str)
                else None
            )
            if name == edge.dependency_key or (
                edge.target_repo is not None and target_repo == edge.target_repo
            ):
                match = (trail, item)
                break
        if match is None:
            enriched.append(edge)
            continue
        trail, item = match
        sha = item.get("rev") or item.get("sha") or item.get("commit")
        version = item.get("version")
        metadata = dict(edge.metadata)
        metadata["lockTrail"] = list(trail)
        enriched.append(
            dataclasses.replace(
                edge,
                current_sha=str(sha) if isinstance(sha, str) else None,
                current_version=(
                    strip_version_operator(str(version))
                    if isinstance(version, str)
                    else None
                ),
                metadata=metadata,
            )
        )
    return enriched


def discover_zpkg_lock_only(repository: Repository, worktree: Path, existing: Sequence[DependencyEdge]) -> list[DependencyEdge]:
    parsed = _read_zpkg_lock(worktree)
    if parsed is None:
        return []
    known = {edge.dependency_key for edge in existing if edge.kind == "zed-package"}
    edges: list[DependencyEdge] = []
    for trail, item in _walk_lock_entries(parsed):
        name = _zpkg_lock_name(trail, item)
        if not isinstance(name, str) or name in known:
            continue
        raw_url = item.get("git") or item.get("url") or item.get("repository")
        target_url = raw_url if isinstance(raw_url, str) else None
        target_repo = normalize_github_repo_url(target_url, repository.owner) if target_url else (
            name if "/" in name else None
        )
        version = item.get("version")
        sha = item.get("rev") or item.get("sha") or item.get("commit")
        edges.append(
            DependencyEdge(
                source_repo=repository.full_name,
                source_path=".zpkg.lock",
                kind="zed-package-lock-only",
                dependency_key=name,
                target_repo=target_repo,
                target_url=target_url,
                current_version=str(version) if isinstance(version, str) else None,
                current_sha=str(sha) if isinstance(sha, str) else None,
                metadata={"trail": list(trail), "graphOnly": True},
            )
        )
    return edges


def discover_edges(repository: Repository, worktree: Path) -> list[DependencyEdge]:
    edges = discover_gitmodules(repository, worktree)
    zpkg = enrich_zpkg_from_lock(repository, worktree, discover_zpkg(repository, worktree))
    edges.extend(zpkg)
    edges.extend(discover_zpkg_lock_only(repository, worktree, zpkg))
    edges.extend(discover_flake_lock(repository, worktree))
    return edges


def detect_profile(worktree: Path) -> str | None:
    override = worktree / ".portfolio-dependency-bot.json"
    if override.is_file():
        try:
            value = json.loads(override.read_text(encoding="utf-8"))
            profile = value.get("profile") if isinstance(value, Mapping) else None
            if isinstance(profile, str) and profile:
                return profile
        except (OSError, json.JSONDecodeError):
            pass
    if (worktree / "flake.nix").is_file():
        return "nix-verify"
    if (worktree / "Cargo.toml").is_file():
        return "rust-verify"
    if (worktree / "pubspec.yaml").is_file():
        return "flutter-verify"
    if any((worktree / name).is_file() for name in ("pnpm-lock.yaml", "yarn.lock", "package-lock.json", "npm-shrinkwrap.json")):
        return "node-verify"
    if any((worktree / name).is_file() for name in ("pyproject.toml", "requirements.txt")):
        return "python-verify"
    return None
