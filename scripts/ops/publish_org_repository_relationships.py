#!/usr/bin/env python3
"""Publish privacy-safe relationship maps to the fixed org `.github` fleet."""

from __future__ import annotations

import argparse
import base64
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sys
from typing import Any
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import bootstrap_org_dotgithub_repositories as base  # noqa: E402
from org_repository_relationships_graph import (  # noqa: E402
    build_external_relationships,
    build_internal_relationships,
)
from org_repository_relationships_model import (  # noqa: E402
    JSON_PATH,
    MD_PATH,
    SCHEMA_PATH,
    SCHEMA_VERSION,
    build_manifest,
    relationship_schema,
)
from org_repository_relationships_render import (  # noqa: E402
    BEGIN_MARKER,
    END_MARKER,
    merge_managed_block,
    relationship_readme_block,
    render_markdown,
)
from org_repository_relationships_roles import (  # noqa: E402
    classify_repository,
    public_repository_entry,
)

ORGANIZATIONS = base.ORGANIZATIONS
README_PATHS = ("README.md", "profile/README.md")


def list_repositories(
    api: base.GitHubApi,
    organization: str,
) -> list[dict[str, Any]]:
    repositories: list[dict[str, Any]] = []
    for page in range(1, 101):
        endpoint = (
            f"/orgs/{quote(organization)}/repos"
            f"?type=all&sort=full_name&per_page=100&page={page}"
        )
        status, payload, _ = api.request("GET", endpoint)
        if status != 200 or not isinstance(payload, list):
            raise RuntimeError(
                f"invalid repository inventory for {organization}"
            )
        repositories.extend(
            item for item in payload if isinstance(item, dict)
        )
        if len(payload) < 100:
            return sorted(
                repositories,
                key=lambda item: str(item.get("name", "")).lower(),
            )
    raise RuntimeError(
        f"repository inventory exceeds bounded pagination for {organization}"
    )


def write_file(
    api: base.GitHubApi,
    organization: str,
    path: str,
    branch: str,
    content: str,
    existing: Any,
) -> None:
    request = {
        "message": f"docs: reconcile repository relationships in {path}",
        "content": base64.b64encode(content.encode()).decode(),
        "branch": branch,
    }
    if existing:
        request["sha"] = existing.sha
    endpoint = (
        f"/repos/{quote(organization)}/.github/contents/"
        f"{quote(path, safe='/')}"
    )
    status, payload, _ = api.request("PUT", endpoint, request)
    if status not in (200, 201) or not isinstance(payload, dict):
        raise RuntimeError(
            f"failed to write relationship registry for {organization}"
        )


def build_plan(
    api: base.GitHubApi,
    organization: str,
    dotgithub: dict[str, Any],
    inventory: list[dict[str, Any]],
) -> tuple[str, dict[str, tuple[str, Any]], set[str], dict[str, Any]]:
    branch = dotgithub.get("default_branch")
    if not branch:
        raise RuntimeError(
            f"missing default branch for {organization}/.github"
        )

    data = build_manifest(organization, inventory)
    private_names = {
        str(repository.get("name"))
        for repository in inventory
        if repository.get("private")
        or repository.get("visibility") == "private"
    }
    rendered_json = json.dumps(data, indent=2, sort_keys=True) + "\n"
    rendered_markdown = render_markdown(data)
    if any(
        name and name in rendered_json + rendered_markdown
        for name in private_names
    ):
        raise RuntimeError(
            f"privacy preflight failed for {organization}/.github"
        )

    desired = {
        JSON_PATH: rendered_json,
        SCHEMA_PATH: (
            json.dumps(relationship_schema(), indent=2, sort_keys=True) + "\n"
        ),
        MD_PATH: rendered_markdown,
    }
    files = {
        path: (content, base.fetch_file(api, organization, path, branch))
        for path, content in desired.items()
    }
    for path in README_PATHS:
        existing = base.fetch_file(api, organization, path, branch)
        content = merge_managed_block(
            existing.content if existing else None,
            relationship_readme_block(organization),
        )
        files[path] = (content, existing)

    result = {
        "organization": organization,
        "public_repository_count": len(data["repositories"]),
        "private_repository_count": data["privacy"][
            "private_repository_count"
        ],
        "relationship_count": len(data["relationships"]),
        "changed_files": [],
        "unchanged_files": [],
        "verified": False,
    }
    return branch, files, private_names, result


