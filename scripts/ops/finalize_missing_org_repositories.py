#!/usr/bin/env python3
"""Finalize and verify the critical cross-organization repository publication.

This script is intentionally bounded to the repository fleet approved on 2026-07-31.
It activates staged CI in the two extracted repositories, verifies the complete
owner-visible organization inventory, and emits a durable Markdown/JSON report.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import sys
from pathlib import Path
from typing import Any
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen

API = "https://api.github.com"
API_VERSION = "2026-03-10"
EXPECTED_ORGS = {"hypesiege": 15, "StreemPilot": 17}
EXTRACTED = {
    "meta-agents-demo/meta-agent-control-plane.rs": ".meta-agent-ci.yml.pending",
    "file-tunnel/ftnl-mcp-server.rs": ".ftnl-mcp-ci.yml.pending",
}
TRIGGER_PULLS = (227, 229, 230, 231)


class PublicationError(RuntimeError):
    pass


def token() -> str:
    value = os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN", "").strip()
    if not value:
        raise PublicationError("GITHUB_REPOSITORY_ADMIN_TOKEN is required")
    return value


def api(
    method: str,
    path: str,
    credential: str,
    payload: dict[str, Any] | None = None,
    *,
    allow_missing: bool = False,
) -> Any:
    body = None
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {credential}",
        "X-GitHub-Api-Version": API_VERSION,
        "User-Agent": "oresoftware-critical-org-publication-finalizer",
    }
    if payload is not None:
        body = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = Request(f"{API}{path}", data=body, headers=headers, method=method)
    try:
        with urlopen(request, timeout=45) as response:
            raw = response.read()
            if not raw:
                return None
            return json.loads(raw.decode("utf-8"))
    except HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        if allow_missing and exc.code == 404:
            return None
        raise PublicationError(
            f"GitHub API {method} {path} failed ({exc.code}): {raw[:1000]}"
        ) from exc


def get_content(slug: str, path: str, credential: str) -> dict[str, Any] | None:
    encoded = quote(path, safe="/")
    result = api(
        "GET",
        f"/repos/{slug}/contents/{encoded}?ref=main",
        credential,
        allow_missing=True,
    )
    if result is not None and not isinstance(result, dict):
        raise PublicationError(f"unexpected content response for {slug}:{path}")
    return result


def decoded_content(record: dict[str, Any]) -> bytes:
    encoded = record.get("content")
    if not isinstance(encoded, str):
        raise PublicationError("Contents API response lacks inline content")
    return base64.b64decode(encoded.replace("\n", ""), validate=True)


def activate_ci(slug: str, pending_path: str, credential: str) -> str:
    ci_path = ".github/workflows/ci.yml"
    pending = get_content(slug, pending_path, credential)
    current = get_content(slug, ci_path, credential)

    if pending is None:
        if current is None:
            raise PublicationError(f"{slug}: neither staged nor canonical CI exists")
        return "already-active"

    pending_bytes = decoded_content(pending)
    if current is None:
        api(
            "PUT",
            f"/repos/{slug}/contents/{quote(ci_path, safe='/')}",
            credential,
            {
                "message": "ci: activate canonical validation after repository publication",
                "content": base64.b64encode(pending_bytes).decode("ascii"),
                "branch": "main",
            },
        )
        action = "activated"
    else:
        if decoded_content(current) != pending_bytes:
            raise PublicationError(f"{slug}: canonical CI conflicts with staged carrier")
        action = "already-active"

    pending_sha = pending.get("sha")
    if not isinstance(pending_sha, str) or not pending_sha:
        raise PublicationError(f"{slug}: staged CI carrier lacks a blob SHA")
    api(
        "DELETE",
        f"/repos/{slug}/contents/{quote(pending_path, safe='/')}",
        credential,
        {
            "message": "chore: remove staged CI carrier after activation",
            "sha": pending_sha,
            "branch": "main",
        },
    )
    return action


def org_repositories(org: str, credential: str) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for page in range(1, 20):
        batch = api(
            "GET",
            f"/orgs/{org}/repos?type=all&per_page=100&page={page}",
            credential,
        )
        if not isinstance(batch, list):
            raise PublicationError(f"unexpected repository list for {org}")
        records.extend(batch)
        if len(batch) < 100:
            break
    return records


def verify_repository(slug: str, repository: dict[str, Any], credential: str) -> dict[str, Any]:
    if repository.get("private") is not True:
        raise PublicationError(f"{slug}: repository is not private")
    if repository.get("default_branch") != "main":
        raise PublicationError(
            f"{slug}: default branch is {repository.get('default_branch')!r}, expected 'main'"
        )
    ref = api("GET", f"/repos/{slug}/git/ref/heads/main", credential)
    sha = ((ref or {}).get("object") or {}).get("sha")
    if not isinstance(sha, str) or len(sha) != 40:
        raise PublicationError(f"{slug}: main does not resolve to a full commit SHA")
    return {
        "slug": slug,
        "private": True,
        "default_branch": "main",
        "main_sha": sha,
    }


def close_pull(repo: str, number: int, credential: str) -> None:
    pull = api("GET", f"/repos/{repo}/pulls/{number}", credential, allow_missing=True)
    if not pull or pull.get("state") == "closed":
        return
    api("PATCH", f"/repos/{repo}/pulls/{number}", credential, {"state": "closed"})


def build_report(ci_actions: dict[str, str], credential: str) -> dict[str, Any]:
    report: dict[str, Any] = {
        "success": False,
        "ci_actions": ci_actions,
        "organizations": {},
        "extracted_repositories": {},
    }

    for org, expected in EXPECTED_ORGS.items():
        repositories = org_repositories(org, credential)
        if len(repositories) != expected:
            raise PublicationError(
                f"{org}: expected exactly {expected} repositories, found {len(repositories)}"
            )
        verified = []
        for repository in sorted(repositories, key=lambda item: str(item.get("name", "")).casefold()):
            slug = repository.get("full_name")
            if not isinstance(slug, str):
                raise PublicationError(f"{org}: repository response lacks full_name")
            verified.append(verify_repository(slug, repository, credential))
        report["organizations"][org] = {
            "expected": expected,
            "count": len(verified),
            "repositories": verified,
        }

    for slug, pending_path in EXTRACTED.items():
        repository = api("GET", f"/repos/{slug}", credential)
        verified = verify_repository(slug, repository, credential)
        if get_content(slug, ".github/workflows/ci.yml", credential) is None:
            raise PublicationError(f"{slug}: canonical CI is missing")
        if get_content(slug, pending_path, credential) is not None:
            raise PublicationError(f"{slug}: staged CI carrier remains")
        report["extracted_repositories"][slug] = verified | {
            "ci": "active",
            "pending_carrier": False,
        }

    unreal = org_repositories("unreal-unity-poc", credential)
    if len(unreal) < 25:
        raise PublicationError(
            f"unreal-unity-poc: expected at least 25 repositories, found {len(unreal)}"
        )
    report["organizations"]["unreal-unity-poc"] = {
        "minimum": 25,
        "count": len(unreal),
        "repositories": sorted(str(item.get("full_name")) for item in unreal),
    }
    report["success"] = True
    return report


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Canonical organization repository publication",
        "",
        "Overall result: **SUCCESS**",
        "",
    ]
    for org in ("hypesiege", "StreemPilot"):
        group = report["organizations"][org]
        lines.extend(
            [
                f"## {org}: {group['count']}/{group['expected']}",
                "",
            ]
        )
        for repository in group["repositories"]:
            lines.append(
                f"- `{repository['slug']}` — `main` `{repository['main_sha']}`; private"
            )
        lines.append("")
    for slug, repository in report["extracted_repositories"].items():
        lines.extend(
            [
                f"## {slug}",
                "",
                f"- `main`: `{repository['main_sha']}`",
                "- canonical CI: active",
                "- staged CI carrier: removed",
                "",
            ]
        )
    unreal = report["organizations"]["unreal-unity-poc"]
    lines.extend(
        [
            f"## unreal-unity-poc: {unreal['count']} repositories",
            "",
            "The previously published Unreal/Unity fleet remains visible to the owner session.",
            "",
            "All requested repository publication and verification gates passed.",
        ]
    )
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json-report", type=Path, required=True)
    parser.add_argument("--markdown-report", type=Path, required=True)
    parser.add_argument("--close-carriers", action="store_true")
    args = parser.parse_args()

    credential = token()
    ci_actions = {
        slug: activate_ci(slug, pending, credential)
        for slug, pending in EXTRACTED.items()
    }
    report = build_report(ci_actions, credential)
    args.json_report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    args.markdown_report.write_text(markdown(report))

    if args.close_carriers:
        for number in TRIGGER_PULLS:
            close_pull("ORESoftware/k8s-cluster", number, credential)
        close_pull("ORESoftware/ai-agent-coordinator.rs", 35, credential)
    print(args.markdown_report.read_text(), end="")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublicationError as exc:
        print(f"finalize-missing-org-repositories: {exc}", file=sys.stderr)
        raise SystemExit(1)
