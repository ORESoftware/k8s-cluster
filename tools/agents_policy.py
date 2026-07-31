#!/usr/bin/env python3
"""Resolve and validate hierarchical lowercase agents.md instruction files."""

from __future__ import annotations

import argparse
import errno
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

POINTER_TEXT = "Read and follow the canonical instructions in `../agents.md`.\n"
POINTERS = (
    Path(".claude/CLAUDE.md"),
    Path(".gemini/GEMINI.md"),
    Path(".openai/AGENTS.md"),
)
ROOT_DUPLICATES = (
    Path("AGENTS.md"),
    Path("CLAUDE.md"),
    Path("GEMINI.md"),
)
IGNORED_DIRECTORY_NAMES = {
    ".git",
    ".hg",
    ".jj",
    ".svn",
    ".venv",
    "build",
    "dist",
    "node_modules",
    "target",
    "vendor",
    "zed_modules",
}


@dataclass(frozen=True)
class Resolution:
    chain: tuple[Path, ...]
    diagnostics: tuple[str, ...]


@dataclass(frozen=True)
class Validation:
    chains: tuple[tuple[Path, ...], ...]
    issues: tuple[str, ...]


def _candidate_exists(path: Path) -> bool:
    return path.exists() or path.is_symlink()


def _readable_file(path: Path) -> tuple[bool, str | None]:
    if not path.is_file():
        return False, f"not a regular file: {path}"
    if not os.access(path, os.R_OK):
        return False, f"unreadable agents file: {path}"
    try:
        with path.open("rb") as handle:
            handle.read(1)
    except OSError as error:
        return False, f"cannot read agents file {path}: {error}"
    return True, None


def resolve_chain(start: Path) -> Resolution:
    """Collect readable ancestor agents.md files root-to-leaf.

    Only the current path and its ancestors are considered. Resolved
    device/inode identities are deduplicated, which handles symlink aliases and
    hardlinks without searching siblings. Broken/cyclic/unreadable files are
    reported and skipped.
    """

    try:
        resolved_start = start.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        return Resolution((), (f"cannot resolve start path {start}: {error}",))
    if resolved_start.is_file():
        resolved_start = resolved_start.parent
    if not resolved_start.is_dir():
        return Resolution((), (f"start path is not a directory: {resolved_start}",))

    directories = tuple(reversed((resolved_start, *resolved_start.parents)))
    chain: list[Path] = []
    diagnostics: list[str] = []
    seen: set[tuple[int, int] | tuple[str, str]] = set()

    for directory in directories:
        candidate = directory / "agents.md"
        if not _candidate_exists(candidate):
            continue
        try:
            resolved = candidate.resolve(strict=True)
        except RuntimeError as error:
            diagnostics.append(f"agents symlink cycle at {candidate}: {error}")
            continue
        except OSError as error:
            if error.errno == errno.ELOOP:
                diagnostics.append(f"agents symlink cycle at {candidate}: {error}")
            else:
                diagnostics.append(f"cannot resolve agents file {candidate}: {error}")
            continue

        readable, issue = _readable_file(resolved)
        if not readable:
            diagnostics.append(issue or f"unreadable agents file: {resolved}")
            continue

        try:
            metadata = resolved.stat()
            identity: tuple[int, int] | tuple[str, str] = (
                metadata.st_dev,
                metadata.st_ino,
            )
        except OSError:
            identity = ("path", os.path.normcase(str(resolved)))

        if identity in seen:
            continue
        seen.add(identity)
        chain.append(resolved)

    return Resolution(tuple(chain), tuple(diagnostics))


def _iter_agents_files(repo: Path) -> Iterable[Path]:
    for current, directories, files in os.walk(repo, followlinks=False):
        directories[:] = [
            name for name in directories if name not in IGNORED_DIRECTORY_NAMES
        ]
        if "agents.md" in files:
            yield Path(current) / "agents.md"


