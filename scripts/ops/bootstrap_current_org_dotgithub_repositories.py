#!/usr/bin/env python3
"""Create missing public organization-default `.github` repositories safely."""
from __future__ import annotations

import argparse
import base64
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

API = "https://api.github.com"
EXPECTED_LOGIN = "ORESoftware"
REPOSITORY = ".github"
FLEET_ID = "current-org-dotgithub-20260804-v1"
EXPECTED_COUNT = 62
POLICY_VERSION = "2026-08-04"
ORGANIZATIONS = (
    "channelsiege", "OmniBlitz", "streamkore", "hypeblitz", "fanwaave", "r2g-test",
    "3FA-app", "messaging-intel", "akrion-sim", "athlet-o", "benefactor-cc",
    "canonical-cloud", "claritas-viz", "cliptown", "daedalus-fab",
    "declarative-migrations", "fiducia-cloud", "anticaptrad", "opto-sync",
    "quaestor-ledger", "sagitta-stack", "shared-auth", "scintilla-run",
    "rust-ssr-demos", "sonus-auris", "usa-acc", "voxletra", "zed-pkg",
    "zed-pkg-test", "memebank", "meta-agents-demo", "networking-components",
    "StreemPilot", "unreal-unity-poc", "file-tunnel", "hypesiege",
    "discrete-event-systems", "drone-mngr", "agent-pontifex", "fifa-math",
    "gha-indie-worker", "apostille-me", "embedded-alerts", "evento-globolo",
    "hacker-house-medellin", "3fa-app-test", "claritas-viz-test", "cliptown-test",
    "declarative-migrations-test", "embedded-alerts-test", "evento-globolo-test",
    "fiducia-cloud-test", "memebank-test", "opto-sync-test", "quaestor-ledger-test",
    "sonus-auris-test", "messaging-intel-test", "scintilla-run-test",
    "file-tunnel-test", "shared-auth-test", "hypesiege-test", "streempilot-test",
)
SECRET_RE = re.compile(
    r"(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|"
    r"lin_api_[A-Za-z0-9_]{20,}|BEGIN [A-Z ]*PRIVATE KEY)", re.I,
)


class PublisherError(RuntimeError):
    pass


class GitHub:
    def __init__(self, token: str):
        if not token or any(ch.isspace() for ch in token):
            raise PublisherError("a non-empty, whitespace-free GitHub token is required")
        self.token = token

    def request(self, method: str, path: str, payload: Any = None, allow: tuple[int, ...] = ()):
        body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode()
        headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {self.token}",
            "User-Agent": "current-org-dotgithub-publisher/2",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        if body is not None:
            headers["Content-Type"] = "application/json"
        for attempt in range(6):
            request = urllib.request.Request(API + path, data=body, headers=headers, method=method)
            try:
                with urllib.request.urlopen(request, timeout=60) as response:
                    raw = response.read()
                    return response.status, json.loads(raw) if raw else None
            except urllib.error.HTTPError as error:
                raw = error.read(8192)
                try:
                    message = json.loads(raw).get("message", "unknown GitHub error")
                except Exception:
                    message = raw.decode(errors="replace")
                if error.code in allow:
                    return error.code, None
                retryable = error.code in (429, 500, 502, 503, 504) or (
                    error.code == 403 and any(
                        text in str(message).lower()
                        for text in ("rate limit", "secondary rate", "abuse")
                    )
                )
                if retryable and attempt < 5:
                    retry_after = error.headers.get("Retry-After", "")
                    delay = int(retry_after) if retry_after.isdigit() else min(2 ** (attempt + 1), 30)
                    time.sleep(max(delay, 1))
                    continue
                raise PublisherError(
                    f"GitHub {method} {path} returned {error.code}: {str(message)[:500]}"
                ) from None
            except urllib.error.URLError as error:
                if attempt < 5:
                    time.sleep(min(2 ** (attempt + 1), 20))
                    continue
                raise PublisherError(f"GitHub transport failed for {method} {path}: {error}") from None
        raise AssertionError("unreachable")

    def get(self, path: str, allow: tuple[int, ...] = ()):
        return self.request("GET", path, allow=allow)

    def post(self, path: str, payload: Any):
        return self.request("POST", path, payload)

    def patch(self, path: str, payload: Any):
        return self.request("PATCH", path, payload)


