#!/usr/bin/env python3
"""Quarantine the legacy Elenkos recovery path; never authorize publication.

A marker that hashes its own observed tree is not independent provenance.
Until a separately reviewed predecessor approval contract exists, recovery may
only verify an exact expected tree with an already-matching bootstrap tag.
This module transforms source without importing or executing that source.
"""
from __future__ import annotations

import argparse
import ast
from pathlib import Path

HOLD = "Elenkos publication hold: externally approved predecessor identity required"

REPLACEMENTS = {
    "create_full_commit": '''def create_full_commit(api, expected, parent_sha):
    raise RuntimeError("Elenkos publication hold: externally approved predecessor identity required")
''',
    "ensure_initial_tag": '''def ensure_initial_tag(api, expected, main_sha, main_commit, tag_sha):
    if tag_sha != main_sha:
        raise RuntimeError("Elenkos publication hold: tag creation and movement are prohibited")
    return "ready"
''',
    "recover_existing_repository": '''def recover_existing_repository(api, expected, main_sha):
    if not isinstance(main_sha, str) or SHA_RE.fullmatch(main_sha) is None:
        raise RuntimeError("invalid observed main SHA")
    main_commit, main_files = commit_tree(api, expected, main_sha)
    if main_commit.get("sha") != main_sha:
        raise RuntimeError("observed commit identity changed")
    if main_files != expected.git_files:
        raise RuntimeError("Elenkos publication hold: unapproved tree; reconciliation prohibited")
    verify_marker_blob(expected, main_files)
    if main_commit.get("message") != expected.initial_message:
        raise RuntimeError("Elenkos publication hold: unexpected initial commit message")
    if read_ref(api, expected.full_name, f"tags/{TAG}") != main_sha:
        raise RuntimeError("Elenkos publication hold: tag creation and movement are prohibited")
    if read_ref(api, expected.full_name, "heads/main", empty_allowed=True) != main_sha:
        raise RuntimeError("main changed during read-only verification")
    if read_ref(api, expected.full_name, f"tags/{TAG}") != main_sha:
        raise RuntimeError("tag changed during read-only verification")
    return "full-live-tree:ready", main_sha
''',
}

# Previously injected helpers must not survive a partial or stale rewrite.
LEGACY_HELPERS = frozenset({
    "read_blob_bytes", "managed_marker_document", "observed_source_fingerprint",
    "verify_managed_full_tree", "ref_commit_is_managed", "ensure_managed_tag",
})


def rewrite_source(source: str) -> tuple[str, str]:
    """Return a deterministic read-only replacement or reject an unknown shape."""
    tree = ast.parse(source)
    functions = [node for node in ast.walk(tree)
                 if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))]
    if any(node.name in LEGACY_HELPERS for node in functions):
        raise ValueError("legacy widened recovery detected; rematerialize reviewed source")
    selected = {}
    unchanged = []
    for name, replacement in REPLACEMENTS.items():
        matches = [node for node in functions if node.name == name]
        if len(matches) != 1 or matches[0] not in tree.body:
            raise ValueError(f"expected one top-level {name} function")
        node = matches[0]
        expected_node = ast.parse(replacement).body[0]
        args = node.args
        if (not isinstance(node, ast.FunctionDef) or node.decorator_list
                or args.posonlyargs or args.kwonlyargs or args.vararg or args.kwarg
                or args.defaults or args.kw_defaults
                or [arg.arg for arg in args.args]
                != [arg.arg for arg in expected_node.args.args]):
            raise ValueError(f"unexpected {name} signature")
        selected[name] = node
        unchanged.append(ast.dump(node) == ast.dump(expected_node))
    if all(unchanged):
        return source, "already-applied"
    if any(unchanged):
        raise ValueError("partial quarantine state; rematerialize reviewed source")
    lines = source.splitlines(keepends=True)
    for name, node in sorted(selected.items(), key=lambda item: item[1].lineno, reverse=True):
        lines[node.lineno - 1:node.end_lineno] = [REPLACEMENTS[name]]
    result = "".join(lines)
    compile(result, "<quarantined-elenkos-recovery>", "exec")
    return result, "applied"


def apply(path: Path) -> str:
    if path.is_symlink() or not path.is_file():
        raise ValueError("recovery source must be a regular non-symlink file")
    source = path.read_text(encoding="utf-8")
    rewritten, result = rewrite_source(source)
    # Validation and compilation finish before the only filesystem effect.
    if rewritten != source:
        path.write_text(rewritten, encoding="utf-8")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--recovery", type=Path,
                        default=Path("scripts/ops/recover_elenkos_partial_bootstrap_20260820.py"))
    args = parser.parse_args()
    result = apply(args.recovery)
    print(f"ELENKOS_RECOVERY_QUARANTINED result={result} read_only=true publication_authorized=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
