#!/usr/bin/env python3
"""Harden the bounded organization `.github` fleet publisher.

This wrapper deliberately reuses the reviewed repository-creation, preflight,
managed-block, retry, and verification primitives from
`bootstrap_org_dotgithub_repositories.py`. It replaces only the managed policy
registry, rendered organization defaults, final verification, and report so the
fleet rollout has:

* an exact GitHub organization ↔ Linear project registry;
* the required semantic conflict-resolution declaration;
* a deny-by-default destructive-operation blacklist for agents;
* sane public community-health defaults;
* explicit documentation of GitHub's inheritance boundary.

The publisher remains dry-run by default. Mutation still requires the base
publisher's `--execute` argument and the trusted workflow's fixed allowlist.
"""

from __future__ import annotations

from pathlib import Path
import sys
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import bootstrap_org_dotgithub_repositories as base  # noqa: E402


LINEAR_PROJECTS: dict[str, str] = {
    "channelsiege": "https://linear.app/denman/project/githubcomchannelsiege-6eecb913b93e",
    "OmniBlitz": "https://linear.app/denman/project/githubcomomniblitz-b6c7bba447b0",
    "streamkore": "https://linear.app/denman/project/githubcomstreamkore-b56684b6c8fd",
    "hypeblitz": "https://linear.app/denman/project/githubcomhypeblitz-0290c506bc1d",
    "3FA-app": "https://linear.app/denman/project/githubcom3fa-app-c3db52220894",
    "messaging-intel": "https://linear.app/denman/project/githubcommessaging-intel-e1358db591e8",
    "akrion-sim": "https://linear.app/denman/project/githubcomakrion-sim-c66c5e5e8f12",
    "athlet-o": "https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb",
    "benefactor-cc": "https://linear.app/denman/project/githubcombenefactor-cc-6bef502a1ef0",
    "canonical-cloud": "https://linear.app/denman/project/githubcomcanonical-cloud-1659c8ea1adf",
    "claritas-viz": "https://linear.app/denman/project/githubcomclaritas-viz-09fcc5d7dd9e",
    "cliptown": "https://linear.app/denman/project/githubcomcliptown-adf62fab3f42",
    "daedalus-fab": "https://linear.app/denman/project/githubcomdaedalus-fab-6d311a6d8d19",
    "declarative-migrations": "https://linear.app/denman/project/githubcomdeclarative-migrations-ffa3841a100d",
    "fiducia-cloud": "https://linear.app/denman/project/githubcomfiducia-cloud-8fd5e1bec9d3",
    "anticaptrad": "https://linear.app/denman/project/githubcomanticaptrad-e8b18d73b7f5",
    "opto-sync": "https://linear.app/denman/project/githubcomopto-sync-de6ba65bd559",
    "quaestor-ledger": "https://linear.app/denman/project/githubcomquaestor-ledger-a8cd440b3acc",
    "sagitta-stack": "https://linear.app/denman/project/githubcomsagitta-stack-60010601f42d",
    "shared-auth": "https://linear.app/denman/project/githubcomshared-auth-acbca07bb390",
    "scintilla-run": "https://linear.app/denman/project/githubcomscintilla-run-6d9dd5f5e244",
    "rust-ssr-demos": "https://linear.app/denman/project/githubcomrust-ssr-demos-4aff6fcef4d4",
    "sonus-auris": "https://linear.app/denman/project/githubcomsonus-auris-a557165528ef",
    "usa-acc": "https://linear.app/denman/project/githubcomusa-acc-112232184d74",
    "voxletra": "https://linear.app/denman/project/githubcomvoxletra-5528d72e4a7d",
    "zed-pkg": "https://linear.app/denman/project/githubcomzed-pkg-5a53230ae6cc",
    "zed-pkg-test": "https://linear.app/denman/project/githubcomzed-pkg-test-e0b5db761974",
    "memebank": "https://linear.app/denman/project/memebank-3db5f5cc7452",
    "meta-agents-demo": "https://linear.app/denman/project/meta-agents-demo-e6f63b3acf1f",
    "networking-components": "https://linear.app/denman/project/githubcomnetworking-components-0099b19507ec",
    "StreemPilot": "https://linear.app/denman/project/githubcomstreempilot-e8b8f6dee124",
    "unreal-unity-poc": "https://linear.app/denman/project/githubcomunreal-unity-poc-687af0c16406",
    "file-tunnel": "https://linear.app/denman/project/githubcomfile-tunnel-f46884af1012",
    "hypesiege": "https://linear.app/denman/project/githubcomhypesiege-12bdb95b4116",
    "discrete-event-systems": "https://linear.app/denman/project/githubcomdiscrete-event-systems-4a3086ae0c45",
    "drone-mngr": "https://linear.app/denman/project/githubcomdrone-mngr-8ac391ac308d",
}

