#!/usr/bin/env python3
"""Publish the exact reviewed Canonical public dependency repositories.

The caller supplies a protected GitHub credential through a temporary hosts
file. This script cannot create, rename, archive, delete, or broaden scope
beyond the exact repository IDs below. A private target is published only when
its reviewed ``main`` head still matches the pinned commit.
"""
from __future__ import annotations

import argparse
import ast
import json
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

API = "https://api.github.com"
ORG = "canonical-cloud"
REPOSITORIES: dict[str, dict[str, Any]] = {
    "canonical-api-server.rs": {
        "id": 1324644222,
        "description": (
            "Canonical quote REST and WebSocket API in Rust with Axum, "
            "SeaORM, Shared Auth, and Gemini analysis."
        ),
        "expected_main_sha": None,
    },
    "canonical-lib-core": {
        "id": 1346446456,
        "description": (
            "Canonical shared contract implementations, pure domain validation, "
            "and deterministic conformance assets."
        ),
        "expected_main_sha": "3fee2f107c6c23286e7cd163236acebf4b1d016e",
    },
}


class PublishError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise PublishError(message)


def validate_scope() -> None:
    expected = {
        "canonical-api-server.rs": (1324644222, None),
        "canonical-lib-core": (
            1346446456,
            "3fee2f107c6c23286e7cd163236acebf4b1d016e",
        ),
    }
    actual = {
        name: (record.get("id"), record.get("expected_main_sha"))
        for name, record in REPOSITORIES.items()
    }
    if actual != expected:
        fail("bounded repository identities or reviewed heads changed")

    for name, record in REPOSITORIES.items():
        description = record.get("description")
        if re.fullmatch(r"canonical-[A-Za-z0-9._-]+", name) is None:
            fail(f"invalid bounded repository name: {name!r}")
        if not isinstance(record.get("id"), int) or record["id"] <= 0:
            fail(f"invalid bounded repository ID: {name!r}")
        if not isinstance(description, str) or not description.strip():
            fail(f"invalid bounded repository description: {name!r}")
        expected_main_sha = record.get("expected_main_sha")
        if expected_main_sha is not None and re.fullmatch(
            r"[0-9a-f]{40}", expected_main_sha
        ) is None:
            fail(f"invalid reviewed main SHA: {name!r}")


def token_from_hosts(path: Path) -> str:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        fail(f"protected GitHub profile is unavailable: {error}")
    current_host: str | None = None
    for raw in lines:
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if len(raw) == len(raw.lstrip()) and stripped.endswith(":"):
            current_host = stripped[:-1]
            continue
        if current_host != "github.com":
            continue
        match = re.match(r"^\s+oauth_token:\s*(.*?)\s*$", raw)
        if match is None:
            continue
        value: Any = match.group(1)
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
            value = ast.literal_eval(value)
        if not isinstance(value, str) or not value or any(ch.isspace() for ch in value):
            fail("protected GitHub token is malformed")
        return value
    fail("github.com oauth_token is missing from the protected profile")


