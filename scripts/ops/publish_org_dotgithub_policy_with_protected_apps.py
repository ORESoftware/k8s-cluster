#!/usr/bin/env python3
"""Publish organization-wide branching and GitOps policy through protected GitHub Apps.

The publisher is intentionally bounded and fail-closed:

* it targets an explicit production-organization allowlist and rejects test orgs;
* it discovers GitHub App credentials only through the existing protected-source selector;
* it never accepts or uses a PAT, OAuth token, or inherited GitHub token;
* it requires an all-repositories installation token with repository administration,
  contents, pull-request, and metadata permissions for every target organization;
* it creates only the public ``.github`` repository when absent;
* it preserves text outside explicit managed blocks and preserves unrelated JSON keys;
* it verifies every remote write before reporting success.
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import urllib.parse
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import select_hypesiege_github_app_from_protected_sources as protected_selector

POLICY_VERSION = "2026-08-05.1"
MANAGED_START = "<!-- BEGIN ORESOFTWARE MANAGED BRANCHING AND GITOPS POLICY -->"
MANAGED_END = "<!-- END ORESOFTWARE MANAGED BRANCHING AND GITOPS POLICY -->"
PRODUCTION_ORGANIZATIONS = (
    "3FA-app",
    "agent-pontifex",
    "akrion-sim",
    "anticaptrad",
    "apostille-me",
    "athlet-o",
    "benefactor-cc",
    "canonical-cloud",
    "channelsiege",
    "claritas-viz",
    "cliptown",
    "daedalus-fab",
    "dancing-dragons",
    "declarative-migrations",
    "discrete-event-systems",
    "drone-mngr",
    "embedded-alerts",
    "evento-globolo",
    "fanwaave",
    "fifa-math",
    "fiducia-cloud",
    "file-tunnel",
    "gha-indie-worker",
    "hacker-house-medellin",
    "hypeblitz",
    "hypesiege",
    "led-dynamo",
    "memebank",
    "messaging-intel",
    "meta-agents-demo",
    "networking-components",
    "OmniBlitz",
    "opto-sync",
    "quaestor-ledger",
    "rust-ssr-demos",
    "sagitta-stack",
    "scintilla-run",
    "shared-auth",
    "sonus-auris",
    "StreemPilot",
    "streamkore",
    "unreal-unity-poc",
    "usa-acc",
    "voxletra",
    "zed-pkg",
)
FORBIDDEN_TOKEN_ENV = (
    "GH_PAT",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GITHUB_REPOSITORY_ADMIN_TOKEN",
    "GIT_ASKPASS",
)
REQUIRED_TOKEN_PERMISSIONS = {
    "administration": "write",
    "contents": "write",
    "pull_requests": "write",
    "metadata": "read",
}

POLICY_BLOCK = f"""{MANAGED_START}
## Mandatory branching, merge-confidence, and GitOps policy

Policy version: `{POLICY_VERSION}`.

### Branch names and GitFlow

- **`dev` is the integration branch.** Feature, fix, refactor, and routine dependency branches must normally branch from `dev` and open pull requests back into `dev`.
- The repository's existing **`main` or `master` branch is the production/release branch**. Do not rename a repository's production branch merely to satisfy this policy; document which of the two names that repository uses.
- Strive for a GitFlow-style model: `feature/*`, `fix/*`, and similar short-lived branches flow into `dev`; release promotion flows from `dev` into `main`/`master`; urgent `hotfix/*` work may branch from production but must be merged back into `dev` immediately after production is repaired.
- Avoid direct feature-to-production pull requests. Preserve branch protections, required reviews, security gates, environment approvals, and semantic conflict resolution.

### AI-assisted merge thresholds

The thresholds below are strict greater-than comparisons, not greater-than-or-equal comparisons.

1. **Feature or fix pull request -> `dev`:** merge when all configured tests and required checks pass and the reviewing AI records evidence-based confidence **greater than 99.1%** that the change satisfies its acceptance criteria without introducing material regressions.
2. **`dev` -> `main`/`master`:** merge when all integration, release, deployment, migration, security, and required checks pass and the reviewing AI records evidence-based confidence **greater than 99.7%** that the exact promoted revision is production-ready.
3. Record the numerical confidence score, supporting evidence, exact checks run, unresolved uncertainty, migration/deployment impact, and rollback or roll-forward plan in the pull request.
4. A confidence score never overrides a failed or missing required check, an unresolved review, a branch-protection rule, a security/compliance gate, an environment approval, or known contradictory evidence. Do not invent precision: confidence must be justified by review depth, test coverage, affected contracts, and deployment evidence.

