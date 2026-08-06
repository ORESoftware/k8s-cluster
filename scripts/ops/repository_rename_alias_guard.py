#!/usr/bin/env python3
"""Preserve renamed GitHub repositories while recreating exact sealed names.

GitHub follows REST requests for a renamed repository to the new repository.
That convenience is unsafe for a create-only fleet publisher because the
response can make a missing canonical identity look like an unrelated existing
repository. This guard recognizes only a proven same-owner redirect, records the
redirect target's stable repository id and main head, exposes the old exact name
as missing, and verifies the target stayed unchanged after publication.
"""

from __future__ import annotations

from dataclasses import dataclass
import re
from typing import Any, Callable, TypeAlias
import urllib.error
import urllib.request


RepositoryApi: TypeAlias = Callable[
    [str, str, dict[str, object] | None], tuple[int, object | None]
]
MainRefLookup: TypeAlias = Callable[[str], str | None]
RedirectProbe: TypeAlias = Callable[[str], tuple[int, str | None]]
Emitter: TypeAlias = Callable[[str], None]

_REDIRECT_STATUSES = frozenset({301, 302, 307, 308})
_FULL_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
_LOCATION_RE = re.compile(
    r"(?:https://api\.github\.com)?/repositories/([1-9][0-9]*)/?$"
)


class RepositoryRenameAliasError(RuntimeError):
    """A repository response is not a safe, provable rename alias."""


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        request: urllib.request.Request,
        file_pointer: Any,
        code: int,
        message: str,
        headers: Any,
        new_url: str,
    ) -> None:
        return None


@dataclass(frozen=True)
class RedirectTargetSnapshot:
    requested_full_name: str
    target_full_name: str
    target_repository_id: int
    target_head: str


