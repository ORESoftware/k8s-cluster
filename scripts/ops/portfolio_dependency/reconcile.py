"""Per-edge, repository, and organization reconciliation."""

from __future__ import annotations

import contextlib
import shutil
from pathlib import Path

from .actions import (
    candidate_branch,
    open_or_update_pr,
    resolve_candidates,
    ticket,
    trial_candidate,
)
from .model import (
    BotError,
    Candidate,
    DependencyEdge,
    EdgeResult,
    MutationUnsupported,
    Policy,
    PortfolioProject,
    Repository,
    RuntimeState,
    TestOutcome,
    find_highest_passing,
)
from .providers import GitHubClient, LinearClient, RepairClient, WorkerClient
from .workspace import GitWorkspace, detect_profile, discover_edges

def process_edge(
    *,
    project: PortfolioProject,
    repository: Repository,
    edge: DependencyEdge,
    workspace: GitWorkspace,
    github: GitHubClient,
    linear: LinearClient | None,
    worker: WorkerClient,
    repair: RepairClient | None,
    policy: Policy,
    state: RuntimeState,
    run_id: str,
) -> None:
    old = edge.current_version or edge.current_sha
    if bool(edge.metadata.get("graphOnly")):
        state.add_result(
            EdgeResult(
                repository.full_name,
                edge.dependency_key,
                edge.kind,
                "observed_graph_only",
                "lock-derived transitive dependency; updated through its direct manifest input",
                old=old,
            )
        )
        return
    try:
        candidates, patch_only, majors, resolution_error = resolve_candidates(
            github, policy, edge
        )
        for major in majors[-1:]:
            url = ticket(
                linear,
                project,
                edge,
                reason="major-release-requires-planning",
                detail=(
                    f"A major release `{major.display}` is available. The nightly bot is forbidden "
                    "from applying major changes; plan the migration explicitly."
                ),
                candidate=major,
            )
            state.add_result(
                EdgeResult(
                    repository.full_name,
                    edge.dependency_key,
                    edge.kind,
                    "major_ticketed" if url else "major_ticket_pending_credentials",
                    old=old,
                    new=major.display,
                    linear_issue_url=url,
                )
            )
        if resolution_error:
            url = ticket(
                linear,
                project,
                edge,
                reason="dependency-resolution-blocked",
                detail=resolution_error,
            )
            state.add_result(
                EdgeResult(
                    repository.full_name,
                    edge.dependency_key,
                    edge.kind,
                    "ticketed" if url else "blocked",
                    resolution_error,
                    old=old,
                    linear_issue_url=url,
                )
            )
            return
        if not candidates:
            detail = (
                f"ignored {len(patch_only)} patch-only release(s)"
                if patch_only
                else "already at the newest eligible minor/branch tip"
            )
            state.add_result(
                EdgeResult(
                    repository.full_name,
                    edge.dependency_key,
                    edge.kind,
                    "patch_ignored" if patch_only else "up_to_date",
                    detail,
                    old=old,
                )
            )
            return

        branch = candidate_branch(policy, edge)
        outcomes: dict[int, TestOutcome] = {}

        def test(candidate: Candidate) -> bool:
            index = candidates.index(candidate)
            try:
                outcome = trial_candidate(
                    workspace=workspace,
                    github=github,
                    worker=worker,
                    repair=repair,
                    policy=policy,
                    repository=repository,
                    edge=edge,
                    candidate=candidate,
                    branch=branch,
                    run_id=run_id,
                )
            except (BotError, MutationUnsupported) as exc:
                outcome = TestOutcome(
                    passed=False,
                    profile=detect_profile(workspace.path),
                    status="mutation_or_test_failed",
                    detail=str(exc),
                )
            outcomes[index] = outcome
            return outcome.passed

        best_index, bisect_outcomes = find_highest_passing(candidates, test)
        tested = [
            {
                "index": index,
                "candidate": candidates[index].display,
                "passed": passed,
                "profile": outcomes.get(index).profile if index in outcomes else None,
                "status": outcomes.get(index).status if index in outcomes else "not_run",
                "jobId": outcomes.get(index).job_id if index in outcomes else None,
                "detail": outcomes.get(index).detail if index in outcomes else "",
            }
            for index, passed in sorted(bisect_outcomes.items())
        ]
        if best_index is None:
            newest = candidates[-1]
            failure = outcomes.get(len(candidates) - 1) or next(iter(outcomes.values()), TestOutcome(False, None))
            detail = (
                "No eligible minor candidate passed.\n\n"
                + failure.detail
                + ("\n\nWorker log tail:\n```\n" + failure.logs[-10000:] + "\n```" if failure.logs else "")
            )
            url = ticket(
                linear,
                project,
                edge,
                reason="minor-update-failed",
                detail=detail,
                candidate=newest,
            )
            workspace.remove_remote_branch(branch)
            state.add_result(
                EdgeResult(
                    repository.full_name,
                    edge.dependency_key,
                    edge.kind,
                    "ticketed" if url else "failed",
                    detail=failure.detail,
                    old=old,
                    new=newest.display,
                    linear_issue_url=url,
                    test_profile=failure.profile,
                    tested_candidates=tested,
                )
            )
            return

        best = candidates[best_index]
        # The final trial may not be the selected one because descending verification can test
        # additional candidates. Rebuild and retest the exact selected commit before PR creation.
        final_outcome = trial_candidate(
            workspace=workspace,
            github=github,
            worker=worker,
            repair=repair,
            policy=policy,
            repository=repository,
            edge=edge,
            candidate=best,
            branch=branch,
            run_id=run_id + ":final",
        )
        if not final_outcome.passed:
            url = ticket(
                linear,
                project,
                edge,
                reason="selected-minor-recheck-failed",
                detail=final_outcome.detail + "\n\n" + final_outcome.logs[-10000:],
                candidate=best,
            )
            workspace.remove_remote_branch(branch)
            state.add_result(
                EdgeResult(
                    repository.full_name,
                    edge.dependency_key,
                    edge.kind,
                    "ticketed" if url else "failed",
                    final_outcome.detail,
                    old=old,
                    new=best.display,
                    linear_issue_url=url,
                    test_profile=final_outcome.profile,
                    tested_candidates=tested,
                )
            )
            return

        if not state.reserve_pull_request():
            url = ticket(
                linear,
                project,
                edge,
                reason="nightly-pr-budget-exhausted",
                detail="The update passed but the bounded per-run PR budget was exhausted.",
                candidate=best,
            )
            workspace.remove_remote_branch(branch)
            state.add_result(
                EdgeResult(
                    repository.full_name,
                    edge.dependency_key,
                    edge.kind,
                    "deferred",
                    "pull-request budget exhausted",
                    old=old,
                    new=best.display,
                    linear_issue_url=url,
                    test_profile=final_outcome.profile,
                    tested_candidates=tested,
                )
            )
            return

        pr_url, _ = open_or_update_pr(
            github,
            policy,
            repository,
            edge,
            best,
            final_outcome,
            tested,
            branch,
        )
        state.add_result(
            EdgeResult(
                repository.full_name,
                edge.dependency_key,
                edge.kind,
                "pull_request_opened",
                old=old,
                new=best.display,
                pull_request_url=pr_url,
                test_profile=final_outcome.profile,
                tested_candidates=tested,
            )
        )
    except Exception as exc:  # keep one edge from aborting the portfolio scan
        detail = str(exc)
        url: str | None = None
        with contextlib.suppress(Exception):
            url = ticket(
                linear,
                project,
                edge,
                reason="dependency-bot-internal-blocker",
                detail=detail,
            )
        state.add_result(
            EdgeResult(
                repository.full_name,
                edge.dependency_key,
                edge.kind,
                "ticketed" if url else "error",
                detail,
                old=old,
                linear_issue_url=url,
            )
        )


