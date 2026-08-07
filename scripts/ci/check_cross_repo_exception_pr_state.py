#!/usr/bin/env python3
"""Fail when a temporary workflow-ref exception outlives its owning PR."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


class CheckError(Exception):
    pass


def load_ledger(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CheckError(f"cannot load ledger {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise CheckError("ledger root must be an object")
    exceptions = value.get("feature_ref_exceptions")
    if not isinstance(exceptions, list):
        raise CheckError("feature_ref_exceptions must be an array")
    return value


def github_get_json(url: str, token: str, timeout: float) -> dict[str, Any]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "User-Agent": "k8s-cluster-cross-repo-drift",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            if response.status != 200:
                raise CheckError(f"GitHub returned HTTP {response.status} for {url}")
            body = response.read(1_048_577)
    except urllib.error.HTTPError as exc:
        raise CheckError(f"GitHub returned HTTP {exc.code} for {url}") from exc
    except (urllib.error.URLError, TimeoutError) as exc:
        raise CheckError(f"GitHub request failed for {url}: {exc}") from exc
    if len(body) > 1_048_576:
        raise CheckError(f"GitHub response exceeded 1 MiB for {url}")
    try:
        value = json.loads(body)
    except json.JSONDecodeError as exc:
        raise CheckError(f"GitHub returned invalid JSON for {url}") from exc
    if not isinstance(value, dict):
        raise CheckError(f"GitHub returned a non-object for {url}")
    return value


def check_exception_prs(
    ledger: dict[str, Any], api_base: str, token: str, timeout: float
) -> list[str]:
    findings: list[str] = []
    for row in ledger["feature_ref_exceptions"]:
        if not isinstance(row, dict):
            raise CheckError("feature_ref_exceptions entries must be objects")
        repository = row.get("repository")
        owning_pr = row.get("owning_pr")
        if repository is None and owning_pr is None:
            continue
        workflow = row.get("workflow", "<unknown workflow>")
        if not isinstance(repository, str) or repository.count("/") != 1:
            raise CheckError(f"{workflow}: repository is required with owning_pr")
        if not isinstance(owning_pr, int) or owning_pr <= 0:
            raise CheckError(f"{workflow}: owning_pr must be a positive integer")
        url = f"{api_base.rstrip('/')}/repos/{repository}/pulls/{owning_pr}"
        pull = github_get_json(url, token, timeout)
        state = pull.get("state")
        merged_at = pull.get("merged_at")
        html_url = pull.get("html_url")
        if state != "open" or merged_at is not None:
            findings.append(
                f"{workflow}: temporary exception outlived {repository}#{owning_pr} "
                f"(state={state!r}, merged_at={merged_at!r}, url={html_url!r})"
            )
    return findings


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ledger", required=True, type=Path)
    parser.add_argument(
        "--api-base",
        default=os.environ.get("GITHUB_API_URL", "https://api.github.com"),
    )
    parser.add_argument("--token-env", default="GITHUB_TOKEN")
    parser.add_argument("--timeout", type=float, default=10.0)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    token = os.environ.get(args.token_env, "").strip()
    if not token:
        print(f"missing GitHub token environment variable {args.token_env}", file=sys.stderr)
        return 2
    try:
        findings = check_exception_prs(
            load_ledger(args.ledger), args.api_base, token, args.timeout
        )
    except CheckError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    if findings:
        for finding in findings:
            print(finding, file=sys.stderr)
        return 1
    print("all PR-owned workflow-ref exceptions still have open owning PRs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
