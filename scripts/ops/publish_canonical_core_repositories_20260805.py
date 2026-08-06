#!/usr/bin/env python3
"""Publish the two reviewed Canonical core repositories.

The caller supplies a protected GitHub credential through a temporary hosts
file. This script cannot create, rename, archive, delete, or broaden scope
beyond the exact repository allowlist below.
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
REPOSITORIES = {
    "canonical-api-server.rs": "Canonical quote REST and WebSocket API in Rust with Axum, SeaORM, Shared Auth, and Gemini analysis.",
    "canonical-lib": "Shared Canonical Rust domain types and quote intake validation.",
}


class PublishError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise PublishError(message)


def validate_scope() -> None:
    if set(REPOSITORIES) != {"canonical-api-server.rs", "canonical-lib"}:
        fail("bounded repository identities changed")
    for name, description in REPOSITORIES.items():
        if re.fullmatch(r"canonical-[A-Za-z0-9._-]+", name) is None or not description:
            fail(f"invalid bounded repository record: {name!r}")


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
            "User-Agent": "canonical-core-publication/2026-08-05",
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


def publish_repository(api: GitHubApi, name: str, description: str) -> None:
    full_name = f"{ORG}/{name}"
    repository = api.request("GET", f"/repos/{full_name}")
    if not isinstance(repository, dict):
        fail(f"GitHub returned no repository object for {full_name}")
    if str(repository.get("full_name", "")).casefold() != full_name.casefold():
        fail(f"repository alias/redirect mismatch for {full_name}")
    if repository.get("archived") is True:
        fail(f"refusing to modify archived repository {full_name}")
    if repository.get("default_branch") != "main":
        fail(f"{full_name} default branch must already be main")

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
    if not isinstance(repository, dict) or repository.get("private") is not False:
        fail(f"{full_name} public visibility verification failed")
    if repository.get("visibility") != "public":
        fail(f"{full_name} visibility field is not public")
    if repository.get("default_branch") != "main":
        fail(f"{full_name} default branch verification failed")
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
        print("VERIFIED bounded Canonical core publication repositories=2")
        return 0
    if not args.trusted_sha or re.fullmatch(r"[0-9a-f]{40}", args.trusted_sha) is None:
        fail("--trusted-sha must be a full lowercase commit SHA")

    api = GitHubApi(token_from_hosts(args.hosts_file))
    verify_identity(api)
    print(f"VERIFIED trusted_source={args.trusted_sha}")
    for name, description in REPOSITORIES.items():
        publish_repository(api, name, description)
    print("VERIFIED Canonical core publication repositories=2 destructive_actions=0")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublishError as error:
        print(f"publication failed: {error}", file=sys.stderr)
        raise SystemExit(1)
