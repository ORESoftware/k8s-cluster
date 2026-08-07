"""Exact, bounded dependency mutation adapters."""

from __future__ import annotations

import json
import os
import re
import shutil
from pathlib import Path
from typing import Mapping

from .discovery import _flake_node_reference
from .model import (
    SHA_RE,
    BotError,
    Candidate,
    DependencyEdge,
    MutationUnsupported,
    SemVer,
    run_command,
)

def replace_zpkg_version(path: Path, key: str, new_version: SemVer) -> None:
    content = path.read_text(encoding="utf-8")
    escaped = re.escape(key)
    pattern = re.compile(
        rf'(?m)^(?P<prefix>\s*(?:"{escaped}"|\'{escaped}\'|{escaped})\s*=\s*["\'])(?P<op>[\^~]?)(?P<version>\d+\.\d+\.\d+)(?P<suffix>["\']\s*(?:#.*)?)$'
    )
    matches = list(pattern.finditer(content))
    if len(matches) != 1:
        raise MutationUnsupported(
            f"expected exactly one scalar .zpkg.toml entry for {key!r}; found {len(matches)}"
        )
    match = matches[0]
    replacement = (
        match.group("prefix")
        + match.group("op")
        + str(new_version)
        + match.group("suffix")
    )
    path.write_text(content[: match.start()] + replacement + content[match.end() :], encoding="utf-8")


def replace_zpkg_git_pin(path: Path, key: str, new_sha: str) -> None:
    """Replace exactly one rev/sha/commit field for a mapped Zed dependency.

    The writer supports the two reviewable TOML shapes used by the fleet: an inline
    table under `[dependencies]`, or a dedicated `[dependencies.<key>]` table. It
    refuses ambiguous or multiline inline-table rewrites rather than reserializing
    unrelated TOML.
    """

    if not SHA_RE.fullmatch(new_sha):
        raise MutationUnsupported("Zed Git candidate is not a full hexadecimal commit")
    content = path.read_text(encoding="utf-8")
    escaped = re.escape(key)
    key_pattern = rf'(?:"{escaped}"|\'{escaped}\'|{escaped})'
    replacements: list[tuple[int, int, str]] = []

    inline_pattern = re.compile(
        rf'(?m)^(?P<prefix>\s*{key_pattern}\s*=\s*\{{)(?P<body>[^\n}}]*)(?P<suffix>\}}\s*(?:#.*)?)$'
    )
    inline_pin = re.compile(
        r'(?P<prefix>\b(?:rev|sha|commit)\s*=\s*["\'])(?P<sha>[0-9a-fA-F]{7,40})(?P<suffix>["\'])'
    )
    for match in inline_pattern.finditer(content):
        body = match.group("body")
        pins = list(inline_pin.finditer(body))
        if len(pins) != 1:
            continue
        pin = pins[0]
        new_body = body[: pin.start("sha")] + new_sha + body[pin.end("sha") :]
        replacements.append(
            (
                match.start(),
                match.end(),
                match.group("prefix") + new_body + match.group("suffix"),
            )
        )

    header_pattern = re.compile(
        rf'(?m)^\s*\[dependencies\.{key_pattern}\]\s*(?:#.*)?$'
    )
    next_header_pattern = re.compile(r'(?m)^\s*\[')
    table_pin = re.compile(
        r'(?m)^(?P<prefix>\s*(?:rev|sha|commit)\s*=\s*["\'])(?P<sha>[0-9a-fA-F]{7,40})(?P<suffix>["\']\s*(?:#.*)?)$'
    )
    for header in header_pattern.finditer(content):
        block_start = header.end()
        next_header = next_header_pattern.search(content, block_start)
        block_end = next_header.start() if next_header else len(content)
        block = content[block_start:block_end]
        pins = list(table_pin.finditer(block))
        if len(pins) != 1:
            continue
        pin = pins[0]
        absolute_start = block_start + pin.start("sha")
        absolute_end = block_start + pin.end("sha")
        replacements.append((absolute_start, absolute_end, new_sha))

    if len(replacements) != 1:
        raise MutationUnsupported(
            f"expected exactly one structured Git pin for {key!r}; found {len(replacements)}"
        )
    start, end, replacement = replacements[0]
    path.write_text(content[:start] + replacement + content[end:], encoding="utf-8")


