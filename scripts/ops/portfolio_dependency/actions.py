"""Candidate resolution, fixed-profile trials, PRs, and Linear escalation."""

from __future__ import annotations

import hashlib
import json
from typing import Any, Mapping, Sequence

from .model import (
    SHA_RE,
    BotError,
    Candidate,
    DependencyEdge,
    MutationUnsupported,
    Policy,
    PortfolioProject,
    Repository,
    SemVer,
    TestOutcome,
    managed_marker,
    pr_is_managed,
    safe_slug,
    select_semver_candidates,
)
from .providers import GitHubClient, LinearClient, RepairClient, WorkerClient
from .workspace import GitWorkspace, apply_candidate, detect_profile

def candidate_branch(policy: Policy, edge: DependencyEdge) -> str:
    return (
        policy.branch_prefix
        + safe_slug(edge.dependency_key, 30)
        + "-"
        + edge.stable_key[:10]
    )


def candidate_message(edge: DependencyEdge, candidate: Candidate) -> str:
    return f"chore(deps): advance {edge.dependency_key} to {candidate.display}"


def format_candidate(candidate: Candidate) -> str:
    if candidate.version:
        return str(candidate.version)
    return candidate.git_sha or candidate.display


def build_pr_body(
    policy: Policy,
    edge: DependencyEdge,
    candidate: Candidate,
    outcome: TestOutcome,
    tested_candidates: Sequence[Mapping[str, Any]],
) -> str:
    old = edge.current_version or edge.current_sha or "unresolved"
    target = edge.target_repo or edge.target_url or "unresolved"
    return "\n".join(
        (
            managed_marker(edge),
            f"# Nightly minor dependency advancement: `{edge.dependency_key}`",
            "",
            f"- Source: `{edge.kind}` in `{edge.source_path}`",
            f"- Dependency: `{target}`",
            f"- Previous pin: `{old}`",
            f"- Proposed pin: `{format_candidate(candidate)}`",
            f"- Change class: `{candidate.change_class}`",
            f"- Verification profile: `{outcome.profile}`",
            f"- Worker result: `{outcome.status}` (job `{outcome.job_id}`)",
            "",
            "## Policy",
            "",
            "Patch-only releases are ignored. Minor releases are eligible. Fast-forward movement of",
            "`main`, `master`, or a `release*` branch is treated as a minor lane. Major releases are",
            "never applied by this pull request and are always filed in the mapped Linear project.",
            "",
            "The candidate was selected with binary search plus descending verification so a later",
            "release that fixes an intermediate regression can still be selected.",
            "",
            "## Candidate evidence",
            "",
            "```json",
            json.dumps(list(tested_candidates), indent=2, sort_keys=True),
            "```",
        )
    ) + "\n"


def build_linear_description(
    project: PortfolioProject,
    edge: DependencyEdge,
    *,
    reason: str,
    detail: str,
    candidate: Candidate | None = None,
) -> tuple[str, str, str]:
    candidate_text = format_candidate(candidate) if candidate else "n/a"
    marker = f"portfolio-dependency-bot:v1:{edge.stable_key}:{safe_slug(reason, 32)}"
    title = f"[dependency-bot] {reason}: {edge.source_repo} → {edge.dependency_key}"
    description = "\n".join(
        (
            f"<!-- {marker} -->",
            "## Nightly dependency follow-up",
            "",
            f"- Portfolio: `{project.portfolio_key}`",
            f"- Source repository: `{edge.source_repo}`",
            f"- Dependency kind: `{edge.kind}`",
            f"- Manifest: `{edge.source_path}`",
            f"- Dependency: `{edge.target_repo or edge.target_url or edge.dependency_key}`",
            f"- Current pin: `{edge.current_version or edge.current_sha or 'unknown'}`",
            f"- Candidate: `{candidate_text}`",
            f"- Reason: `{reason}`",
            "",
            detail[:12000],
            "",
            "### Guardrails",
            "",
            "The nightly bot must not apply major releases. Patch-only releases remain ignored.",
            "A branch-tip advancement is treated as minor only for main/master/release lanes and",
            "must still pass the repository's fixed worker profile before a pull request opens.",
        )
    )
    return marker, title[:250], description