class RepositoryRenameAliasGuard:
    """Wrap a GitHub API so verified old-name redirects behave as missing."""

    def __init__(
        self,
        *,
        api_base: str,
        token: str,
        api: RepositoryApi,
        main_ref_lookup: MainRefLookup,
        canonical_full_names: set[str] | frozenset[str],
        redirect_probe: RedirectProbe | None = None,
        emit: Emitter = print,
    ) -> None:
        if not api_base.startswith("https://"):
            raise ValueError("api_base must use HTTPS")
        if not token:
            raise ValueError("token is required")
        canonical = {value.casefold() for value in canonical_full_names}
        if not canonical or any(_FULL_NAME_RE.fullmatch(value) is None for value in canonical):
            raise ValueError("canonical repository identities are malformed")

        self._api_base = api_base.rstrip("/")
        self._token = token
        self._api = api
        self._main_ref_lookup = main_ref_lookup
        self._canonical = frozenset(canonical)
        self._redirect_probe = redirect_probe or self._probe_without_redirect
        self._emit = emit
        self._snapshots: dict[str, RedirectTargetSnapshot] = {}

    @property
    def snapshots(self) -> tuple[RedirectTargetSnapshot, ...]:
        return tuple(
            self._snapshots[key]
            for key in sorted(self._snapshots, key=str.casefold)
        )

    @staticmethod
    def _validate_private_repository(
        expected_full_name: str,
        payload: object,
    ) -> dict[str, object]:
        if not isinstance(payload, dict):
            raise RepositoryRenameAliasError(
                f"invalid repository metadata for {expected_full_name}"
            )
        actual_name = payload.get("full_name")
        if (
            not isinstance(actual_name, str)
            or actual_name.casefold() != expected_full_name.casefold()
        ):
            raise RepositoryRenameAliasError(
                f"repository identity mismatch for {expected_full_name}: {actual_name!r}"
            )
        repository_id = payload.get("id")
        if not isinstance(repository_id, int) or repository_id <= 0:
            raise RepositoryRenameAliasError(
                f"repository {expected_full_name} lacks a stable id"
            )
        if payload.get("private") is not True or payload.get("visibility") != "private":
            raise RepositoryRenameAliasError(
                f"repository {expected_full_name} is not private"
            )
        if payload.get("default_branch") != "main":
            raise RepositoryRenameAliasError(
                f"repository {expected_full_name} does not default to main"
            )
        if payload.get("archived") is True or payload.get("disabled") is True:
            raise RepositoryRenameAliasError(
                f"repository {expected_full_name} is archived or disabled"
            )
        return payload

    def _probe_without_redirect(self, requested_full_name: str) -> tuple[int, str | None]:
        request = urllib.request.Request(
            f"{self._api_base}/repos/{requested_full_name}",
            method="GET",
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "sealed-fleet-rename-alias-guard",
            },
        )
        opener = urllib.request.build_opener(_NoRedirectHandler())
        try:
            with opener.open(request, timeout=30) as response:
                return response.status, response.headers.get("Location")
        except urllib.error.HTTPError as error:
            if error.code in _REDIRECT_STATUSES:
                return error.code, error.headers.get("Location")
            if error.code == 404:
                return 404, None
            raw = error.read(4096).decode("utf-8", errors="replace")
            raise RepositoryRenameAliasError(
                f"GitHub API {error.code} while probing {requested_full_name}: {raw}"
            ) from error
        except urllib.error.URLError as error:
            raise RepositoryRenameAliasError(
                f"GitHub API unavailable while probing {requested_full_name}: {error}"
            ) from error

    def _snapshot_redirect(
        self,
        requested_full_name: str,
        followed_payload: dict[str, object],
    ) -> RedirectTargetSnapshot:
        target_name = followed_payload.get("full_name")
        if not isinstance(target_name, str) or _FULL_NAME_RE.fullmatch(target_name) is None:
            raise RepositoryRenameAliasError(
                f"invalid redirect target for {requested_full_name}"
            )
        if target_name.casefold() == requested_full_name.casefold():
            raise RepositoryRenameAliasError(
                f"GitHub returned mismatched metadata without changing {requested_full_name}"
            )
        requested_owner, _ = requested_full_name.split("/", 1)
        target_owner, _ = target_name.split("/", 1)
        if target_owner.casefold() != requested_owner.casefold():
            raise RepositoryRenameAliasError(
                f"redirect target owner mismatch for {requested_full_name}: {target_name}"
            )
        if target_name.casefold() in self._canonical:
            raise RepositoryRenameAliasError(
                f"redirect target {target_name} is also a canonical fleet identity"
            )

        target = self._validate_private_repository(target_name, followed_payload)
        status, location = self._redirect_probe(requested_full_name)
        if status not in _REDIRECT_STATUSES:
            raise RepositoryRenameAliasError(
                f"identity mismatch for {requested_full_name} is not a GitHub redirect"
            )
        match = _LOCATION_RE.fullmatch(location or "")
        if match is None:
            raise RepositoryRenameAliasError(
                f"unrecognized redirect location for {requested_full_name}: {location!r}"
            )
        repository_id = int(match.group(1))
        if target.get("id") != repository_id:
            raise RepositoryRenameAliasError(
                f"redirect repository id mismatch for {requested_full_name}"
            )
        target_head = self._main_ref_lookup(target_name)
        if not isinstance(target_head, str) or _SHA_RE.fullmatch(target_head) is None:
            raise RepositoryRenameAliasError(
                f"redirect target {target_name} has no valid main SHA"
            )
        return RedirectTargetSnapshot(
            requested_full_name=requested_full_name,
            target_full_name=target_name,
            target_repository_id=repository_id,
            target_head=target_head,
        )

    def repository_lookup(
        self,
        full_name: str,
    ) -> tuple[int, dict[str, object] | None]:
        status, payload = self._api("GET", f"/repos/{full_name}", None)
        if status != 200:
            if payload is not None and not isinstance(payload, dict):
                raise RepositoryRenameAliasError(
                    f"invalid repository response for {full_name}"
                )
            return status, payload
        if not isinstance(payload, dict):
            raise RepositoryRenameAliasError(
                f"invalid repository response for {full_name}"
            )

        actual_name = payload.get("full_name")
        if isinstance(actual_name, str) and actual_name.casefold() == full_name.casefold():
            return status, payload

        snapshot = self._snapshot_redirect(full_name, payload)
        key = full_name.casefold()
        previous = self._snapshots.get(key)
        if previous is not None and previous != snapshot:
            raise RepositoryRenameAliasError(
                f"redirect target changed during publication for {full_name}"
            )
        if previous is None:
            self._snapshots[key] = snapshot
            self._emit(
                "PRESERVE_RENAMED_TARGET "
                f"{snapshot.requested_full_name} -> {snapshot.target_full_name} "
                f"id={snapshot.target_repository_id} head={snapshot.target_head}"
            )

        # The exact requested identity is absent. Returning 404 allows the
        # create-only publisher to recreate that name without touching target.
        return 404, None

    def api(
        self,
        method: str,
        path: str,
        body: dict[str, object] | None = None,
    ) -> tuple[int, object | None]:
        match = re.fullmatch(r"/repos/([^/]+)/([^/]+)", path)
        if method == "GET" and body is None and match is not None:
            return self.repository_lookup(f"{match.group(1)}/{match.group(2)}")
        return self._api(method, path, body)

    def verify_preserved(self) -> None:
        """Require every rename target to retain the same id and main head."""

        for snapshot in self.snapshots:
            status, payload = self._api(
                "GET", f"/repos/{snapshot.target_full_name}", None
            )
            if status != 200:
                raise RepositoryRenameAliasError(
                    f"redirect target verification failed for "
                    f"{snapshot.target_full_name}: HTTP {status}"
                )
            target = self._validate_private_repository(
                snapshot.target_full_name, payload
            )
            if target.get("id") != snapshot.target_repository_id:
                raise RepositoryRenameAliasError(
                    f"redirect target identity changed for {snapshot.target_full_name}"
                )
            actual_head = self._main_ref_lookup(snapshot.target_full_name)
            if actual_head != snapshot.target_head:
                raise RepositoryRenameAliasError(
                    f"redirect target {snapshot.target_full_name} changed during "
                    f"alias recreation: {actual_head!r} != {snapshot.target_head!r}"
                )
            self._emit(
                "VERIFIED_PRESERVED_RENAMED_TARGET "
                f"{snapshot.target_full_name} {snapshot.target_head}"
            )