### `*-infra`, GitHub Actions, branch-based promotion, and GitOps

- Each organization must designate a canonical infrastructure repository whose name ends in **`-infra`**. It owns deployable desired state: environment overlays, Kubernetes/Helm/Kustomize manifests, Terraform/Pulumi or other infrastructure code, GitOps controller configuration, environment policy, and repository-to-environment mappings.
- Individual application, service, library, CLI, web, API, worker, and client repositories own their source code, tests, build definitions, artifact metadata, and repository-specific GitHub Actions workflows. They must not become the source of truth for live cluster or cloud state.
- Pull requests and short-lived branches run CI. They may create bounded ephemeral preview environments, but they do not mutate persistent shared environments directly.
- A merge into **`dev`** builds and verifies an immutable integration artifact, normally identified by commit SHA and digest. GitHub Actions then opens or updates a reviewed change in the canonical `*-infra` repository for the integration/development/staging environment. The GitOps controller reconciles that desired state.
- A merge from **`dev` into `main`/`master`** promotes the already-tested immutable artifact whenever possible rather than rebuilding different bytes. GitHub Actions opens or updates the production desired-state change in `*-infra`; required approvals complete there; the GitOps controller performs reconciliation.
- Infrastructure-repository changes follow the same GitFlow intent: `dev` represents integration desired state and `main`/`master` represents production desired state, unless the infra repository explicitly documents an equally reviewable environment-directory model on one protected branch.
- GitHub Actions is the validation, build, attestation, and promotion orchestration layer. The GitOps controller is the normal deployment authority. Application workflows must not run routine imperative production mutations such as direct `kubectl apply`, ad hoc cloud-console changes, or unreviewed Terraform applies.
- Use least-privilege OIDC or short-lived credentials, protected GitHub environments, immutable action pins, explicit timeouts, concurrency controls, artifact provenance, and deployment status reporting. Never expose repository or cloud credentials to pull requests from untrusted contexts.
- Rollback normally means a reviewed revert or forward fix to desired state in `*-infra`, followed by GitOps reconciliation. Break-glass deployment paths must be exceptional, auditable, time-bounded, and reconciled back into Git immediately.

Repository-local policy may strengthen these requirements but must not weaken the `dev` integration-branch declaration, the strict confidence thresholds, required checks, GitFlow intent, or GitOps separation of responsibilities.
{MANAGED_END}"""

AGENT_BLOCK = f"""{MANAGED_START}
## Required `dev`/GitFlow/GitOps policy

Read and follow [`BRANCHING_AND_DEPLOYMENT.md`](BRANCHING_AND_DEPLOYMENT.md) before reviewing, merging, releasing, or deploying changes.

- `dev` is the integration branch; strive for a GitFlow-style branch and promotion model.
- With all configured tests and required checks passing, merge feature/fix PRs into `dev` only when evidence-based AI confidence is strictly greater than **99.1%**.
- Merge `dev` into `main`/`master` only when integration, release, deployment, migration, security, and required checks pass and evidence-based AI confidence is strictly greater than **99.7%**.
- Record the score, evidence, checks, remaining uncertainty, deployment impact, immutable artifact identity, `*-infra` desired-state change, and rollback or roll-forward plan.
- Use the organization's canonical `*-infra` repository, GitHub Actions, immutable artifacts, and GitOps reconciliation for branch-based deployment promotion.
- Required reviews, branch protection, security/compliance gates, and environment approvals always take precedence over any confidence score.
{MANAGED_END}"""

COPILOT_BLOCK = f"""{MANAGED_START}
## Required `dev`/GitFlow/GitOps policy

Read and follow [`BRANCHING_AND_DEPLOYMENT.md`](../BRANCHING_AND_DEPLOYMENT.md) before reviewing, merging, releasing, or deploying changes.