def quote(value: str, safe: str = "") -> str:
    return urllib.parse.quote(value, safe=safe)


def repo_path(org: str) -> str:
    return f"/repos/{quote(org)}/{REPOSITORY}"


def common_policy(org: str) -> str:
    return f"""## Durable engineering policy

- This repository defines public organization-wide defaults for `{org}`.
- Never commit credentials, private keys, access tokens, customer data, or private-repository inventories.
- Resolve Git conflicts semantically: inspect both sides, the merge base, nearby tests and contracts, and normally 3–10 relevant prior commits. Never blindly select all of `ours` or all of `theirs`.
- Prefer focused pull requests, explicit validation, non-destructive Git operations, and documented tradeoffs.
- Cross-repository integration uses versioned interfaces, APIs, SDKs, events, or explicitly owned replicated read models. Services do not reach into another service's database by default.
- `*-infra` repositories and `*-monorepo` application source remain separate. A `*-infra` repository must never appear as a Git submodule under `*-monorepo/apps`.
- Git submodules are reserved for explicitly coordinated editable source composition. Zed packages or immutable artifacts are preferred for package dependencies. Production deploys immutable artifacts or OCI digests, not source clones.
"""


def documents(org: str) -> dict[str, str]:
    policy = common_policy(org)
    files = {
        "README.md": f"""# `{org}` organization defaults

This public `.github` repository is the canonical home for shared community-health files, organization profile content, contribution guidance, security reporting, agent policy, and repository-boundary notes.

{policy}
## Inheritance note

GitHub can inherit supported community-health files from this repository when a target repository does not define its own version. Workflows, branch protections, rulesets, repository settings, and arbitrary documentation are not inherited automatically.
""",
        "profile/README.md": f"""# {org}

The `{org}` organization builds and maintains software through explicit contracts, focused repositories, repeatable validation, and secure delivery practices.

{policy}
Project-specific repositories remain authoritative for their own product behavior and implementation details.
""",
        "AGENTS.md": f"""# Organization agent policy

These instructions apply to automation and coding agents working in `{org}` repositories unless a repository defines stricter local policy.

{policy}
## Required workflow

1. Read repository-local instructions and relevant contracts before editing.
2. Inspect affected tests and 3–10 relevant commits when history is material.
3. Keep changes scoped and do not overwrite stronger local policy.
4. Run the most relevant formatter, linter, tests, and secret scan available.
5. Report exactly what changed, what was validated, and remaining uncertainty.
""",
        ".github/copilot-instructions.md": f"""# Copilot instructions for `{org}`

{policy}
Before proposing code, identify repository boundaries and existing interfaces. Preserve compatible intent during conflict resolution, avoid destructive recovery commands, and add or update tests with behavior changes.
""",
        "CONTRIBUTING.md": f"""# Contributing to `{org}` repositories

{policy}
## Pull requests

Keep pull requests focused and reviewable. Describe motivation, changed behavior, interface or migration impact, security considerations, validation performed, and rollback or compatibility plans where relevant.
""",
        "SECURITY.md": f"""# Security policy

Do not publish exploit details, secrets, customer data, or private infrastructure information in a public issue. Use GitHub private vulnerability reporting when enabled, or contact an organization owner through a private authenticated channel. Include the affected repository and revision, impact, reproduction conditions, and a minimal remediation suggestion.

{policy}
""",
        "SUPPORT.md": f"""# Support

Use the relevant repository issue tracker for reproducible bugs and feature requests. Include expected behavior, actual behavior, environment, minimal reproduction, redacted logs, and the exact revision tested. Security reports belong in the private process described by `SECURITY.md`.

{policy}
""",
        "CODE_OF_CONDUCT.md": f"""# Code of conduct

Participants in `{org}` repositories are expected to communicate respectfully, critique ideas rather than people, protect private information, and make technical disagreements evidence-driven and actionable. Harassment, threats, discrimination, doxxing, credential disclosure, and deliberate disruption are not acceptable.
""",
        "GOVERNANCE.md": f"""# Governance

Organization owners are accountable for repository creation, visibility, access, archival, and durable cross-repository policy. Repository maintainers own implementation quality and release decisions within published contracts.

{policy}
Material architecture decisions should be documented in the owning repository and reflected in interfaces, tests, deployment ownership, and observability expectations.
""",
        "REPOSITORY_BOUNDARIES.md": f"""# Repository boundaries

{policy}
## Mandatory infrastructure/application separation

Infrastructure repositories own deployment definitions, cloud resources, cluster policy, secrets integration, and environment automation. Application monorepos own application source and application-local packages.

A repository named `*-infra` must **not** be added as a Git submodule anywhere under a `*-monorepo/apps` directory. Connect the two through immutable deployment artifacts, declared interfaces, environment configuration, and release metadata instead.

## Composition

- Interfaces and schemas are versioned before dependent implementations consume them.
- Client SDKs are generated from or tested against canonical interfaces.
- End-to-end repositories test deployed boundaries without becoming a source-ownership shortcut.
- Monorepos coordinate application source; they do not absorb independently owned infrastructure history.
""",
        "PULL_REQUEST_TEMPLATE.md": """## Purpose

Describe the problem, intended behavior, and why this repository owns the change.

## Scope and boundaries

- [ ] The change is focused and does not silently cross repository ownership boundaries.
- [ ] No `*-infra` repository is introduced as a Git submodule under `*-monorepo/apps`.
- [ ] Public contracts, migrations, compatibility, and rollback needs are documented.

## Validation

List formatters, linters, tests, builds, security checks, and manual verification performed.

## Safety

- [ ] No credentials, customer data, or private-repository inventory is included.
- [ ] Conflicts were resolved semantically using both sides and relevant history.
- [ ] Destructive Git recovery and history rewrites were not used.
""",
        "agents/semantic-conflict-resolver.md": f"""# Semantic conflict resolver

When resolving conflicts in `{org}` repositories:

1. Identify the merge base and inspect both complete sides of every conflict.
2. Read surrounding implementation, tests, documentation, interfaces, and normally 3–10 relevant commits with `git log`, `git show`, and `git blame`.
3. Inspect related repositories when the conflict changes a shared contract or deployment boundary.
4. Synthesize compatible intent. Never resolve by blindly selecting all of `ours` or all of `theirs`.
5. Preserve the rule that `*-infra` repositories are separate from `*-monorepo` application source and cannot be Git submodules under `*-monorepo/apps`.
6. Remove every conflict marker, run affected validation, and document tradeoffs or intentional behavior changes.
7. Prefer a normal merge; do not rebase, force-push, reset, or discard work as a shortcut.
""",
        ".github/ISSUE_TEMPLATE/bug_report.yml": f"""name: Bug report
description: Report reproducible incorrect behavior in a {org} repository
title: "[Bug]: "
body:
  - type: textarea
    id: behavior
    attributes:
      label: Expected and actual behavior
      description: Describe what should happen and what happened instead.
    validations:
      required: true
  - type: textarea
    id: reproduction
    attributes:
      label: Minimal reproduction
      description: Include exact steps, revision, and environment. Remove credentials and private data.
    validations:
      required: true
""",
        ".github/ISSUE_TEMPLATE/feature_request.yml": f"""name: Feature request
description: Propose a focused improvement for a {org} repository
title: "[Feature]: "
body:
  - type: textarea
    id: problem
    attributes:
      label: Problem
      description: Describe the user or engineering problem, not only the implementation.
    validations:
      required: true
  - type: textarea
    id: proposal
    attributes:
      label: Proposed behavior
      description: Explain repository ownership, interfaces, compatibility, and validation.
    validations:
      required: true
  - type: checkboxes
    id: boundaries
    attributes:
      label: Repository boundaries
      options:
        - label: This does not place a `*-infra` repository under `*-monorepo/apps` as a Git submodule.
          required: true
""",
        ".github/ISSUE_TEMPLATE/config.yml": "blank_issues_enabled: true\ncontact_links: []\n",
    }
    return {path: text.rstrip() + "\n" for path, text in files.items()}


