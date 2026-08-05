#!/usr/bin/env python3
"""Fail-closed classification of canonical repository-fleet remote state.

Missing repositories may be created from sealed histories. Existing repositories
are immutable inputs to a gap publication run: later reviewed work must be
preserved even when the live main SHA no longer equals the old sealed root.

GitHub follows repository rename redirects. A sealed identity may therefore
resolve to a different canonical ``full_name``. Only source-bound, repository-ID
pinned aliases from the reviewed alias ledger are accepted.
"""

from __future__ import annotations

from copy import deepcopy
import re
from typing import Any, Callable, Mapping

from repository_fleet_aliases import RepositoryAlias


_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
_FULL_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


class RemoteFleetStateError(RuntimeError):
    """Remote repository state is unsafe, ambiguous, or changed during a run."""


RepositoryLookup = Callable[[str], tuple[int, dict[str, Any] | None]]
MainRefLookup = Callable[[str], str | None]
RepositoryAliases = Mapping[str, RepositoryAlias]


def _record_identity(record: dict[str, Any], index: int) -> tuple[str, str]:
    full_name = record.get("full_name")
    if not isinstance(full_name, str) or _FULL_NAME_RE.fullmatch(full_name) is None:
        raise RemoteFleetStateError(f"repository record {index} has invalid full_name")
    commit = record.get("commit")
    if not isinstance(commit, str) or _SHA_RE.fullmatch(commit) is None:
        raise RemoteFleetStateError(f"repository record {full_name} has invalid commit")
    return full_name, commit


def _validate_private_remote(
    full_name: str,
    payload: dict[str, Any] | None,
    *,
    repository_aliases: RepositoryAliases | None = None,
) -> tuple[dict[str, Any], str, bool]:
    if not isinstance(payload, dict):
        raise RemoteFleetStateError(f"invalid repository metadata for {full_name}")
    remote_name = payload.get("full_name")
    if not isinstance(remote_name, str) or _FULL_NAME_RE.fullmatch(remote_name) is None:
        raise RemoteFleetStateError(
            f"GitHub returned an invalid repository identity for {full_name}"
        )

    alias = (
        repository_aliases.get(full_name.casefold())
        if repository_aliases is not None
        else None
    )
    renamed = alias is not None
    if alias is not None:
        if remote_name != alias.remote_full_name:
            raise RemoteFleetStateError(
                f"GitHub returned an unreviewed rename for {full_name}: "
                f"{remote_name!r} != {alias.remote_full_name!r}"
            )
        if payload.get("id") != alias.repository_id:
            raise RemoteFleetStateError(
                f"reviewed alias repository ID changed for {full_name}"
            )
    elif remote_name.casefold() != full_name.casefold():
        raise RemoteFleetStateError(
            f"GitHub returned an unexpected repository for {full_name}: {remote_name}"
        )

    if payload.get("private") is not True or payload.get("visibility") != "private":
        raise RemoteFleetStateError(f"repository {full_name} is not private")
    if payload.get("default_branch") != "main":
        raise RemoteFleetStateError(f"repository {full_name} does not default to main")
    if payload.get("archived") is True or payload.get("disabled") is True:
        raise RemoteFleetStateError(f"repository {full_name} is archived or disabled")
    return payload, remote_name, renamed