- `dev` is the integration branch; strive for a GitFlow-style branch and promotion model.
- With all configured tests and required checks passing, merge feature/fix PRs into `dev` only when evidence-based AI confidence is strictly greater than **99.1%**.
- Merge `dev` into `main`/`master` only when integration, release, deployment, migration, security, and required checks pass and evidence-based AI confidence is strictly greater than **99.7%**.
- Record the score, evidence, checks, remaining uncertainty, deployment impact, immutable artifact identity, `*-infra` desired-state change, and rollback or roll-forward plan.
- Use the organization's canonical `*-infra` repository, GitHub Actions, immutable artifacts, and GitOps reconciliation for branch-based deployment promotion.
- Required reviews, branch protection, security/compliance gates, and environment approvals always take precedence over any confidence score.
{MANAGED_END}"""

PR_TEMPLATE_BLOCK = f"""{MANAGED_START}
## Branching and promotion evidence

**Change path** (select one):
- [ ] Feature/fix/refactor/dependency branch -> `dev` (the integration branch)
- [ ] `dev` -> `main`/`master` production promotion
- [ ] Emergency `hotfix/*` -> production, with an immediate semantic merge back into `dev`

**AI review confidence:** `____.__%`

**Threshold gate:**
- [ ] For a PR into `dev`, confidence is strictly greater than **99.1%**
- [ ] For a `dev` promotion into `main`/`master`, confidence is strictly greater than **99.7%**

**Evidence:**
- Acceptance criteria and linked work item:
- Tests and required checks that passed:
- Review scope, affected contracts, and repositories inspected:
- Remaining uncertainty or known limitations:
- Migration and deployment impact:
- Exact immutable artifact SHA/digest being promoted:
- Canonical `*-infra` desired-state PR/commit and target environment:
- Rollback or roll-forward plan:

