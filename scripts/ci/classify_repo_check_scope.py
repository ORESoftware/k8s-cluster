#!/usr/bin/env python3
"""Decide whether a pull request needs credential-backed private contract jobs."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path, PurePosixPath
from typing import Iterable

CONTROL_FILES = {
    ".github/workflows/repo-checks.yml",
    "scripts/ci/classify_repo_check_scope.py",
    "scripts/ci/test_classify_repo_check_scope.py",
}

GOVERNANCE_FILES = {
    ".github/workflows/ops-current-org-dotgithub-relationships-ephemeral-publish.yml",
    ".github/workflows/ops-sync-org-project-docs-rate-aware-once.yml",
    ".github/workflows/test-org-project-docs-evidence.yml",
    "docs/operations/org-dotgithub-relationship-publication.md",
    "docs/org-project-delivery-ledger-2026-08-05.md",
    "ops/portfolio/github-linear-project-registry.tsv",
    "scripts/ops/build_org_project_docs_retry_registry.py",
    "scripts/ops/org_repository_relationships_graph.py",
    "scripts/ops/org_repository_relationships_model.py",
    "scripts/ops/org_repository_relationships_render.py",
    "scripts/ops/org_repository_relationships_roles.py",
    "scripts/ops/prepare_org_project_docs_reconciler.py",
    "scripts/ops/publish_current_org_repository_relationships.py",
    "scripts/ops/publish_org_repository_relationships.py",
    "scripts/ops/sync_org_project_docs.sh",
    "scripts/ops/sync_org_project_docs_rate_aware.py",
    "scripts/ops/test_build_org_project_docs_retry_registry.py",
    "scripts/ops/test_sync_org_project_docs_rate_aware.py",
    "scripts/ops/test_upsert_managed_markdown_block.py",
    "scripts/ops/upsert_managed_markdown_block.py",
    "scripts/ops/validate_org_project_docs_evidence.py",
    "scripts/ops/tests/test_org_project_delivery_ledger.py",
    "scripts/ops/tests/test_validate_org_project_docs_evidence.py",
    "tests/ops/test_publish_current_org_repository_relationships.py",
}

GOVERNANCE_PREFIXES = (
    "ops/evidence/org-project-docs/",
    "ops/evidence/org-project-docs-rate-aware/",
)


class ScopeError(RuntimeError):
    """Raised when changed-file evidence cannot be trusted."""


def validate_path(value: str) -> str:
    if not value or value.startswith("/") or "\\" in value:
        raise ScopeError(f"invalid repository-relative path: {value!r}")
    path = PurePosixPath(value)
    if any(part in {"", ".", ".."} for part in path.parts):
        raise ScopeError(f"unsafe repository-relative path: {value!r}")
    return value


def is_governance_path(path: str) -> bool:
    return path in GOVERNANCE_FILES or any(
        path.startswith(prefix) for prefix in GOVERNANCE_PREFIXES
    )


def classify(event_name: str, changed_files: Iterable[str]) -> dict[str, object]:
    changed = sorted({validate_path(path) for path in changed_files})
    if event_name != "pull_request":
        return {
            "schema_version": 1,
            "event_name": event_name,
            "changed_files": changed,
            "governance_only": False,
            "private_contracts_required": True,
            "reason": "non_pull_request_runs_are_full_fleet_checks",
        }
    if not changed:
        raise ScopeError("pull-request scope requires at least one changed file")

    non_control = [path for path in changed if path not in CONTROL_FILES]
    governance_only = (
        bool(non_control)
        and all(is_governance_path(path) for path in non_control)
        and all(
            path in CONTROL_FILES or is_governance_path(path)
            for path in changed
        )
    )
    return {
        "schema_version": 1,
        "event_name": event_name,
        "changed_files": changed,
        "governance_only": governance_only,
        "private_contracts_required": not governance_only,
        "reason": (
            "governance_only_no_private_gitlinks"
            if governance_only
            else "repository_or_private_contract_surface_changed"
        ),
    }


def read_changed_files(path: Path, *, nul_delimited: bool) -> list[str]:
    raw = path.read_bytes()
    if nul_delimited:
        values = raw.split(b"\0")
        return [value.decode("utf-8") for value in values if value]
    return [line for line in raw.decode("utf-8").splitlines() if line]


def append_github_output(path: Path, result: dict[str, object]) -> None:
    required = str(bool(result["private_contracts_required"])).lower()
    with path.open("a", encoding="utf-8") as handle:
        handle.write(f"private_contracts_required={required}\n")
        handle.write(f"scope_reason={result['reason']}\n")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event-name", required=True)
    parser.add_argument("--changed-files", required=True)
    parser.add_argument("--nul-delimited", action="store_true")
    parser.add_argument("--github-output")
    parser.add_argument("--json-output")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        result = classify(
            args.event_name,
            read_changed_files(
                Path(args.changed_files),
                nul_delimited=args.nul_delimited,
            ),
        )
        if args.github_output:
            append_github_output(Path(args.github_output), result)
        if args.json_output:
            output = Path(args.json_output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(
                json.dumps(result, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
        print(json.dumps(result, sort_keys=True))
        return 0
    except (ScopeError, OSError, UnicodeDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