MANAGED_PATHS: tuple[str, ...] = (
    "README.md",
    "profile/README.md",
    "AGENTS.md",
    ".github/copilot-instructions.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "SUPPORT.md",
    "CODE_OF_CONDUCT.md",
    "PULL_REQUEST_TEMPLATE.md",
)

ORIGINAL_SEMANTIC_DIRECTIVE = (
    "resolve any and all git conflicts semantically, will full context, even "
    "looking back 3-10 commits in git log history for more context - never "
    "hastily pick sides in a conflict but merge things conceptually, using max "
    "context and complete conceptual awareness for a given github organization's "
    "repos and external org repos too"
)

SEMANTIC_POLICY = f"""## Mandatory semantic conflict resolution

The original organization directive is preserved verbatim:

> {ORIGINAL_SEMANTIC_DIRECTIVE}

Operationally, resolve every Git conflict **semantically and with full context**.

Before finalizing a conflict resolution:

1. Read both sides, surrounding code or documentation, relevant tests, contracts, schemas, generated artifacts, infrastructure, and deployment assumptions—not only conflict markers.
2. Inspect the relevant history. When available and useful, review at least 3 and up to 10 prior commits with `git log`, `git show`, and `git blame`.
3. Review related repositories in this GitHub organization and relevant repositories in external organizations whenever APIs, schemas, shared libraries, generated artifacts, infrastructure, release behavior, or documentation cross repository boundaries.
4. Preserve all compatible intent and invariants. Synthesize a conceptual merge instead of accepting `ours` or `theirs` wholesale.
5. Search the resolved tree for leftover conflict markers and run the most relevant tests, formatters, linters, builds, contract checks, security checks, and end-to-end checks.
6. Document intentional behavioral choices, incompatible requirements, and discarded intent in the commit or pull-request description.

Never hastily pick a side, delete unfamiliar work, or resolve from conflict markers alone. Maximize historical, conceptual, organization-wide, and cross-organization context.
"""

DESTRUCTIVE_POLICY = """## Deny-by-default destructive operations

Automated agents must not execute or recommend commands whose purpose or practical effect is to discard, hide, rewrite, purge, delete, revoke, rotate, or irreversibly mutate existing state. A dirty worktree, an inconvenient branch, a failed migration, or a merge conflict is never permission to destroy state.

The following operations are explicitly blacklisted for agents:

- **Git state destruction or concealment:** every form of `git stash`, every form of `git reset`, `git clean`, `git filter-repo`, `git filter-branch`, history-rewriting `git rebase`, `git commit --amend`, `git checkout -- <path>`, destructive `git restore`, `git branch -D`, deletion of refs or tags, `git reflog expire`, aggressive or pruning `git gc`, `git push --force`, `git push --force-with-lease`, and equivalent history or worktree rewrites.
- **Filesystem destruction:** `rm -rf`, recursive or bulk deletion, `find -delete`, destructive overwrites, disk formatting, mass moves that erase destinations, and permission or ownership changes that can remove access.
- **Data destruction:** `DROP`, `TRUNCATE`, unbounded `DELETE`, destructive schema rollback, irreversible migrations, bucket or object purges, queue or topic deletion, and bulk record mutation without a reviewed, bounded, reversible plan.
- **Infrastructure and identity destruction:** `kubectl delete`, `helm uninstall`, `terraform destroy`, `pulumi destroy`, cloud-provider delete or purge commands, cluster or namespace teardown, and autonomous revocation or rotation of live secrets, keys, certificates, credentials, or sessions.
- **Release and governance destruction:** package or release unpublishing, artifact or registry purges, disabling branch protection, bypassing required reviews, disabling tests or security checks, and using `--no-verify` or an equivalent bypass.

This blacklist is illustrative, not exhaustive. Treat any operation that may destroy, discard, conceal, purge, revoke, or rewrite state as prohibited by default. An agent may prepare a bounded, reviewed, reversible runbook for a human, but must not execute the destructive step.

### Required safe alternatives

- Inspect with `git status`, `git diff`, `git log`, `git show`, and `git blame`.
- Leave unrelated and uncommitted work untouched.
- Use a new additive branch, a separate clean worktree, or a separate clone.
- Stage only explicit intended paths.
- Commit normally and push without force.
- Prefer read-only queries, dry runs, backups, additive migrations, and reversible roll-forward changes.
- When safe progress is impossible, preserve all state and report the exact blocker.
"""

