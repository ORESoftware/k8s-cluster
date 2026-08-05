#!/usr/bin/env python3
"""Create and verify the nine remaining organization-level .github repositories.

The caller supplies a previously authorized GitHub PAT only through the process
environment after decrypting it with a one-time in-memory key. The token is
removed from the environment immediately and is never printed or persisted.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import publish_org_dotgithub_policy_with_protected_apps as policy

TARGET_ORGANIZATIONS = (
    "agent-pontifex",
    "apostille-me",
    "dancing-dragons",
    "embedded-alerts",
    "evento-globolo",
    "fanwaave",
    "fifa-math",
    "gha-indie-worker",
    "hacker-house-medellin",
)
EXPECTED_FILES = (
    "BRANCHING_AND_DEPLOYMENT.md",
    "AGENTS.md",
    "agents.md",
    ".github/copilot-instructions.md",
    ".github/PULL_REQUEST_TEMPLATE.md",
    "organization-policy.json",
)
TOKEN_ENV = "GH_ORG_BOOTSTRAP_TOKEN"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def validate_targets() -> None:
    if len(TARGET_ORGANIZATIONS) != 9:
        raise RuntimeError("exactly nine target organizations are required")
    if len(set(TARGET_ORGANIZATIONS)) != len(TARGET_ORGANIZATIONS):
        raise RuntimeError("duplicate target organization")
    if any(name.casefold().endswith("-test") for name in TARGET_ORGANIZATIONS):
        raise RuntimeError("test organizations are forbidden")
    if len(EXPECTED_FILES) != 6:
        raise RuntimeError("exactly six policy files are required")


def load_token() -> str:
    token = os.environ.pop(TOKEN_ENV, "").strip()
    if not token or any(character.isspace() for character in token):
        raise RuntimeError("ephemeral GitHub token is missing or malformed")
    if len(token) < 20 or len(token) > 512:
        raise RuntimeError("ephemeral GitHub token length is invalid")
    return token


def validate_token(token: str) -> str:
    status, document = policy.api_request("GET", "/user", token)
    if status != 200 or not isinstance(document, dict):
        raise RuntimeError(f"ephemeral GitHub token validation failed: HTTP {status}")
    login = document.get("login")
    if not isinstance(login, str) or not login:
        raise RuntimeError("validated GitHub token has no account login")
    return login


def publish_one(organization: str, token: str) -> dict[str, Any]:
    repository_created, repository = policy.ensure_repository(organization, token)
    desired = policy.desired_files(organization, token)
    if tuple(desired) != EXPECTED_FILES:
        raise RuntimeError(
            f"canonical policy file set changed unexpectedly for {organization}: "
            f"{tuple(desired)}"
        )

    actions: dict[str, str] = {}
    for path in EXPECTED_FILES:
        actions[path] = policy.put_text_file(organization, path, desired[path], token)

    for path in EXPECTED_FILES:
        actual, _ = policy.get_text_file(organization, path, token)
        if actual != desired[path]:
            raise RuntimeError(f"read-after-write mismatch for {organization}/.github:{path}")

    status, refreshed = policy.get_repository(organization, token)
    if status != 200 or not isinstance(refreshed, dict):
        raise RuntimeError(f"repository verification failed for {organization}/.github")
    if refreshed.get("visibility") != "public" or refreshed.get("archived") is True:
        raise RuntimeError(f"{organization}/.github is not public and active")

    return {
        "organization": organization,
        "repository_created": repository_created,
        "repository_url": repository.get("html_url"),
        "files_created": sum(value == "created" for value in actions.values()),
        "files_updated": sum(value == "updated" for value in actions.values()),
        "files_unchanged": sum(value == "unchanged" for value in actions.values()),
        "verified": True,
    }


def self_test() -> None:
    validate_targets()
    assert "greater than 99.1%" in policy.POLICY_BLOCK
    assert "greater than 99.7%" in policy.POLICY_BLOCK
    assert "`dev` is the integration branch" in policy.POLICY_BLOCK
    assert "*-infra" in policy.POLICY_BLOCK
    assert "GitOps controller" in policy.POLICY_BLOCK
    assert set(EXPECTED_FILES) == {
        "BRANCHING_AND_DEPLOYMENT.md",
        "AGENTS.md",
        "agents.md",
        ".github/copilot-instructions.md",
        ".github/PULL_REQUEST_TEMPLATE.md",
        "organization-policy.json",
    }
    print("nine-organization .github bootstrap self-test: ok")


def write_report(path: Path, report: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0

    validate_targets()
    token = load_token()
    report: dict[str, Any] = {
        "schema_version": 1,
        "policy_version": policy.POLICY_VERSION,
        "target_count": len(TARGET_ORGANIZATIONS),
        "token_persisted": False,
        "organizations": [],
        "failures": [],
    }

    try:
        report["authenticated_login"] = validate_token(token)
        for organization in TARGET_ORGANIZATIONS:
            print(f"TARGET {organization}/.github", file=sys.stderr)
            try:
                result = publish_one(organization, token)
                report["organizations"].append(result)
                print(
                    f"VERIFIED {organization}/.github "
                    f"created_repo={result['repository_created']} "
                    f"created={result['files_created']} "
                    f"updated={result['files_updated']} "
                    f"unchanged={result['files_unchanged']}",
                    file=sys.stderr,
                )
            except Exception as error:
                report["failures"].append(
                    {"organization": organization, "error": str(error)[:1200]}
                )
                print(f"FAILED {organization}: {error}", file=sys.stderr)
    except Exception as error:
        report["failures"].append({"organization": "__token__", "error": str(error)[:1200]})
        print(f"FAILED token validation: {error}", file=sys.stderr)
    finally:
        token = ""

    report["verified_count"] = sum(
        item.get("verified") is True for item in report["organizations"]
    )
    report["repository_created_count"] = sum(
        item.get("repository_created") is True for item in report["organizations"]
    )
    write_report(args.report, report)
    print(json.dumps(report, separators=(",", ":"), sort_keys=True))
    return 0 if report["verified_count"] == 9 and not report["failures"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
