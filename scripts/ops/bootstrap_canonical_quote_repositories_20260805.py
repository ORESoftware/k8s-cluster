#!/usr/bin/env python3
"""Create the bounded missing Canonical quote repositories.

This runs only after a reviewed workflow obtains a short-lived or one-use
repository-administration credential. Existing repositories are preserved.
"""
from __future__ import annotations

import argparse
import ast
import json
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

API = "https://api.github.com"
ORG = "canonical-cloud"
REPOSITORIES = {
    "canonical-api-server.rs": "Canonical quote REST and WebSocket API in Rust with Axum, SeaORM, Shared Auth, and Gemini analysis.",
    "canonical-infra": "Canonical Cloudflare, Kubernetes, Postgres, and deployment infrastructure.",
    "canonical-lib": "Shared Canonical Rust domain types and quote intake validation.",
    "canonical-flutter": "Canonical Flutter companion application for authenticated quote workflows.",
}


class BootstrapError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise BootstrapError(message)


def validate_scope() -> None:
    if set(REPOSITORIES) != {
        "canonical-api-server.rs",
        "canonical-infra",
        "canonical-lib",
        "canonical-flutter",
    }:
        fail("bounded repository identities changed")
    for name, description in REPOSITORIES.items():
        if re.fullmatch(r"canonical-[A-Za-z0-9._-]+", name) is None or not description:
            fail(f"invalid bounded repository record: {name!r}")


def selected_repositories(values: list[str] | None) -> list[str]:
    selected = list(REPOSITORIES) if values is None else values
    if not selected:
        fail("at least one repository must be selected")
    if len(selected) != len(set(selected)):
        fail("duplicate repository selections are not allowed")
    unexpected = set(selected) - set(REPOSITORIES)
    if unexpected:
        fail(f"repository selection escaped bounded scope: {sorted(unexpected)}")
    return selected


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
        indent = len(raw) - len(raw.lstrip())
        if indent == 0 and stripped.endswith(":"):
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
            "User-Agent": "canonical-quote-repository-bootstrap/2026-08-05",
        }

    def request(
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
        *,
        expected: tuple[int, ...] = (200,),
        allow_missing: bool = False,
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
            if allow_missing and error.code == 404:
                return None
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
    print(f"VERIFIED protected publisher=ORESoftware org={ORG} membership=admin:active")


def ensure_repository(api: GitHubApi, name: str, description: str) -> None:
    full_name = f"{ORG}/{name}"
    repository = api.request("GET", f"/repos/{full_name}", allow_missing=True)
    disposition = "PRESERVED"
    if repository is None:
        repository = api.request(
            "POST",
            f"/orgs/{ORG}/repos",
            {
                "name": name,
                "description": description,
                "private": True,
                "has_issues": True,
                "has_projects": True,
                "has_wiki": False,
                "auto_init": True,
            },
            expected=(201,),
        )
        disposition = "CREATED"
    if not isinstance(repository, dict):
        fail(f"GitHub returned no repository object for {full_name}")
    if str(repository.get("full_name", "")).casefold() != full_name.casefold():
        fail(f"repository alias/redirect mismatch for {full_name}")
    if repository.get("private") is not True:
        fail(f"{full_name} is not private")

    default_branch = repository.get("default_branch")
    if not isinstance(default_branch, str) or not default_branch:
        fail(f"{full_name} has no initialized default branch")
    if default_branch != "main":
        encoded = urllib.parse.quote(default_branch, safe="")
        api.request(
            "POST",
            f"/repos/{full_name}/branches/{encoded}/rename",
            {"new_name": "main"},
            expected=(201,),
        )

    repository = api.request(
        "PATCH",
        f"/repos/{full_name}",
        {
            "description": description,
            "default_branch": "main",
            "private": True,
            "has_issues": True,
            "has_projects": True,
            "has_wiki": False,
            "delete_branch_on_merge": True,
        },
    )
    if not isinstance(repository, dict) or repository.get("private") is not True:
        fail(f"{full_name} post-write privacy verification failed")
    if repository.get("default_branch") != "main":
        fail(f"{full_name} default branch verification failed")
    print(f"VERIFIED_{disposition}_PRIVATE {full_name} default_branch=main")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--trusted-sha")
    parser.add_argument(
        "--repository",
        action="append",
        choices=sorted(REPOSITORIES),
        dest="repositories",
        help="Create or reconcile only this bounded repository (repeatable).",
    )
    parser.add_argument(
        "--hosts-file",
        type=Path,
        default=Path.home() / ".config" / "gh" / "hosts.yml",
    )
    args = parser.parse_args()
    validate_scope()
    targets = selected_repositories(args.repositories)
    if args.validate_only:
        print(
            "VERIFIED bounded Canonical quote repository bootstrap "
            f"available={len(REPOSITORIES)} selected={len(targets)}"
        )
        return 0
    if not args.trusted_sha or re.fullmatch(r"[0-9a-f]{40}", args.trusted_sha) is None:
        fail("--trusted-sha must be a full lowercase commit SHA")
    api = GitHubApi(token_from_hosts(args.hosts_file))
    verify_identity(api)
    print(f"VERIFIED trusted_source={args.trusted_sha}")
    for name in targets:
        ensure_repository(api, name, REPOSITORIES[name])
    print(
        "VERIFIED Canonical quote repository bootstrap "
        f"selected={len(targets)} overwrite=0"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BootstrapError as error:
        print(f"bootstrap failed: {error}", file=sys.stderr)
        raise SystemExit(1)