SECRETS_POLICY = """## Secrets and sensitive data

Never print, log, commit, paste into issues, or expose tokens, credentials, private keys, session material, personal data, production data, or secret-bearing environment variables. Use placeholders in examples and redact diagnostics. Automated agents must not autonomously rotate or revoke live production credentials.
"""


def linear_project_url(organization: str) -> str:
    direct = LINEAR_PROJECTS.get(organization)
    if direct:
        return direct
    lowered = organization.lower()
    for key, value in LINEAR_PROJECTS.items():
        if key.lower() == lowered:
            return value
    raise KeyError(f"missing Linear project mapping for {organization}")


def governance_header(organization: str) -> str:
    linear_url = linear_project_url(organization)
    return f"""## Canonical organization links

- GitHub organization: https://github.com/{organization}
- Public organization defaults: https://github.com/{organization}/.github
- Canonical Linear project: {linear_url}
"""


def render_managed_body(path: str, organization: str) -> str:
    repo = f"{organization}/.github"
    linear_url = linear_project_url(organization)
    links = governance_header(organization)

    if path == "README.md":
        return f"""# {organization} organization defaults

This public `.github` repository is the canonical source for GitHub-supported community defaults, organization profile content, contribution guidance, public security/support policy, pull-request guidance, and organization-wide agent-safety declarations.

{links}
{SEMANTIC_POLICY}
{DESTRUCTIVE_POLICY}
## How GitHub applies this repository

- Supported community-health files act as fallbacks when a member repository does not define its own local version.
- `profile/README.md` renders on the organization profile.
- `AGENTS.md`, `.github/copilot-instructions.md`, branch protections, repository settings, and workflows are **not automatically inherited merely because they exist here**. Each member repository must explicitly synchronize compatible instructions or use supported organization settings and reusable workflows.
- Repository-local policy may add stricter rules, but it must not weaken the semantic conflict policy, destructive-operation blacklist, secret handling, or validation requirements.

Planning and delivery are tracked in the [canonical Linear project]({linear_url}).
"""

    if path == "profile/README.md":
        return f"""# {organization}

Shared community defaults and governance are published from [`{repo}`](https://github.com/{repo}).

{links}
All substantial work should be linked to Linear, all Git conflicts must be resolved semantically with full context, including historical and cross-repository context, and automated agents must operate non-destructively. The canonical blacklist explicitly includes `git stash`, `git reset`, `git clean`, `git filter-repo`, history rewrites, force pushes, recursive deletion, destructive data or infrastructure commands, and equivalent state-destroying operations.
"""

    if path == "AGENTS.md":
        return f"""# Organization-wide agent instructions

These instructions are mandatory in this repository and are the canonical minimum policy to synchronize into repositories owned by `{organization}`. GitHub does not automatically inherit this file into every member repository.

{links}
{SEMANTIC_POLICY}
{DESTRUCTIVE_POLICY}
{SECRETS_POLICY}
## Pull requests and validation

Reference the relevant Linear issue or project in substantial pull requests. Keep changes scoped, explain cross-repository and migration effects, list validation performed, and state whether conflicts occurred and how they were resolved.

## Precedence

Repository-local instructions may add stricter requirements, but they must not weaken this semantic conflict-resolution policy, destructive-operation blacklist, secret-handling rules, or validation expectations.
"""

    if path == ".github/copilot-instructions.md":
        return f"""# GitHub Copilot repository instructions

`/AGENTS.md` is canonical for this repository. Follow it in full and keep this mirror aligned. These instructions are repository-scoped and are not automatically inherited by every member repository.

{links}
Resolve every conflict semantically and with full context. When relevant and available, inspect 3–10 relevant prior commits, related repositories in `{organization}`, and relevant external-organization repositories. Never hastily accept `ours` or `theirs`.

Operate non-destructively. Do not use `git stash`, `git reset`, `git clean`, `git filter-repo`, `git filter-branch`, history-rewriting rebase or amend operations, destructive checkout or restore, force pushes, ref deletion, pruning, recursive deletion, destructive database or infrastructure commands, live credential revocation or rotation, release deletion, or policy bypasses. Leave unrelated work untouched and prefer additive branches, clean worktrees or clones, explicit staging, normal pushes, dry runs, backups, and reversible roll-forward changes.

Never expose secrets or sensitive data. Run relevant validation and document conflict decisions, risks, and the linked Linear work item.
"""

    if path == "CONTRIBUTING.md":
        return f"""# Contributing

Thank you for contributing to `{organization}`.

{links}
Before starting, read the target repository's local instructions, architecture, tests, contracts, and deployment documentation. Find or create the relevant work item in the [canonical Linear project]({linear_url}).

{SEMANTIC_POLICY}
{DESTRUCTIVE_POLICY}
## Pull-request expectations

- Keep changes coherent and reviewable.
- Link the relevant Linear issue or project.
- Explain cross-repository, compatibility, migration, security, and operational effects.
- State the tests, builds, linters, formatters, contract checks, security checks, and end-to-end validation performed.
- Document the context and intent retained from each side of any conflict.
- Never commit secrets, credentials, personal data, production data, or local environment files.
"""

    if path == "SECURITY.md":
        return f"""# Security policy

{links}
## Reporting a vulnerability

Do not disclose vulnerabilities, exploit details, credentials, personal data, or production data in a public issue, discussion, pull request, commit, or Linear comment.

Use the affected repository's **Security** tab and private vulnerability-reporting flow when available. Otherwise contact organization maintainers through a verified private channel shown on the organization or maintainer profile. Share only the minimum information needed until the private channel is confirmed.

Include the affected repository and version, impact, prerequisites, reproducible steps, and a minimal proof of concept with secrets and personal data removed.

## Sensitive operations

Never commit live credentials or customer data. Credential rotation, revocation, evidence preservation, and production remediation require an approved human-run incident procedure. Automated agents must not destroy evidence, purge data, rewrite history, or invalidate live production state.

Organization governance still requires semantic conflict resolution with full context and the non-destructive rules in [`AGENTS.md`](AGENTS.md).
"""

    if path == "SUPPORT.md":
        return f"""# Support

{links}
For roadmap, cross-repository planning, and delivery status, use the [canonical Linear project]({linear_url}).

For a reproducible bug or scoped feature request, open an issue in the affected repository. Include the repository, version or commit, environment, expected behavior, actual behavior, reproduction steps, impact, and sanitized logs.

Do not post vulnerabilities, credentials, personal data, or production data in public support channels. Follow [`SECURITY.md`](SECURITY.md) for private reporting.

Organization governance still requires semantic conflict resolution with full context and the non-destructive rules in [`AGENTS.md`](AGENTS.md).
"""

    if path == "CODE_OF_CONDUCT.md":
        return f"""# Code of conduct

{links}
We are committed to a professional, inclusive, privacy-respecting, and technically rigorous collaboration environment.

Communicate respectfully, critique ideas rather than people, provide evidence for technical claims, protect confidential and personal information, and avoid harassment, discrimination, threats, unwanted sexual attention, doxxing, or sustained disruption.

Good-faith disagreement is welcome. Reviews should be specific, actionable, and proportionate. Conflict resolution must preserve contributor work and project intent; unfamiliar changes must not be deleted merely to simplify a merge.

Report conduct concerns privately to organization maintainers through a verified private channel. Retaliation against good-faith reporters is prohibited.

Organization governance requires semantic conflict resolution with full context and prohibits destructive treatment of contributor work or repository state.
"""

    if path == "PULL_REQUEST_TEMPLATE.md":
        return f"""## Linear

- Issue or project: <!-- Required for substantial changes; canonical project: {linear_url} -->

## Summary

<!-- What changed, why, and what cross-repository or deployment behavior is affected? -->

## Validation

- [ ] Relevant tests, builds, linters, formatters, contract checks, security checks, and end-to-end checks passed.
- [ ] No secrets, credentials, personal data, or production data are included.
- [ ] Unrelated work was left untouched and only intended paths were staged.

## Semantic conflict-resolution record

- [ ] No conflicts occurred, or every conflict was resolved semantically with full context.
- [ ] Both sides and surrounding code, documentation, tests, contracts, schemas, and deployment assumptions were reviewed.
- [ ] When useful and available, 3–10 relevant prior commits were inspected with `git log`, `git show`, or `git blame`.
- [ ] Related repositories in `{organization}` and relevant external organizations were reviewed where shared behavior crossed repository boundaries.
- [ ] Compatible intent was preserved; no wholesale `ours` or `theirs` selection was used.
- [ ] Intentional tradeoffs are documented below.

## Non-destructive operation record

- [ ] No `git stash`, `git reset`, `git clean`, `git filter-repo`, force push, destructive history rewrite, recursive delete, destructive data or infrastructure operation, live credential revocation or rotation, release deletion, or policy bypass was used.
- [ ] Rollout and recovery use reversible roll-forward changes wherever practical.

## Risks, rollout, and conflict rationale

<!-- Explain compatibility, migration, security, operational risk, monitoring, and any conceptual merge decisions. -->
"""

    raise KeyError(f"unsupported managed path: {path}")


