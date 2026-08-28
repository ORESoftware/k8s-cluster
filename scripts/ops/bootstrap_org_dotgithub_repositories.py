#!/usr/bin/env python3
"""Bootstrap and reconcile public organization-level `.github` repositories.

This publisher is intentionally bounded:

* the organization allowlist and repository name are fixed in source;
* every organization membership is preflighted before any mutation;
* existing non-managed content is preserved;
* the GitHub token is read only from GH_TOKEN and is never printed;
* execution is dry-run unless --execute is supplied.

The `.github` repository provides GitHub-supported organization defaults such as
CONTRIBUTING.md and pull-request templates. AGENTS.md and Copilot repository
instructions are maintained here as canonical source material, but GitHub does
not automatically inherit those two files into every member repository.
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import random
import time
from typing import Any, Iterable
import urllib.error
import urllib.parse
import urllib.request

API_ROOT = "https://api.github.com"
EXPECTED_ACTOR = "ORESoftware"
REPOSITORY_NAME = ".github"
BEGIN_MARKER = "<!-- BEGIN MANAGED ORGANIZATION GOVERNANCE v1 -->"
END_MARKER = "<!-- END MANAGED ORGANIZATION GOVERNANCE v1 -->"

# Exact organization logins returned by the connected GitHub App inventory on
# 2026-08-03. Mutation is impossible outside this fixed set without review.
ORGANIZATIONS: tuple[str, ...] = (
    "channelsiege",
    "OmniBlitz",
    "streamkore",
    "hypeblitz",
    "3FA-app",
    "messaging-intel",
    "akrion-sim",
    "athlet-o",
    "benefactor-cc",
    "canonical-cloud",
    "claritas-viz",
    "cliptown",
    "daedalus-fab",
    "declarative-migrations",
    "fiducia-cloud",
    "anticaptrad",
    "opto-sync",
    "quaestor-ledger",
    "sagitta-stack",
    "shared-auth",
    "scintilla-run",
    "rust-ssr-demos",
    "sonus-auris",
    "usa-acc",
    "voxletra",
    "zed-pkg",
    "zed-pkg-test",
    "memebank",
    "meta-agents-demo",
    "networking-components",
    "StreemPilot",
    "unreal-unity-poc",
    "file-tunnel",
    "hypesiege",
    "discrete-event-systems",
    "drone-mngr",
)

MANAGED_PATHS: tuple[str, ...] = (
    "README.md",
    "profile/README.md",
    "AGENTS.md",
    ".github/copilot-instructions.md",
    "CONTRIBUTING.md",
    "PULL_REQUEST_TEMPLATE.md",
)

POLICY_PARAGRAPHS = """## Mandatory semantic conflict resolution

Resolve any and all Git conflicts **semantically and with full context**.

Before finalizing a conflict resolution:

1. Read both sides, the surrounding code or documentation, and the relevant tests, contracts, schemas, generated artifacts, and deployment assumptions—not only the conflict markers.
2. Inspect the relevant Git history. When available, review at least 3 and up to 10 prior commits for the affected files or subsystem with `git log`, `git show`, and `git blame` as useful.
3. Review related repositories in this GitHub organization and relevant repositories in external organizations whenever APIs, schemas, shared libraries, generated artifacts, infrastructure, deployment behavior, or documentation cross repository boundaries.
4. Preserve the intent and invariants of all compatible changes. Synthesize a conceptual merge instead of accepting `ours` or `theirs` wholesale.
5. Search the resolved tree for leftover conflict markers and run the most relevant tests, formatters, linters, builds, contract checks, and end-to-end checks.
6. Document intentional behavioral choices, incompatible requirements, or discarded intent in the commit or pull-request description.

