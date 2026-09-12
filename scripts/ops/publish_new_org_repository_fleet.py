#!/usr/bin/env python3
"""Create and initialize the bounded canonical repository fleet for newer orgs.

Safety properties:
- exact organization and repository allowlists;
- explicit fleet ID and repository-count confirmations for writes;
- no token command-line option and no credential logging;
- private product repositories by default; `.github` is public;
- existing non-empty repositories are never mutated;
- initial repository contents are one atomic root commit;
- every created `main` ref and repository identity is verified;
- reruns are idempotent and preserve existing histories.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import pathlib
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any, Iterable, Mapping, Protocol

from new_org_repository_templates import SEED_BUILDERS, files_for_repository

API_VERSION = "2022-11-28"
DEFAULT_API_ROOT = "https://api.github.com"
EXPECTED_FLEET_ID = "new-org-core-v1"
EXPECTED_REPOSITORY_COUNT = 65
ALLOWED_ORGANIZATIONS = {
    "channelsiege",
    "OmniBlitz",
    "streamkore",
    "hypeblitz",
    "meta-agents-demo",
    "networking-components",
    "unreal-unity-poc",
}
TOKEN_ENV = "GITHUB_REPOSITORY_ADMIN_TOKEN"
COMMIT_IDENTITY = {
    "name": "ORESoftware Repository Fleet Automation",
    "email": "repository-fleet@users.noreply.github.com",
    "date": "2026-08-04T12:00:00Z",
}


class FleetError(RuntimeError):
    """Raised when a fleet safety invariant or GitHub operation fails."""


@dataclass(frozen=True)
class ApiResponse:
    status: int
    data: Any
    headers: Mapping[str, str]


class ApiLike(Protocol):
    def request(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | list[Any] | None = None,
        *,
        allowed_statuses: Iterable[int] = (200,),
    ) -> ApiResponse: ...


class GitHubApi:
    """Minimal GitHub REST client that never exposes the bearer token."""

    def __init__(self, token: str, *, api_root: str = DEFAULT_API_ROOT, timeout: int = 45) -> None:
        if not token.strip():
            raise FleetError(f"{TOKEN_ENV} is empty")
        self._token = token.strip()
        self._api_root = api_root.rstrip("/")
        self._timeout = timeout

    def request(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | list[Any] | None = None,
        *,
        allowed_statuses: Iterable[int] = (200,),
    ) -> ApiResponse:
        allowed = set(allowed_statuses)
        body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode("utf-8")
        request = urllib.request.Request(
            f"{self._api_root}{path}",
            data=body,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "Content-Type": "application/json",
                "User-Agent": "oresoftware-new-org-repository-fleet/1",
                "X-GitHub-Api-Version": API_VERSION,
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=self._timeout) as response:
                raw = response.read()
                status = int(response.status)
                headers = dict(response.headers.items())
        except urllib.error.HTTPError as error:
            raw = error.read()
            status = int(error.code)
            headers = dict(error.headers.items()) if error.headers else {}
        except urllib.error.URLError as error:
            raise FleetError(f"GitHub request {method} {path} failed: {error.reason}") from error

        try:
            data: Any = json.loads(raw.decode("utf-8")) if raw else None
        except (UnicodeDecodeError, json.JSONDecodeError):
            data = {"raw": raw.decode("utf-8", errors="replace")[:2000]}

        if status not in allowed:
            message = data.get("message") if isinstance(data, dict) else None
            detail = f": {message}" if message else ""
            raise FleetError(f"GitHub request {method} {path} returned HTTP {status}{detail}")
        return ApiResponse(status=status, data=data, headers=headers)


def quote_segment(value: str) -> str:
    return urllib.parse.quote(value, safe="")


def repo_path(owner: str, name: str) -> str:
    return f"/repos/{quote_segment(owner)}/{quote_segment(name)}"


def canonical_manifest_bytes(path: pathlib.Path) -> bytes:
    document = json.loads(path.read_text(encoding="utf-8"))
    return (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")


def validate_manifest(manifest: Mapping[str, Any]) -> list[tuple[Mapping[str, Any], Mapping[str, Any]]]:
    if manifest.get("schema_version") != 1:
        raise FleetError("manifest schema_version must be 1")
    if manifest.get("fleet_id") != EXPECTED_FLEET_ID:
        raise FleetError(f"manifest fleet_id must be {EXPECTED_FLEET_ID!r}")
    if manifest.get("expected_repository_count") != EXPECTED_REPOSITORY_COUNT:
        raise FleetError(f"manifest expected_repository_count must be {EXPECTED_REPOSITORY_COUNT}")

    organizations = manifest.get("organizations")
    if not isinstance(organizations, list):
        raise FleetError("manifest organizations must be a list")
    owners = [str(org.get("owner", "")) for org in organizations if isinstance(org, dict)]
    if set(owners) != ALLOWED_ORGANIZATIONS or len(owners) != len(ALLOWED_ORGANIZATIONS):
        raise FleetError(
            "manifest organizations must exactly match the bounded allowlist: "
            + ", ".join(sorted(ALLOWED_ORGANIZATIONS, key=str.lower))
        )

    flattened: list[tuple[Mapping[str, Any], Mapping[str, Any]]] = []
    full_names: set[str] = set()
    for org in organizations:
        if not isinstance(org, dict):
            raise FleetError("every organization entry must be an object")
        owner = str(org["owner"])
        default_visibility = str(org.get("default_visibility", ""))
        if default_visibility not in {"private", "public"}:
            raise FleetError(f"{owner}: invalid default visibility")
        if not str(org.get("prefix", "")).strip() or not str(org.get("product", "")).strip():
            raise FleetError(f"{owner}: prefix and product are required")

        existing = org.get("existing_repositories", [])
        repositories = org.get("repositories")
        if not isinstance(existing, list) or not isinstance(repositories, list):
            raise FleetError(f"{owner}: repository collections must be lists")

        local_names: set[str] = set()
        roles: list[str] = []
        for item in [*existing, *repositories]:
            if not isinstance(item, dict):
                raise FleetError(f"{owner}: repository entry must be an object")
            name = str(item.get("name", ""))
            role = str(item.get("role", ""))
            if not name or name.startswith("/") or "/" in name or name in {".", ".."}:
                raise FleetError(f"{owner}: invalid repository name {name!r}")
            if name.casefold() in local_names:
                raise FleetError(f"{owner}: duplicate repository name {name!r}")
            local_names.add(name.casefold())
            if not role:
                raise FleetError(f"{owner}/{name}: role is required")

        for repo in repositories:
            name = str(repo["name"])
            role = str(repo["role"])
            seed = str(repo.get("seed", ""))
            visibility = str(repo.get("visibility", ""))
            full_name = f"{owner}/{name}"
            if full_name.casefold() in full_names:
                raise FleetError(f"duplicate managed repository {full_name}")
            full_names.add(full_name.casefold())
            if seed not in SEED_BUILDERS:
                raise FleetError(f"{full_name}: unsupported seed {seed!r}")
            if role != seed:
                raise FleetError(f"{full_name}: role and seed must match")
            if visibility not in {"private", "public"}:
                raise FleetError(f"{full_name}: invalid visibility")
            if role == "governance":
                if name != ".github" or visibility != "public":
                    raise FleetError(f"{owner}: governance repository must be public and named .github")
            elif name == ".github":
                raise FleetError(f"{owner}: .github must have governance role")
            elif visibility != default_visibility:
                raise FleetError(f"{full_name}: visibility must match organization default")
            if role == "mcp" and not name.endswith("-mcp-server.rs"):
                raise FleetError(f"{full_name}: MCP repository must end with -mcp-server.rs")
            roles.append(role)
            flattened.append((org, repo))

        if roles.count("governance") != 1 or roles.count("mcp") != 1 or roles.count("interfaces") != 1:
            raise FleetError(f"{owner}: exactly one governance, MCP, and interfaces repository is required")

    if len(flattened) != EXPECTED_REPOSITORY_COUNT:
        raise FleetError(
            f"manifest contains {len(flattened)} managed repositories; expected {EXPECTED_REPOSITORY_COUNT}"
        )

    for org, repo in flattened:
        files = files_for_repository(org, repo)
        total_bytes = 0
        for path, content in files.items():
            encoded = content.encode("utf-8")
            total_bytes += len(encoded)
            if len(encoded) > 500_000:
                raise FleetError(f"{org['owner']}/{repo['name']}:{path} exceeds 500 KiB")
            if any(marker in content for marker in ("ghp_", "github_pat_", "-----BEGIN PRIVATE KEY-----")):
                raise FleetError(f"credential-like content detected in {org['owner']}/{repo['name']}:{path}")
        if total_bytes > 2_000_000:
            raise FleetError(f"{org['owner']}/{repo['name']} seed exceeds 2 MiB")
    return flattened


class RepositoryPublisher:
    def __init__(self, api: ApiLike, *, execute: bool) -> None:
        self.api = api
        self.execute = execute
        self.warnings: list[str] = []

    def publish(self, manifest: Mapping[str, Any], *, manifest_sha256: str) -> dict[str, Any]:
        flattened = validate_manifest(manifest)
        summary: dict[str, Any] = {
            "fleet_id": EXPECTED_FLEET_ID,
            "manifest_sha256": manifest_sha256,
            "execute": self.execute,
            "expected_repository_count": EXPECTED_REPOSITORY_COUNT,
            "created": [],
            "initialized": [],
            "preserved": [],
            "warnings": self.warnings,
        }
        if not self.execute:
            summary["planned"] = [f"{org['owner']}/{repo['name']}" for org, repo in flattened]
            return summary

        for index, (org, repo) in enumerate(flattened, start=1):
            full_name = f"{org['owner']}/{repo['name']}"
            print(
                json.dumps(
                    {
                        "event": "repository_start",
                        "index": index,
                        "total": len(flattened),
                        "repository": full_name,
                        "role": repo["role"],
                    },
                    sort_keys=True,
                ),
                flush=True,
            )
            result = self._publish_repository(org, repo)
            summary[result["state"]].append(result)
            print(json.dumps({"event": "repository_complete", **result}, sort_keys=True), flush=True)

        summary["observed_repository_count"] = sum(
            len(summary[key]) for key in ("created", "initialized", "preserved")
        )
        if summary["observed_repository_count"] != EXPECTED_REPOSITORY_COUNT:
            raise FleetError("publisher summary count does not match the confirmed fleet size")
        return summary

    def _publish_repository(self, org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, Any]:
        owner = str(org["owner"])
        name = str(repo["name"])
        full_name = f"{owner}/{name}"
        path = repo_path(owner, name)
        existing_response = self.api.request("GET", path, allowed_statuses=(200, 404))
        created = existing_response.status == 404
        if created:
            response = self.api.request(
                "POST",
                f"/orgs/{quote_segment(owner)}/repos",
                {
                    "name": name,
                    "description": str(repo["description"]),
                    "visibility": str(repo["visibility"]),
                    "private": repo["visibility"] == "private",
                    "has_issues": True,
                    "has_projects": False,
                    "has_wiki": False,
                    "is_template": False,
                    "auto_init": False,
                    "allow_squash_merge": True,
                    "allow_merge_commit": False,
                    "allow_rebase_merge": False,
                    "delete_branch_on_merge": True,
                },
                allowed_statuses=(201,),
            )
            repository = response.data
        else:
            repository = existing_response.data

        if not isinstance(repository, dict) or str(repository.get("full_name", "")).casefold() != full_name.casefold():
            raise FleetError(f"GitHub returned an unexpected repository identity for {full_name}")
        repository_id = int(repository["id"])
        branches = self.api.request(
            "GET",
            f"{path}/git/matching-refs/heads/",
            allowed_statuses=(200, 409),
        )
        branch_items = branches.data if isinstance(branches.data, list) else []
        if branch_items:
            before = sorted(
                (str(item.get("ref")), str((item.get("object") or {}).get("sha")))
                for item in branch_items
                if isinstance(item, dict)
            )
            verification = self.api.request("GET", path, allowed_statuses=(200,)).data
            if int(verification["id"]) != repository_id:
                raise FleetError(f"repository identity changed while preserving {full_name}")
            after_branches = self.api.request(
                "GET", f"{path}/git/matching-refs/heads/", allowed_statuses=(200,)
            ).data
            after = sorted(
                (str(item.get("ref")), str((item.get("object") or {}).get("sha")))
                for item in after_branches
                if isinstance(item, dict)
            )
            if before != after:
                raise FleetError(f"existing branch refs changed while preserving {full_name}")
            return {
                "state": "preserved",
                "repository": full_name,
                "repository_id": repository_id,
                "default_branch": repository.get("default_branch"),
                "branches": before,
                "reason": "existing history preserved without writes",
            }

        files = files_for_repository(org, repo)
        tree = self.api.request(
            "POST",
            f"{path}/git/trees",
            {
                "tree": [
                    {
                        "path": file_path,
                        "mode": "100644",
                        "type": "blob",
                        "content": content,
                    }
                    for file_path, content in files.items()
                ]
            },
            allowed_statuses=(201,),
        ).data
        tree_sha = str(tree.get("sha", "")) if isinstance(tree, dict) else ""
        if len(tree_sha) != 40:
            raise FleetError(f"GitHub did not return a valid tree SHA for {full_name}")

        commit = self.api.request(
            "POST",
            f"{path}/git/commits",
            {
                "message": f"chore: initialize {repo['role']} repository",
                "tree": tree_sha,
                "parents": [],
                "author": COMMIT_IDENTITY,
                "committer": COMMIT_IDENTITY,
            },
            allowed_statuses=(201,),
        ).data
        commit_sha = str(commit.get("sha", "")) if isinstance(commit, dict) else ""
        if len(commit_sha) != 40:
            raise FleetError(f"GitHub did not return a valid commit SHA for {full_name}")

        self.api.request(
            "POST",
            f"{path}/git/refs",
            {"ref": "refs/heads/main", "sha": commit_sha},
            allowed_statuses=(201,),
        )
        self.api.request(
            "PATCH",
            path,
            {
                "default_branch": "main",
                "allow_squash_merge": True,
                "allow_merge_commit": False,
                "allow_rebase_merge": False,
                "delete_branch_on_merge": True,
                "has_projects": False,
                "has_wiki": False,
            },
            allowed_statuses=(200,),
        )
        topics = sorted(
            {
                "oresoftware",
                "new-org-core-v1",
                str(repo["role"]).replace("_", "-"),
                str(org["prefix"]).lower().replace("_", "-"),
            }
        )
        self.api.request(
            "PUT",
            f"{path}/topics",
            {"names": topics},
            allowed_statuses=(200,),
        )
        self._enable_security_feature(path, "vulnerability-alerts")
        self._enable_security_feature(path, "automated-security-fixes")

        ref = self.api.request("GET", f"{path}/git/ref/heads/main", allowed_statuses=(200,)).data
        observed_sha = str(((ref or {}).get("object") or {}).get("sha", ""))
        verification = self.api.request("GET", path, allowed_statuses=(200,)).data
        if int(verification["id"]) != repository_id:
            raise FleetError(f"repository identity changed while initializing {full_name}")
        if observed_sha != commit_sha:
            raise FleetError(f"main SHA mismatch for {full_name}: expected {commit_sha}, observed {observed_sha}")
        if str(verification.get("default_branch")) != "main":
            raise FleetError(f"default branch verification failed for {full_name}")
        if created and str(verification.get("visibility")) != str(repo["visibility"]):
            raise FleetError(f"visibility verification failed for newly created {full_name}")

        marker = self.api.request(
            "GET",
            f"{path}/contents/repo-relationships.json?ref=main",
            allowed_statuses=(200,),
        ).data
        if not isinstance(marker, dict) or marker.get("encoding") != "base64":
            raise FleetError(f"relationship marker verification failed for {full_name}")
        try:
            decoded = base64.b64decode(str(marker["content"]), validate=False).decode("utf-8")
            marker_document = json.loads(decoded)
        except (KeyError, ValueError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise FleetError(f"relationship marker is unreadable for {full_name}") from error
        if marker_document.get("fleet_id") != EXPECTED_FLEET_ID or marker_document.get("repository") != full_name:
            raise FleetError(f"relationship marker identity mismatch for {full_name}")

        return {
            "state": "created" if created else "initialized",
            "repository": full_name,
            "repository_id": repository_id,
            "main_sha": commit_sha,
            "file_count": len(files),
            "visibility": verification.get("visibility"),
        }

    def _enable_security_feature(self, path: str, feature: str) -> None:
        try:
            self.api.request("PUT", f"{path}/{feature}", allowed_statuses=(204, 403, 404, 422))
        except FleetError as error:
            self.warnings.append(f"optional security feature {feature} failed for {path}: {error}")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=pathlib.Path,
        default=pathlib.Path(__file__).with_name("new_org_repository_fleet.json"),
    )
    parser.add_argument("--execute", action="store_true", help="Perform GitHub writes. Default is a local dry run.")
    parser.add_argument("--confirm-fleet", help=f"Must equal {EXPECTED_FLEET_ID!r} when --execute is used.")
    parser.add_argument(
        "--confirm-repository-count",
        type=int,
        help=f"Must equal {EXPECTED_REPOSITORY_COUNT} when --execute is used.",
    )
    parser.add_argument("--summary-file", type=pathlib.Path)
    parser.add_argument("--api-root", default=os.environ.get("GITHUB_API_URL", DEFAULT_API_ROOT))
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        manifest_bytes = canonical_manifest_bytes(args.manifest)
        manifest = json.loads(manifest_bytes)
        validate_manifest(manifest)
        manifest_sha256 = hashlib.sha256(manifest_bytes).hexdigest()

        if args.execute:
            if args.confirm_fleet != EXPECTED_FLEET_ID:
                raise FleetError(f"--confirm-fleet must equal {EXPECTED_FLEET_ID!r}")
            if args.confirm_repository_count != EXPECTED_REPOSITORY_COUNT:
                raise FleetError(
                    f"--confirm-repository-count must equal {EXPECTED_REPOSITORY_COUNT}"
                )
            token = os.environ.get(TOKEN_ENV, "")
            api: ApiLike = GitHubApi(token, api_root=args.api_root)
        else:
            api = _DryRunApi()

        publisher = RepositoryPublisher(api, execute=args.execute)
        started = time.monotonic()
        summary = publisher.publish(manifest, manifest_sha256=manifest_sha256)
        summary["elapsed_seconds"] = round(time.monotonic() - started, 3)
        rendered = json.dumps(summary, indent=2, sort_keys=True) + "\n"
        if args.summary_file:
            args.summary_file.parent.mkdir(parents=True, exist_ok=True)
            args.summary_file.write_text(rendered, encoding="utf-8")
        print(rendered, end="")
        return 0
    except (FleetError, OSError, json.JSONDecodeError) as error:
        print(f"new-org repository fleet publisher failed: {error}", file=sys.stderr)
        return 1


class _DryRunApi:
    def request(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | list[Any] | None = None,
        *,
        allowed_statuses: Iterable[int] = (200,),
    ) -> ApiResponse:
        raise AssertionError(f"dry-run attempted GitHub request {method} {path}")


if __name__ == "__main__":
    raise SystemExit(main())
