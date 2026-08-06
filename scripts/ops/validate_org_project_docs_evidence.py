#!/usr/bin/env python3
"""Validate organization Project reconciliation evidence.

A row may be reported as successful only when every identity and landing-evidence
field is internally consistent. API error payloads, blank URLs, synthetic action
claims, duplicate owners, and incomplete PR/issue evidence fail closed.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

ORG_LOGIN_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
INTEGER_RE = re.compile(r"^[1-9][0-9]*$")
RATE_LIMIT_RE = re.compile(r"rate limit|secondary rate|abuse detection", re.IGNORECASE)


def _string(value: Any) -> str:
    return value if isinstance(value, str) else ""


def validate_results(results: Any, expected_count: int) -> list[str]:
    errors: list[str] = []
    if not isinstance(results, list):
        return ["top-level evidence must be a JSON array"]

    if len(results) != expected_count:
        errors.append(f"expected {expected_count} rows, found {len(results)}")

    seen_requested: set[str] = set()
    seen_canonical: set[str] = set()

    for index, row in enumerate(results):
        prefix = f"row {index + 1}"
        if not isinstance(row, dict):
            errors.append(f"{prefix}: row must be an object")
            continue

        requested = _string(row.get("requested_org"))
        canonical = _string(row.get("canonical_org"))
        status = _string(row.get("status"))
        project_title = _string(row.get("project_title"))
        project_number = _string(row.get("project_number"))
        project_url = _string(row.get("project_url"))
        project_action = _string(row.get("project_action"))
        repository_action = _string(row.get("repository_action"))
        documentation_action = _string(row.get("documentation_action"))
        error_message = _string(row.get("error"))

        if not requested or not ORG_LOGIN_RE.fullmatch(requested) or "--" in requested:
            errors.append(f"{prefix}: invalid requested_org {requested!r}")
        elif requested.casefold() in seen_requested:
            errors.append(f"{prefix}: duplicate requested_org {requested!r}")
        else:
            seen_requested.add(requested.casefold())

        if not canonical or not ORG_LOGIN_RE.fullmatch(canonical) or "--" in canonical:
            errors.append(f"{prefix}: invalid canonical_org {canonical!r}")
        elif canonical.casefold() in seen_canonical:
            errors.append(f"{prefix}: duplicate canonical_org {canonical!r}")
        else:
            seen_canonical.add(canonical.casefold())

        if requested and canonical and requested.casefold() != canonical.casefold():
            errors.append(
                f"{prefix}: requested_org {requested!r} does not resolve to canonical_org {canonical!r}"
            )

        serialized = json.dumps(row, sort_keys=True)
        if RATE_LIMIT_RE.search(serialized) or '"status": "403"' in serialized:
            errors.append(f"{prefix}: API rate-limit/error payload leaked into evidence")

        if status != "ok":
            errors.append(f"{prefix}: status must be 'ok', found {status!r}")
        if error_message:
            errors.append(f"{prefix}: successful row contains error text")

        if project_title != f"{canonical}-project":
            errors.append(f"{prefix}: inconsistent project_title {project_title!r}")
        if not INTEGER_RE.fullmatch(project_number):
            errors.append(f"{prefix}: invalid project_number {project_number!r}")
        expected_project_url = (
            f"https://github.com/orgs/{canonical}/projects/{project_number}"
            if canonical and project_number
            else ""
        )
        if project_url != expected_project_url:
            errors.append(f"{prefix}: inconsistent project_url {project_url!r}")
        if project_action not in {
            "existing",
            "created",
            "renamed-project-1",
            "renamed-preferred-project",
            "reopened",
        }:
            errors.append(f"{prefix}: unsupported project_action {project_action!r}")
        if not repository_action or repository_action == "unknown":
            errors.append(f"{prefix}: missing repository_action")
        if documentation_action not in {"unchanged", "updated"}:
            errors.append(f"{prefix}: invalid documentation_action {documentation_action!r}")

        pull_request = row.get("pull_request")
        if not isinstance(pull_request, dict):
            errors.append(f"{prefix}: pull_request must be an object")
        else:
            pr_number = _string(pull_request.get("number"))
            pr_url = _string(pull_request.get("url"))
            pr_state = _string(pull_request.get("state"))
            if documentation_action == "updated":
                if not INTEGER_RE.fullmatch(pr_number):
                    errors.append(f"{prefix}: updated docs require a numeric PR number")
                expected_pr_url = (
                    f"https://github.com/{canonical}/.github/pull/{pr_number}"
                    if canonical and pr_number
                    else ""
                )
                if pr_url != expected_pr_url:
                    errors.append(f"{prefix}: inconsistent PR URL {pr_url!r}")
                if not pr_state.startswith("merged-"):
                    errors.append(f"{prefix}: updated docs require a merged PR, found {pr_state!r}")
            elif any((pr_number, pr_url)) or pr_state != "not-needed":
                errors.append(f"{prefix}: unchanged docs require PR state 'not-needed' and no PR URL")

        governance = row.get("governance_issue")
        if not isinstance(governance, dict):
            errors.append(f"{prefix}: governance_issue must be an object")
        else:
            issue_number = _string(governance.get("number"))
            issue_url = _string(governance.get("url"))
            item_action = _string(governance.get("project_item_action"))
            if not INTEGER_RE.fullmatch(issue_number):
                errors.append(f"{prefix}: governance issue number must be numeric")
            expected_issue_url = (
                f"https://github.com/{canonical}/.github/issues/{issue_number}"
                if canonical and issue_number
                else ""
            )
            if issue_url != expected_issue_url:
                errors.append(f"{prefix}: inconsistent governance issue URL {issue_url!r}")
            if item_action not in {"added", "existing"}:
                errors.append(f"{prefix}: invalid project item action {item_action!r}")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("results_json", type=Path)
    parser.add_argument("expected_count", type=int)
    args = parser.parse_args()

    try:
        results = json.loads(args.results_json.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"unable to load evidence: {exc}", file=sys.stderr)
        return 2

    errors = validate_results(results, args.expected_count)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    print(f"validated {len(results)} organization reconciliation rows")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
