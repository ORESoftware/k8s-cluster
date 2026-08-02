#!/usr/bin/env python3
"""Visibility projections for sealed repository-fleet manifests.

The reviewed fleet ledger remains immutable evidence of source histories and
original product intent. Operational publication may deliberately choose a more
restrictive visibility, but it must not alter repository identities, commits,
file counts, gitlinks, descriptions, or ordering.
"""

from __future__ import annotations

from copy import deepcopy
from typing import Any


class VisibilityProjectionError(ValueError):
    """Raised when a fleet cannot be projected without semantic drift."""


def project_private_execution_manifest(
    reviewed_manifest: dict[str, Any],
) -> dict[str, Any]:
    """Return a deep-copied execution manifest with every repo private.

    The function verifies that the projection changes only each repository's
    ``visibility`` field. This preserves the reviewed deterministic Git
    histories while applying the current fail-closed operational policy.
    """

    repositories = reviewed_manifest.get("repositories")
    if not isinstance(repositories, list) or not repositories:
        raise VisibilityProjectionError("fleet manifest has no repository ledger")

    projected = deepcopy(reviewed_manifest)
    projected_repositories = projected.get("repositories")
    if not isinstance(projected_repositories, list):
        raise VisibilityProjectionError("projected repository ledger is malformed")
    if len(projected_repositories) != len(repositories):
        raise VisibilityProjectionError("repository count changed during projection")

    for index, (reviewed, execution) in enumerate(
        zip(repositories, projected_repositories, strict=True)
    ):
        if not isinstance(reviewed, dict) or not isinstance(execution, dict):
            raise VisibilityProjectionError(
                f"repository record {index} is not an object"
            )
        visibility = reviewed.get("visibility")
        if visibility not in {"public", "private"}:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid visibility {visibility!r}"
            )
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