def _validate_pointer(repo: Path, relative: Path) -> list[str]:
    path = repo / relative
    issues: list[str] = []
    if not _candidate_exists(path):
        return [f"missing tool pointer: {relative.as_posix()}"]

    if path.is_symlink():
        try:
            target = os.readlink(path)
        except OSError as error:
            return [f"cannot read pointer symlink {relative.as_posix()}: {error}"]
        if target != "../agents.md":
            issues.append(
                f"{relative.as_posix()} must target ../agents.md, found {target!r}"
            )
        try:
            resolved = path.resolve(strict=True)
        except (OSError, RuntimeError) as error:
            issues.append(f"broken/cyclic pointer {relative.as_posix()}: {error}")
        else:
            if resolved != (repo / "agents.md").resolve(strict=True):
                issues.append(
                    f"{relative.as_posix()} resolves outside root agents.md: {resolved}"
                )
        return issues

    if not path.is_file():
        return [f"tool pointer is not a regular file or symlink: {relative.as_posix()}"]
    try:
        content = path.read_text(encoding="utf-8")
    except OSError as error:
        return [f"cannot read tool pointer {relative.as_posix()}: {error}"]
    if content != POINTER_TEXT:
        issues.append(
            f"{relative.as_posix()} must contain only the canonical one-line pointer"
        )
    if len(content.encode("utf-8")) > 128:
        issues.append(f"{relative.as_posix()} duplicates instructions instead of pointing")
    return issues


def validate_repo(repo: Path) -> Validation:
    try:
        repo = repo.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        return Validation((), (f"cannot resolve repository {repo}: {error}",))
    issues: list[str] = []
    chains: list[tuple[Path, ...]] = []
    root_agents = repo / "agents.md"

    if not root_agents.exists():
        issues.append("missing lowercase root agents.md")
    elif root_agents.is_symlink() or not root_agents.is_file():
        issues.append("root agents.md must be a regular, non-symlink file")
    else:
        try:
            content = root_agents.read_text(encoding="utf-8")
        except OSError as error:
            issues.append(f"cannot read root agents.md: {error}")
        else:
            if not content.strip():
                issues.append("root agents.md is empty")

    for duplicate in ROOT_DUPLICATES:
        if _candidate_exists(repo / duplicate):
            issues.append(
                f"duplicate root instruction file is forbidden: {duplicate.as_posix()}"
            )

    for pointer in POINTERS:
        issues.extend(_validate_pointer(repo, pointer))

    root_resolved: Path | None
    try:
        root_resolved = root_agents.resolve(strict=True)
    except (OSError, RuntimeError):
        root_resolved = None

    discovered = sorted(_iter_agents_files(repo))
    if root_agents.exists() and root_agents not in discovered:
        discovered.insert(0, root_agents)
    for agents_file in discovered:
        resolution = resolve_chain(agents_file.parent)
        chains.append(resolution.chain)
        issues.extend(resolution.diagnostics)
        if root_resolved is not None:
            if not resolution.chain:
                issues.append(
                    f"empty hierarchy for nested instructions: {agents_file.relative_to(repo)}"
                )
            elif resolution.chain[0] != root_resolved:
                issues.append(
                    "hierarchy does not begin with repository root agents.md for "
                    f"{agents_file.relative_to(repo)}"
                )

    smoke_start = repo / ".github/workflows"
    if not smoke_start.is_dir():
        smoke_start = repo / ".claude"
    smoke = resolve_chain(smoke_start)
    chains.append(smoke.chain)
    issues.extend(smoke.diagnostics)
    if root_resolved is not None and (
        not smoke.chain or smoke.chain[0] != root_resolved
    ):
        issues.append("nested-directory smoke chain omitted root agents.md")

    return Validation(
        tuple(dict.fromkeys(chains)),
        tuple(dict.fromkeys(issues)),
    )


def _display(path: Path, repo: Path | None = None) -> str:
    if repo is not None:
        try:
            return path.relative_to(repo).as_posix()
        except ValueError:
            pass
    return str(path)


def _print_chains(chains: Sequence[Sequence[Path]], repo: Path) -> None:
    for index, chain in enumerate(chains, start=1):
        print(f"agents-chain[{index}]:")
        for path in chain:
            print(f"  {_display(path, repo)}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Resolve and validate hierarchical agents.md files."
    )
    subcommands = parser.add_subparsers(dest="command", required=True)

    validate = subcommands.add_parser("validate")
    validate.add_argument("--repo", type=Path, default=Path.cwd())
    validate.add_argument("--print-chains", action="store_true")

    resolve = subcommands.add_parser("resolve")
    resolve.add_argument("--start", type=Path, default=Path.cwd())

    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "resolve":
        resolution = resolve_chain(args.start)
        for path in resolution.chain:
            print(path)
        for diagnostic in resolution.diagnostics:
            print(f"error: {diagnostic}", file=sys.stderr)
        return 2 if resolution.diagnostics else 0

    repo = args.repo.resolve()
    validation = validate_repo(repo)
    if args.print_chains:
        _print_chains(validation.chains, repo)
    if validation.issues:
        for issue in validation.issues:
            print(f"error: {issue}", file=sys.stderr)
        return 1
    print(
        f"agents policy valid: {repo} "
        f"({len(validation.chains)} hierarchy checks)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