def verify_organization(api: Any, organization: str, branch: str) -> None:
    repository = base.get_repository(api, organization)
    if repository is None:
        raise RuntimeError(f"verification failed: missing {organization}/.github")
    base.validate_repository(repository, organization)

    linear_url = linear_project_url(organization)
    for path in MANAGED_PATHS:
        current = base.fetch_file(api, organization, path, branch)
        if current is None:
            raise RuntimeError(f"verification failed: missing {organization}/.github:{path}")
        if current.content.count(base.BEGIN_MARKER) != 1 or current.content.count(base.END_MARKER) != 1:
            raise RuntimeError(f"verification failed: malformed managed block in {organization}/.github:{path}")
        if linear_url not in current.content:
            raise RuntimeError(f"verification failed: Linear mapping absent in {organization}/.github:{path}")
        if "semantic" not in current.content.lower() or "full context" not in current.content.lower():
            raise RuntimeError(f"verification failed: semantic policy absent in {organization}/.github:{path}")

    agents = base.fetch_file(api, organization, "AGENTS.md", branch)
    assert agents is not None
    required_agent_phrases = (
        ORIGINAL_SEMANTIC_DIRECTIVE,
        "at least 3 and up to 10 prior commits",
        "git stash",
        "git reset",
        "git clean",
        "git filter-repo",
        "git filter-branch",
        "git push --force",
        "rm -rf",
        "terraform destroy",
        "prohibited by default",
    )
    for phrase in required_agent_phrases:
        if phrase not in agents.content:
            raise RuntimeError(
                f"verification failed: AGENTS.md missing required phrase {phrase!r} in {organization}/.github"
            )
    print(f"VERIFIED {organization}/.github with Linear and non-destructive agent policy")


