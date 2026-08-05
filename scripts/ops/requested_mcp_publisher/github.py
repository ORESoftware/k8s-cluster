"""Bounded GitHub API client and repository metadata checks."""

from __future__ import annotations

import json
import re
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

from .model import PublisherError, RepositorySpec

API = "https://api.github.com"
EXPECTED_LOGIN = "ORESoftware"
MAX_API_RESPONSE_BYTES = 2 * 1024 * 1024


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        raise PublisherError(f"unexpected GitHub API redirect: HTTP {code}")


class GitHubClient:
    def __init__(self, token: str):
        if not token or "\n" in token or "\r" in token:
            raise PublisherError("GH_TOKEN must be a nonempty single-line value")
        self._token = token
        self._opener = urllib.request.build_opener(_NoRedirect())

    def request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        *,
        allow_not_found: bool = False,
        allow_empty_repository: bool = False,
    ) -> tuple[int, Any | None]:
        if not path.startswith("/") or "?" in path or "#" in path:
            raise PublisherError(f"invalid GitHub API path: {path!r}")
        payload = None if body is None else json.dumps(body, separators=(",", ":")).encode()
        request = urllib.request.Request(API + path, method=method, data=payload)
        request.add_header("Accept", "application/vnd.github+json")
        request.add_header("Authorization", f"Bearer {self._token}")
        request.add_header("X-GitHub-Api-Version", "2022-11-28")
        request.add_header("User-Agent", "requested-mcp-repository-publisher")
        if payload is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with self._opener.open(request, timeout=30) as response:
                raw = response.read(MAX_API_RESPONSE_BYTES + 1)
                if len(raw) > MAX_API_RESPONSE_BYTES:
                    raise PublisherError(f"GitHub API response too large for {method} {path}")
                return response.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            error.read(4096)
            if allow_not_found and error.code == 404:
                return 404, None
            if allow_empty_repository and error.code == 409:
                return 409, None
            raise PublisherError(f"GitHub API returned HTTP {error.code} for {method} {path}") from error


def preflight(client: GitHubClient, specs: tuple[RepositorySpec, ...]) -> None:
    status, identity = client.request("GET", "/user")
    if status != 200 or not isinstance(identity, dict) or identity.get("login") != EXPECTED_LOGIN:
        observed = identity.get("login") if isinstance(identity, dict) else None
        raise PublisherError(f"unexpected publisher identity: {observed!r}")

    for owner in sorted({spec.owner for spec in specs}, key=str.casefold):
        encoded_owner = urllib.parse.quote(owner, safe="")
        status, membership = client.request("GET", f"/user/memberships/orgs/{encoded_owner}")
        observed = (
            membership.get("role") if isinstance(membership, dict) else None,
            membership.get("state") if isinstance(membership, dict) else None,
        )
        if status != 200 or observed != ("admin", "active"):
            raise PublisherError(f"{owner} membership is not active admin: {observed!r}")


def repository(client: GitHubClient, spec: RepositorySpec) -> tuple[int, dict[str, Any] | None]:
    path = f"/repos/{urllib.parse.quote(spec.owner, safe='')}/{urllib.parse.quote(spec.name, safe='')}"
    status, payload = client.request("GET", path, allow_not_found=True)
    if payload is not None and not isinstance(payload, dict):
        raise PublisherError(f"invalid repository response for {spec.full_name}")
    return status, payload


def create_repository(client: GitHubClient, spec: RepositorySpec) -> dict[str, Any]:
    status, payload = client.request(
        "POST",
        f"/orgs/{urllib.parse.quote(spec.owner, safe='')}/repos",
        {
            "name": spec.name,
            "description": spec.description,
            "private": spec.private,
            "has_issues": True,
            "has_projects": False,
            "has_wiki": False,
            "auto_init": False,
            "allow_squash_merge": True,
            "allow_merge_commit": True,
            "allow_rebase_merge": False,
            "delete_branch_on_merge": True,
        },
    )
    if status != 201 or not isinstance(payload, dict):
        raise PublisherError(f"failed to create {spec.full_name}: HTTP {status}")
    return payload


def validate_repository_metadata(spec: RepositorySpec, payload: dict[str, Any]) -> None:
    if payload.get("full_name") != spec.full_name:
        raise PublisherError(f"repository identity mismatch for {spec.full_name}")
    if payload.get("private") is not spec.private or payload.get("visibility") != spec.visibility:
        raise PublisherError(f"repository visibility mismatch for {spec.full_name}")
    if payload.get("archived") is not False or payload.get("disabled") is not False:
        raise PublisherError(f"repository is archived or disabled: {spec.full_name}")


def main_ref(client: GitHubClient, spec: RepositorySpec) -> str | None:
    path = (
        f"/repos/{urllib.parse.quote(spec.owner, safe='')}/"
        f"{urllib.parse.quote(spec.name, safe='')}/git/ref/heads/main"
    )
    status, payload = client.request(
        "GET", path, allow_not_found=True, allow_empty_repository=True
    )
    if status in {404, 409}:
        return None
    if not isinstance(payload, dict):
        raise PublisherError(f"invalid main ref response for {spec.full_name}")
    target = payload.get("object")
    sha = target.get("sha") if isinstance(target, dict) else None
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise PublisherError(f"invalid main SHA for {spec.full_name}")
    return sha


def bootstrap_is_ancestor(
    client: GitHubClient, spec: RepositorySpec, bootstrap_sha: str, current_sha: str
) -> bool:
    if bootstrap_sha == current_sha:
        return True
    path = (
        f"/repos/{urllib.parse.quote(spec.owner, safe='')}/"
        f"{urllib.parse.quote(spec.name, safe='')}/compare/{bootstrap_sha}...{current_sha}"
    )
    status, payload = client.request("GET", path, allow_not_found=True)
    return status == 200 and isinstance(payload, dict) and payload.get("status") in {"ahead", "identical"}


def configure_repository(client: GitHubClient, spec: RepositorySpec) -> None:
    path = f"/repos/{urllib.parse.quote(spec.owner, safe='')}/{urllib.parse.quote(spec.name, safe='')}"
    status, _ = client.request(
        "PATCH",
        path,
        {
            "description": spec.description,
            "default_branch": "main",
            "has_issues": True,
            "has_projects": False,
            "has_wiki": False,
            "allow_squash_merge": True,
            "allow_merge_commit": True,
            "allow_rebase_merge": False,
            "delete_branch_on_merge": True,
        },
    )
    if status != 200:
        raise PublisherError(f"failed to configure {spec.full_name}: HTTP {status}")