def classify_remote_fleet(
    records: list[dict[str, Any]],
    *,
    repository_lookup: RepositoryLookup,
    main_ref_lookup: MainRefLookup,
    repository_aliases: RepositoryAliases | None = None,
) -> tuple[list[dict[str, Any]], dict[str, dict[str, Any]]]:
    """Return missing records and a snapshot of existing repositories.

    Existing repositories are accepted at their current reviewed ``main`` SHA.
    The caller must later use :func:`verify_preserved_existing` to prove that no
    existing repository changed while missing gaps were published. Reviewed
    rename aliases are resolved to their exact canonical target and stable ID.
    """

    missing: list[dict[str, Any]] = []
    existing: dict[str, dict[str, Any]] = {}
    seen: set[str] = set()

    for index, record in enumerate(records):
        if not isinstance(record, dict):
            raise RemoteFleetStateError(f"repository record {index} is not an object")
        full_name, sealed_commit = _record_identity(record, index)
        key = full_name.casefold()
        if key in seen:
            raise RemoteFleetStateError(f"duplicate repository identity {full_name}")
        seen.add(key)

        status, payload = repository_lookup(full_name)
        if status == 404:
            if repository_aliases is not None and key in repository_aliases:
                raise RemoteFleetStateError(
                    f"reviewed alias source no longer resolves on GitHub: {full_name}"
                )
            missing.append(deepcopy(record))
            continue
        if status != 200:
            raise RemoteFleetStateError(
                f"repository lookup failed for {full_name}: HTTP {status}"
            )
        remote, remote_full_name, renamed = _validate_private_remote(
            full_name,
            payload,
            repository_aliases=repository_aliases,
        )
        head = main_ref_lookup(remote_full_name)
        if not isinstance(head, str) or _SHA_RE.fullmatch(head) is None:
            raise RemoteFleetStateError(f"repository {full_name} has no valid main SHA")
        existing[full_name] = {
            "head": head,
            "sealed_commit": sealed_commit,
            "matches_sealed_commit": head == sealed_commit,
            "repository_id": remote.get("id"),
            "remote_full_name": remote_full_name,
            "renamed": renamed,
        }

    # Missing leaf histories must be published before a missing monorepo, whose
    # sealed gitlinks depend on the reviewed leaf identities.
    missing.sort(
        key=lambda record: (
            record.get("kind") == "monorepo",
            str(record.get("full_name", "")).casefold(),
        )
    )
    return missing, existing


def verify_created_repositories(
    records: list[dict[str, Any]],
    *,
    repository_lookup: RepositoryLookup,
    main_ref_lookup: MainRefLookup,
) -> None:
    """Require newly created repositories to match their sealed roots exactly.

    Rename aliases are deliberately not accepted here: a newly created gap must
    exist at the exact sealed identity requested by the reviewed manifest.
    """

    for index, record in enumerate(records):
        full_name, sealed_commit = _record_identity(record, index)
        status, payload = repository_lookup(full_name)
        if status != 200:
            raise RemoteFleetStateError(
                f"created repository verification failed for {full_name}: HTTP {status}"
            )
        _, remote_full_name, renamed = _validate_private_remote(full_name, payload)
        if renamed:
            raise RemoteFleetStateError(
                f"created repository {full_name} unexpectedly resolved through an alias"
            )
        actual = main_ref_lookup(remote_full_name)
        if actual != sealed_commit:
            raise RemoteFleetStateError(
                f"created repository {full_name} main drift: {actual!r} != {sealed_commit}"
            )


def verify_preserved_existing(
    existing_snapshot: dict[str, dict[str, Any]],
    *,
    repository_lookup: RepositoryLookup,
    main_ref_lookup: MainRefLookup,
    repository_aliases: RepositoryAliases | None = None,
) -> None:
    """Prove a gap publication did not mutate or redirect existing repositories."""

    for full_name in sorted(existing_snapshot, key=str.casefold):
        expected = existing_snapshot[full_name]
        status, payload = repository_lookup(full_name)
        if status != 200:
            raise RemoteFleetStateError(
                f"preserved repository verification failed for {full_name}: HTTP {status}"
            )
        remote, remote_full_name, renamed = _validate_private_remote(
            full_name,
            payload,
            repository_aliases=repository_aliases,
        )
        if remote_full_name != expected.get("remote_full_name"):
            raise RemoteFleetStateError(
                f"repository alias target changed for {full_name}: "
                f"{remote_full_name!r} != {expected.get('remote_full_name')!r}"
            )
        if renamed is not bool(expected.get("renamed")):
            raise RemoteFleetStateError(
                f"repository rename disposition changed for {full_name}"
            )
        repository_id = expected.get("repository_id")
        if repository_id is not None and remote.get("id") != repository_id:
            raise RemoteFleetStateError(f"repository identity changed for {full_name}")
        actual = main_ref_lookup(remote_full_name)
        if actual != expected.get("head"):
            raise RemoteFleetStateError(
                f"existing repository {full_name} changed during gap publication: "
                f"{actual!r} != {expected.get('head')!r}"
            )