def resolve_candidates(
    github: GitHubClient,
    policy: Policy,
    edge: DependencyEdge,
) -> tuple[list[Candidate], list[Candidate], list[Candidate], str | None]:
    if not edge.target_repo:
        return [], [], [], "dependency does not resolve to a GitHub repository"

    current_version = SemVer.parse(edge.current_version or "")
    if current_version is not None:
        minor, patch, major = select_semver_candidates(
            current_version, github.tags(edge.target_repo)
        )
        if len(minor) > policy.max_candidates:
            return [], patch, major, (
                f"{len(minor)} newer minor lines exceed the safe candidate cap "
                f"of {policy.max_candidates}"
            )
        return minor, patch, major, None

    target = github.repository(edge.target_repo)
    branch = edge.tracked_branch or target.default_branch
    if not policy.branch_is_minor_lane(branch):
        return [], [], [], f"branch {branch!r} is not an allowed minor lane"
    if not edge.current_sha or not SHA_RE.fullmatch(edge.current_sha):
        return [], [], [], "current Git commit is unavailable"
    tip = github.branch_sha(edge.target_repo, branch)
    if tip == edge.current_sha:
        return [], [], [], None
    status, commits, total = github.compare_commits(
        edge.target_repo, edge.current_sha, tip, policy.max_candidates
    )
    if status not in {"ahead", "identical"}:
        return [], [], [], f"tracked branch is not a fast-forward ({status})"
    if total > policy.max_candidates:
        return [], [], [], (
            f"branch advanced by {total} commits, exceeding the safe candidate cap "
            f"of {policy.max_candidates}"
        )
    candidates = [
        Candidate(sha[:12], git_sha=sha, git_ref=branch, change_class="minor-branch-tip")
        for sha in commits
    ]
    return candidates, [], [], None


def open_or_update_pr(
    github: GitHubClient,
    policy: Policy,
    repository: Repository,
    edge: DependencyEdge,
    candidate: Candidate,
    outcome: TestOutcome,
    tested_candidates: Sequence[Mapping[str, Any]],
    branch: str,
) -> tuple[str, int]:
    title = f"{policy.raw['pullRequests']['titlePrefix']} advance {edge.dependency_key} to {candidate.display}"
    body = build_pr_body(policy, edge, candidate, outcome, tested_candidates)
    current: Mapping[str, Any] | None = None
    superseded: list[Mapping[str, Any]] = []
    for pr in github.open_pull_requests(repository.full_name):
        head = pr.get("head")
        head_ref = head.get("ref") if isinstance(head, Mapping) else None
        pr_body = pr.get("body")
        if pr_is_managed(
            pr_body if isinstance(pr_body, str) else None,
            head_ref if isinstance(head_ref, str) else None,
            edge,
            policy.branch_prefix,
        ):
            if head_ref == branch:
                current = pr
            else:
                superseded.append(pr)

    if current is not None:
        number = int(current["number"])
        updated = github.update_pull_request(
            repository.full_name, number, title=title, body=body
        )
        url = updated.get("html_url")
    else:
        created = github.create_pull_request(
            repository.full_name,
            title=title,
            body=body,
            head=branch,
            base=repository.default_branch,
            draft=bool(policy.raw["pullRequests"]["draftAfterPassingTests"]),
        )
        number = int(created["number"])
        url = created.get("html_url")

    if bool(policy.raw["pullRequests"]["closeSuperseded"]):
        for pr in superseded:
            old_number = int(pr["number"])
            github.comment_issue(
                repository.full_name,
                old_number,
                (
                    f"Closing as superseded by managed dependency PR #{number}. "
                    "This action is restricted to PRs carrying the same dependency-bot marker."
                ),
            )
            github.update_pull_request(repository.full_name, old_number, state="closed")
    if not isinstance(url, str):
        raise BotError("GitHub pull request response omitted html_url")
    return url, number


def ticket(
    linear: LinearClient | None,
    project: PortfolioProject,
    edge: DependencyEdge,
    *,
    reason: str,
    detail: str,
    candidate: Candidate | None = None,
) -> str | None:
    if linear is None:
        return None
    marker, title, description = build_linear_description(
        project, edge, reason=reason, detail=detail, candidate=candidate
    )
    return linear.ensure_issue(
        project, marker=marker, title=title, description=description
    )


def trial_candidate(
    *,
    workspace: GitWorkspace,
    github: GitHubClient,
    worker: WorkerClient,
    repair: RepairClient | None,
    policy: Policy,
    repository: Repository,
    edge: DependencyEdge,
    candidate: Candidate,
    branch: str,
    run_id: str,
) -> TestOutcome:
    workspace.reset_to_base(branch)
    apply_candidate(workspace.path, edge, candidate)
    workspace.commit_and_push(branch, candidate_message(edge, candidate))
    profile = detect_profile(workspace.path)
    if not profile:
        return TestOutcome(
            passed=False,
            profile=None,
            status="unsupported",
            detail="repository has no fixed gha-indie-worker verification profile",
        )
    outcome = worker.test_branch(
        repository,
        branch,
        profile,
        f"{run_id}:{edge.stable_key}:{hashlib.sha256(candidate.display.encode()).hexdigest()[:12]}",
    )
    if outcome.passed or repair is None:
        return outcome
    for attempt in range(1, policy.repair_attempts + 1):
        repaired = repair.repair(repository, branch, edge, candidate, outcome, attempt)
        if not repaired:
            break
        outcome = worker.test_branch(
            repository,
            branch,
            profile,
            f"{run_id}:{edge.stable_key}:repair-{attempt}",
        )
        if outcome.passed:
            return outcome
    return outcome