Never hastily pick one side, delete unfamiliar changes, or resolve from conflict markers alone. Maximize contextual and conceptual awareness across the organization and its external dependencies before completing the merge.
"""


@dataclass(frozen=True)
class ExistingFile:
    content: str
    sha: str


@dataclass
class OrganizationResult:
    organization: str
    repository: str
    created_repository: bool = False
    changed_files: list[str] | None = None
    unchanged_files: list[str] | None = None
    default_branch: str = ""
    verified: bool = False

    def __post_init__(self) -> None:
        if self.changed_files is None:
            self.changed_files = []
        if self.unchanged_files is None:
            self.unchanged_files = []

    def as_dict(self) -> dict[str, Any]:
        return {
            "organization": self.organization,
            "repository": self.repository,
            "created_repository": self.created_repository,
            "default_branch": self.default_branch,
            "changed_files": list(self.changed_files or []),
            "unchanged_files": list(self.unchanged_files or []),
            "verified": self.verified,
        }


class GitHubApi:
    def __init__(self, token: str, *, max_attempts: int = 5) -> None:
        if not token or any(ch in token for ch in "\r\n"):
            raise ValueError("GH_TOKEN must be a non-empty single-line value")
        self._token = token
        self._max_attempts = max_attempts

    def request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        *,
        allow_statuses: Iterable[int] = (),
    ) -> tuple[int, Any | None, dict[str, str]]:
        payload = None if body is None else json.dumps(body).encode("utf-8")
        url = API_ROOT + path
        allowed = set(allow_statuses)

        for attempt in range(1, self._max_attempts + 1):
            request = urllib.request.Request(url, data=payload, method=method)
            request.add_header("Accept", "application/vnd.github+json")
            request.add_header("Authorization", f"Bearer {self._token}")
            request.add_header("X-GitHub-Api-Version", "2022-11-28")
            request.add_header("User-Agent", "bounded-org-dotgithub-governance-publisher")
            if payload is not None:
                request.add_header("Content-Type", "application/json")

            try:
                with urllib.request.urlopen(request, timeout=45) as response:
                    raw = response.read()
                    parsed = json.loads(raw) if raw else None
                    return response.status, parsed, dict(response.headers.items())
            except urllib.error.HTTPError as error:
                raw = error.read(8192)
                headers = dict(error.headers.items()) if error.headers else {}
                if error.code in allowed:
                    parsed = None
                    if raw:
                        try:
                            parsed = json.loads(raw)
                        except json.JSONDecodeError:
                            parsed = None
                    return error.code, parsed, headers

                retryable = error.code in {403, 409, 429, 500, 502, 503, 504}
                if retryable and attempt < self._max_attempts:
                    delay = self._retry_delay(headers, attempt)
                    time.sleep(delay)
                    continue

                detail = raw.decode("utf-8", errors="replace")[:2000]
                raise RuntimeError(
                    f"GitHub API request failed: {method} {path} -> HTTP {error.code}: {detail}"
                ) from error
            except urllib.error.URLError as error:
                if attempt < self._max_attempts:
                    time.sleep(min(2 ** attempt, 12) + random.random())
                    continue
                raise RuntimeError(f"GitHub API transport failed for {method} {path}: {error}") from error

        raise AssertionError("unreachable request retry state")

    @staticmethod
    def _retry_delay(headers: dict[str, str], attempt: int) -> float:
        retry_after = headers.get("Retry-After")
        if retry_after and retry_after.isdigit():
            return min(float(retry_after), 60.0)
        reset = headers.get("X-RateLimit-Reset")
        if reset and reset.isdigit():
            return max(1.0, min(float(reset) - time.time() + 1.0, 60.0))
        return min(2 ** attempt, 20) + random.random()


def managed_block(body: str) -> str:
    return f"{BEGIN_MARKER}\n{body.rstrip()}\n{END_MARKER}"


def merge_managed_block(existing: str | None, body: str) -> str:
    """Preserve all non-managed text and idempotently upsert one managed block."""
    block = managed_block(body)
    if not existing:
        return block + "\n"

    start = existing.find(BEGIN_MARKER)
    end = existing.find(END_MARKER)
    if start >= 0 or end >= 0:
        if start < 0 or end < 0 or end < start:
            raise ValueError("malformed managed governance markers")
        end += len(END_MARKER)
        merged = existing[:start].rstrip() + "\n\n" + block + existing[end:]
        return merged.rstrip() + "\n"

    return existing.rstrip() + "\n\n" + block + "\n"


def render_managed_body(path: str, organization: str) -> str:
    repo = f"{organization}/{REPOSITORY_NAME}"
    if path == "README.md":
        return f"""# Organization-wide GitHub defaults

This public repository is the canonical GitHub-defaults and governance source for `{organization}`.

{POLICY_PARAGRAPHS}
## How GitHub applies this repository

- `CONTRIBUTING.md` and `PULL_REQUEST_TEMPLATE.md` act as fallback defaults for organization repositories that do not define local versions.
- `profile/README.md` is rendered on the organization profile.
- Workflow templates may be published here for organization-wide discovery, but they are not automatically installed into member repositories.
- `AGENTS.md` and `.github/copilot-instructions.md` are canonical source material in `{repo}`; they are **not automatically inherited** by every member repository. Synchronize them into repositories that need repository-scoped agent instructions, or configure supported organization-level Copilot custom instructions in GitHub settings.

