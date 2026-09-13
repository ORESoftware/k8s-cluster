#!/usr/bin/env python3
"""Validate the reviewed mapping from sealed repository names to GitHub renames."""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import re
from typing import Any, Iterable


_FULL_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
_SHA_RE = re.compile(r"^[0-9a-f]{40}$")


class RepositoryAliasError(RuntimeError):
    """The repository-alias ledger is malformed or does not match its source."""


@dataclass(frozen=True)
class RepositoryAlias:
    sealed_full_name: str
    remote_full_name: str
    repository_id: int


def _full_name(value: Any, label: str) -> str:
    if not isinstance(value, str) or _FULL_NAME_RE.fullmatch(value) is None:
        raise RepositoryAliasError(f"{label} is not a valid owner/repository identity")
    return value


def validate_repository_alias_payload(
    payload: Any,
    *,
    sealed_full_names: Iterable[str],
    expected_source_repository: str,
    expected_source_sha: str,
) -> dict[str, RepositoryAlias]:
    if not isinstance(payload, dict):
        raise RepositoryAliasError("repository-alias ledger must be a JSON object")
    if payload.get("schema_version") != 1:
        raise RepositoryAliasError("repository-alias ledger must use schema version 1")
    if payload.get("source_repository") != expected_source_repository:
        raise RepositoryAliasError("repository-alias ledger source repository changed")
    if payload.get("source_sha") != expected_source_sha:
        raise RepositoryAliasError("repository-alias ledger source commit changed")
    if _SHA_RE.fullmatch(expected_source_sha) is None:
        raise RepositoryAliasError("expected source commit is not a 40-hex SHA")

    sealed_by_key: dict[str, str] = {}
    for index, value in enumerate(sealed_full_names):
        validated = _full_name(value, f"sealed repository {index}")
        key = validated.casefold()
        if key in sealed_by_key:
            raise RepositoryAliasError(f"sealed fleet contains duplicate identity {validated}")
        sealed_by_key[key] = validated

    raw_aliases = payload.get("aliases")
    if not isinstance(raw_aliases, list):
        raise RepositoryAliasError("repository-alias ledger aliases must be a list")

    aliases: dict[str, RepositoryAlias] = {}
    remote_names: set[str] = set()
    repository_ids: set[int] = set()
    expected_keys = {"sealed_full_name", "remote_full_name", "repository_id"}
    for index, raw in enumerate(raw_aliases):
        if not isinstance(raw, dict) or set(raw) != expected_keys:
            raise RepositoryAliasError(
                f"repository alias {index} must contain exactly {sorted(expected_keys)}"
            )
        sealed = _full_name(
            raw.get("sealed_full_name"), f"repository alias {index} sealed_full_name"
        )
        remote = _full_name(
            raw.get("remote_full_name"), f"repository alias {index} remote_full_name"
        )
        repository_id = raw.get("repository_id")
        if not isinstance(repository_id, int) or isinstance(repository_id, bool) or repository_id <= 0:
            raise RepositoryAliasError(f"repository alias {sealed} has an invalid repository_id")

        sealed_key = sealed.casefold()
        remote_key = remote.casefold()
        if sealed_key not in sealed_by_key:
            raise RepositoryAliasError(f"repository alias source is not in the sealed fleet: {sealed}")
        if sealed != sealed_by_key[sealed_key]:
            raise RepositoryAliasError(
                f"repository alias source casing differs from the sealed ledger: {sealed}"
            )
        if sealed_key == remote_key:
            raise RepositoryAliasError(f"repository alias {sealed} does not change identity")
        if remote_key in sealed_by_key:
            raise RepositoryAliasError(
                f"repository alias target collides with a sealed identity: {remote}"
            )
        if sealed_key in aliases:
            raise RepositoryAliasError(f"duplicate alias source: {sealed}")
        if remote_key in remote_names:
            raise RepositoryAliasError(f"duplicate alias target: {remote}")
        if repository_id in repository_ids:
            raise RepositoryAliasError(f"duplicate alias repository ID: {repository_id}")

        aliases[sealed_key] = RepositoryAlias(sealed, remote, repository_id)
        remote_names.add(remote_key)
        repository_ids.add(repository_id)

    return aliases


def load_repository_aliases(
    path: Path,
    *,
    sealed_full_names: Iterable[str],
    expected_source_repository: str,
    expected_source_sha: str,
) -> dict[str, RepositoryAlias]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise RepositoryAliasError(f"repository-alias ledger is missing: {path}") from error
    except json.JSONDecodeError as error:
        raise RepositoryAliasError(f"repository-alias ledger is not valid JSON: {path}") from error
    return validate_repository_alias_payload(
        payload,
        sealed_full_names=sealed_full_names,
        expected_source_repository=expected_source_repository,
        expected_source_sha=expected_source_sha,
    )
