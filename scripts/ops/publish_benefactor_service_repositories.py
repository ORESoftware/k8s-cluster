#!/usr/bin/env python3
"""Create and verify the exact private Benefactor service repository set.

The caller supplies a short-lived, organization-scoped GitHub App installation
token through ``GH_TOKEN``. The token is never written to a remote URL, source,
evidence, or logs. Existing exact private repositories are preserved; public,
renamed, archived, disabled, or otherwise mismatched repositories fail closed.
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
from typing import Callable, TypeAlias
import urllib.error
import urllib.request

API = "https://api.github.com"
API_VERSION = "2022-11-28"
ORGANIZATION = "benefactor-cc"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")


@dataclass(frozen=True)
class RepositorySpec:
    name: str
    description: str

    @property
    def full_name(self) -> str:
        return f"{ORGANIZATION}/{self.name}"


REPOSITORIES = (
    RepositorySpec(
        "benefactor-web-server.rs",
        "Rust Axum and Maud web application for app.benefactor.cc using SeaORM",
    ),
    RepositorySpec(
        "benefactor-api-server.rs",
        "Rust JSON API for api.benefactor.cc using Axum and SeaORM",
    ),
    RepositorySpec(
        "benefactor-infra",
        "Cloudflare, Kubernetes, and deployment infrastructure for Benefactor services",
    ),
)

RepositoryApi: TypeAlias = Callable[
    [str, str, dict[str, object] | None], tuple[int, object | None]
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--trusted-k8s-cluster-sha", required=True)
    return parser.parse_args()


def api_client(token: str) -> RepositoryApi:
    def api(
        method: str,
        path: str,
        body: dict[str, object] | None = None,
    ) -> tuple[int, object | None]:
        payload = None if body is None else json.dumps(body).encode("utf-8")
        request = urllib.request.Request(API + path, data=payload, method=method)
        request.add_header("Accept", "application/vnd.github+json")
        request.add_header("Authorization", f"Bearer {token}")
        request.add_header("X-GitHub-Api-Version", API_VERSION)
        request.add_header("User-Agent", "benefactor-service-repository-publisher")
        if payload is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                raw = response.read()
                return response.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            raw = error.read(16_384)
            try:
                document: object | None = json.loads(raw) if raw else None
            except json.JSONDecodeError:
                document = {"message": raw.decode("utf-8", "replace")[:1000]}
            return error.code, document

    return api


def require_document(
    status: int,
    document: object | None,
    *,
    expected_status: int,
    operation: str,
) -> dict[str, object]:
    if status != expected_status or not isinstance(document, dict):
        raise RuntimeError(f"{operation} failed: HTTP {status}")
    return document


def verify_repository(document: object, spec: RepositorySpec) -> dict[str, object]:
    if not isinstance(document, dict):
        raise RuntimeError(f"invalid repository response for {spec.full_name}")
    if document.get("full_name") != spec.full_name:
        raise RuntimeError(
            f"repository identity mismatch for {spec.full_name}: {document.get('full_name')!r}"
        )
    if document.get("private") is not True or document.get("visibility") != "private":
        raise RuntimeError(f"repository must already be private: {spec.full_name}")
    if document.get("archived") is True or document.get("disabled") is True:
        raise RuntimeError(f"repository is archived or disabled: {spec.full_name}")
    repository_id = document.get("id")
    if not isinstance(repository_id, int) or repository_id <= 0:
        raise RuntimeError(f"repository has no valid id: {spec.full_name}")
    return document


def main_ref(api: RepositoryApi, spec: RepositorySpec) -> str | None:
    status, document = api(
        "GET",
        f"/repos/{spec.full_name}/git/ref/heads/main",
        None,
    )
    if status == 404:
        return None
    payload = require_document(
        status,
        document,
        expected_status=200,
        operation=f"read main ref for {spec.full_name}",
    )
    object_value = payload.get("object")
    if not isinstance(object_value, dict):
        raise RuntimeError(f"main ref object is missing for {spec.full_name}")
    sha = object_value.get("sha")
    if not isinstance(sha, str) or SHA_RE.fullmatch(sha) is None:
        raise RuntimeError(f"main ref SHA is invalid for {spec.full_name}")
    return sha


def initialize_empty_main(api: RepositoryApi, spec: RepositorySpec) -> str:
    readme = (
        f"# {spec.name}\n\n"
        f"{spec.description}.\n\n"
        "This repository was initialized by the reviewed Benefactor repository publisher.\n"
    )
    status, blob_document = api(
        "POST",
        f"/repos/{spec.full_name}/git/blobs",
        {"content": readme, "encoding": "utf-8"},
    )
    blob = require_document(
        status,
        blob_document,
        expected_status=201,
        operation=f"create initial README blob for {spec.full_name}",
    )
    blob_sha = blob.get("sha")
    if not isinstance(blob_sha, str) or SHA_RE.fullmatch(blob_sha) is None:
        raise RuntimeError(f"initial README blob SHA is invalid for {spec.full_name}")

    status, tree_document = api(
        "POST",
        f"/repos/{spec.full_name}/git/trees",
        {
            "tree": [
                {
                    "path": "README.md",
                    "mode": "100644",
                    "type": "blob",
                    "sha": blob_sha,
                }
            ]
        },
    )
    tree = require_document(
        status,
        tree_document,
        expected_status=201,
        operation=f"create initial tree for {spec.full_name}",
    )
    tree_sha = tree.get("sha")
    if not isinstance(tree_sha, str) or SHA_RE.fullmatch(tree_sha) is None:
        raise RuntimeError(f"initial tree SHA is invalid for {spec.full_name}")

    status, commit_document = api(
        "POST",
        f"/repos/{spec.full_name}/git/commits",
        {
            "message": f"Initialize {spec.name}",
            "tree": tree_sha,
            "parents": [],
        },
    )
    commit = require_document(
        status,
        commit_document,
        expected_status=201,
        operation=f"create initial commit for {spec.full_name}",
    )
    commit_sha = commit.get("sha")
    if not isinstance(commit_sha, str) or SHA_RE.fullmatch(commit_sha) is None:
        raise RuntimeError(f"initial commit SHA is invalid for {spec.full_name}")

    status, _ = api(
        "POST",
        f"/repos/{spec.full_name}/git/refs",
        {"ref": "refs/heads/main", "sha": commit_sha},
    )
    if status not in {201, 422}:
        raise RuntimeError(f"create main ref failed for {spec.full_name}: HTTP {status}")
    observed = main_ref(api, spec)
    if observed is None:
        raise RuntimeError(f"main ref is still absent for {spec.full_name}")
    if status == 201 and observed != commit_sha:
        raise RuntimeError(
            f"initial main changed unexpectedly for {spec.full_name}: {observed} != {commit_sha}"
        )
    return observed


def create_payload(spec: RepositorySpec) -> dict[str, object]:
    return {
        "name": spec.name,
        "description": spec.description,
        "private": True,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "auto_init": True,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }


def patch_payload(spec: RepositorySpec) -> dict[str, object]:
    return {
        "description": spec.description,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }


def ensure_repository(api: RepositoryApi, spec: RepositorySpec) -> dict[str, object]:
    status, current = api("GET", f"/repos/{spec.full_name}", None)
    created = False
    if status == 404:
        create_status, created_document = api(
            "POST",
            f"/orgs/{ORGANIZATION}/repos",
            create_payload(spec),
        )
        if create_status == 201:
            current = created_document
            created = True
        elif create_status in {409, 422}:
            reconcile_status, current = api("GET", f"/repos/{spec.full_name}", None)
            if reconcile_status != 200:
                raise RuntimeError(
                    f"create race did not reconcile for {spec.full_name}: "
                    f"POST {create_status}, GET {reconcile_status}"
                )
        else:
            raise RuntimeError(f"create failed for {spec.full_name}: HTTP {create_status}")
    elif status != 200:
        raise RuntimeError(f"preflight failed for {spec.full_name}: HTTP {status}")

    repository = verify_repository(current, spec)
    patch_status, patch_document = api(
        "PATCH",
        f"/repos/{spec.full_name}",
        patch_payload(spec),
    )
    repository = verify_repository(
        require_document(
            patch_status,
            patch_document,
            expected_status=200,
            operation=f"configure {spec.full_name}",
        ),
        spec,
    )

    main_sha = main_ref(api, spec)
    if main_sha is None:
        size = repository.get("size")
        if size not in {0, None}:
            raise RuntimeError(
                f"refusing to initialize nonempty repository without main: {spec.full_name}"
            )
        main_sha = initialize_empty_main(api, spec)

    status, final_document = api("GET", f"/repos/{spec.full_name}", None)
    final_repository = verify_repository(
        require_document(
            status,
            final_document,
            expected_status=200,
            operation=f"postflight read for {spec.full_name}",
        ),
        spec,
    )
    if final_repository.get("default_branch") != "main":
        status, changed_document = api(
            "PATCH",
            f"/repos/{spec.full_name}",
            {"default_branch": "main"},
        )
        final_repository = verify_repository(
            require_document(
                status,
                changed_document,
                expected_status=200,
                operation=f"set default branch for {spec.full_name}",
            ),
            spec,
        )
    if final_repository.get("default_branch") != "main":
        raise RuntimeError(f"default branch is not main for {spec.full_name}")
    if final_repository.get("has_issues") is not True:
        raise RuntimeError(f"issues are not enabled for {spec.full_name}")
    if final_repository.get("has_wiki") is not False:
        raise RuntimeError(f"wiki is not disabled for {spec.full_name}")

    verified_main = main_ref(api, spec)
    if verified_main != main_sha:
        raise RuntimeError(
            f"main changed during postflight for {spec.full_name}: {verified_main} != {main_sha}"
        )
    return {
        "full_name": spec.full_name,
        "created": created,
        "repository_id": final_repository["id"],
        "visibility": "private",
        "default_branch": "main",
        "main_sha": main_sha,
    }


def publish(api: RepositoryApi, trusted_sha: str) -> dict[str, object]:
    if SHA_RE.fullmatch(trusted_sha) is None:
        raise RuntimeError("trusted k8s-cluster SHA is invalid")
    repositories = [ensure_repository(api, spec) for spec in REPOSITORIES]
    if {item["full_name"] for item in repositories} != {
        spec.full_name for spec in REPOSITORIES
    }:
        raise RuntimeError("postflight repository set escaped the exact allowlist")
    return {
        "schema_version": 1,
        "trusted_k8s_cluster_sha": trusted_sha,
        "organization": ORGANIZATION,
        "repositories": repositories,
    }


def main() -> int:
    args = parse_args()
    token = os.environ.get("GH_TOKEN", "")
    if len(token) < 20 or any(character.isspace() for character in token):
        raise SystemExit("GH_TOKEN must be a short-lived installation token")
    report = publish(api_client(token), args.trusted_k8s_cluster_sha)
    args.evidence_out.parent.mkdir(parents=True, exist_ok=True)
    args.evidence_out.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    for repository in report["repositories"]:
        print(
            "BENEFACTOR_SERVICE_REPOSITORY_READY "
            f"repository={repository['full_name']} "
            f"created={str(repository['created']).lower()} "
            f"main={repository['main_sha']} "
            f"trusted_k8s_cluster_sha={args.trusted_k8s_cluster_sha}"
        )
    print(
        "BENEFACTOR_SERVICE_REPOSITORIES_COMPLETE "
        f"count={len(report['repositories'])} "
        f"trusted_k8s_cluster_sha={args.trusted_k8s_cluster_sha}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
