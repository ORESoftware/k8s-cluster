#!/usr/bin/env python3
"""Nightly, portfolio-wide dependency graph and minor-update controller.

Patch-only releases are ignored, minor releases and reviewed branch-tip advancement
are tested, and major releases always become Linear planning work.
"""

from __future__ import annotations

import sys

from portfolio_dependency.controller import main
from portfolio_dependency.model import (
    BotError,
    Candidate,
    DependencyEdge,
    Policy,
    Repository,
    SemVer,
    find_highest_passing,
    load_policy,
    managed_marker,
    normalize_github_repo_url,
    pr_is_managed,
    select_semver_candidates,
    validate_policy,
    worker_repository_url,
)
from portfolio_dependency.workspace import (
    detect_profile,
    discover_edges,
    replace_zpkg_git_pin,
    replace_zpkg_version,
)

__all__ = [
    "BotError",
    "Candidate",
    "DependencyEdge",
    "Policy",
    "Repository",
    "SemVer",
    "detect_profile",
    "discover_edges",
    "find_highest_passing",
    "load_policy",
    "main",
    "managed_marker",
    "normalize_github_repo_url",
    "pr_is_managed",
    "replace_zpkg_git_pin",
    "replace_zpkg_version",
    "select_semver_candidates",
    "validate_policy",
    "worker_repository_url",
]


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BotError as exc:
        print(f"portfolio dependency bot: {exc}", file=sys.stderr)
        raise SystemExit(2)
