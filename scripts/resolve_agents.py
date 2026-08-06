#!/usr/bin/env python3
"""Discover and validate hierarchical lowercase ``agents.md`` instructions.

Discovery resolves the starting directory, walks only its ancestors to the
filesystem root, reads lowercase ``agents.md`` files root-to-leaf, deduplicates
resolved files by inode, and reports broken, cyclic, non-regular, or unreadable
candidates. Sibling directories are never searched.
"""

from __future__ import annotations

import argparse
import os
import stat
import sys
import tempfile
from pathlib import Path
from typing import Iterable, Sequence

ROOT_POINTER = """# Agent instructions

Canonical repository instructions live in [`agents.md`](agents.md).
"""
TOOL_POINTER = """# Agent instructions

Canonical repository instructions live in [`agents.md`](../agents.md).
"""
POINTERS = {
    Path("AGENTS.md"): ROOT_POINTER,
    Path(".claude/CLAUDE.md"): TOOL_POINTER,
    Path(".gemini/GEMINI.md"): TOOL_POINTER,
    Path(".openai/AGENTS.md"): TOOL_POINTER,
}


class DiscoveryError(RuntimeError):
    """One or more candidate instruction files could not be used safely."""


def _ancestors_root_to_leaf(directory: Path) -> list[Path]:
    lineage: list[Path] = []
    current = directory
    while True:
        lineage.append(current)
        if current.parent == current:
            break
        current = current.parent
    lineage.reverse()
    return lineage


def discover(start: Path | str) -> list[Path]:
    """Return readable lowercase ``agents.md`` files in root-to-leaf order."""

    requested = Path(start).expanduser()
    try:
        resolved_start = requested.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        raise DiscoveryError(f"cannot resolve start path {requested}: {error}") from error
    if not resolved_start.is_dir():
        resolved_start = resolved_start.parent

    discovered: list[Path] = []
    seen_files: set[tuple[int, int]] = set()
    errors: list[str] = []

    for directory in _ancestors_root_to_leaf(resolved_start):
        candidate = directory / "agents.md"
        if not candidate.exists() and not candidate.is_symlink():
            continue
        try:
            resolved = candidate.resolve(strict=True)
            metadata = resolved.stat()
            if not stat.S_ISREG(metadata.st_mode):
                raise OSError("resolved target is not a regular file")
            with resolved.open("r", encoding="utf-8") as handle:
                handle.read(1)
        except (OSError, RuntimeError, UnicodeError) as error:
            errors.append(f"{candidate}: {error}")
            continue

        identity = (metadata.st_dev, metadata.st_ino)
        if identity in seen_files:
            continue
        seen_files.add(identity)
        discovered.append(resolved)

    if errors:
        details = "\n".join(f"- {message}" for message in errors)
        raise DiscoveryError(f"unusable agents.md candidate(s):\n{details}")
    return discovered


def render(paths: Iterable[Path]) -> str:
    sections: list[str] = []
    for path in paths:
        text = path.read_text(encoding="utf-8")
        sections.append(f"===== BEGIN {path} =====\n{text.rstrip()}\n===== END {path} =====")
    return "\n\n".join(sections)


def repository_root() -> Path:
    return Path(__file__).resolve(strict=True).parents[1]


def resolve_repository_root(value: Path | None) -> Path:
    requested = repository_root() if value is None else value.expanduser()
    try:
        resolved = requested.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        raise DiscoveryError(f"cannot resolve repository root {requested}: {error}") from error
    if not resolved.is_dir():
        raise DiscoveryError(f"repository root is not a directory: {resolved}")
    return resolved


