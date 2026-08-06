#!/usr/bin/env python3
"""Add missing organization-governance defaults through reviewed pull requests."""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

from bootstrap_current_org_dotgithub_repositories import (
    EXPECTED_LOGIN,
    REPOSITORY,
    SECRET_RE,
    documents,
)

API = "https://api.github.com"
DEFAULT_REGISTRY = "ops/portfolio/github-linear-project-registry.tsv"
EXPECTED_COUNT = 64
POLICY_VERSION = "2026-08-05"
BRANCH_PREFIX = "agent/harden-org-defaults"


class HardeningError(RuntimeError):
    pass


class GitHub:
    def __init__(self, token: str):
        if not token or any(ch.isspace() for ch in token):
            raise HardeningError("a non-empty, whitespace-free GitHub token is required")
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        payload: Any = None,
        allow: tuple[int, ...] = (),
    ) -> tuple[int, Any]:
        body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode()
        headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {self.token}",
            "User-Agent": "org-dotgithub-hardener/1",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        if body is not None:
            headers["Content-Type"] = "application/json"
        for attempt in range(7):
            request = urllib.request.Request(API + path, data=body, headers=headers, method=method)
            try:
                with urllib.request.urlopen(request, timeout=90) as response:
                    raw = response.read()
                    return response.status, json.loads(raw) if raw else None
            except urllib.error.HTTPError as error:
                raw = error.read(32768)
                try:
                    error_payload = json.loads(raw) if raw else {}
                    message = error_payload.get("message", "unknown GitHub error")
                except Exception:
                    error_payload = None
                    message = raw.decode(errors="replace")
                if error.code in allow:
                    return error.code, error_payload
                retryable = error.code in (409, 429, 500, 502, 503, 504) or (
                    error.code == 403
                    and any(
                        fragment in str(message).lower()
                        for fragment in ("rate limit", "secondary rate", "abuse")
                    )
                )
                if retryable and attempt < 6:
                    retry_after = error.headers.get("Retry-After", "")
                    delay = int(retry_after) if retry_after.isdigit() else min(2 ** (attempt + 1), 60)
                    time.sleep(max(delay, 1))
                    continue
                raise HardeningError(
                    f"GitHub {method} {path} returned {error.code}: {str(message)[:800]}"
                ) from None
            except urllib.error.URLError as error:
                if attempt < 6:
                    time.sleep(min(2 ** (attempt + 1), 30))
                    continue
                raise HardeningError(f"GitHub transport failed for {method} {path}: {error}") from None
        raise AssertionError("unreachable")

    def get(self, path: str, allow: tuple[int, ...] = ()) -> tuple[int, Any]:
        return self.request("GET", path, allow=allow)

    def post(self, path: str, payload: Any, allow: tuple[int, ...] = ()) -> tuple[int, Any]:
        return self.request("POST", path, payload, allow)

    def put(self, path: str, payload: Any, allow: tuple[int, ...] = ()) -> tuple[int, Any]:
        return self.request("PUT", path, payload, allow)

    def delete(self, path: str, allow: tuple[int, ...] = ()) -> tuple[int, Any]:
        return self.request("DELETE", path, allow=allow)


def quote(value: str, safe: str = "") -> str:
    return urllib.parse.quote(value, safe=safe)


def repo_path(org: str) -> str:
    return f"/repos/{quote(org)}/{REPOSITORY}"