Repository-local policies may add stricter requirements, but they must not weaken the semantic conflict-resolution policy above.
"""
    if path == "profile/README.md":
        return f"""# {organization}

This organization publishes shared community defaults and contribution governance from [`{repo}`](https://github.com/{repo}).

{POLICY_PARAGRAPHS}
"""
    if path == "AGENTS.md":
        return f"""# Organization-wide agent instructions

These instructions are mandatory in this repository and are the canonical source for agent guidance synchronized into repositories owned by `{organization}`.

{POLICY_PARAGRAPHS}
## Precedence and propagation

Repository-local instructions may add stricter requirements, but they must not weaken this policy. GitHub does not automatically inherit this file into every member repository; automation that copies or validates it must preserve any stricter local guidance.
"""
    if path == ".github/copilot-instructions.md":
        return f"""# GitHub Copilot repository instructions

`/AGENTS.md` is the canonical guidance for this repository. Keep synchronized copies aligned without weakening repository-local requirements.

{POLICY_PARAGRAPHS}
## Scope

These are repository-scoped instructions for `{repo}`. Do not assume GitHub automatically applies this file to other repositories in the organization; use supported organization settings or an explicit synchronization workflow for broader coverage.
"""
    if path == "CONTRIBUTING.md":
        return f"""# Contributing

Thank you for contributing to `{organization}` repositories. Read the target repository's local contribution guide, architecture documents, tests, contracts, and agent instructions first; local requirements may be stricter than this fallback.

{POLICY_PARAGRAPHS}
## Pull-request expectations

- Keep changes scoped and explain cross-repository effects.
- State which tests, builds, linters, formatters, and contract checks were run.
- For conflict resolutions, summarize the intent retained from each side and identify the 3–10-commit history or related repositories consulted when that context was available.
- Never commit credentials, tokens, private keys, or unredacted production data.
"""
    if path == "PULL_REQUEST_TEMPLATE.md":
        return f"""## Summary

Describe what changed, why it changed, and any cross-repository or deployment impact.

## Validation

- [ ] Relevant tests, builds, linters, formatters, and contract checks passed.
- [ ] No credentials, tokens, private keys, or unredacted production data are included.

## Semantic conflict-resolution check

- [ ] This pull request does not contain a conflict resolution; **or** every conflict was resolved semantically with full context.
- [ ] Both sides, surrounding code or documentation, tests, contracts, and schemas were reviewed.
- [ ] When available, 3–10 relevant prior commits were inspected with `git log`, `git show`, or `git blame`.
- [ ] Related repositories in `{organization}` and relevant external organizations were reviewed where shared behavior crossed repository boundaries.
- [ ] Compatible intent from both sides was preserved; neither `ours` nor `theirs` was accepted wholesale without justification.
- [ ] The resolved tree was checked for leftover conflict markers, and intentional tradeoffs are documented below.

## Conflict-resolution rationale or tradeoffs

Not applicable, or explain the conceptual merge and any intentionally discarded incompatible behavior.
"""
    raise KeyError(f"unsupported managed path: {path}")


def quote_path(value: str) -> str:
    return urllib.parse.quote(value, safe="/")


def get_repository(api: GitHubApi, organization: str) -> dict[str, Any] | None:
    status, payload, _ = api.request(
        "GET",
        f"/repos/{quote_path(organization)}/{REPOSITORY_NAME}",
        allow_statuses=(404,),
    )
    if status == 404:
        return None
    if not isinstance(payload, dict):
        raise RuntimeError(f"invalid repository payload for {organization}/{REPOSITORY_NAME}")
    return payload


def validate_repository(repository: dict[str, Any], organization: str) -> None:
    full_name = f"{organization}/{REPOSITORY_NAME}"
    if repository.get("full_name", "").lower() != full_name.lower():
        raise RuntimeError(f"unexpected repository identity for {full_name}")
    if repository.get("visibility") != "public" or repository.get("private") is True:
        raise RuntimeError(f"refusing non-public repository {full_name}")
    if repository.get("archived") is True:
        raise RuntimeError(f"refusing archived repository {full_name}")


def preflight(api: GitHubApi) -> dict[str, dict[str, Any] | None]:
    status, user, _ = api.request("GET", "/user")
    if status != 200 or not isinstance(user, dict) or user.get("login") != EXPECTED_ACTOR:
        observed = user.get("login") if isinstance(user, dict) else None
        raise RuntimeError(f"unexpected GitHub publisher identity: {observed!r}")

    repositories: dict[str, dict[str, Any] | None] = {}
    for organization in ORGANIZATIONS:
        status, membership, _ = api.request(
            "GET", f"/user/memberships/orgs/{quote_path(organization)}"
        )
        if status != 200 or not isinstance(membership, dict):
            raise RuntimeError(f"missing organization membership payload for {organization}")
        observed = (membership.get("role"), membership.get("state"))
        if observed != ("admin", "active"):
            raise RuntimeError(f"{organization} owner membership is {observed!r}, expected admin/active")

        repository = get_repository(api, organization)
        if repository is not None:
            validate_repository(repository, organization)
        repositories[organization] = repository
        print(f"PREFLIGHT {organization} repository={'present' if repository else 'missing'}")
    return repositories


def create_repository(api: GitHubApi, organization: str) -> dict[str, Any]:
    body = {
        "name": REPOSITORY_NAME,
        "description": "Organization-wide GitHub defaults, community health files, templates, and governance",
        "private": False,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "auto_init": True,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }
    status, payload, _ = api.request(
        "POST",
        f"/orgs/{quote_path(organization)}/repos",
        body,
        allow_statuses=(422,),
    )
    if status == 422:
        repository = get_repository(api, organization)
        if repository is None:
            raise RuntimeError(f"repository creation raced but {organization}/{REPOSITORY_NAME} is absent")
        validate_repository(repository, organization)
        return repository
    if status != 201 or not isinstance(payload, dict):
        raise RuntimeError(f"failed to create {organization}/{REPOSITORY_NAME}: HTTP {status}")
    validate_repository(payload, organization)
    print(f"CREATED {organization}/{REPOSITORY_NAME}")
    return payload


def resolve_default_branch(api: GitHubApi, organization: str, repository: dict[str, Any]) -> str:
    branch = repository.get("default_branch")
    if isinstance(branch, str) and branch:
        return branch
    for _ in range(8):
        time.sleep(1.5)
        refreshed = get_repository(api, organization)
        branch = refreshed.get("default_branch") if refreshed else None
        if isinstance(branch, str) and branch:
            return branch
    raise RuntimeError(f"default branch did not initialize for {organization}/{REPOSITORY_NAME}")


def fetch_file(
    api: GitHubApi, organization: str, path: str, branch: str
) -> ExistingFile | None:
    endpoint = (
        f"/repos/{quote_path(organization)}/{REPOSITORY_NAME}/contents/{quote_path(path)}"
        f"?ref={urllib.parse.quote(branch, safe='')}"
    )
    status, payload, _ = api.request("GET", endpoint, allow_statuses=(404,))
    if status == 404:
        return None
    if not isinstance(payload, dict) or payload.get("type") != "file":
        raise RuntimeError(f"unexpected file payload for {organization}/{REPOSITORY_NAME}:{path}")
    encoded = payload.get("content")
    sha = payload.get("sha")
    if not isinstance(encoded, str) or not isinstance(sha, str):
        raise RuntimeError(f"missing file content or SHA for {organization}/{REPOSITORY_NAME}:{path}")
    try:
        content = base64.b64decode(encoded, validate=False).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as error:
        raise RuntimeError(f"file is not UTF-8 text: {organization}/{REPOSITORY_NAME}:{path}") from error
    return ExistingFile(content=content, sha=sha)


def put_file(
    api: GitHubApi,
    organization: str,
    path: str,
    branch: str,
    content: str,
    existing: ExistingFile | None,
) -> None:
    body: dict[str, Any] = {
        "message": f"docs: reconcile organization governance in {path}",
        "content": base64.b64encode(content.encode("utf-8")).decode("ascii"),
        "branch": branch,
    }
    if existing is not None:
        body["sha"] = existing.sha
    endpoint = f"/repos/{quote_path(organization)}/{REPOSITORY_NAME}/contents/{quote_path(path)}"
    status, payload, _ = api.request("PUT", endpoint, body)
    if status not in {200, 201} or not isinstance(payload, dict):
        raise RuntimeError(
            f"failed to update {organization}/{REPOSITORY_NAME}:{path}: HTTP {status}"
        )


def reconcile_organization(
    api: GitHubApi,
    organization: str,
    repository: dict[str, Any] | None,
    *,
    execute: bool,
) -> OrganizationResult:
    full_name = f"{organization}/{REPOSITORY_NAME}"
    result = OrganizationResult(organization=organization, repository=full_name)
    if repository is None:
        if not execute:
            result.created_repository = True
            result.changed_files.extend(MANAGED_PATHS)
            return result
        repository = create_repository(api, organization)
        result.created_repository = True

    branch = resolve_default_branch(api, organization, repository)
    result.default_branch = branch

    for path in MANAGED_PATHS:
        existing = fetch_file(api, organization, path, branch)
        desired = merge_managed_block(
            existing.content if existing else None,
            render_managed_body(path, organization),
        )
        if existing is not None and existing.content == desired:
            result.unchanged_files.append(path)
            continue
        result.changed_files.append(path)
        if execute:
            put_file(api, organization, path, branch, desired, existing)
            print(f"UPDATED {full_name}:{path}")

    if execute:
        verify_organization(api, organization, branch)
        result.verified = True
    return result


def verify_organization(api: GitHubApi, organization: str, branch: str) -> None:
    repository = get_repository(api, organization)
    if repository is None:
        raise RuntimeError(f"verification failed: missing {organization}/{REPOSITORY_NAME}")
    validate_repository(repository, organization)
    for path in MANAGED_PATHS:
        current = fetch_file(api, organization, path, branch)
        if current is None:
            raise RuntimeError(f"verification failed: missing {organization}/{REPOSITORY_NAME}:{path}")
        if BEGIN_MARKER not in current.content or END_MARKER not in current.content:
            raise RuntimeError(f"verification failed: unmanaged {organization}/{REPOSITORY_NAME}:{path}")
        if "3 and up to 10 prior commits" not in current.content and path != "PULL_REQUEST_TEMPLATE.md":
            raise RuntimeError(f"verification failed: policy absent in {organization}/{REPOSITORY_NAME}:{path}")
    print(f"VERIFIED {organization}/{REPOSITORY_NAME}")


def markdown_report(results: list[OrganizationResult], *, execute: bool) -> str:
    mode = "executed" if execute else "dry-run"
    created = sum(1 for item in results if item.created_repository)
    changed = sum(len(item.changed_files or []) for item in results)
    verified = sum(1 for item in results if item.verified)
    lines = [
        "# Organization `.github` governance publication",
        "",
        f"- Mode: **{mode}**",
        f"- Organizations: **{len(results)}**",
        f"- Repositories created or planned: **{created}**",
        f"- Files changed or planned: **{changed}**",
        f"- Repositories verified: **{verified}**",
        "",
        "| Organization | Repository | Created | Changed files | Verified |",
        "|---|---|---:|---:|---:|",
    ]
    for item in results:
        lines.append(
            f"| `{item.organization}` | `{item.repository}` | "
            f"{'yes' if item.created_repository else 'no'} | "
            f"{len(item.changed_files or [])} | {'yes' if item.verified else 'no'} |"
        )
    lines.extend(
        [
            "",
            "## Policy",
            "",
            "All managed repositories require semantic conflict resolution with full context, normally including 3–10 relevant prior commits and related same-organization and external-organization repositories when shared behavior crosses boundaries.",
            "",
            "## Propagation note",
            "",
            "GitHub-supported fallback community files and pull-request templates apply to member repositories that lack local overrides. `AGENTS.md` and `.github/copilot-instructions.md` remain canonical source files and require explicit synchronization or supported organization-level settings for member-repository coverage.",
        ]
    )
    return "\n".join(lines) + "\n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", help="perform GitHub mutations")
    parser.add_argument("--json-report", type=Path)
    parser.add_argument("--markdown-report", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    token = os.environ.get("GH_TOKEN", "")
    if not token:
        raise SystemExit("GH_TOKEN is required for both preflight and execution")

    if len(ORGANIZATIONS) != 36 or len(set(name.lower() for name in ORGANIZATIONS)) != 36:
        raise SystemExit("fixed organization allowlist must contain exactly 36 unique logins")

    api = GitHubApi(token)
    existing = preflight(api)
    results = [
        reconcile_organization(
            api,
            organization,
            existing[organization],
            execute=args.execute,
        )
        for organization in ORGANIZATIONS
    ]

    report = {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "mode": "execute" if args.execute else "dry-run",
        "expected_actor": EXPECTED_ACTOR,
        "repository_name": REPOSITORY_NAME,
        "managed_paths": list(MANAGED_PATHS),
        "organizations": [item.as_dict() for item in results],
    }
    if args.json_report:
        args.json_report.parent.mkdir(parents=True, exist_ok=True)
        args.json_report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    rendered = markdown_report(results, execute=args.execute)
    if args.markdown_report:
        args.markdown_report.parent.mkdir(parents=True, exist_ok=True)
        args.markdown_report.write_text(rendered, encoding="utf-8")
    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