def run_plan(
    api: base.GitHubApi,
    plan: tuple[
        str,
        str,
        dict[str, tuple[str, Any]],
        set[str],
        dict[str, Any],
    ],
    execute: bool,
) -> None:
    organization, branch, files, private_names, result = plan
    for path, (desired, existing) in files.items():
        changed = not existing or existing.content != desired
        target = (
            result["changed_files"]
            if changed
            else result["unchanged_files"]
        )
        target.append(path)
        if execute and changed:
            write_file(
                api,
                organization,
                path,
                branch,
                desired,
                existing,
            )
            print(f"UPDATED {organization}/.github:{path}")

    if not execute:
        return
    for path, (desired, _) in files.items():
        observed = base.fetch_file(api, organization, path, branch)
        leaked = observed and any(
            name and name in observed.content for name in private_names
        )
        if not observed or observed.content != desired or leaked:
            raise RuntimeError(
                f"relationship verification failed for {organization}/.github"
            )
    result["verified"] = True
    print(f"VERIFIED {organization}/.github relationships")


def render_report(
    results: list[dict[str, Any]],
    execute: bool,
) -> str:
    lines = [
        "# Organization repository-relationship publication",
        "",
        f"- Mode: **{'executed' if execute else 'dry-run'}**",
        f"- Organizations: **{len(results)}**",
        (
            "- Public repositories declared: "
            f"**{sum(item['public_repository_count'] for item in results)}**"
        ),
        (
            "- Private repository names withheld: "
            f"**{sum(item['private_repository_count'] for item in results)}**"
        ),
        (
            "- Relationship edges: "
            f"**{sum(item['relationship_count'] for item in results)}**"
        ),
        (
            "- Organizations verified: "
            f"**{sum(bool(item['verified']) for item in results)}**"
        ),
        "",
        (
            "| Organization | Public | Private names withheld | Edges | "
            "Changed | Verified |"
        ),
        "|---|---:|---:|---:|---:|---:|",
    ]
    lines.extend(
        (
            f"| `{item['organization']}` | "
            f"{item['public_repository_count']} | "
            f"{item['private_repository_count']} | "
            f"{item['relationship_count']} | "
            f"{len(item['changed_files'])} | "
            f"{'yes' if item['verified'] else 'no'} |"
        )
        for item in results
    )
    return "\n".join(lines) + "\n"


def write_report(path: Path | None, content: str) -> None:
    if not path:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--json-report", type=Path)
    parser.add_argument("--markdown-report", type=Path)
    arguments = parser.parse_args()

    api = base.GitHubApi(os.environ.get("GH_TOKEN", ""))
    dotgithub = base.preflight(api)
    if any(repository is None for repository in dotgithub.values()):
        raise RuntimeError(
            "all organization .github repositories must exist before "
            "relationship publication"
        )

    inventories = {
        organization: list_repositories(api, organization)
        for organization in ORGANIZATIONS
    }
    plans = []
    for organization in ORGANIZATIONS:
        branch, files, private_names, result = build_plan(
            api,
            organization,
            dotgithub[organization],
            inventories[organization],
        )
        plans.append(
            (organization, branch, files, private_names, result)
        )
    for plan in plans:
        run_plan(api, plan, arguments.execute)

    results = [plan[-1] for plan in plans]
    payload = {
        "mode": "execute" if arguments.execute else "dry-run",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "organizations": results,
    }
    report = render_report(results, arguments.execute)
    if arguments.json_report:
        write_report(
            arguments.json_report,
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
        )
    write_report(arguments.markdown_report, report)
    print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