def validate_static() -> None:
    if len(ORGANIZATIONS) != EXPECTED_COUNT:
        raise PublisherError(f"expected {EXPECTED_COUNT} organizations, observed {len(ORGANIZATIONS)}")
    lowered = [org.lower() for org in ORGANIZATIONS]
    if len(set(lowered)) != EXPECTED_COUNT or "oresoftware" in lowered:
        raise PublisherError("organization allowlist is duplicated or includes the personal account")
    if any(not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?", org) for org in ORGANIZATIONS):
        raise PublisherError("organization allowlist contains an invalid login")
    sample = documents("sample-org")
    if len(sample) != 15 or len(set(sample)) != 15:
        raise PublisherError("expected exactly 15 unique governance files")
    for path, content in sample.items():
        if path.startswith("/") or ".." in Path(path).parts or not content.endswith("\n"):
            raise PublisherError(f"unsafe governance path or content: {path}")
        if SECRET_RE.search(content):
            raise PublisherError(f"credential-shaped content detected in {path}")
    required = ("*-infra", "*-monorepo/apps", "Git submodule")
    for path in ("README.md", "AGENTS.md", "REPOSITORY_BOUNDARIES.md", "PULL_REQUEST_TEMPLATE.md"):
        if any(fragment not in sample[path] for fragment in required):
            raise PublisherError(f"repository-boundary policy is incomplete in {path}")


def get_repo(api: GitHub, org: str) -> dict[str, Any] | None:
    status, payload = api.get(repo_path(org), allow=(404,))
    if status == 404:
        return None
    if not isinstance(payload, dict):
        raise PublisherError(f"invalid repository payload for {org}/{REPOSITORY}")
    return payload


def validate_repo(org: str, repo: dict[str, Any]) -> None:
    owner = repo.get("owner") or {}
    if repo.get("name") != REPOSITORY or str(owner.get("login", "")).lower() != org.lower():
        raise PublisherError(f"unexpected repository identity for {org}/{REPOSITORY}")
    if repo.get("private") is not False or repo.get("visibility") not in (None, "public"):
        raise PublisherError(f"{org}/{REPOSITORY} must be public")
    if repo.get("archived") is True:
        raise PublisherError(f"{org}/{REPOSITORY} is archived")


def preflight(api: GitHub) -> dict[str, dict[str, Any] | None]:
    _, user = api.get("/user")
    if not isinstance(user, dict) or user.get("login") != EXPECTED_LOGIN:
        raise PublisherError(f"publisher must be authorized as {EXPECTED_LOGIN}")
    repos: dict[str, dict[str, Any] | None] = {}
    for org in ORGANIZATIONS:
        _, membership = api.get(f"/user/memberships/orgs/{quote(org)}")
        if not isinstance(membership, dict) or membership.get("role") != "admin" or membership.get("state") != "active":
            raise PublisherError(f"active admin membership is required for {org}")
        repo = get_repo(api, org)
        if repo is not None:
            validate_repo(org, repo)
        repos[org] = repo
    return repos


def create_repo(api: GitHub, org: str) -> dict[str, Any]:
    _, repo = api.post(
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
    if not isinstance(repo, dict):
        raise PublisherError(f"invalid create-repository response for {org}")
    validate_repo(org, repo)
    return repo


def initialized_ref(api: GitHub, org: str, branch: str) -> tuple[str, str]:
    path = f"{repo_path(org)}/git/ref/heads/{quote(branch, safe='/')}"
    for attempt in range(10):
        status, ref = api.get(path, allow=(404, 409))
        if status not in (404, 409) and isinstance(ref, dict):
            sha = ((ref.get("object") or {}).get("sha"))
            if isinstance(sha, str) and sha:
                _, commit = api.get(f"{repo_path(org)}/git/commits/{sha}")
                tree_sha = ((commit or {}).get("tree") or {}).get("sha") if isinstance(commit, dict) else None
                if isinstance(tree_sha, str) and tree_sha:
                    return sha, tree_sha
        time.sleep(min(attempt + 1, 5))
    raise PublisherError(f"auto-initialized branch was not ready for {org}/{REPOSITORY}")


def publish_documents(api: GitHub, org: str, repo: dict[str, Any]) -> tuple[str, str]:
    branch = repo.get("default_branch") or "main"
    if not isinstance(branch, str) or not branch:
        branch = "main"
    parent_sha, base_tree = initialized_ref(api, org, branch)
    files = documents(org)
    tree_entries = [
        {"path": path, "mode": "100644", "type": "blob", "content": content}
        for path, content in sorted(files.items())
    ]
    _, tree = api.post(f"{repo_path(org)}/git/trees", {"base_tree": base_tree, "tree": tree_entries})
    tree_sha = tree.get("sha") if isinstance(tree, dict) else None
    if not isinstance(tree_sha, str) or not tree_sha:
        raise PublisherError(f"GitHub did not return a tree SHA for {org}/{REPOSITORY}")
    _, commit = api.post(
        f"{repo_path(org)}/git/commits",
        {
            "message": f"docs: establish organization defaults for {org}",
            "tree": tree_sha,
            "parents": [parent_sha],
        },
    )
    commit_sha = commit.get("sha") if isinstance(commit, dict) else None
    if not isinstance(commit_sha, str) or not commit_sha:
        raise PublisherError(f"GitHub did not return a commit SHA for {org}/{REPOSITORY}")
    api.patch(
        f"{repo_path(org)}/git/refs/heads/{quote(branch, safe='/')}",
        {"sha": commit_sha, "force": False},
    )
    return branch, tree_sha


def verify_created(api: GitHub, org: str, branch: str, tree_sha: str) -> None:
    repo = get_repo(api, org)
    if repo is None:
        raise PublisherError(f"verification cannot find {org}/{REPOSITORY}")
    validate_repo(org, repo)
    _, tree = api.get(f"{repo_path(org)}/git/trees/{tree_sha}?recursive=1")
    paths = {
        item.get("path")
        for item in (tree.get("tree") if isinstance(tree, dict) else [])
        if isinstance(item, dict) and item.get("type") == "blob"
    }
    expected = set(documents(org))
    if not expected.issubset(paths):
        raise PublisherError(f"verification is missing governance files for {org}/{REPOSITORY}")
    _, payload = api.get(
        f"{repo_path(org)}/contents/REPOSITORY_BOUNDARIES.md?ref={quote(branch)}"
    )
    if not isinstance(payload, dict) or payload.get("encoding") != "base64":
        raise PublisherError(f"cannot verify repository boundaries for {org}/{REPOSITORY}")
    try:
        text = base64.b64decode(payload.get("content", ""), validate=False).decode("utf-8")
    except Exception as error:
        raise PublisherError(f"cannot decode repository boundaries for {org}: {error}") from None
    for fragment in ("*-infra", "*-monorepo/apps", "must **not** be added as a Git submodule"):
        if fragment not in text:
            raise PublisherError(f"repository-boundary verification failed for {org}/{REPOSITORY}")
    if SECRET_RE.search(text):
        raise PublisherError(f"credential-shaped content detected in {org}/{REPOSITORY}")


def build_report(mode: str, rows: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    if rows is None:
        rows = [
            {"organization": org, "repository": f"{org}/{REPOSITORY}", "planned": True}
            for org in ORGANIZATIONS
        ]
    return {
        "schema_version": 1,
        "mode": mode,
        "fleet_id": FLEET_ID,
        "policy_version": POLICY_VERSION,
        "expected_count": EXPECTED_COUNT,
        "organizations": rows,
    }


def markdown_report(report: dict[str, Any]) -> str:
    rows = report["organizations"]
    lines = [
        "# Current organization `.github` governance publication", "",
        f"- Mode: `{report['mode']}`", f"- Fleet: `{report['fleet_id']}`",
        f"- Organizations: `{len(rows)}`", f"- Policy version: `{report['policy_version']}`", "",
        "| Organization | Repository | Result |", "|---|---|---|",
    ]
    for row in rows:
        result = "planned" if report["mode"] == "dry-run" else (
            "created, documented, verified" if row["created_repository"] else "preserved existing, verified"
        )
        lines.append(f"| `{row['organization']}` | `{row['repository']}` | {result} |")
    lines.extend([
        "", "Existing `.github` repositories are validated and left untouched; only missing repositories are created.",
        "New repositories receive organization profile, governance, contribution, security, support, issue, pull-request, agent, and boundary defaults.",
        "The permanent boundary forbids `*-infra` repositories as Git submodules under `*-monorepo/apps`.",
    ])
    return "\n".join(lines).rstrip() + "\n"


def write(path: str | None, content: str) -> None:
    if path:
        target = Path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--fleet-id", default="")
    parser.add_argument("--expected-count", type=int, default=EXPECTED_COUNT)
    parser.add_argument("--json-report")
    parser.add_argument("--markdown-report")
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)
    validate_static()
    if args.expected_count != EXPECTED_COUNT:
        raise PublisherError(f"expected-count acknowledgement must equal {EXPECTED_COUNT}")
    if not args.execute:
        report = build_report("dry-run")
    else:
        if args.fleet_id != FLEET_ID:
            raise PublisherError(f"execution requires exact --fleet-id {FLEET_ID}")
        token = os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN") or os.environ.get("GH_TOKEN")
        if not token:
            raise PublisherError("GITHUB_REPOSITORY_ADMIN_TOKEN or GH_TOKEN is required for execution")
        api = GitHub(token)
        repos = preflight(api)
        rows: list[dict[str, Any]] = []
        for index, org in enumerate(ORGANIZATIONS, 1):
            repo = repos[org]
            created = repo is None
            print(f"[{index:02d}/{EXPECTED_COUNT}] {'creating' if created else 'preserving'} {org}/{REPOSITORY}", flush=True)
            if created:
                repo = create_repo(api, org)
                branch, tree_sha = publish_documents(api, org, repo)
                verify_created(api, org, branch, tree_sha)
            else:
                validate_repo(org, repo)
            rows.append({
                "organization": org,
                "repository": f"{org}/{REPOSITORY}",
                "created_repository": created,
                "preserved_repository": not created,
                "verified": True,
            })
        if len(rows) != EXPECTED_COUNT or any(row["verified"] is not True for row in rows):
            raise PublisherError("execute report cannot certify the exact fleet")
        report = build_report("execute", rows)
    json_text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    markdown = markdown_report(report)
    write(args.json_report, json_text)
    write(args.markdown_report, markdown)
    if not args.json_report and not args.markdown_report:
        sys.stdout.write(markdown)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublisherError as error:
        print(f"publisher failed: {error}", file=sys.stderr)
        raise SystemExit(1)
