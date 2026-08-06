#!/usr/bin/env python3
"""Visibility projections for sealed repository-fleet manifests.

The reviewed fleet ledger remains immutable evidence of source histories and
original product intent. Operational publication may deliberately choose a more
restrictive visibility, but it must not alter repository identities, commits,
file counts, gitlinks, descriptions, ordering, or cardinality.
"""

from __future__ import annotations

from collections import Counter
from copy import deepcopy
import re
from typing import Any


_FULL_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
_KIND_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
_CONTROL_CHARACTER_RE = re.compile(r"[\x00-\x1f\x7f]")


class VisibilityProjectionError(ValueError):
    """Raised when a fleet cannot be projected without semantic drift."""


def _exact_non_negative_int(value: Any) -> bool:
    return type(value) is int and value >= 0


def _validate_reviewed_manifest(
    reviewed_manifest: dict[str, Any],
) -> list[dict[str, Any]]:
    if not isinstance(reviewed_manifest, dict):
        raise VisibilityProjectionError("fleet manifest is not an object")

    repositories = reviewed_manifest.get("repositories")
    if not isinstance(repositories, list) or not repositories:
        raise VisibilityProjectionError("fleet manifest has no repository ledger")

    schema_version = reviewed_manifest.get("schema_version")
    if type(schema_version) is not int or schema_version != 2:
        raise VisibilityProjectionError("fleet manifest must use schema version 2")

    default_branch = reviewed_manifest.get("default_branch")
    if default_branch != "main":
        raise VisibilityProjectionError("fleet manifest default_branch must be main")

    expected_count = reviewed_manifest.get("repository_count")
    if type(expected_count) is not int or expected_count != len(repositories):
        raise VisibilityProjectionError(
            "repository_count does not match the repository ledger"
        )

    organizations = reviewed_manifest.get("organizations")
    if not isinstance(organizations, dict) or not organizations:
        raise VisibilityProjectionError("fleet manifest has no organization counts")

    expected_organization_counts: dict[str, int] = {}
    for organization, count in organizations.items():
        if (
            not isinstance(organization, str)
            or not organization
            or "/" in organization
            or _FULL_NAME_RE.fullmatch(f"{organization}/repository") is None
        ):
            raise VisibilityProjectionError(
                f"fleet manifest has invalid organization {organization!r}"
            )
        normalized_organization = organization.casefold()
        if normalized_organization in expected_organization_counts:
            raise VisibilityProjectionError(
                f"fleet manifest duplicates organization {organization!r}"
            )
        if type(count) is not int or count <= 0:
            raise VisibilityProjectionError(
                f"fleet manifest has invalid organization count for {organization!r}"
            )
        expected_organization_counts[normalized_organization] = count

    if sum(expected_organization_counts.values()) != expected_count:
        raise VisibilityProjectionError(
            "organization counts do not match repository_count"
        )

    expected_total_files = reviewed_manifest.get("total_tracked_files")
    if not _exact_non_negative_int(expected_total_files):
        raise VisibilityProjectionError(
            "fleet manifest has invalid total_tracked_files"
        )

    expected_total_gitlinks = reviewed_manifest.get("total_gitlinks")
    if not _exact_non_negative_int(expected_total_gitlinks):
        raise VisibilityProjectionError("fleet manifest has invalid total_gitlinks")

    seen_full_names: set[str] = set()
    actual_organization_counts: Counter[str] = Counter()
    actual_total_files = 0
    actual_total_gitlinks = 0
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

        organization, repository_name = full_name.split("/", 1)
        actual_organization_counts[organization.casefold()] += 1

        if record.get("org") != organization:
            raise VisibilityProjectionError(
                f"repository record {index} org does not match full_name"
            )
        if record.get("name") != repository_name:
            raise VisibilityProjectionError(
                f"repository record {index} name does not match full_name"
            )

        expected_remote = f"https://github.com/{full_name}.git"
        if record.get("remote") != expected_remote:
            raise VisibilityProjectionError(
                f"repository record {index} remote does not match full_name"
            )

        if record.get("default_branch") != default_branch:
            raise VisibilityProjectionError(
                f"repository record {index} default_branch does not match fleet"
            )

        kind = record.get("kind")
        if not isinstance(kind, str) or _KIND_RE.fullmatch(kind) is None:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid kind {kind!r}"
            )

        description = record.get("description")
        if (
            not isinstance(description, str)
            or not description
            or description != description.strip()
            or _CONTROL_CHARACTER_RE.search(description) is not None
        ):
            raise VisibilityProjectionError(
                f"repository record {index} has invalid description"
            )

        commit = record.get("commit")
        if not isinstance(commit, str) or _COMMIT_RE.fullmatch(commit) is None:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid commit {commit!r}"
            )

        files = record.get("files")
        if not _exact_non_negative_int(files):
            raise VisibilityProjectionError(
                f"repository record {index} has invalid files {files!r}"
            )
        actual_total_files += files

        gitlinks = record.get("gitlinks")
        if not _exact_non_negative_int(gitlinks):
            raise VisibilityProjectionError(
                f"repository record {index} has invalid gitlinks {gitlinks!r}"
            )
        actual_total_gitlinks += gitlinks

        visibility = record.get("visibility")
        if visibility not in {"public", "private"}:
            raise VisibilityProjectionError(
                f"repository record {index} has invalid visibility {visibility!r}"
            )
        validated.append(record)

    if dict(actual_organization_counts) != expected_organization_counts:
        raise VisibilityProjectionError(
            "organization counts do not match repository identities"
        )
    if actual_total_files != expected_total_files:
        raise VisibilityProjectionError(
            "total_tracked_files does not match repository records"
        )
    if actual_total_gitlinks != expected_total_gitlinks:
        raise VisibilityProjectionError(
            "total_gitlinks does not match repository records"
        )

    return validated


def project_private_execution_manifest(
    reviewed_manifest: dict[str, Any],
) -> dict[str, Any]:
    """Return a deep-copied execution manifest with every repository private.

    The function validates schema version, repository cardinality, organization
    accounting, redundant repository identities and remotes, source metadata,
    commits, file/gitlink counts, and aggregate totals before projecting
    visibility. It then proves that the projection changed no field other than
    each repository's ``visibility`` value. The reviewed manifest is never
    mutated.
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
