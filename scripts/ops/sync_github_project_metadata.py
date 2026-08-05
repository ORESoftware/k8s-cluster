#!/usr/bin/env python3
"""Synchronize GitHub Projects v2 metadata from the portfolio-link registry."""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

GRAPHQL_URL = "https://api.github.com/graphql"
REGISTRY_URL = (
    "https://github.com/ORESoftware/k8s-cluster/blob/main/"
    "ops/registries/portfolio-project-links.csv"
)

PROJECT_QUERY = """
query ProjectByOrganization($login: String!) {
  organization(login: $login) {
    login
    projectsV2(first: 100) {
      nodes {
        id
        number
        title
        url
        closed
        shortDescription
        readme
      }
    }
  }
}
"""

UPDATE_MUTATION = """
mutation UpdateProjectMetadata(
  $projectId: ID!
  $shortDescription: String!
  $readme: String!
) {
  updateProjectV2(
    input: {
      projectId: $projectId
      shortDescription: $shortDescription
      readme: $readme
    }
  ) {
    projectV2 {
      id
      number
      title
      url
      shortDescription
      readme
    }
  }
}
"""


class GraphQLClient:
    def __init__(self, token: str) -> None:
        self._token = token

    def execute(self, query: str, variables: dict[str, Any]) -> dict[str, Any]:
        payload = json.dumps(
            {"query": query, "variables": variables},
            separators=(",", ":"),
        ).encode("utf-8")
        request = urllib.request.Request(
            GRAPHQL_URL,
            data=payload,
            method="POST",
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "Content-Type": "application/json",
                "User-Agent": "oresoftware-portfolio-project-link-sync/1",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                body = json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")
            raise RuntimeError(
                f"GitHub GraphQL HTTP {error.code}: {detail[:1000]}"
            ) from error
        except urllib.error.URLError as error:
            raise RuntimeError(f"GitHub GraphQL request failed: {error}") from error

        errors = body.get("errors")
        if errors:
            raise RuntimeError(
                "GitHub GraphQL errors: "
                + json.dumps(errors, separators=(",", ":"))[:2000]
            )
        data = body.get("data")
        if not isinstance(data, dict):
            raise RuntimeError("GitHub GraphQL response omitted data")
        return data


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "registry",
        nargs="?",
        type=Path,
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    parser.add_argument(
        "--evidence",
        type=Path,
        default=Path("ops/evidence/portfolio-project-metadata-sync.json"),
    )
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def project_readme(row: dict[str, str]) -> str:
    key = row["portfolio_key"]
    return f"""# {row['github_project_title']}

Canonical cross-system project linkage for `{key}`.

| System | Canonical reference |
| --- | --- |
| Portfolio key | `portfolio_key={key}` |
| ChatGPT Project | `{row['chatgpt_project_name']}` |
| GitHub organization | [{row['github_org']}](https://github.com/{row['github_org']}) |
| GitHub Project | [{row['github_project_title']}]({row['github_project_url']}) |
| Linear Project | [{row['linear_project_name']}]({row['linear_project_url']}) (`{row['linear_project_id']}`) |
| Slack channel | [#{row['slack_channel_name']}]({row['slack_channel_url']}) (`{row['slack_channel_id']}`) |

Source of truth: [portfolio-project-links.csv]({REGISTRY_URL})

Marker: `portfolio-link-registry:v1:{key}`
"""


def short_description(row: dict[str, str]) -> str:
    return (
        f"key={row['portfolio_key']} · "
        f"Linear {row['linear_project_name']} · "
        f"Slack #{row['slack_channel_name']}"
    )


def load_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))
    if not rows:
        raise RuntimeError(f"registry is empty: {path}")
    return rows


def synchronize(
    client: GraphQLClient,
    rows: list[dict[str, str]],
    *,
    dry_run: bool,
) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []

    for row in rows:
        key = row["portfolio_key"]
        org = row["github_org"]
        expected_number = int(row["github_project_number"])
        expected_title = row["github_project_title"]
        expected_url = row["github_project_url"]

        data = client.execute(PROJECT_QUERY, {"login": org})
        organization = data.get("organization")
        if not isinstance(organization, dict):
            raise RuntimeError(
                f"{key}: organization {org!r} is not visible to the supplied token"
            )

        projects = organization.get("projectsV2", {}).get("nodes", [])
        candidates = [
            project
            for project in projects
            if int(project["number"]) == expected_number
        ]
        if len(candidates) != 1:
            raise RuntimeError(
                f"{key}: expected exactly one GitHub Project #{expected_number}; "
                f"found {len(candidates)}"
            )

        project = candidates[0]
        if project["title"] != expected_title:
            raise RuntimeError(
                f"{key}: Project #{expected_number} title is {project['title']!r}; "
                f"expected {expected_title!r}"
            )
        if project["url"] != expected_url:
            raise RuntimeError(
                f"{key}: Project URL is {project['url']!r}; expected {expected_url!r}"
            )
        if project.get("closed"):
            raise RuntimeError(f"{key}: canonical GitHub Project is closed")

        desired_short = short_description(row)
        desired_readme = project_readme(row)
        changed = (
            project.get("shortDescription") != desired_short
            or project.get("readme") != desired_readme
        )

        action = "unchanged"
        if changed:
            action = "would-update" if dry_run else "updated"
            if not dry_run:
                updated = client.execute(
                    UPDATE_MUTATION,
                    {
                        "projectId": project["id"],
                        "shortDescription": desired_short,
                        "readme": desired_readme,
                    },
                )["updateProjectV2"]["projectV2"]
                if (
                    updated.get("shortDescription") != desired_short
                    or updated.get("readme") != desired_readme
                ):
                    raise RuntimeError(
                        f"{key}: GitHub Project metadata did not verify after update"
                    )

        results.append(
            {
                "portfolio_key": key,
                "github_org": org,
                "github_project_number": expected_number,
                "github_project_title": expected_title,
                "github_project_url": expected_url,
                "linear_project_id": row["linear_project_id"],
                "slack_channel_id": row["slack_channel_id"],
                "action": action,
            }
        )
        print(f"{key}: {action}")

    return results


def main() -> int:
    args = parse_args()
    token = os.environ.get("GITHUB_PROJECTS_TOKEN", "").strip()
    if not token:
        print("GITHUB_PROJECTS_TOKEN is required", file=sys.stderr)
        return 2

    try:
        rows = load_rows(args.registry)
        results = synchronize(GraphQLClient(token), rows, dry_run=args.dry_run)
    except (OSError, RuntimeError, ValueError, KeyError, TypeError) as error:
        print(f"project metadata synchronization failed: {error}", file=sys.stderr)
        return 1

    evidence = {
        "schema_version": 1,
        "generated_at": dt.datetime.now(dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat(),
        "registry": str(args.registry),
        "dry_run": args.dry_run,
        "mapping_count": len(results),
        "updated_count": sum(
            result["action"] in {"updated", "would-update"} for result in results
        ),
        "unchanged_count": sum(
            result["action"] == "unchanged" for result in results
        ),
        "results": results,
    }
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    args.evidence.write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(
        f"synchronized {len(results)} GitHub Projects; "
        f"evidence written to {args.evidence}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