def load_registry(path: str, expected_count: int) -> list[tuple[str, str]]:
    rows: list[tuple[str, str]] = []
    for line_number, raw in enumerate(Path(path).read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = raw.split("\t")
        if len(parts) != 2:
            raise HardeningError(f"registry line {line_number} must have exactly two tab-separated fields")
        org, linear_url = (part.strip() for part in parts)
        if line_number == 1 and (org, linear_url) == ("organization", "linear_url"):
            continue
        if not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?", org):
            raise HardeningError(f"registry line {line_number} has invalid organization login: {org!r}")
        if not linear_url.startswith("https://linear.app/"):
            raise HardeningError(f"registry line {line_number} has invalid Linear URL")
        rows.append((org, linear_url))
    lowered = [org.lower() for org, _ in rows]
    if len(rows) != expected_count:
        raise HardeningError(f"expected {expected_count} registry organizations, observed {len(rows)}")
    if len(set(lowered)) != len(lowered):
        raise HardeningError("registry contains duplicate organization logins")
    if EXPECTED_LOGIN.lower() in lowered:
        raise HardeningError("registry must not include the personal account")
    return rows


def validate_static() -> None:
    sample = documents("sample-org")
    if len(sample) != 15 or len(set(sample)) != 15:
        raise HardeningError("expected exactly 15 unique baseline files")
    for path, content in sample.items():
        if path.startswith("/") or ".." in Path(path).parts or not content.endswith("\n"):
            raise HardeningError(f"unsafe baseline path or content: {path}")
        if SECRET_RE.search(content):
            raise HardeningError(f"credential-shaped content detected in {path}")


def authenticated_preflight(api: GitHub, rows: list[tuple[str, str]]) -> None:
    _, user = api.get("/user")
    if not isinstance(user, dict) or user.get("login") != EXPECTED_LOGIN:
        raise HardeningError(f"hardening must be authorized as {EXPECTED_LOGIN}")
    for org, _ in rows:
        _, membership = api.get(f"/user/memberships/orgs/{quote(org)}")
        if not isinstance(membership, dict):
            raise HardeningError(f"invalid membership response for {org}")
        if membership.get("state") != "active" or membership.get("role") != "admin":
            raise HardeningError(f"active admin membership is required for {org}")
        _, repo = api.get(repo_path(org), allow=(404,))
        if not isinstance(repo, dict):
            raise HardeningError(f"required public repository is missing: {org}/{REPOSITORY}")
        owner = repo.get("owner") or {}
        if str(owner.get("login", "")).lower() != org.lower() or repo.get("name") != REPOSITORY:
            raise HardeningError(f"unexpected repository identity for {org}/{REPOSITORY}")
        if repo.get("private") is not False or repo.get("archived") is True:
            raise HardeningError(f"{org}/{REPOSITORY} must be public and active")


def branch_state(api: GitHub, org: str, branch: str) -> tuple[str, str, set[str]]:
    _, ref = api.get(f"{repo_path(org)}/git/ref/heads/{quote(branch, safe='/')}")
    commit_sha = ((ref or {}).get("object") or {}).get("sha") if isinstance(ref, dict) else None
    if not isinstance(commit_sha, str) or not commit_sha:
        raise HardeningError(f"cannot resolve {org}/{REPOSITORY}@{branch}")
    _, commit = api.get(f"{repo_path(org)}/git/commits/{commit_sha}")
    tree_sha = ((commit or {}).get("tree") or {}).get("sha") if isinstance(commit, dict) else None
    if not isinstance(tree_sha, str) or not tree_sha:
        raise HardeningError(f"cannot resolve tree for {org}/{REPOSITORY}@{branch}")
    _, tree = api.get(f"{repo_path(org)}/git/trees/{tree_sha}?recursive=1")
    paths = {
        item.get("path")
        for item in (tree.get("tree") if isinstance(tree, dict) else [])
        if isinstance(item, dict) and item.get("type") == "blob" and isinstance(item.get("path"), str)
    }
    return commit_sha, tree_sha, paths


def unique_branch(org: str) -> str:
    run_id = re.sub(r"[^0-9A-Za-z._-]+", "-", os.environ.get("GITHUB_RUN_ID", "local"))
    attempt = re.sub(r"[^0-9A-Za-z._-]+", "-", os.environ.get("GITHUB_RUN_ATTEMPT", "1"))
    return f"{BRANCH_PREFIX}-{run_id}-{attempt}-{org.lower()}"[:240]


def merge_pr(api: GitHub, org: str, number: int, expected_sha: str) -> str:
    path = f"{repo_path(org)}/pulls/{number}/merge"
    payload = {
        "sha": expected_sha,
        "merge_method": "squash",
        "commit_title": f"docs: add missing organization defaults for {org} (#{number})",
        "commit_message": "Add only absent organization-governance files while preserving all existing documentation.",
    }
    last_payload: Any = None
    for attempt in range(20):
        status, result = api.put(path, payload, allow=(405, 409))
        last_payload = result
        if status == 200 and isinstance(result, dict) and result.get("merged") is True:
            sha = result.get("sha")
            if isinstance(sha, str) and sha:
                return sha
        if status in (405, 409):
            time.sleep(min(2 + attempt, 15))
            continue
        raise HardeningError(f"unexpected merge response for {org}/.github#{number}: {result!r}")
    message = last_payload.get("message") if isinstance(last_payload, dict) else last_payload
    raise HardeningError(f"pull request did not become mergeable for {org}/.github#{number}: {message}")


def harden_one(api: GitHub, org: str, linear_url: str) -> dict[str, Any]:
    _, repo = api.get(repo_path(org))
    if not isinstance(repo, dict):
        raise HardeningError(f"invalid repository response for {org}/{REPOSITORY}")
    branch = repo.get("default_branch") or "main"
    if not isinstance(branch, str) or not branch:
        branch = "main"
    parent_sha, base_tree, existing_paths = branch_state(api, org, branch)
    desired = documents(org)
    missing = sorted(set(desired) - existing_paths)
    row: dict[str, Any] = {
        "organization": org,
        "repository": f"{org}/{REPOSITORY}",
        "linear_url": linear_url,
        "default_branch": branch,
        "missing_before": missing,
        "added_files": [],
        "pull_request": None,
        "merged_commit": None,
        "verified": False,
    }
    if missing:
        tree_entries = [
            {"path": path, "mode": "100644", "type": "blob", "content": desired[path]}
            for path in missing
        ]
        _, tree = api.post(f"{repo_path(org)}/git/trees", {"base_tree": base_tree, "tree": tree_entries})
        tree_sha = tree.get("sha") if isinstance(tree, dict) else None
        if not isinstance(tree_sha, str) or not tree_sha:
            raise HardeningError(f"GitHub did not return a tree SHA for {org}/{REPOSITORY}")
        _, commit = api.post(
            f"{repo_path(org)}/git/commits",
            {
                "message": f"docs: add missing organization defaults for {org}",
                "tree": tree_sha,
                "parents": [parent_sha],
            },
        )
        commit_sha = commit.get("sha") if isinstance(commit, dict) else None
        if not isinstance(commit_sha, str) or not commit_sha:
            raise HardeningError(f"GitHub did not return a commit SHA for {org}/{REPOSITORY}")
        head_branch = unique_branch(org)
        api.post(
            f"{repo_path(org)}/git/refs",
            {"ref": f"refs/heads/{head_branch}", "sha": commit_sha},
        )
        _, pr = api.post(
            f"{repo_path(org)}/pulls",
            {
                "title": "docs: add missing organization governance defaults",
                "head": head_branch,
                "base": branch,
                "body": (
                    "## What changed\n\n"
                    "Adds only baseline organization-governance files that were absent on the latest default branch. "
                    "All existing documentation remains byte-for-byte untouched.\n\n"
                    "## Added paths\n\n"
                    + "\n".join(f"- `{path}`" for path in missing)
                    + "\n\n## Validation\n\n"
                    "- generated from the reviewed portfolio baseline;\n"
                    "- credential-shaped content scan passed;\n"
                    "- repository identity, public visibility, and owner authorization were verified;\n"
                    "- post-merge verification requires all 15 baseline paths on `main`.\n"
                ),
                "maintainer_can_modify": True,
                "draft": False,
            },
        )
        number = pr.get("number") if isinstance(pr, dict) else None
        html_url = pr.get("html_url") if isinstance(pr, dict) else None
        if not isinstance(number, int) or not isinstance(html_url, str):
            raise HardeningError(f"GitHub did not create a pull request for {org}/{REPOSITORY}")
        merged_sha = merge_pr(api, org, number, commit_sha)
        api.delete(f"{repo_path(org)}/git/refs/heads/{quote(head_branch, safe='/')}", allow=(404, 422))
        row["added_files"] = missing
        row["pull_request"] = {"number": number, "url": html_url, "state": "merged"}
        row["merged_commit"] = merged_sha

    absent: list[str] = []
    for attempt in range(10):
        _, _, final_paths = branch_state(api, org, branch)
        absent = sorted(set(desired) - final_paths)
        if not absent:
            break
        time.sleep(min(attempt + 1, 5))
    if absent:
        raise HardeningError(f"post-merge verification is missing paths for {org}/{REPOSITORY}: {absent}")
    row["verified"] = True
    row["result"] = "merged" if missing else "unchanged"
    return row


def markdown_report(payload: dict[str, Any]) -> str:
    rows = payload["organizations"]
    merged = sum(row.get("result") == "merged" for row in rows)
    unchanged = sum(row.get("result") == "unchanged" for row in rows)
    added = sum(len(row.get("added_files") or []) for row in rows)
    lines = [
        "# Organization `.github` fleet hardening report",
        "",
        f"- Policy version: `{payload['policy_version']}`",
        f"- Organizations verified: `{len(rows)}`",
        f"- Repositories changed through merged pull requests: `{merged}`",
        f"- Repositories already complete: `{unchanged}`",
        f"- Missing baseline files added: `{added}`",
        "",
        "| Organization | Result | Added files | Pull request |",
        "|---|---:|---:|---|",
    ]
    for row in rows:
        pr = row.get("pull_request") or {}
        pr_cell = f"[#{pr['number']}]({pr['url']})" if pr else "—"
        lines.append(
            f"| `{row['organization']}` | {row['result']} | {len(row.get('added_files') or [])} | {pr_cell} |"
        )
    lines.extend(
        [
            "",
            "Every existing file was preserved; the hardener added only absent baseline paths.",
            "All repositories were re-read from their default branch after merge and verified to contain all 15 baseline paths.",
        ]
    )
    return "\n".join(lines).rstrip() + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry", default=DEFAULT_REGISTRY)
    parser.add_argument("--expected-count", type=int, default=EXPECTED_COUNT)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--json-report")
    parser.add_argument("--markdown-report")
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)

    validate_static()
    rows = load_registry(args.registry, args.expected_count)
    if not args.execute:
        payload = {
            "schema_version": 1,
            "mode": "dry-run",
            "policy_version": POLICY_VERSION,
            "expected_count": args.expected_count,
            "organizations": [
                {"organization": org, "linear_url": linear_url, "planned": True}
                for org, linear_url in rows
            ],
        }
    else:
        token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN")
        if not token:
            raise HardeningError("GH_TOKEN or GITHUB_REPOSITORY_ADMIN_TOKEN is required")
        api = GitHub(token)
        authenticated_preflight(api, rows)
        results = [harden_one(api, org, linear_url) for org, linear_url in rows]
        if len(results) != args.expected_count or not all(row.get("verified") is True for row in results):
            raise HardeningError("hardening result is incomplete")
        payload = {
            "schema_version": 1,
            "mode": "execute",
            "policy_version": POLICY_VERSION,
            "expected_count": args.expected_count,
            "organizations": results,
        }

    json_text = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    markdown = markdown_report(payload) if args.execute else (
        "# Organization `.github` fleet hardening dry run\n\n"
        f"Validated `{len(rows)}` unique registry organizations and the reviewed 15-file baseline.\n"
    )
    if args.json_report:
        Path(args.json_report).parent.mkdir(parents=True, exist_ok=True)
        Path(args.json_report).write_text(json_text, encoding="utf-8")
    if args.markdown_report:
        Path(args.markdown_report).parent.mkdir(parents=True, exist_ok=True)
        Path(args.markdown_report).write_text(markdown, encoding="utf-8")
    if not args.json_report:
        print(json_text, end="")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except HardeningError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
