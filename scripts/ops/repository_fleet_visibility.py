#!/usr/bin/env python3
"""Visibility projections for sealed repository-fleet manifests.

The reviewed fleet ledger remains immutable evidence of source histories and
original product intent. Operational publication may deliberately choose a more
restrictive visibility, but it must not alter repository identities, commits,
file counts, gitlinks, descriptions, ordering, or cardinality.
"""

from __future__ import annotations

from copy import deepcopy
import re
from typing import Any


_FULL_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")


class VisibilityProjectionError(ValueError):
    """Raised when a fleet cannot be projected without semantic drift."""


def _validate_reviewed_manifest(
    reviewed_manifest: dict[str, Any],
) -> list[dict[str, Any]]:
    if not isinstance(reviewed_manifest, dict):
        raise VisibilityProjectionError("fleet manifest is not an object")

    repositories = reviewed_manifest.get("repositories")
    if not isinstance(repositories, list) or not repositories:
        raise VisibilityProjectionError("fleet manifest has no repository ledger")

    expected_count = reviewed_manifest.get("repository_count")
    if type(expected_count) is not int or expected_count != len(repositories):
        raise VisibilityProjectionError(
            "repository_count does not match the repository ledger"
        )

    seen_full_names: set[str] = set()
    validated: list[dict[str, Any]] = []
    for index, record in enumerate(repositories):
        if not isinstance(record, dict):
            raise VisibilityProjectionError(
                f"repository record {index} is not an object"
            )

        full_name = record.get("full_name")
        if not isinstance(full_name, str) or _FULL_NAME_RE.fullmatch(full_name) is None:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid full_name {full_name!r}"
            )
        normalized_full_name = full_name.casefold()
        if normalized_full_name in seen_full_names:
            raise VisibilityProjectionError(
                f"repository record {index} duplicates full_name {full_name!r}"
            )
        seen_full_names.add(normalized_full_name)

        commit = record.get("commit")
        if not isinstance(commit, str) or _COMMIT_RE.fullmatch(commit) is None:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid commit {commit!r}"
            )

        visibility = record.get("visibility")
        if visibility not in {"public", "private"}:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid visibility {visibility!r}"
            )
        validated.append(record)

    return validated


def project_private_execution_manifest(
    reviewed_manifest: dict[str, Any],
) -> dict[str, Any]:
    """Return a deep-copied execution manifest with every repository private.

    The function validates repository cardinality and immutable identities before
    projecting visibility. It then proves that the projection changed no field
    other than each repository's ``visibility`` value. The reviewed manifest is
    never mutated.
    """

    repositories = _validate_reviewed_manifest(reviewed_manifest)

    projected = deepcopy(reviewed_manifest)
    projected_repositories = projected.get("repositories")
    if not isinstance(projected_repositories, list):
        raise VisibilityProjectionError("projected repository ledger is malformed")
    if len(projected_repositories) != len(repositories):
        raise VisibilityProjectionError("repository count changed during projection")

    for execution in projected_repositories:
        if not isinstance(execution, dict):
            raise VisibilityProjectionError("projected repository record is malformed")
        execution["visibility"] = "private"

    restored = deepcopy(projected)
    restored_repositories = restored["repositories"]
    for reviewed, restored_record in zip(
        repositories, restored_repositories, strict=True
    ):
        restored_record["visibility"] = reviewed["visibility"]
    if restored != reviewed_manifest:
        raise VisibilityProjectionError(
            "private execution projection changed fields other than visibility"
        )

    return projected