class GitHubApi:
    def __init__(self, token: str) -> None:
        self.headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "canonical-dependency-publication/2026-09-01",
        }

    def request(
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
        *,
        expected: tuple[int, ...] = (200,),
    ) -> dict[str, Any] | list[Any] | None:
        body = None
        headers = dict(self.headers)
        if payload is not None:
            body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(API + path, data=body, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                raw = response.read()
                if response.status not in expected:
                    fail(f"GitHub {method} {path} returned HTTP {response.status}")
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            detail = error.read(4096).decode("utf-8", "replace")
            fail(f"GitHub {method} {path} failed with HTTP {error.code}: {detail[:500]}")
        except urllib.error.URLError as error:
            fail(f"GitHub {method} {path} transport failed: {error.reason}")


def verify_identity(api: GitHubApi) -> None:
    identity = api.request("GET", "/user")
    if not isinstance(identity, dict) or identity.get("login") != "ORESoftware":
        fail(f"unexpected protected publisher identity: {identity!r}")
    membership = api.request("GET", f"/user/memberships/orgs/{ORG}")
    if not isinstance(membership, dict) or (
        membership.get("role"), membership.get("state")
    ) != ("admin", "active"):
        fail(f"{ORG} owner membership is not active admin")
    print(f"VERIFIED publisher=ORESoftware org={ORG} membership=admin:active")


def verify_repository_identity(
    repository: dict[str, Any],
    *,
    full_name: str,
    expected_id: int,
) -> None:
    if str(repository.get("full_name", "")).casefold() != full_name.casefold():
        fail(f"repository alias/redirect mismatch for {full_name}")
    if repository.get("id") != expected_id:
        fail(f"stable repository ID mismatch for {full_name}")
    owner = repository.get("owner")
    if not isinstance(owner, dict) or owner.get("login") != ORG:
        fail(f"repository owner mismatch for {full_name}")
    if repository.get("fork") is not False:
        fail(f"refusing to modify fork or ambiguous repository {full_name}")
    if repository.get("archived") is True:
        fail(f"refusing to modify archived repository {full_name}")
    if repository.get("default_branch") != "main":
        fail(f"{full_name} default branch must already be main")


def verify_reviewed_main(
    api: GitHubApi,
    *,
    full_name: str,
    expected_sha: str | None,
) -> None:
    if expected_sha is None:
        return
    branch = api.request("GET", f"/repos/{full_name}/branches/main")
    if not isinstance(branch, dict):
        fail(f"GitHub returned no main branch object for {full_name}")
    commit = branch.get("commit")
    actual_sha = commit.get("sha") if isinstance(commit, dict) else None
    if actual_sha != expected_sha:
        fail(
            f"{full_name} main moved after review: expected {expected_sha}, got {actual_sha}"
        )
    print(f"VERIFIED_REVIEWED_HEAD {full_name} main={actual_sha}")


def publish_repository(api: GitHubApi, name: str, record: dict[str, Any]) -> None:
    full_name = f"{ORG}/{name}"
    expected_id = int(record["id"])
    description = str(record["description"])
    expected_main_sha = record.get("expected_main_sha")

    repository = api.request("GET", f"/repos/{full_name}")
    if not isinstance(repository, dict):
        fail(f"GitHub returned no repository object for {full_name}")
    verify_repository_identity(
        repository,
        full_name=full_name,
        expected_id=expected_id,
    )
    verify_reviewed_main(
        api,
        full_name=full_name,
        expected_sha=expected_main_sha,
    )

    disposition = "PRESERVED_PUBLIC"
    if repository.get("private") is not False:
        repository = api.request(
            "PATCH",
            f"/repos/{full_name}",
            {
                "description": description,
                "private": False,
                "has_issues": True,
                "has_projects": True,
                "has_wiki": False,
                "delete_branch_on_merge": True,
            },
        )
        disposition = "PUBLISHED"

    repository = api.request("GET", f"/repos/{full_name}")
    if not isinstance(repository, dict):
        fail(f"GitHub returned no verification object for {full_name}")
    verify_repository_identity(
        repository,
        full_name=full_name,
        expected_id=expected_id,
    )
    if repository.get("private") is not False:
        fail(f"{full_name} public visibility verification failed")
    if repository.get("visibility") != "public":
        fail(f"{full_name} visibility field is not public")
    print(f"VERIFIED_{disposition} {full_name} visibility=public default_branch=main")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--trusted-sha")
    parser.add_argument(
        "--hosts-file",
        type=Path,
        default=Path.home() / ".config" / "gh" / "hosts.yml",
    )
    args = parser.parse_args()
    validate_scope()
    if args.validate_only:
        print(
            "VERIFIED bounded Canonical dependency publication "
            "repositories=2 target=canonical-lib-core"
        )
        return 0
    if not args.trusted_sha or re.fullmatch(r"[0-9a-f]{40}", args.trusted_sha) is None:
        fail("--trusted-sha must be a full lowercase commit SHA")

    api = GitHubApi(token_from_hosts(args.hosts_file))
    verify_identity(api)
    print(f"VERIFIED trusted_source={args.trusted_sha}")
    for name, record in REPOSITORIES.items():
        publish_repository(api, name, record)
    print("VERIFIED Canonical dependency publication repositories=2 destructive_actions=0")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublishError as error:
        print(f"publication failed: {error}", file=sys.stderr)
        raise SystemExit(1)
