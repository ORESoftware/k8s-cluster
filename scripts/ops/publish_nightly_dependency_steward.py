#!/usr/bin/env python3
"""Publish a credential-free dependency-steward analysis plan.

This phase never runs repository-controlled build or test commands. It validates
that every plan entry still targets the exact default-branch SHA tested by the
analysis job, applies the recorded bounded patch, creates or updates the managed
minor-upgrade PR, closes only superseded PRs carrying the steward marker, and
creates deduplicated Linear issues for planned escalations.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
import shutil
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Mapping, Sequence

from nightly_dependency_steward import (
    JOB_MARKER,
    DependencyRef,
    GitHubClient,
    LinearClient,
    SemVer,
    StewardError,
    branch_name,
    changed_files,
    clone_exact_repository,
    issue_marker,
    load_portfolio_registry,
    managed_pr_numbers_to_close,
    minor_line_candidates,
    parse_pr_marker,
    push_branch,
    redact,
    remote_versions,
    resolve_current_version,
    run_process,
    scan_repository,
    safe_slug,
    sensitive_environment_key,
    validate_patch,
)


@dataclass
class PublishResult:
    repository: str
    status: str
    pull_request: str | None = None
    closed_pull_requests: list[str] = field(default_factory=list)
    linear_ticket: str | None = None
    warnings: list[str] = field(default_factory=list)
    error: str | None = None


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument(
        "--artifacts", type=Path, default=Path("artifacts/dependency-steward-publish")
    )
    parser.add_argument("--work-root", type=Path)
    parser.add_argument("--max-prs", type=int, default=200)
    return parser.parse_args(argv)


def require_string(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise StewardError(f"publish plan field {field!r} must be a non-empty string")
    return value


def dependency_from_plan(value: Any, repository: str) -> DependencyRef:
    if not isinstance(value, dict):
        raise StewardError("publish plan dependency must be an object")
    return DependencyRef(
        repository=repository,
        kind=require_string(value.get("kind"), "dependency.kind"),
        key=require_string(value.get("key"), "dependency.key"),
        name=require_string(value.get("name"), "dependency.name"),
        source_url=require_string(value.get("source_url"), "dependency.source_url"),
        manifest_path=require_string(value.get("manifest_path"), "dependency.manifest_path"),
        current_ref=value.get("current_ref") if isinstance(value.get("current_ref"), str) else None,
        current_version=SemVer.parse(value.get("current_version")),
        mutable=bool(value.get("mutable", True)),
        locator={
            str(key): str(item)
            for key, item in (value.get("locator") or {}).items()
            if isinstance(key, str) and isinstance(item, (str, int, float))
        },
        note=value.get("note") if isinstance(value.get("note"), str) else None,
    )


def load_plan(path: Path) -> dict[str, Any]:
    try:
        plan = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise StewardError(f"cannot read publish plan {path}: {exc}") from exc
    if not isinstance(plan, dict) or plan.get("contract") != JOB_MARKER:
        raise StewardError("publish plan contract is missing or unsupported")
    if plan.get("phase") != "analyze":
        raise StewardError("publish plan did not come from the analyze phase")
    if not isinstance(plan.get("tickets", []), list):
        raise StewardError("publish plan tickets must be a list")
    if not isinstance(plan.get("pull_requests", []), list):
        raise StewardError("publish plan pull_requests must be a list")
    return plan


def resolve_patch(plan_path: Path, relative_value: str, expected_sha: str) -> str:
    relative = Path(relative_value)
    if relative.is_absolute() or ".." in relative.parts:
        raise StewardError(f"unsafe publish-plan patch path: {relative_value}")
    root = plan_path.parent.resolve()
    path = (root / relative).resolve()
    if not path.is_relative_to(root):
        raise StewardError(f"publish-plan patch escaped artifact root: {relative_value}")
    try:
        patch = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise StewardError(f"cannot read planned patch {relative_value}: {exc}") from exc
    actual = hashlib.sha256(patch.encode()).hexdigest()
    if actual != expected_sha:
        raise StewardError(
            f"planned patch digest mismatch: expected {expected_sha}, observed {actual}"
        )
    validate_patch(patch)
    if not patch.strip():
        raise StewardError("planned patch is empty")
    return patch


def plan_owner(repository: str) -> str:
    if repository.count("/") != 1:
        raise StewardError(f"invalid repository coordinate in plan: {repository}")
    return repository.split("/", 1)[0].lower()


def publish_ticket(
    *,
    item: Mapping[str, Any],
    by_org: Mapping[str, Any],
    linear: LinearClient,
) -> PublishResult:
    repository = require_string(item.get("repository"), "ticket.repository")
    owner = plan_owner(repository)
    link = by_org.get(owner)
    if link is None:
        raise StewardError(f"ticket repository owner is not in canonical registry: {repository}")
    project_id = require_string(item.get("project_id"), "ticket.project_id")
    if project_id != link.linear_project_id:
        raise StewardError(
            f"ticket project drift for {repository}: plan={project_id}, registry={link.linear_project_id}"
        )
    marker = require_string(item.get("marker"), "ticket.marker")
    if not marker.startswith(f"{JOB_MARKER}:"):
        raise StewardError(f"ticket marker is not steward-owned: {marker}")
    url = linear.ensure_issue(
        project_id=project_id,
        title=require_string(item.get("title"), "ticket.title"),
        description=require_string(item.get("description"), "ticket.description"),
        marker=marker,
    )
    return PublishResult(repository=repository, status="ticket-published", linear_ticket=url)


def stale_analysis_ticket(
    *,
    repository: str,
    dep: DependencyRef,
    base_sha: str,
    current_sha: str,
    project_id: str,
    linear: LinearClient,
) -> str:
    marker = issue_marker("stale-analysis", repository, dep, current_sha[:12])
    return linear.ensure_issue(
        project_id=project_id,
        title=f"[dependency-steward] {repository}: retry stale verified update",
        description=(
            "The analyze phase produced a verified minor-upgrade plan, but the default "
            "branch moved before publication. The publisher refused to replay a patch onto "
            "an untested head. The next nightly run will analyze the new immutable SHA.\n\n"
            f"- Dependency: `{dep.name}`\n"
            f"- Tested base SHA: `{base_sha}`\n"
            f"- Current base SHA: `{current_sha}`"
        ),
        marker=marker,
    )


def publish_pull(
    *,
    item: Mapping[str, Any],
    plan_path: Path,
    by_org: Mapping[str, Any],
    github: GitHubClient,
    linear: LinearClient,
    work_root: Path,
) -> PublishResult:
    repository = require_string(item.get("repository"), "pull.repository")
    owner = plan_owner(repository)
    link = by_org.get(owner)
    if link is None:
        raise StewardError(f"pull repository owner is not in canonical registry: {repository}")
    default_branch = require_string(item.get("default_branch"), "pull.default_branch")
    base_sha = require_string(item.get("base_sha"), "pull.base_sha")
    if len(base_sha) != 40 or any(value not in "0123456789abcdef" for value in base_sha):
        raise StewardError(f"invalid exact base SHA in plan: {base_sha}")
    dep = dependency_from_plan(item.get("dependency"), repository)
    current = SemVer.parse(require_string(item.get("current_version"), "pull.current_version"))
    target = SemVer.parse(require_string(item.get("target_version"), "pull.target_version"))
    if current is None or target is None:
        raise StewardError("publish plan versions must be stable SemVer")
    if target.major != current.major or target.minor <= current.minor:
        raise StewardError(
            f"publisher rejected non-minor movement for {repository}/{dep.key}: {current} -> {target}"
        )
    target_tag = require_string(item.get("target_tag"), "pull.target_tag")
    target_sha = require_string(item.get("target_sha"), "pull.target_sha")
    versions = remote_versions(dep.source_url, token=github.token)
    eligible = minor_line_candidates(current, versions)
    selected = next(
        (
            candidate
            for candidate in eligible
            if candidate.version == target
            and candidate.tag == target_tag
            and candidate.sha == target_sha
        ),
        None,
    )
    if selected is None:
        raise StewardError(
            f"publisher could not independently verify eligible target "
            f"{target_tag}/{target_sha} for {repository}/{dep.key}"
        )
    branch = require_string(item.get("branch"), "pull.branch")
    expected_branch = branch_name(dep, target)
    if branch != expected_branch:
        raise StewardError(
            f"managed branch drift for {repository}/{dep.key}: {branch} != {expected_branch}"
        )
    title = require_string(item.get("title"), "pull.title")
    body = require_string(item.get("body"), "pull.body")
    marker = parse_pr_marker(body)
    if not marker or marker.get("key") != dep.key or marker.get("target") != str(target):
        raise StewardError(f"PR body marker does not match plan for {repository}/{dep.key}")
    patch = resolve_patch(
        plan_path,
        require_string(item.get("patch_path"), "pull.patch_path"),
        require_string(item.get("patch_sha256"), "pull.patch_sha256"),
    )

    current_sha = github.branch_sha(repository, default_branch)
    if current_sha != base_sha:
        ticket = stale_analysis_ticket(
            repository=repository,
            dep=dep,
            base_sha=base_sha,
            current_sha=current_sha,
            project_id=link.linear_project_id,
            linear=linear,
        )
        return PublishResult(
            repository=repository,
            status="stale-base-ticketed",
            linear_ticket=ticket,
            warnings=[f"default branch moved from {base_sha} to {current_sha}"],
        )

    metadata = github.repository(repository)
    clone_url = require_string(metadata.get("clone_url"), "repository.clone_url")
    observed_default = require_string(
        metadata.get("default_branch"), "repository.default_branch"
    )
    if observed_default != default_branch:
        raise StewardError(
            f"default-branch name drift for {repository}: plan={default_branch}, observed={observed_default}"
        )
    destination = work_root / safe_slug(repository, 60)
    if destination.exists():
        shutil.rmtree(destination)
    clone_exact_repository(
        full_name=repository,
        clone_url=clone_url,
        branch=default_branch,
        sha=base_sha,
        token=github.token,
        destination=destination,
    )
    observed_edges = scan_repository(destination, repository)
    observed = next(
        (
            edge
            for edge in observed_edges
            if edge.key == dep.key
            and edge.kind == dep.kind
            and edge.manifest_path == dep.manifest_path
        ),
        None,
    )
    if observed is None:
        raise StewardError(f"planned dependency edge no longer exists: {repository}/{dep.key}")
    if (
        observed.kind != dep.kind
        or observed.manifest_path != dep.manifest_path
        or observed.source_url != dep.source_url
    ):
        raise StewardError(f"planned dependency metadata drifted: {repository}/{dep.key}")
    observed_current = resolve_current_version(observed, versions)
    if observed_current != current:
        raise StewardError(
            f"planned current version drifted for {repository}/{dep.key}: "
            f"plan={current}, observed={observed_current}"
        )

    patch_file = destination / ".git" / "dependency-steward-plan.patch"
    patch_file.write_text(patch, encoding="utf-8")
    run_process(["git", "apply", "--check", str(patch_file)], cwd=destination)
    run_process(["git", "apply", str(patch_file)], cwd=destination)
    if not changed_files(destination):
        raise StewardError("planned patch produced no tracked changes")
    updated_edges = scan_repository(destination, repository)
    updated = next(
        (
            edge
            for edge in updated_edges
            if edge.key == dep.key
            and edge.kind == dep.kind
            and edge.manifest_path == dep.manifest_path
        ),
        None,
    )
    if updated is None:
        raise StewardError(f"dependency edge disappeared after planned patch: {repository}/{dep.key}")
    updated_version = resolve_current_version(updated, versions)
    if updated.kind == "git-submodule":
        if updated.current_ref != target_sha:
            raise StewardError(
                f"submodule patch did not pin verified target SHA {target_sha}"
            )
    elif updated.kind in {"zpkg", "nix-expression"}:
        if updated_version != target:
            raise StewardError(
                f"planned patch did not encode verified target {target} for {dep.key}"
            )
    elif updated.kind == "nix-flake":
        if updated.current_ref != target_sha and updated_version != target:
            raise StewardError(
                f"flake patch did not pin verified target {target_tag}/{target_sha}"
            )

    push_branch(
        root=destination,
        branch=branch,
        base_sha=base_sha,
        token=github.token,
        message=title,
    )
    pulls = github.open_pulls(repository)
    existing = next(
        (
            pull
            for pull in pulls
            if str((pull.get("head") or {}).get("ref")) == branch
            and parse_pr_marker(str(pull.get("body") or ""))
        ),
        None,
    )
    if existing:
        number = int(existing["number"])
        pull = github.update_pull(repository, number, title=title, body=body)
    else:
        pull = github.create_pull(
            repository,
            title=title,
            head=branch,
            base=default_branch,
            body=body,
        )
        number = int(pull["number"])

    warnings: list[str] = []
    try:
        github.add_labels(
            repository,
            number,
            ["dependencies", "dependency-steward", "minor-update"],
        )
    except StewardError as exc:
        warnings.append(f"could not apply labels to PR #{number}: {redact(str(exc))}")

    closed: list[str] = []
    for old_number in managed_pr_numbers_to_close(
        pulls,
        dependency_key=dep.key,
        target=target,
        keep_number=number,
    ):
        github.comment(
            repository,
            old_number,
            (
                f"Closed by `{JOB_MARKER}` because PR #{number} supersedes this "
                f"managed dependency update with verified target `{target}`."
            ),
        )
        github.update_pull(repository, old_number, state="closed")
        closed.append(f"{repository}#{old_number}")

    return PublishResult(
        repository=repository,
        status="pull-published",
        pull_request=str(pull.get("html_url") or f"{repository}#{number}"),
        closed_pull_requests=closed,
        warnings=warnings,
    )


def write_report(path: Path, results: Sequence[PublishResult], errors: Sequence[str]) -> None:
    path.mkdir(parents=True, exist_ok=True)
    payload = {
        "contract": JOB_MARKER,
        "phase": "publish",
        "results": [dataclasses.asdict(item) for item in results],
        "errors": list(errors),
        "pull_requests": sum(1 for item in results if item.pull_request),
        "linear_tickets": sum(1 for item in results if item.linear_ticket),
        "closed_pull_requests": sum(len(item.closed_pull_requests) for item in results),
    }
    (path / "publish-report.json").write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    lines = [
        "# Dependency steward publication",
        "",
        f"- Pull requests opened or updated: **{payload['pull_requests']}**",
        f"- Linear tickets created or reused: **{payload['linear_tickets']}**",
        f"- Obsolete managed PRs closed: **{payload['closed_pull_requests']}**",
        f"- Errors: **{len(errors)}**",
        "",
        "| Repository | Status | PR | Linear | Closed |",
        "|---|---|---|---|---:|",
    ]
    for item in results:
        lines.append(
            f"| `{item.repository}` | {item.status} | {item.pull_request or ''} | "
            f"{item.linear_ticket or ''} | {len(item.closed_pull_requests)} |"
        )
    if errors:
        lines.extend(["", "## Errors", ""])
        lines.extend(f"- {redact(error)}" for error in errors)
    (path / "publish-report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    if args.max_prs < 1 or args.max_prs > 1000:
        raise StewardError("--max-prs must be between 1 and 1000")
    plan = load_plan(args.plan)
    planned_pulls = plan.get("pull_requests", [])
    if len(planned_pulls) > args.max_prs:
        raise StewardError(
            f"publish plan has {len(planned_pulls)} PRs, above cap {args.max_prs}"
        )

    github_token = os.getenv("DEPENDENCY_STEWARD_GITHUB_TOKEN") or os.getenv(
        "PROJECT_SYNC_GITHUB_TOKEN"
    )
    linear_token = os.getenv("LINEAR_API_KEY")
    if not github_token:
        raise StewardError("DEPENDENCY_STEWARD_GITHUB_TOKEN is required")
    if not linear_token:
        raise StewardError("LINEAR_API_KEY is required")
    github_api_url = os.getenv("GITHUB_API_URL", "https://api.github.com")
    linear_team = os.getenv("DEPENDENCY_STEWARD_LINEAR_TEAM_ID")

    github = GitHubClient(github_token, github_api_url)
    linear = LinearClient(linear_token, linear_team)
    for key in list(os.environ):
        if sensitive_environment_key(key):
            os.environ.pop(key, None)

    portfolio = load_portfolio_registry(args.registry)
    by_org = {item.github_org.lower(): item for item in portfolio}
    temporary: tempfile.TemporaryDirectory[str] | None = None
    if args.work_root:
        work_root = args.work_root
        work_root.mkdir(parents=True, exist_ok=True)
    else:
        temporary = tempfile.TemporaryDirectory(prefix="dependency-steward-publish-")
        work_root = Path(temporary.name)

    results: list[PublishResult] = []
    errors: list[str] = []
    for raw in plan.get("tickets", []):
        try:
            if not isinstance(raw, dict):
                raise StewardError("ticket plan entry must be an object")
            results.append(publish_ticket(item=raw, by_org=by_org, linear=linear))
        except Exception as exc:
            errors.append(f"ticket publication failed: {redact(str(exc))}")
    for raw in planned_pulls:
        try:
            if not isinstance(raw, dict):
                raise StewardError("pull plan entry must be an object")
            results.append(
                publish_pull(
                    item=raw,
                    plan_path=args.plan,
                    by_org=by_org,
                    github=github,
                    linear=linear,
                    work_root=work_root,
                )
            )
        except Exception as exc:
            repository = raw.get("repository", "unknown") if isinstance(raw, dict) else "unknown"
            message = redact(str(exc))
            errors.append(f"{repository}: {message}")
            results.append(
                PublishResult(
                    repository=str(repository),
                    status="failed",
                    error=message,
                )
            )

    write_report(args.artifacts, results, errors)
    if temporary is not None:
        temporary.cleanup()
    return 1 if errors else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except StewardError as exc:
        print(f"dependency steward publisher failed: {redact(str(exc))}", file=sys.stderr)
        raise SystemExit(2)