def process_repository(
    *,
    project: PortfolioProject,
    repository: Repository,
    github: GitHubClient,
    linear: LinearClient | None,
    worker: WorkerClient,
    repair: RepairClient | None,
    policy: Policy,
    state: RuntimeState,
    work_root: Path,
    token: str,
    run_id: str,
) -> None:
    workspace = GitWorkspace(repository, token, work_root)
    try:
        workspace.clone()
        state.add_node(repository.full_name)
        edges = discover_edges(repository, workspace.path)
        state.add_edges(edges)
        for edge in edges:
            if edge.target_repo:
                state.add_node(edge.target_repo)
            process_edge(
                project=project,
                repository=repository,
                edge=edge,
                workspace=workspace,
                github=github,
                linear=linear,
                worker=worker,
                repair=repair,
                policy=policy,
                state=state,
                run_id=run_id,
            )
    except Exception as exc:
        state.add_error(f"{repository.full_name}: {exc}")
    finally:
        shutil.rmtree(workspace.path, ignore_errors=True)


def process_organization(
    *,
    project: PortfolioProject,
    github: GitHubClient,
    linear: LinearClient | None,
    worker: WorkerClient,
    repair: RepairClient | None,
    policy: Policy,
    state: RuntimeState,
    work_root: Path,
    token: str,
    run_id: str,
    repository_filter: str | None,
) -> None:
    try:
        repositories = github.list_org_repositories(
            project.github_org, policy.max_repositories_per_org
        )
    except Exception as exc:
        state.add_error(f"{project.github_org}: cannot list repositories: {exc}")
        return
    for repository in repositories:
        if repository_filter and repository.full_name.lower() != repository_filter.lower():
            continue
        if repository.archived and not bool(policy.raw["scope"]["includeArchivedRepositories"]):
            continue
        if repository.fork and not bool(policy.raw["scope"]["includeForks"]):
            continue
        process_repository(
            project=project,
            repository=repository,
            github=github,
            linear=linear,
            worker=worker,
            repair=repair,
            policy=policy,
            state=state,
            work_root=work_root,
            token=token,
            run_id=run_id,
        )