def markdown_report(results: list[Any], *, execute: bool) -> str:
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
        "| Organization | Repository | Linear project | Created | Changed files | Verified |",
        "|---|---|---|---:|---:|---:|",
    ]
    for item in results:
        linear_url = linear_project_url(item.organization)
        lines.append(
            f"| `{item.organization}` | `{item.repository}` | [Linear]({linear_url}) | "
            f"{'yes' if item.created_repository else 'no'} | "
            f"{len(item.changed_files or [])} | {'yes' if item.verified else 'no'} |"
        )
    lines.extend(
        [
            "",
            "## Required policy",
            "",
            "All managed repositories contain the exact organization conflict directive, the expanded semantic merge procedure, the GitHub↔Linear mapping, and a deny-by-default destructive-operation blacklist.",
            "",
            "Explicitly prohibited agent operations include every form of `git stash`, `git reset`, `git clean`, `git filter-repo`, history rewrites, force pushes, recursive deletion, destructive data or infrastructure commands, autonomous live credential revocation or rotation, release deletion, and policy bypasses.",
            "",
            "## Propagation boundary",
            "",
            "GitHub-supported fallback community files apply when member repositories lack local overrides. `AGENTS.md`, Copilot repository instructions, workflows, branch protections, and repository settings require explicit synchronization or supported organization-level configuration.",
        ]
    )
    return "\n".join(lines) + "\n"


def validate_registry() -> None:
    organizations = {name.lower() for name in base.ORGANIZATIONS}
    mapped = {name.lower() for name in LINEAR_PROJECTS}
    if organizations != mapped:
        missing = sorted(organizations - mapped)
        extra = sorted(mapped - organizations)
        raise RuntimeError(f"Linear registry mismatch: missing={missing}, extra={extra}")
    if len(LINEAR_PROJECTS) != 36:
        raise RuntimeError("Linear registry must contain exactly 36 organizations")
    for organization in base.ORGANIZATIONS:
        url = linear_project_url(organization)
        if not url.startswith("https://linear.app/denman/project/"):
            raise RuntimeError(f"invalid Linear project URL for {organization}")


def install_hardening() -> None:
    """Patch only the reviewed publisher's policy surface for execution."""
    base.MANAGED_PATHS = MANAGED_PATHS
    base.render_managed_body = render_managed_body
    base.verify_organization = verify_organization
    base.markdown_report = markdown_report


def main() -> int:
    validate_registry()
    install_hardening()
    return base.main()


if __name__ == "__main__":
    raise SystemExit(main())
