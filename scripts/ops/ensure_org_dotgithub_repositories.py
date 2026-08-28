#!/usr/bin/env python3
"""Create only missing public `.github` repositories from the canonical registry."""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

from bootstrap_current_org_dotgithub_repositories import EXPECTED_LOGIN, REPOSITORY
from harden_org_dotgithub_fleet import GitHub, HardeningError, load_registry, quote, repo_path

EXPECTED_COUNT = 64


def validate_repository(org: str, repository: dict[str, Any]) -> None:
    owner = repository.get("owner") or {}
    if repository.get("name") != REPOSITORY or str(owner.get("login", "")).lower() != org.lower():
        raise HardeningError(f"unexpected repository identity for {org}/{REPOSITORY}")
    if repository.get("private") is not False or repository.get("archived") is True:
        raise HardeningError(f"{org}/{REPOSITORY} must be public and active")


def wait_for_initial_branch(api: GitHub, org: str, branch: str) -> str:
    for attempt in range(15):
        status, ref = api.get(
            f"{repo_path(org)}/git/ref/heads/{quote(branch, safe='/')}",
            allow=(404, 409),
        )
        if status not in (404, 409) and isinstance(ref, dict):
            sha = ((ref.get("object") or {}).get("sha"))
            if isinstance(sha, str) and len(sha) == 40:
                return sha
        time.sleep(min(attempt + 1, 5))
    raise HardeningError(f"auto-initialized branch was not ready for {org}/{REPOSITORY}")


def execute(api: GitHub, rows: list[tuple[str, str]]) -> list[dict[str, Any]]:
    _, user = api.get("/user")
    if not isinstance(user, dict) or user.get("login") != EXPECTED_LOGIN:
        raise HardeningError(f"repository creation must be authorized as {EXPECTED_LOGIN}")

    observed: dict[str, dict[str, Any] | None] = {}
    for org, _ in rows:
        _, membership = api.get(f"/user/memberships/orgs/{quote(org)}")
        if not isinstance(membership, dict):
            raise HardeningError(f"invalid membership response for {org}")
        if membership.get("state") != "active" or membership.get("role") != "admin":
            raise HardeningError(f"active admin membership is required for {org}")
        status, repository = api.get(repo_path(org), allow=(404,))
        if status == 404:
            observed[org] = None
        elif isinstance(repository, dict):
            validate_repository(org, repository)
            observed[org] = repository
        else:
            raise HardeningError(f"invalid repository response for {org}/{REPOSITORY}")

    results: list[dict[str, Any]] = []
    for org, linear_url in rows:
        repository = observed[org]
        created = repository is None
        if created:
            _, repository = api.post(
                f"/orgs/{quote(org)}/repos",
                {
                    "name": REPOSITORY,
                    "description": f"Organization-wide defaults, governance, and community health for {org}",
                    "private": False,
                    "visibility": "public",
                    "has_issues": True,
                    "has_projects": False,
                    "has_wiki": False,
                    "has_discussions": False,
                    "auto_init": True,
                    "delete_branch_on_merge": True,
                },
            )
        if not isinstance(repository, dict):
            raise HardeningError(f"GitHub did not return repository metadata for {org}/{REPOSITORY}")
        validate_repository(org, repository)
        branch = repository.get("default_branch") or "main"
        if not isinstance(branch, str) or not branch:
            branch = "main"
        main_sha = wait_for_initial_branch(api, org, branch)
        status, verified = api.get(repo_path(org))
        if status != 200 or not isinstance(verified, dict):
            raise HardeningError(f"cannot verify {org}/{REPOSITORY}")
        validate_repository(org, verified)
        results.append(
            {
                "organization": org,
                "repository": f"{org}/{REPOSITORY}",
                "linear_url": linear_url,
                "created_repository": created,
                "preserved_repository": not created,
                "default_branch": branch,
                "main_sha": main_sha,
                "verified": True,
                "url": verified.get("html_url"),
            }
        )
    return results


def markdown_report(payload: dict[str, Any]) -> str:
    rows = payload["organizations"]
    created = sum(item["created_repository"] for item in rows)
    preserved = sum(item["preserved_repository"] for item in rows)
    lines = [
        "# Organization `.github` repository publication",
        "",
        f"- Organizations verified: `{len(rows)}`",
        f"- Missing repositories created: `{created}`",
        f"- Existing repositories preserved: `{preserved}`",
        "",
        "| Organization | Result | Repository |",
        "|---|---|---|",
    ]
    for item in rows:
        result = "created" if item["created_repository"] else "preserved"
        lines.append(f"| `{item['organization']}` | {result} | [{item['repository']}]({item['url']}) |")
    return "\n".join(lines).rstrip() + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry", default="ops/portfolio/github-linear-project-registry.tsv")
    parser.add_argument("--expected-count", type=int, default=EXPECTED_COUNT)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--json-report")
    parser.add_argument("--markdown-report")
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)

    rows = load_registry(args.registry, args.expected_count)
    if args.execute:
        token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN")
        if not token:
            raise HardeningError("GH_TOKEN or GITHUB_REPOSITORY_ADMIN_TOKEN is required")
        organizations = execute(GitHub(token), rows)
        mode = "execute"
    else:
        organizations = [
            {"organization": org, "linear_url": linear_url, "planned": True}
            for org, linear_url in rows
        ]
        mode = "dry-run"

    payload = {
        "schema_version": 1,
        "mode": mode,
        "expected_count": args.expected_count,
        "organizations": organizations,
    }
    json_text = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    markdown = markdown_report(payload) if args.execute else (
        "# Organization `.github` repository publication dry run\n\n"
        f"Validated `{len(rows)}` unique canonical registry organizations.\n"
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
