#!/usr/bin/env python3
"""Create and verify the exact private HHaus repository fleet.

The caller supplies a protected GitHub repository-administration credential via
GH_TOKEN or GITHUB_REPOSITORY_ADMIN_TOKEN. The credential is never serialized,
logged, committed, uploaded, or explicitly revoked by this program.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

API = "https://api.github.com"
ORG = "hhaus-org"
ACTOR = "ORESoftware"
API_VERSION = "2022-11-28"
USER_AGENT = "hhaus-standard-repository-publisher/1"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")

REPOSITORIES: tuple[tuple[str, str], ...] = (
    (
        "hhaus-clients",
        "Transport-injected HHaus clients in 17 languages consuming parity-gated interfaces and shared core through zed-pkg",
    ),
    (
        "hhaus-sync",
        "Offline-first HHaus synchronization across client, IndexedDB, SQLite, Supabase, PostgreSQL, and backend layers",
    ),
    (
        "hhaus-lib-core",
        "Shared HHaus client, server, edge, and isomorphic policy and domain library",
    ),
    (
        "hhaus-orm-core",
        "Private backend-only HHaus persistence library with Diesel and SeaORM parity checks",
    ),
    (
        "hhaus-flutter",
        "HHaus Flutter application for iOS, Android, web, macOS, Linux, and Windows",
    ),
    (
        "hhaus-desktop-app.rs",
        "Native Rust HHaus desktop application with shared-auth, sync, telemetry, and layered rate limiting",
    ),
    (
        "hhaus-lambdas",
        "Provider-neutral HHaus cross-platform function runtime and provider adapters",
    ),
    (
        "hhaus-interfaces",
        "Independent TypeSpec and JSON Schema HHaus contracts with generated interfaces for 17 languages",
    ),
)


@dataclass(frozen=True)
class ApiResponse:
    status: int
    payload: Any


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


def token_from_environment() -> str:
    token = (
        os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN", "").strip()
        or os.environ.get("GH_TOKEN", "").strip()
    )
    if not token or any(character.isspace() for character in token):
        raise RuntimeError("protected repository-administration credential is missing or malformed")
    return token


def api_request(
    token: str,
    method: str,
    path: str,
    payload: dict[str, object] | None = None,
) -> ApiResponse:
    if not path.startswith("/") or ".." in path:
        raise ValueError(f"unsafe API path: {path!r}")
    data = None if payload is None else json.dumps(payload, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        API + path,
        data=data,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": USER_AGENT,
            **({"Content-Type": "application/json"} if data is not None else {}),
        },
    )
    opener = urllib.request.build_opener(NoRedirect())
    try:
        with opener.open(request, timeout=30) as response:
            raw = response.read()
            return ApiResponse(
                response.status,
                json.loads(raw.decode("utf-8")) if raw else None,
            )
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            decoded: Any = json.loads(raw.decode("utf-8")) if raw else None
        except (UnicodeDecodeError, json.JSONDecodeError):
            decoded = None
        return ApiResponse(error.code, decoded)
    except (urllib.error.URLError, TimeoutError) as error:
        raise RuntimeError(f"GitHub API transport failed for {method} {path}") from error


def require_mapping(response: ApiResponse, operation: str) -> dict[str, Any]:
    if not isinstance(response.payload, dict):
        raise RuntimeError(f"{operation} returned non-object payload at HTTP {response.status}")
    return response.payload


def preflight(token: str) -> None:
    user = api_request(token, "GET", "/user")
    if user.status != 200 or require_mapping(user, "actor preflight").get("login") != ACTOR:
        raise RuntimeError("protected credential does not authenticate the required ORESoftware actor")

    membership = api_request(token, "GET", f"/user/memberships/orgs/{ORG}")
    value = require_mapping(membership, "organization membership preflight")
    if membership.status != 200 or value.get("role") != "admin" or value.get("state") != "active":
        raise RuntimeError("protected credential is not an active hhaus-org administrator")


def create_payload(name: str, description: str) -> dict[str, object]:
    return {
        "name": name,
        "description": description,
        "private": True,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "is_template": False,
        "auto_init": True,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }


def patch_payload(description: str) -> dict[str, object]:
    return {
        "description": description,
        "private": True,
        "visibility": "private",
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "allow_auto_merge": False,
        "delete_branch_on_merge": True,
    }


def validate_repository(value: dict[str, Any], name: str, description: str) -> dict[str, object]:
    full_name = f"{ORG}/{name}"
    expected = {
        "full_name": full_name,
        "private": True,
        "visibility": "private",
        "default_branch": "main",
        "description": description,
        "archived": False,
        "disabled": False,
        "has_issues": True,
        "has_projects": False,
      "has_wiki": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "allow_auto_merge": False,
        "delete_branch_on_merge": True,
    }
    for key, expected_value in expected.items():
        if value.get(key) != expected_value:
            raise RuntimeError(
                f"repository invariant mismatch for {full_name}: {key}={value.get(key)!r} expect {expected_value!r}"
            )
    repository_id = value.get("id")
    if not isinstance(repository_id, int) or repository_id <= 0:
        raise RuntimeError(f"repository id missing for {full_name}")
    return {"full_name": full_name, "repository_id": repository_id}


def ensure_repository(token: str, name: str, description: str) -> dict[str, object]:
    full_name = f"{ORG}/{name}"
    inspected = api_request(token, "GET", f"/repos/{full_name}")
    created = False
    if inspected.status == 404:
        created_response = api_request(token, "POST", f"/orgs/{ORG}/repos", create_payload(name, description))
        if created_response.status == 201:
            created = True
        elif created_response.status not in {409, 422}:
            raise RuntimeError(f"repository creation failed for {full_name}: HTTP {created_response.status}")
    elif inspected.status != 200:
        raise RuntimeError(f"repository inspection failed for {full_name}: HTTP {inspected.status}")

    patched = api_request(token, "PATCH", f"/repos/{full_name}", patch_payload(description))
    if patched.status != 200:
        raise RuntimeError(f"repository policy update failed for {full_name}: HTTP {patched.status}")
    metadata = require_mapping(patched, f"repository policy update for {full_name}")
    result = validate_repository(metadata, name, description)

    reference = api_request(token, "GET", f"/repos/{full_name}/git/ref/heads/main")
    if reference.status != 200:
        raise RuntimeError(f"main ref missing for {full_name}: HTTP {reference.status}")
    reference_payload = require_mapping(reference, f"main ref for {full_name}")
    object_payload = reference_payload.get("object")
    main_sha = object_payload.get("sha") if isinstance(object_payload, dict) else None
    if not isinstance(main_sha, str) or SHA_RE.fullmatch(main_sha) is None:
        raise RuntimeError(f"invalid main SHA for {full_name}")

    return {**result, "created": created, "visibility": "private", "default_branch": "main", "main_sha": main_sha}


def publish(evidence_path: Path) -> int:
    token = token_from_environment()
    preflight(token)
    records: list[dict[str, object]] = []
    for name, description in REPOSITORIES:
        record = ensure_repository(token, name, description)
        records.append(record)
        print(
            "HHAUS_REPOSITORY_READY "
            f"repository={record['full_name']} created={str(record['created']).lower()} main={record['main_sha']}"
        )

    evidence = {
        "schema_version": 1,
        "organization": ORG,
        "actor": ACTOR,
        "repository_count": len(records),
        "repositories": records,
        "credential_source": "protected-actions-secret",
        "credential_persisted": False,
        "credential_revoked": False,
    }
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    evidence_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    evidence_path.chmod(0o600)
    print(f"HHAUS_STANDARD_REPOSITORIES_READY total={len(records)}")
    return 0


def self_test() -> int:
    names = [name for name, _ in REPOSITORIES]
    if len(names) != 8 or len(set(names)) != 8:
        raise RuntimeError("repository allowlist must contain exactly eight unique names")
    expected = {
        "hhaus-clients",
        "hhaus-sync",
        "hhaus-lib-core",
        "hhaus-orm-core",
        "hhaus-flutter",
        "hhaus-desktop-app.rs",
        "hhaus-lambdas",
        "hhaus-interfaces",
    }
    if set(names) != expected:
        raise RuntimeError("repository allowlist differs from the approved fleet")
    for name, description in REPOSITORIES:
        payload = create_payload(name, description)
        if payload["private"] is not True or payload["auto_init"] is not True:
            raise RuntimeError(f"unsafe create payload for {name}")
        if patch_payload(description)["visibility"] != "private":
            raise RuntimeError(f"unsafe visibility payload for {name}")
    sample = {
        "id": 42,
        "full_name": f"{ORG}/hhaus-clients",
        "private": True,
        "visibility": "private",
        "default_branch": "main",
        "description": REPOSITORIES[0][1],
        "archived": False,
        "disabled": False,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "allow_auto_merge": False,
        "delete_branch_on_merge": True,
    }
    validate_repository(sample, "hhaus-clients", REPOSITORIES[0][1])
    unsafe = dict(sample, visibility="public")
    try:
        validate_repository(unsafe, "hhaus-clients", REPOSITORIES[0][1])
    except RuntimeError:
        pass
    else:
        raise RuntimeError("public visibility must fail closed")
    print("HHaus direct repository publisher self-test passed")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--evidence",
        type=Path,
        default=Path("ops/evidence/hhaus-standard-repositories/direct-publication.json"),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    return self_test() if args.self_test else publish(args.evidence)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:  # bounded, credential-free failure surface
        print(f"hhaus-direct-publisher=failed reason={type(error).__name__}: {error}", file=sys.stderr)
        raise SystemExit(1)
