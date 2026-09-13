#!/usr/bin/env python3
"""Wait for a bounded GitHub REST/GraphQL API budget without consuming it."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any


class BudgetError(RuntimeError):
    pass


@dataclass(frozen=True)
class RateBudget:
    core_remaining: int
    core_reset: int
    graphql_remaining: int
    graphql_reset: int


def _bounded_int(value: Any, *, field: str) -> int:
    if isinstance(value, bool):
        raise BudgetError(f"{field} must be an integer")
    try:
        parsed = int(value)
    except (TypeError, ValueError) as error:
        raise BudgetError(f"{field} must be an integer") from error
    if parsed < 0:
        raise BudgetError(f"{field} must not be negative")
    return parsed


def parse_budget(payload: Any) -> RateBudget:
    try:
        resources = payload["resources"]
        core = resources["core"]
        graphql = resources["graphql"]
    except (KeyError, TypeError) as error:
        raise BudgetError("GitHub rate-limit response is missing core/graphql resources") from error

    budget = RateBudget(
        core_remaining=_bounded_int(core.get("remaining"), field="core.remaining"),
        core_reset=_bounded_int(core.get("reset"), field="core.reset"),
        graphql_remaining=_bounded_int(
            graphql.get("remaining"), field="graphql.remaining"
        ),
        graphql_reset=_bounded_int(graphql.get("reset"), field="graphql.reset"),
    )
    if budget.core_reset == 0 or budget.graphql_reset == 0:
        raise BudgetError("GitHub rate-limit reset epochs must be positive")
    return budget


def get_budget(*, timeout_seconds: int = 60) -> RateBudget:
    completed = subprocess.run(
        ["gh", "api", "rate_limit"],
        text=True,
        capture_output=True,
        timeout=timeout_seconds,
        check=False,
    )
    if completed.returncode != 0:
        raise BudgetError(
            f"gh api rate_limit failed with exit status {completed.returncode}"
        )
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise BudgetError("gh api rate_limit returned invalid JSON") from error
    return parse_budget(payload)


def wait_for_budget(
    *,
    min_core: int,
    min_graphql: int,
    max_wait_seconds: int,
    poll_seconds: int = 120,
) -> RateBudget:
    for value, name in (
        (min_core, "min_core"),
        (min_graphql, "min_graphql"),
        (max_wait_seconds, "max_wait_seconds"),
        (poll_seconds, "poll_seconds"),
    ):
        if value < 0:
            raise BudgetError(f"{name} must not be negative")
    if poll_seconds < 1:
        raise BudgetError("poll_seconds must be at least 1")

    started = time.monotonic()
    while True:
        budget = get_budget()
        if (
            budget.core_remaining >= min_core
            and budget.graphql_remaining >= min_graphql
        ):
            print(
                "RATE_BUDGET_READY "
                f"core={budget.core_remaining} graphql={budget.graphql_remaining}",
                flush=True,
            )
            return budget

        now_epoch = int(time.time())
        required_resets: list[int] = []
        if budget.core_remaining < min_core:
            required_resets.append(budget.core_reset)
        if budget.graphql_remaining < min_graphql:
            required_resets.append(budget.graphql_reset)
        reset_epoch = max(required_resets) if required_resets else now_epoch + poll_seconds
        until_reset = max(5, reset_epoch - now_epoch + 10)
        sleep_seconds = min(poll_seconds, until_reset)
        elapsed = int(time.monotonic() - started)
        if elapsed + sleep_seconds > max_wait_seconds:
            raise BudgetError(
                "GitHub API budget did not recover before timeout: "
                f"core={budget.core_remaining}/{min_core}, "
                f"graphql={budget.graphql_remaining}/{min_graphql}, "
                f"waited={elapsed}s"
            )
        print(
            "WAIT_RATE_BUDGET "
            f"core={budget.core_remaining}/{min_core} "
            f"graphql={budget.graphql_remaining}/{min_graphql} "
            f"sleep={sleep_seconds}s "
            f"reset={datetime.fromtimestamp(reset_epoch, timezone.utc).isoformat()}",
            flush=True,
        )
        time.sleep(sleep_seconds)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Wait for sufficient GitHub API budget before fleet mutation"
    )
    parser.add_argument("--min-core", type=int, required=True)
    parser.add_argument("--min-graphql", type=int, required=True)
    parser.add_argument("--max-wait-seconds", type=int, default=10800)
    parser.add_argument("--poll-seconds", type=int, default=120)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        wait_for_budget(
            min_core=args.min_core,
            min_graphql=args.min_graphql,
            max_wait_seconds=args.max_wait_seconds,
            poll_seconds=args.poll_seconds,
        )
        return 0
    except (BudgetError, subprocess.TimeoutExpired) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
