#!/usr/bin/env python3
"""Nightly minor-only interdependency steward for the canonical GitHub portfolio.

The implementation is split into focused modules under ``dependency_steward``.
This entry point re-exports the public policy surface for tests and the isolated
publisher, then runs the portfolio controller when invoked as a script.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any, Mapping, Sequence

from dependency_steward.models import *
from dependency_steward.providers import *
from dependency_steward.runtime import *
from dependency_steward.scanners import *
from dependency_steward.operations import *
from dependency_steward.controller import *

def repository_is_eligible(item: Mapping[str, Any]) -> bool:
    return not any(
        bool(item.get(key)) for key in ("archived", "disabled", "fork", "is_template")
    ) and bool(item.get("clone_url")) and bool(item.get("default_branch"))


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path("ops/registries/portfolio-project-links.csv"),
    )
    parser.add_argument("--artifacts", type=Path, default=Path("artifacts/dependency-steward"))
    parser.add_argument("--work-root", type=Path)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--scheduled-cron")
    parser.add_argument("--workers", type=int, default=3)
    parser.add_argument("--max-repositories", type=int, default=0)
    parser.add_argument("--max-prs", type=int, default=200)
    parser.add_argument("--org", action="append", default=[])
    parser.add_argument("--allow-external", action="store_true")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    if args.workers < 1 or args.workers > 12:
        raise StewardError("--workers must be between 1 and 12")
    if args.max_prs < 1 or args.max_prs > 1000:
        raise StewardError("--max-prs must be between 1 and 1000")
    if args.scheduled_cron and not scheduled_cron_is_active(
        dt.datetime.now(dt.timezone.utc), args.scheduled_cron
    ):
        args.artifacts.mkdir(parents=True, exist_ok=True)
        (args.artifacts / "report.md").write_text(
            "# Nightly dependency steward\n\nInactive DST cron lane; clean no-op.\n",
            encoding="utf-8",
        )
        (args.artifacts / "report.json").write_text(
            json.dumps({"contract": JOB_MARKER, "status": "inactive-cron-lane"}, indent=2)
            + "\n",
            encoding="utf-8",
        )
        return 0

    if args.apply:
        raise StewardError(
            "direct apply is disabled: run analysis without write credentials, then "
            "publish with publish_nightly_dependency_steward.py"
        )
    github_token = (
        os.getenv("DEPENDENCY_STEWARD_READ_TOKEN")
        or os.getenv("DEPENDENCY_STEWARD_GITHUB_TOKEN")
        or os.getenv("PROJECT_SYNC_GITHUB_TOKEN")
    )
    if not github_token:
        raise StewardError("DEPENDENCY_STEWARD_READ_TOKEN is required")
    github_api_url = os.getenv("GITHUB_API_URL", "https://api.github.com")
    remediation_endpoint = os.getenv("DEPENDENCY_STEWARD_REMEDIATION_ENDPOINT")
    remediation_token = os.getenv("DEPENDENCY_STEWARD_REMEDIATION_TOKEN")
    remediation_command = os.getenv("DEPENDENCY_STEWARD_REMEDIATION_COMMAND")
    allow_external_env = os.getenv("DEPENDENCY_STEWARD_ALLOW_EXTERNAL") == "1"

    # Provider credentials are retained only in client objects. They are removed
    # from the inherited environment before any repository-controlled command runs.
    for key in list(os.environ):
        if sensitive_environment_key(key):
            os.environ.pop(key, None)

    portfolio = load_portfolio_registry(args.registry)
    filters = {value.lower() for value in args.org}
    env_filter = {
        value.strip().lower()
        for value in os.getenv("DEPENDENCY_STEWARD_ORGS", "").split(",")
        if value.strip()
    }
    filters.update(env_filter)
    if filters:
        portfolio = [item for item in portfolio if item.github_org.lower() in filters]
        unknown = filters - {item.github_org.lower() for item in portfolio}
        if unknown:
            raise StewardError(f"unknown filtered organizations: {sorted(unknown)}")

    github = GitHubClient(github_token, github_api_url)
    linear = LinearClient("plan-only", os.getenv("DEPENDENCY_STEWARD_LINEAR_TEAM_ID"))
    temporary: tempfile.TemporaryDirectory[str] | None = None
    if args.work_root:
        work_root = args.work_root
        work_root.mkdir(parents=True, exist_ok=True)
    else:
        temporary = tempfile.TemporaryDirectory(prefix="dependency-steward-")
        work_root = Path(temporary.name)

    controller = StewardController(
        github=github,
        linear=linear,
        portfolio=portfolio,
        work_root=work_root,
        artifacts=args.artifacts,
        apply=args.apply,
        max_prs=args.max_prs,
        remediation_endpoint=remediation_endpoint,
        remediation_token=remediation_token,
        remediation_command=remediation_command,
        allow_external=args.allow_external or allow_external_env,
    )

    repositories: list[dict[str, Any]] = []
    for link in portfolio:
        try:
            repositories.extend(
                item
                for item in github.list_org_repositories(link.github_org)
                if repository_is_eligible(item)
            )
        except Exception as exc:
            controller.record_provider_error(
                f"cannot list {link.github_org}: {redact(str(exc))}"
            )
    repositories.sort(key=lambda item: str(item.get("full_name", "")).lower())
    if args.max_repositories:
        repositories = repositories[: args.max_repositories]

    with ThreadPoolExecutor(max_workers=args.workers) as executor:
        futures = {
            executor.submit(controller.process_repository, item): str(item["full_name"])
            for item in repositories
        }
        for future in as_completed(futures):
            try:
                controller.record_summary(future.result())
            except Exception as exc:
                controller.record_provider_error(
                    f"unhandled repository failure {futures[future]}: {redact(str(exc))}"
                )

    controller.write_artifacts()
    if temporary is not None:
        temporary.cleanup()
    return 1 if controller.provider_errors else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except StewardError as exc:
        print(f"dependency steward failed: {redact(str(exc))}", file=sys.stderr)
        raise SystemExit(2)