def regenerate_zpkg_lock(worktree: Path) -> None:
    if not (worktree / ".zpkg.lock").exists():
        return
    zed = os.environ.get("ZED_BIN") or shutil.which("zed")
    if not zed:
        raise MutationUnsupported(".zpkg.lock exists but no pinned zed CLI is available")
    before = run_command(("git", "status", "--porcelain"), cwd=worktree).stdout.splitlines()
    unexpected_before = []
    for line in before:
        candidate = line[3:].strip() if len(line) >= 4 else ""
        if candidate != ".zpkg.toml":
            unexpected_before.append(candidate)
    if unexpected_before:
        raise BotError(
            "only .zpkg.toml may be dirty before zed lock regeneration: "
            + ", ".join(unexpected_before[:20])
        )
    env = dict(os.environ)
    env.update(
        {
            "ZED_PKG_INTERACTIVE": "false",
            "ZED_PKG_ALLOW_BUILD": "false",
            "ZED_PKG_INSTALL_MODE": "copy",
            "GIT_TERMINAL_PROMPT": "0",
        }
    )
    completed = run_command(
        (zed, "install", "--do-not-write-new-manifest"),
        cwd=worktree,
        env=env,
        timeout=1800,
        check=False,
    )
    if completed.returncode != 0:
        raise MutationUnsupported(f"zed lock regeneration failed: {completed.stdout[-5000:]}")
    changed = run_command(("git", "status", "--porcelain"), cwd=worktree).stdout.splitlines()
    unexpected = []
    for line in changed:
        candidate = line[3:].strip() if len(line) >= 4 else ""
        if candidate not in {".zpkg.toml", ".zpkg.lock"}:
            unexpected.append(candidate)
    if unexpected:
        raise MutationUnsupported(
            "zed install changed files outside the manifest/lock pair: " + ", ".join(unexpected[:20])
        )


def update_flake_lock(worktree: Path, edge: DependencyEdge, candidate: Candidate) -> None:
    if not edge.input_name or not edge.target_repo or not candidate.git_sha:
        raise MutationUnsupported("Nix candidate lacks input name, GitHub repository, or commit")
    nix = shutil.which("nix")
    if not nix:
        raise MutationUnsupported("Nix is unavailable for deterministic flake.lock regeneration")
    flake_url = f"github:{edge.target_repo}/{candidate.git_sha}"
    completed = run_command(
        (
            nix,
            "--extra-experimental-features",
            "nix-command flakes",
            "flake",
            "lock",
            "--override-input",
            edge.input_name,
            flake_url,
        ),
        cwd=worktree,
        timeout=1800,
        check=False,
    )
    if completed.returncode != 0:
        raise MutationUnsupported(f"nix flake lock failed: {completed.stdout[-5000:]}")
    value = json.loads((worktree / "flake.lock").read_text(encoding="utf-8"))
    nodes = value.get("nodes") if isinstance(value, Mapping) else None
    root_key = value.get("root") if isinstance(value, Mapping) else None
    root_node = nodes.get(root_key) if isinstance(nodes, Mapping) and isinstance(root_key, str) else None
    root_inputs = root_node.get("inputs") if isinstance(root_node, Mapping) else None
    node_key = (
        _flake_node_reference(root_inputs.get(edge.input_name))
        if isinstance(root_inputs, Mapping)
        else None
    )
    node = nodes.get(node_key) if isinstance(nodes, Mapping) and node_key else None
    actual = node.get("locked", {}).get("rev") if isinstance(node, Mapping) else None
    if actual != candidate.git_sha:
        raise MutationUnsupported(
            f"flake.lock did not pin {edge.input_name} to requested {candidate.git_sha}"
        )


def apply_candidate(worktree: Path, edge: DependencyEdge, candidate: Candidate) -> None:
    if edge.kind == "git-submodule":
        path = str(edge.metadata.get("gitlinkPath", edge.dependency_key))
        if not candidate.git_sha:
            raise MutationUnsupported("submodule candidate has no commit SHA")
        run_command(
            ("git", "update-index", "--add", "--cacheinfo", f"160000,{candidate.git_sha},{path}"),
            cwd=worktree,
        )
        return
    if edge.kind == "zed-package":
        if candidate.version is not None:
            replace_zpkg_version(worktree / ".zpkg.toml", edge.dependency_key, candidate.version)
        elif candidate.git_sha is not None:
            replace_zpkg_git_pin(worktree / ".zpkg.toml", edge.dependency_key, candidate.git_sha)
        else:
            raise MutationUnsupported("Zed dependency candidate has neither a version nor Git commit")
        if bool(edge.metadata.get("lockfilePresent")):
            regenerate_zpkg_lock(worktree)
        return
    if edge.kind == "nix-flake":
        update_flake_lock(worktree, edge, candidate)
        return
    raise MutationUnsupported(f"no safe mutation adapter for dependency kind {edge.kind}")
