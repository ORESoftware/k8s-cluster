#!/usr/bin/env python3
"""Create private repositories without weakening create-only safety.

A concurrent administrator may create the requested repository after the
publisher's preflight GET but before its POST. GitHub reports that race as a
conflict/unprocessable create response. Reconcile only by re-reading the exact
repository and accepting it when its canonical identity and private visibility
already match. Never patch visibility or overwrite repository contents.
"""

from __future__ import annotations

import re
from typing import Callable, TypeAlias


RepositoryApi: TypeAlias = Callable[
    [str, str, dict[str, object] | None], tuple[int, object | None]
]
Emitter: TypeAlias = Callable[[str], None]

_CREATE_CONFLICT_STATUSES = frozenset({409, 422})


def _private_repository_metadata(
    owner: str, name: str, payload: object
) -> dict[str, object]:
    full_name = f"{owner}/{name}"
    if not isinstance(payload, dict):
        raise RuntimeError(f"invalid repository response for {full_name}")

    remote_full_name = payload.get("full_name")
    if (
        not isinstance(remote_full_name, str)
        or remote_full_name.casefold() != full_name.casefold()
    ):
        raise RuntimeError(
            f"repository identity mismatch for {full_name}: {remote_full_name!r}"
        )

    if payload.get("private") is not True or payload.get("visibility") != "private":
        raise RuntimeError(
            f"visibility mismatch for {full_name}: "
            f"private={payload.get('private')!r}, "
            f"visibility={payload.get('visibility')!r}"
        )
    return payload


def _create_payload(name: str, description: str) -> dict[str, object]:
    return {
        "name": name,
        "description": description,
        "private": True,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "auto_init": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }


def _create_repository(
    api: RepositoryApi,
    owner: str,
    name: str,
    description: str,
) -> tuple[int, object | None]:
    path = f"/orgs/{owner}/repos"
    try:
        return api("POST", path, _create_payload(name, description))
    except RuntimeError as error:
        match = re.fullmatch(
            rf"GitHub API (409|422) for POST {re.escape(path)}:.*",
            str(error),
            flags=re.DOTALL,
        )
        if match is None:
            raise
        return int(match.group(1)), None


def ensure_private_repository(
    api: RepositoryApi,
    owner: str,
    name: str,
    description: str,
    *,
    emit: Emitter = print,
) -> dict[str, object]:
    """Return exact private metadata, creating or safely reconciling once.

    The function is intentionally create-only. Existing repositories are never
    patched. A 409/422 create race is accepted only after an exact GET proves
    that the requested repository now exists under the expected identity and is
    already private. Every other response fails closed.
    """

    full_name = f"{owner}/{name}"
    repository_path = f"/repos/{full_name}"
    status, current = api("GET", repository_path, None)
    if status == 200:
        return _private_repository_metadata(owner, name, current)
    if status != 404:
        raise RuntimeError(
            f"failed to inspect {full_name} before creation: HTTP {status}"
        )

    create_status, created = _create_repository(api, owner, name, description)
    if create_status == 201:
        metadata = _private_repository_metadata(owner, name, created)
        emit(f"CREATED_PRIVATE {full_name}")
        return metadata

    if create_status not in _CREATE_CONFLICT_STATUSES:
        raise RuntimeError(f"failed to create {full_name}: HTTP {create_status}")

    reconcile_status, reconciled = api("GET", repository_path, None)
    if reconcile_status != 200:
        raise RuntimeError(
            f"failed to create {full_name}: HTTP {create_status}; "
            f"reconciliation GET returned HTTP {reconcile_status}"
        )

    metadata = _private_repository_metadata(owner, name, reconciled)
    emit(f"RECONCILED_PRIVATE {full_name} after HTTP {create_status}")
    return metadata
