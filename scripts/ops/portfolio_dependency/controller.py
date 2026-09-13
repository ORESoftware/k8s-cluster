"""Portfolio scheduling, reporting, and command-line orchestration."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import tempfile
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import MutableMapping, Sequence

from .model import (
    DEFAULT_POLICY,
    DEFAULT_REGISTRY,
    BotError,
    RunReport,
    RuntimeState,
    load_policy,
    load_portfolio_projects,
    safe_slug,
)
from .providers import GitHubClient, LinearClient, RepairClient, WorkerClient
from .reconcile import process_organization

def write_report(report: RunReport, json_path: Path, dot_path: Path, markdown_path: Path) -> None:
    json_path.parent.mkdir(parents=True, exist_ok=True)
    value = report.json_value()
    json_path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    lines = ["digraph portfolio_dependencies {", "  rankdir=LR;"]
    for node in sorted(report.nodes):
        lines.append(f"  {json.dumps(node)};")
    for edge in report.edges:
        target = edge.target_repo or edge.target_url or f"unresolved:{edge.dependency_key}"
        label = f"{edge.kind}:{edge.dependency_key}"
        lines.append(
            f"  {json.dumps(edge.source_repo)} -> {json.dumps(target)} [label={json.dumps(label)}];"
        )
    lines.append("}")
    dot_path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    counts: MutableMapping[str, int] = defaultdict(int)
    for result in report.results:
        counts[result.status] += 1
    markdown = [
        "# Nightly portfolio dependency report",
        "",
        f"- Mode: `{report.mode}`",
        f"- Organizations: `{len(report.organizations)}`",
        f"- Graph nodes: `{len(report.nodes)}`",
        f"- Graph edges: `{len(report.edges)}`",
        f"- Errors: `{len(report.errors)}`",
        "",
        "## Result counts",
        "",
    ]
    if counts:
        markdown.extend(f"- `{key}`: {value}" for key, value in sorted(counts.items()))
    else:
        markdown.append("- No update evaluation was performed.")
    if report.results:
        markdown.extend(("", "## Changes and blockers", ""))
        for result in report.results:
            target = result.pull_request_url or result.linear_issue_url or result.detail
            markdown.append(
                f"- `{result.status}` — `{result.source_repo}` / `{result.dependency_key}`: {target}"
            )
    if report.errors:
        markdown.extend(("", "## Errors", ""))
        markdown.extend(f"- {error}" for error in report.errors[:200])
    markdown_path.write_text("\n".join(markdown) + "\n", encoding="utf-8")


def credential_free_validation(
    policy_path: Path,
    registry_path: Path,
    json_path: Path,
    dot_path: Path,
    markdown_path: Path,
) -> int:
    policy = load_policy(policy_path)
    projects = load_portfolio_projects(registry_path)
    report = RunReport(
        started_at=datetime.now(timezone.utc).isoformat(),
        mode="validate",
        organizations=[project.github_org for project in projects],
    )
    report.finished_at = datetime.now(timezone.utc).isoformat()
    write_report(report, json_path, dot_path, markdown_path)
    print(
        f"validated dependency policy for {len(projects)} organizations; "
        f"minor-only={policy.raw['updates']['allowMinor']}"
    )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    parser.add_argument("--registry", type=Path, default=DEFAULT_REGISTRY)
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--organization")
    parser.add_argument("--repository")
    parser.add_argument("--json-output", type=Path, default=Path("artifacts/portfolio-dependencies.json"))
    parser.add_argument("--dot-output", type=Path, default=Path("artifacts/portfolio-dependencies.dot"))
    parser.add_argument("--markdown-output", type=Path, default=Path("artifacts/portfolio-dependencies.md"))
    args = parser.parse_args(argv)

    if args.validate_only or not args.apply:
        return credential_free_validation(
            args.policy,
            args.registry,
            args.json_output,
            args.dot_output,
            args.markdown_output,
        )

    policy = load_policy(args.policy)
    projects = load_portfolio_projects(args.registry)
    if args.organization:
        projects = [
            project
            for project in projects
            if project.github_org.lower() == args.organization.lower()
        ]
        if not projects:
            raise BotError(f"organization is not in the canonical registry: {args.organization}")

    token = os.environ.get("PORTFOLIO_DEPENDENCY_GITHUB_TOKEN", "").strip()
    linear_token = os.environ.get("LINEAR_API_KEY", "").strip()
    worker_url = os.environ.get("GHA_INDIE_WORKER_URL", "").strip()
    worker_auth = os.environ.get("GHA_INDIE_WORKER_AUTH", "").strip()
    missing = [
        name
        for name, value in (
            ("PORTFOLIO_DEPENDENCY_GITHUB_TOKEN", token),
            ("LINEAR_API_KEY", linear_token),
            ("GHA_INDIE_WORKER_URL", worker_url),
            ("GHA_INDIE_WORKER_AUTH", worker_auth),
        )
        if not value
    ]
    if missing:
        raise BotError("apply mode is missing protected credentials: " + ", ".join(missing))

    github = GitHubClient(token)
    linear = LinearClient(linear_token)
    worker = WorkerClient(worker_url, worker_auth, policy)
    repair_endpoint = os.environ.get("DEPENDENCY_REPAIR_ENDPOINT", "").strip()
    repair_token = os.environ.get("DEPENDENCY_REPAIR_TOKEN", "").strip() or None
    repair = RepairClient(repair_endpoint, repair_token) if repair_endpoint else None

    now = datetime.now(timezone.utc)
    run_id = os.environ.get("GITHUB_RUN_ID") or now.strftime("%Y%m%dT%H%M%SZ")
    report = RunReport(
        started_at=now.isoformat(),
        mode="apply",
        organizations=[project.github_org for project in projects],
    )
    state = RuntimeState(report, policy.maximum_pull_requests)

    with tempfile.TemporaryDirectory(prefix="portfolio-deps-") as temporary:
        root = Path(temporary)
        with concurrent.futures.ThreadPoolExecutor(
            max_workers=policy.org_concurrency,
            thread_name_prefix="portfolio-org",
        ) as pool:
            futures = [
                pool.submit(
                    process_organization,
                    project=project,
                    github=github,
                    linear=linear,
                    worker=worker,
                    repair=repair,
                    policy=policy,
                    state=state,
                    work_root=root / safe_slug(project.github_org, 80),
                    token=token,
                    run_id=run_id,
                    repository_filter=args.repository,
                )
                for project in projects
            ]
            for future in concurrent.futures.as_completed(futures):
                try:
                    future.result()
                except Exception as exc:
                    state.add_error(f"organization worker crashed: {exc}")

    report.finished_at = datetime.now(timezone.utc).isoformat()
    write_report(report, args.json_output, args.dot_output, args.markdown_output)
    return 1 if report.errors else 0
