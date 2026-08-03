#!/usr/bin/env python3
"""Visibility projections for sealed repository-fleet manifests.

The reviewed fleet ledger remains immutable evidence of source histories and
original product intent. Operational publication may deliberately choose a more
restrictive visibility, but it must not alter repository identities, commits,
file counts, gitlinks, descriptions, metadata, counts, or ordering.
"""

from __future__ import annotations

from copy import deepcopy
from typing import Any


class VisibilityProjectionError(ValueError):
    """Raised when a fleet cannot be projected without semantic drift."""


def _validate_repository_full_name(full_name: object, index: int) -> str:
    """Return an exact ``owner/name`` identity or fail closed."""

    if not isinstance(full_name, str):
        raise VisibilityProjectionError(
            f"repository record {index} has invalid full_name"
        )
    if (
        full_name.count("/") != 1
        or any(character.isspace() for character in full_name)
    ):
        raise VisibilityProjectionError(
            f"repository record {index} has invalid full_name {full_name!r}"
        )
    owner, name = full_name.split("/", 1)
    if not owner or not name:
        raise VisibilityProjectionError(
            f"repository record {index} has invalid full_name {full_name!r}"
        )
    return full_name


def project_private_execution_manifest(
    reviewed_manifest: dict[str, Any],
) -> dict[str, Any]:
    """Return a deep-copied execution manifest with every repository private.

    The function proves that the projection changes only each repository's
    ``visibility`` field. The reviewed manifest is never mutated.
    """

    if not isinstance(reviewed_manifest, dict):
        raise VisibilityProjectionError("fleet manifest is not an object")

    repositories = reviewed_manifest.get("repositories")
    if not isinstance(repositories, list) or not repositories:
        raise VisibilityProjectionError("fleet manifest has no repository ledger")

    declared_count = reviewed_manifest.get("repository_count")
    if declared_count is not None:
        if isinstance(declared_count, bool) or not isinstance(declared_count, int):
            raise VisibilityProjectionError(
                "fleet manifest repository_count is not an integer"
            )
        if declared_count != len(repositories):
            raise VisibilityProjectionError(
                "fleet manifest repository_count does not match repository ledger"
            )

    projected = deepcopy(reviewed_manifest)
    projected_repositories = projected.get("repositories")
    if not isinstance(projected_repositories, list):
        raise VisibilityProjectionError("projected repository ledger is malformed")
    if len(projected_repositories) != len(repositories):
        raise VisibilityProjectionError("repository count changed during projection")

    seen_full_names: set[str] = set()
    for index, (reviewed, execution) in enumerate(
        zip(repositories, projected_repositories, strict=True)
    ):
        if not isinstance(reviewed, dict) or not isinstance(execution, dict):
            raise VisibilityProjectionError(
                f"repository record {index} is not an object"
            )

        full_name = _validate_repository_full_name(reviewed.get("full_name"), index)
        if full_name in seen_full_names:
            raise VisibilityProjectionError(
                f"repository manifest contains duplicate full_name {full_name!r}"
            )
        seen_full_names.add(full_name)

        visibility = reviewed.get("visibility")
        if visibility not in {"public", "private"}:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid visibility {visibility!r}"
            )
        execution["visibility"] = "private"

    restored = deepcopy(projected)
    restored_repositories = restored.get("repositories")
    if not isinstance(restored_repositories, list):
        raise VisibilityProjectionError("restored repository ledger is malformed")
    for reviewed, restored_record in zip(
        repositories, restored_repositories, strict=True
    ):
        if not isinstance(reviewed, dict) or not isinstance(restored_record, dict):
            raise VisibilityProjectionError(
                "repository record changed type during projection"
            )
        restored_record["visibility"] = reviewed["visibility"]
    if restored != reviewed_manifest:
        raise VisibilityProjectionError(
            "private execution projection changed fields other than visibility"
        )

    return projected