def validate_layout(root: Path) -> None:
    canonical = root / "agents.md"
    if not canonical.is_file():
        raise DiscoveryError(f"missing canonical instruction file: {canonical}")
    canonical_text = canonical.read_text(encoding="utf-8")
    if len(canonical_text.strip()) < 80:
        raise DiscoveryError("canonical agents.md is unexpectedly small")

    failures: list[str] = []
    for relative, expected in POINTERS.items():
        pointer = root / relative
        try:
            actual = pointer.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            failures.append(f"{relative}: {error}")
            continue
        if actual != expected:
            target = "agents.md" if relative == Path("AGENTS.md") else "../agents.md"
            failures.append(f"{relative}: must be the minimal pointer to {target}")
        if actual == canonical_text:
            failures.append(f"{relative}: duplicates canonical instructions")

    chain = discover(root)
    if chain != [canonical.resolve(strict=True)]:
        failures.append(f"repository-root discovery mismatch: {chain!r}")
    if failures:
        raise DiscoveryError("invalid agent instruction layout:\n- " + "\n- ".join(failures))


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="agents-hierarchy-") as temporary:
        root = Path(temporary).resolve(strict=True) / "workspace"
        nested = root / "services" / "api" / "src"
        sibling = root / "sibling"
        nested.mkdir(parents=True)
        sibling.mkdir(parents=True)
        _write(root / "agents.md", "root instructions\n")
        _write(root / "services" / "agents.md", "service instructions\n")
        _write(sibling / "agents.md", "sibling instructions must not load\n")

        expected = [
            (root / "agents.md").resolve(strict=True),
            (root / "services" / "agents.md").resolve(strict=True),
        ]
        chain = discover(nested)
        if chain != expected:
            raise AssertionError(f"root-to-leaf chain mismatch: {chain!r}")
        print("nested root-to-leaf chain:")
        for path in chain:
            print(f"- {path.relative_to(root)}")

        duplicate = root / "services" / "api" / "agents.md"
        duplicate.symlink_to(root / "agents.md")
        if discover(nested) != expected:
            raise AssertionError("resolved-file deduplication failed")

        broken_root = root / "broken"
        broken_leaf = broken_root / "leaf"
        broken_leaf.mkdir(parents=True)
        (broken_root / "agents.md").symlink_to(root / "missing.md")
        try:
            discover(broken_leaf)
        except DiscoveryError as error:
            if "broken/agents.md" not in str(error):
                raise AssertionError("broken-link diagnostic omitted the candidate") from error
        else:
            raise AssertionError("broken symlink was not reported")

        cycle_root = root / "cycle"
        cycle_leaf = cycle_root / "leaf"
        cycle_leaf.mkdir(parents=True)
        (cycle_root / "agents.md").symlink_to(cycle_root / "agents.md")
        try:
            discover(cycle_leaf)
        except DiscoveryError as error:
            if "cycle/agents.md" not in str(error):
                raise AssertionError("cycle diagnostic omitted the candidate") from error
        else:
            raise AssertionError("symlink cycle was not reported")

        unreadable_root = root / "unreadable"
        unreadable_leaf = unreadable_root / "leaf"
        unreadable_leaf.mkdir(parents=True)
        unreadable = unreadable_root / "agents.md"
        _write(unreadable, "private instructions\n")
        unreadable.chmod(0)
        try:
            if os.name != "nt" and not os.access(unreadable, os.R_OK):
                try:
                    discover(unreadable_leaf)
                except DiscoveryError as error:
                    if "unreadable/agents.md" not in str(error):
                        raise AssertionError("unreadable diagnostic omitted the candidate") from error
                else:
                    raise AssertionError("unreadable file was not reported")
            else:
                print("unreadable-file check skipped: current user can still read mode 000")
        finally:
            unreadable.chmod(0o600)

    print("agents.md hierarchy self-test: PASS")


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, help="repository root to validate")
    parser.add_argument("--cwd", type=Path, default=Path.cwd(), help="discovery start path")
    parser.add_argument("--print-chain", action="store_true", help="print resolved files")
    parser.add_argument("--render", action="store_true", help="render merged instructions")
    parser.add_argument("--check-layout", action="store_true", help="validate repository pointers")
    parser.add_argument("--self-test", action="store_true", help="run hermetic discovery tests")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        if args.check_layout:
            validate_layout(resolve_repository_root(args.repo_root))
            print("agent instruction layout: PASS")
        if args.self_test:
            self_test()
        if args.print_chain or args.render or (not args.check_layout and not args.self_test):
            chain = discover(args.cwd)
            if args.render:
                print(render(chain))
            else:
                for path in chain:
                    print(path)
    except (DiscoveryError, OSError, UnicodeError, AssertionError) as error:
        print(f"agents hierarchy error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