- [ ] No branch protection, required review, security/compliance gate, or environment approval is being bypassed.
- [ ] Deployment is represented in the canonical `*-infra` repository and reconciled through GitOps, or this PR documents an approved break-glass exception and immediate follow-up reconciliation.
{MANAGED_END}"""


@dataclass(frozen=True)
class AppCredential:
    app_id: str
    private_key: str
    private_key_fingerprint: str
    app_slug: str


@dataclass(frozen=True)
class OrganizationInstallation:
    organization: str
    app: AppCredential
    installation_id: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--region", default=os.environ.get("AWS_REGION", "us-east-1"))
    parser.add_argument("--report", type=Path)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def reject_inherited_tokens() -> None:
    present = [name for name in FORBIDDEN_TOKEN_ENV if os.environ.get(name)]
    if present:
        raise RuntimeError(f"PAT/OAuth environment is forbidden: {', '.join(present)}")


def validate_targets() -> None:
    if len(PRODUCTION_ORGANIZATIONS) != 45:
        raise RuntimeError("production organization allowlist must contain exactly 45 entries")
    if len(set(PRODUCTION_ORGANIZATIONS)) != len(PRODUCTION_ORGANIZATIONS):
        raise RuntimeError("duplicate production organization")
    forbidden = [
        organization
        for organization in PRODUCTION_ORGANIZATIONS
        if organization.casefold().endswith("-test") or organization.casefold() == "r2g-test"
    ]
    if forbidden:
        raise RuntimeError(f"test organizations are forbidden: {forbidden}")


def replace_managed_block(existing: str, block: str, preamble: str = "") -> str:
    existing = existing.replace("\r\n", "\n")
    start = existing.find(MANAGED_START)
    end = existing.find(MANAGED_END)
    if (start == -1) != (end == -1):
        raise ValueError("managed block has only one marker")
    if start != -1:
        if end < start:
            raise ValueError("managed block end marker precedes start marker")
        end += len(MANAGED_END)
        prefix = existing[:start].rstrip()
        suffix = existing[end:].strip()
        parts = [part for part in (prefix, block.rstrip(), suffix) if part]
        return "\n\n".join(parts).rstrip() + "\n"
    base = existing.strip() or preamble.strip()
    return ((base + "\n\n") if base else "") + block.rstrip() + "\n"


def merge_machine_policy(existing: str | None) -> str:
    if existing and existing.strip():
        value = json.loads(existing)
        if not isinstance(value, dict):
            raise ValueError("organization-policy.json root must be an object")
    else:
        value = {}
    schema = value.get("schema_version", 0)
    value["schema_version"] = max(schema if isinstance(schema, int) else 0, 3)
    value["branching_and_delivery"] = {
        "policy_version": POLICY_VERSION,
        "strategy": "gitflow",
        "integration_branch": "dev",
        "production_branch": "main_or_master_as_configured",
        "feature_branch_base": "dev",
        "feature_pull_request_target": "dev",
        "hotfix_backmerge_target": "dev",
        "feature_to_dev_gate": {
            "tests_and_required_checks": "pass",
            "ai_confidence_operator": ">",
            "ai_confidence_percent": 99.1,
            "confidence_must_be_recorded_with_evidence": True,
        },
        "dev_to_production_gate": {
            "integration_release_deployment_migration_security_and_required_checks": "pass",
            "ai_confidence_operator": ">",
            "ai_confidence_percent": 99.7,
            "confidence_must_be_recorded_with_evidence": True,
        },
        "required_controls_take_precedence": True,
        "infrastructure_repository": {
            "required_suffix": "-infra",
            "owns": [
                "environment desired state",
                "GitOps controller configuration",
                "deployment manifests and overlays",
                "infrastructure as code",
                "environment and promotion policy",
            ],
        },
        "delivery": {
            "pull_requests": "ci_and_optional_ephemeral_preview",
            "dev": "immutable_integration_artifact_then_infra_desired_state_change",
            "main_or_master": "promote_tested_artifact_then_production_infra_desired_state_change",
            "deployment_authority": "gitops_controller",
            "github_actions_role": "validate_build_attest_and_orchestrate_promotion",
            "routine_imperative_production_deployment": "forbidden",
        },
    }
    return json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + "\n"


def api_request(
    method: str,
    path: str,
    bearer: str,
    body: dict[str, Any] | None = None,
) -> tuple[int | None, Any | None]:
    return protected_selector.request_json(method, path, bearer, body)


def discover_app_credentials(region: str) -> tuple[list[AppCredential], dict[str, int]]:
    app_ids: dict[str, protected_selector.AppIdCandidate] = {}
    keys: dict[str, protected_selector.KeyCandidate] = {}
    kubernetes_stats, external_secret_names = protected_selector.discover_kubernetes_material(
        app_ids, keys
    )
    aws_stats = protected_selector.discover_aws_secret_material(
        region, external_secret_names, app_ids, keys
    )
    ssm_stats = protected_selector.discover_ssm_material(region, app_ids, keys)
    stats = {**kubernetes_stats, **aws_stats, **ssm_stats}
    stats["candidate_app_ids"] = len(app_ids)
    stats["candidate_private_keys"] = len(keys)
    if not app_ids or not keys:
        raise RuntimeError(
            "no protected GitHub App material found "
            f"app_ids={len(app_ids)} private_keys={len(keys)}"
        )

    valid: dict[str, AppCredential] = {}
    with tempfile.TemporaryDirectory(prefix="org-dotgithub-app-validation-") as temporary:
        directory = Path(temporary)
        for app_id in sorted(app_ids.values(), key=lambda item: int(item.value)):
            matches: list[AppCredential] = []
            for key in sorted(keys.values(), key=lambda item: item.fingerprint):
                try:
                    app_jwt = protected_selector.mint_app_jwt(
                        app_id.value, key.value, directory
                    )
                except ValueError:
                    continue
                status, app = api_request("GET", "/app", app_jwt)
                if status != 200 or not isinstance(app, dict):
                    continue
                if app.get("id") != int(app_id.value):
                    continue
                slug = app.get("slug")
                if not isinstance(slug, str) or not slug:
                    continue
                matches.append(
                    AppCredential(
                        app_id=app_id.value,
                        private_key=key.value,
                        private_key_fingerprint=key.fingerprint,
                        app_slug=slug,
                    )
                )
            if len(matches) > 1:
                raise RuntimeError(
                    f"multiple private keys validated for protected App ID {app_id.value}"
                )
            if matches:
                valid[app_id.value] = matches[0]
    if not valid:
        raise RuntimeError("no protected GitHub App credential pair validated")
    stats["validated_apps"] = len(valid)
    return list(valid.values()), stats


def app_jwt(app: AppCredential, directory: Path) -> str:
    return protected_selector.mint_app_jwt(app.app_id, app.private_key, directory)


def list_installations(app: AppCredential, directory: Path) -> list[dict[str, Any]]:
    installations: list[dict[str, Any]] = []
    for page in range(1, 11):
        jwt = app_jwt(app, directory)
        status, document = api_request(
            "GET", f"/app/installations?per_page=100&page={page}", jwt
        )
        if status != 200 or not isinstance(document, list):
            raise RuntimeError(
                f"could not list installations for protected App {app.app_slug}: HTTP {status}"
            )
        items = [item for item in document if isinstance(item, dict)]
        installations.extend(items)
        if len(items) < 100:
            break
    return installations


def mint_installation_token(
    installation: OrganizationInstallation, directory: Path
) -> str:
    jwt = app_jwt(installation.app, directory)
    status, document = api_request(
        "POST",
        f"/app/installations/{installation.installation_id}/access_tokens",
        jwt,
        {},
    )
    if status != 201 or not isinstance(document, dict):
        raise RuntimeError(
            f"could not mint token for {installation.organization}: HTTP {status}"
        )
    token = document.get("token")
    permissions = document.get("permissions")
    if not isinstance(token, str) or not token or not isinstance(permissions, dict):
        raise RuntimeError(f"invalid token document for {installation.organization}")
    observed = {
        name: str(permissions.get(name, "none")) for name in REQUIRED_TOKEN_PERMISSIONS
    }
    if observed != REQUIRED_TOKEN_PERMISSIONS:
        api_request("DELETE", "/installation/token", token)
        raise RuntimeError(
            f"insufficient token permissions for {installation.organization}: {observed}"
        )
    return token


def revoke_installation_token(token: str) -> None:
    api_request("DELETE", "/installation/token", token)


def map_organization_installations(
    apps: list[AppCredential], directory: Path
) -> dict[str, OrganizationInstallation]:
    targets = {organization.casefold(): organization for organization in PRODUCTION_ORGANIZATIONS}
    matches: dict[str, list[OrganizationInstallation]] = {key: [] for key in targets}
    for app in apps:
        for item in list_installations(app, directory):
            account = item.get("account")
            login = account.get("login") if isinstance(account, dict) else None
            account_type = account.get("type") if isinstance(account, dict) else None
            installation_id = item.get("id")
            if (
                not isinstance(login, str)
                or login.casefold() not in targets
                or account_type != "Organization"
                or not isinstance(installation_id, int)
                or installation_id <= 0
                or item.get("repository_selection") != "all"
            ):
                continue
            organization = targets[login.casefold()]
            candidate = OrganizationInstallation(
                organization=organization,
                app=app,
                installation_id=installation_id,
            )
            token = mint_installation_token(candidate, directory)
            revoke_installation_token(token)
            matches[login.casefold()].append(candidate)

    resolved: dict[str, OrganizationInstallation] = {}
    failures: list[str] = []
    for folded, organization in targets.items():
        candidates = matches[folded]
        unique = {
            (candidate.app.app_id, candidate.installation_id): candidate
            for candidate in candidates
        }
        if len(unique) != 1:
            failures.append(
                f"{organization}: expected exactly one protected all-repositories admin App; "
                f"found {len(unique)}"
            )
            continue
        resolved[organization] = next(iter(unique.values()))
    if failures:
        raise RuntimeError("; ".join(failures))
    return resolved


def quote_repository_path(path: str) -> str:
    return "/".join(urllib.parse.quote(part, safe="") for part in path.split("/"))


def repo_api_path(organization: str) -> str:
    return f"/repos/{urllib.parse.quote(organization, safe='')}/.github"


def get_repository(organization: str, token: str) -> tuple[int | None, dict[str, Any] | None]:
    status, value = api_request("GET", repo_api_path(organization), token)
    return status, value if isinstance(value, dict) else None


def ensure_repository(organization: str, token: str) -> tuple[bool, dict[str, Any]]:
    status, repository = get_repository(organization, token)
    created = False
    if status == 404:
        status, value = api_request(
            "POST",
            f"/orgs/{urllib.parse.quote(organization, safe='')}/repos",
            token,
            {
                "name": ".github",
                "description": "Organization-wide GitHub, branching, agent, and delivery policy",
                "private": False,
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "auto_init": True,
                "delete_branch_on_merge": True,
            },
        )
        if status != 201 or not isinstance(value, dict):
            raise RuntimeError(
                f"failed to create {organization}/.github: HTTP {status}"
            )
        repository = value
        created = True
        time.sleep(1)
    elif status != 200 or repository is None:
        raise RuntimeError(f"failed to inspect {organization}/.github: HTTP {status}")
    visibility = repository.get("visibility")
    if visibility != "public" or repository.get("archived") is True:
        raise RuntimeError(
            f"{organization}/.github must be public and active; "
            f"visibility={visibility!r} archived={repository.get('archived')!r}"
        )
    return created, repository


def get_text_file(
    organization: str, path: str, token: str
) -> tuple[str | None, str | None]:
    encoded = quote_repository_path(path)
    status, value = api_request(
        "GET", f"{repo_api_path(organization)}/contents/{encoded}", token
    )
    if status == 404:
        return None, None
    if status != 200 or not isinstance(value, dict) or value.get("type") != "file":
        raise RuntimeError(
            f"unexpected contents response for {organization}/.github:{path}: HTTP {status}"
        )
    content = value.get("content")
    sha = value.get("sha")
    if not isinstance(content, str) or not isinstance(sha, str):
        raise RuntimeError(f"missing content or sha for {organization}/.github:{path}")
    try:
        decoded = base64.b64decode(content).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as error:
        raise RuntimeError(
            f"invalid UTF-8 content for {organization}/.github:{path}"
        ) from error
    return decoded, sha


def put_text_file(
    organization: str,
    path: str,
    desired: str,
    token: str,
) -> str:
    for attempt in range(1, 4):
        existing, sha = get_text_file(organization, path, token)
        if existing == desired:
            return "unchanged"
        body: dict[str, Any] = {
            "message": "docs: define dev GitFlow and GitOps promotion policy",
            "content": base64.b64encode(desired.encode("utf-8")).decode("ascii"),
        }
        if sha:
            body["sha"] = sha
        encoded = quote_repository_path(path)
        status, _ = api_request(
            "PUT", f"{repo_api_path(organization)}/contents/{encoded}", token, body
        )
        if status not in (200, 201):
            if attempt < 3 and status in (404, 409, 422):
                time.sleep(attempt)
                continue
            raise RuntimeError(
                f"failed to write {organization}/.github:{path}: HTTP {status}"
            )
        actual, _ = get_text_file(organization, path, token)
        if actual == desired:
            return "updated" if sha else "created"
        if attempt < 3:
            time.sleep(attempt)
            continue
        raise RuntimeError(f"verification mismatch for {organization}/.github:{path}")
    raise AssertionError("unreachable")


def desired_files(organization: str, token: str) -> dict[str, str]:
    current: dict[str, str | None] = {}
    for path in (
        "BRANCHING_AND_DEPLOYMENT.md",
        "AGENTS.md",
        "agents.md",
        ".github/copilot-instructions.md",
        ".github/PULL_REQUEST_TEMPLATE.md",
        "organization-policy.json",
    ):
        current[path], _ = get_text_file(organization, path, token)
    return {
        "BRANCHING_AND_DEPLOYMENT.md": replace_managed_block(
            current["BRANCHING_AND_DEPLOYMENT.md"] or "",
            POLICY_BLOCK,
            "# Branching, merge-confidence, and GitOps deployment policy\n\n"
            "This document defines the default branching and delivery contract for "
            "repositories in this owner account.",
        ),
        "AGENTS.md": replace_managed_block(
            current["AGENTS.md"] or "",
            AGENT_BLOCK,
            "# Organization-wide agent instructions",
        ),
        "agents.md": replace_managed_block(
            current["agents.md"] or "",
            AGENT_BLOCK,
            "# Organization-wide agent instructions",
        ),
        ".github/copilot-instructions.md": replace_managed_block(
            current[".github/copilot-instructions.md"] or "",
            COPILOT_BLOCK,
            "# Organization-wide coding-agent instructions\n\n"
            "Read repository-local instructions first. The managed policy below is "
            "the organization minimum; local policy may strengthen it but may not "
            "weaken it.",
        ),
        ".github/PULL_REQUEST_TEMPLATE.md": replace_managed_block(
            current[".github/PULL_REQUEST_TEMPLATE.md"] or "",
            PR_TEMPLATE_BLOCK,
        ),
        "organization-policy.json": merge_machine_policy(
            current["organization-policy.json"]
        ),
    }


def publish_one(
    installation: OrganizationInstallation,
    directory: Path,
) -> dict[str, Any]:
    repository_token = mint_installation_token(installation, directory)
    try:
        repository_created, _ = ensure_repository(
            installation.organization, repository_token
        )
    finally:
        revoke_installation_token(repository_token)

    # Mint a fresh all-repositories token after repository creation so a newly
    # created .github repository is certainly included in token scope.
    write_token = mint_installation_token(installation, directory)
    try:
        files = desired_files(installation.organization, write_token)
        actions: dict[str, str] = {}
        for path, desired in files.items():
            actions[path] = put_text_file(
                installation.organization, path, desired, write_token
            )
        for path, desired in files.items():
            actual, _ = get_text_file(installation.organization, path, write_token)
            if actual != desired:
                raise RuntimeError(
                    f"final verification mismatch for "
                    f"{installation.organization}/.github:{path}"
                )
    finally:
        revoke_installation_token(write_token)

    counts = {
        action: sum(1 for observed in actions.values() if observed == action)
        for action in ("created", "updated", "unchanged")
    }
    return {
        "organization": installation.organization,
        "repository_created": repository_created,
        "app_slug": installation.app.app_slug,
        "installation_id": installation.installation_id,
        "files_created": counts["created"],
        "files_updated": counts["updated"],
        "files_unchanged": counts["unchanged"],
        "verified": True,
    }


def write_report(path: Path | None, report: dict[str, Any]) -> None:
    serialized = json.dumps(report, separators=(",", ":"), sort_keys=True) + "\n"
    if path is not None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(serialized, encoding="utf-8")
    print(serialized, end="")


def self_test() -> None:
    validate_targets()
    original = "# Existing\n\nHuman-owned text.\n"
    once = replace_managed_block(original, POLICY_BLOCK)
    twice = replace_managed_block(once, POLICY_BLOCK)
    assert once == twice
    assert once.startswith(original.strip())
    assert once.count(MANAGED_START) == 1
    assert once.count(MANAGED_END) == 1
    machine = json.loads(merge_machine_policy('{"schema_version":2,"keep":true}\n'))
    assert machine["keep"] is True
    assert machine["schema_version"] == 3
    delivery = machine["branching_and_delivery"]
    assert delivery["integration_branch"] == "dev"
    assert delivery["feature_to_dev_gate"]["ai_confidence_percent"] == 99.1
    assert delivery["dev_to_production_gate"]["ai_confidence_percent"] == 99.7
    assert "../BRANCHING_AND_DEPLOYMENT.md" in COPILOT_BLOCK
    assert "*-infra" in POLICY_BLOCK
    print("protected-App organization .github publisher self-test: ok")


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0

    reject_inherited_tokens()
    validate_targets()
    report: dict[str, Any] = {
        "schema_version": 1,
        "policy_version": POLICY_VERSION,
        "target_count": len(PRODUCTION_ORGANIZATIONS),
        "pat_used": False,
        "organizations": [],
        "failures": [],
    }
    try:
        apps, discovery = discover_app_credentials(args.region)
        report["discovery"] = discovery
        report["validated_app_count"] = len(apps)
        with tempfile.TemporaryDirectory(prefix="org-dotgithub-publication-") as temporary:
            directory = Path(temporary)
            installations = map_organization_installations(apps, directory)
            for organization in PRODUCTION_ORGANIZATIONS:
                print(f"TARGET {organization}/.github", file=sys.stderr)
                try:
                    result = publish_one(installations[organization], directory)
                    report["organizations"].append(result)
                    print(
                        f"VERIFIED {organization}/.github "
                        f"created_repo={result['repository_created']} "
                        f"files_created={result['files_created']} "
                        f"files_updated={result['files_updated']} "
                        f"files_unchanged={result['files_unchanged']}",
                        file=sys.stderr,
                    )
                except Exception as error:  # preserve a complete fleet audit
                    report["failures"].append(
                        {"organization": organization, "error": str(error)[:1000]}
                    )
                    print(f"FAILED {organization}: {error}", file=sys.stderr)
    except Exception as error:
        report["failures"].append({"organization": "__fleet__", "error": str(error)[:4000]})
        print(f"FAILED fleet: {error}", file=sys.stderr)

    report["verified_count"] = sum(
        1 for item in report["organizations"] if item.get("verified") is True
    )
    report["repository_created_count"] = sum(
        1 for item in report["organizations"] if item.get("repository_created") is True
    )
    write_report(args.report, report)
    if report["failures"] or report["verified_count"] != len(PRODUCTION_ORGANIZATIONS):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
