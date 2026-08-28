#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

ORG_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
LINEAR_RE = re.compile(r"^https://linear\.app/[A-Za-z0-9_-]+/project/[A-Za-z0-9._~-]+$")
PROJECT_URL_RE = re.compile(r"^https://github\.com/orgs/([^/]+)/projects/([1-9][0-9]*)$")
RATE_LIMIT_PATTERNS = (
    "api rate limit exceeded",
    "secondary rate limit",
    "rate limit",
    "abuse detection",
)
VALID_PROJECT_ITEM_ACTIONS = {"added", "existing"}
VALID_DOC_ACTIONS = {"updated", "unchanged"}


class ReconcileError(RuntimeError):
    pass


@dataclass(frozen=True)
class RegistryRow:
    organization: str
    linear_url: str


@dataclass(frozen=True)
class RateBudget:
    core_remaining: int
    core_reset: int
    graphql_remaining: int
    graphql_reset: int


def utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def run_command(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    timeout: int | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        args,
        env=env,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if check and completed.returncode != 0:
        raise ReconcileError(
            f"command failed ({completed.returncode}): {' '.join(args)}\n"
            f"stdout:\n{completed.stdout}\n"
            f"stderr:\n{completed.stderr}"
        )
    return completed


def gh_json(args: list[str], *, timeout: int = 120) -> Any:
    completed = run_command(["gh", *args], timeout=timeout)
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise ReconcileError(
            f"GitHub CLI returned non-JSON output for {' '.join(args)}: {completed.stdout!r}"
        ) from error


def get_rate_budget() -> RateBudget:
    payload = gh_json(["api", "rate_limit"])
    try:
        core = payload["resources"]["core"]
        graphql = payload["resources"]["graphql"]
        return RateBudget(
            core_remaining=int(core["remaining"]),
            core_reset=int(core["reset"]),
            graphql_remaining=int(graphql["remaining"]),
            graphql_reset=int(graphql["reset"]),
        )
    except (KeyError, TypeError, ValueError) as error:
        raise ReconcileError(f"malformed GitHub rate-limit response: {payload!r}") from error


def wait_for_budget(
    *,
    min_core: int,
    min_graphql: int,
    max_wait_seconds: int,
    poll_seconds: int = 120,
) -> RateBudget:
    started = time.monotonic()
    while True:
        budget = get_rate_budget()
        if (
            budget.core_remaining >= min_core
            and budget.graphql_remaining >= min_graphql
        ):
            print(
                "RATE_BUDGET "
                f"core={budget.core_remaining} "
                f"graphql={budget.graphql_remaining}",
                flush=True,
            )
            return budget

        now_epoch = int(time.time())
        resets = []
        if budget.core_remaining < min_core:
            resets.append(budget.core_reset)
        if budget.graphql_remaining < min_graphql:
            resets.append(budget.graphql_reset)
        reset_epoch = max(resets) if resets else now_epoch + poll_seconds
        until_reset = max(5, reset_epoch - now_epoch + 10)
        sleep_seconds = min(poll_seconds, until_reset)
        elapsed = int(time.monotonic() - started)
        if elapsed + sleep_seconds > max_wait_seconds:
            raise ReconcileError(
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


def load_registry(path: Path, *, expected_count: int | None = None) -> list[RegistryRow]:
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        if reader.fieldnames != ["organization", "linear_url"]:
            raise ReconcileError(
                f"registry header must be organization<TAB>linear_url, got {reader.fieldnames!r}"
            )
        rows = [
            RegistryRow(
                organization=(record.get("organization") or "").strip(),
                linear_url=(record.get("linear_url") or "").strip(),
            )
            for record in reader
            if (record.get("organization") or "").strip()
        ]

    if expected_count is not None and len(rows) != expected_count:
        raise ReconcileError(
            f"registry contains {len(rows)} organizations, expected {expected_count}"
        )

    seen: set[str] = set()
    for row in rows:
        if not ORG_RE.fullmatch(row.organization):
            raise ReconcileError(f"invalid GitHub organization login: {row.organization!r}")
        key = row.organization.casefold()
        if key in seen:
            raise ReconcileError(f"duplicate GitHub organization login: {row.organization}")
        seen.add(key)
        if not LINEAR_RE.fullmatch(row.linear_url):
            raise ReconcileError(
                f"invalid Linear project URL for {row.organization}: {row.linear_url!r}"
            )
    return rows


def validate_result(
    result: Any,
    row: RegistryRow,
    *,
    require_merged_docs: bool = True,
) -> dict[str, Any]:
    if not isinstance(result, dict):
        raise ReconcileError(f"{row.organization}: result must be an object")

    if result.get("status") != "ok":
        raise ReconcileError(
            f"{row.organization}: result status is not ok: {result.get('error')!r}"
        )
    if result.get("requested_org") != row.organization:
        raise ReconcileError(
            f"{row.organization}: requested_org mismatch: {result.get('requested_org')!r}"
        )
    if result.get("linear_url") != row.linear_url:
        raise ReconcileError(f"{row.organization}: Linear URL mismatch")

    canonical = result.get("canonical_org")
    if not isinstance(canonical, str) or not ORG_RE.fullmatch(canonical):
        raise ReconcileError(
            f"{row.organization}: canonical organization is invalid: {canonical!r}"
        )
    if canonical.casefold() != row.organization.casefold():
        raise ReconcileError(
            f"{row.organization}: canonical organization changed identity to {canonical!r}"
        )

    expected_title = f"{canonical}-project"
    if result.get("project_title") != expected_title:
        raise ReconcileError(
            f"{row.organization}: project title mismatch: {result.get('project_title')!r}"
        )
    project_number = str(result.get("project_number") or "")
    if not project_number.isdigit() or int(project_number) < 1:
        raise ReconcileError(
            f"{row.organization}: invalid project number: {project_number!r}"
        )
    project_url = result.get("project_url")
    if not isinstance(project_url, str):
        raise ReconcileError(f"{row.organization}: missing project URL")
    match = PROJECT_URL_RE.fullmatch(project_url)
    if not match:
        raise ReconcileError(
            f"{row.organization}: malformed project URL: {project_url!r}"
        )
    if match.group(1).casefold() != canonical.casefold():
        raise ReconcileError(f"{row.organization}: project URL owner mismatch")
    if match.group(2) != project_number:
        raise ReconcileError(f"{row.organization}: project URL number mismatch")

    for field in ("project_action", "repository_action", "documentation_action"):
        value = result.get(field)
        if not isinstance(value, str) or not value or value == "unknown":
            raise ReconcileError(
                f"{row.organization}: missing or unknown {field}: {value!r}"
            )
    if result["documentation_action"] not in VALID_DOC_ACTIONS:
        raise ReconcileError(
            f"{row.organization}: invalid documentation action: "
            f"{result['documentation_action']!r}"
        )

    pull_request = result.get("pull_request")
    if not isinstance(pull_request, dict):
        raise ReconcileError(f"{row.organization}: pull_request must be an object")
    pr_state = str(pull_request.get("state") or "")
    if result["documentation_action"] == "unchanged":
        if pr_state != "not-needed":
            raise ReconcileError(
                f"{row.organization}: unchanged docs require not-needed PR state"
            )
    elif require_merged_docs and not pr_state.startswith("merged-"):
        raise ReconcileError(
            f"{row.organization}: documentation PR is not merged: {pr_state!r}"
        )

    issue = result.get("governance_issue")
    if not isinstance(issue, dict):
        raise ReconcileError(f"{row.organization}: governance_issue must be an object")
    issue_number = str(issue.get("number") or "")
    if not issue_number.isdigit() or int(issue_number) < 1:
        raise ReconcileError(
            f"{row.organization}: invalid governance issue number: {issue_number!r}"
        )
    expected_issue_prefix = f"https://github.com/{canonical}/.github/issues/"
    issue_url = issue.get("url")
    if not isinstance(issue_url, str) or not issue_url.startswith(expected_issue_prefix):
        raise ReconcileError(
            f"{row.organization}: malformed governance issue URL: {issue_url!r}"
        )
    if issue.get("project_item_action") not in VALID_PROJECT_ITEM_ACTIONS:
        raise ReconcileError(
            f"{row.organization}: governance issue was not added to the Project"
        )

    error_message = result.get("error")
    if error_message not in ("", None):
        raise ReconcileError(
            f"{row.organization}: successful result contains an error: {error_message!r}"
        )

    serialized = json.dumps(result, sort_keys=True).casefold()
    if "api rate limit exceeded" in serialized:
        raise ReconcileError(
            f"{row.organization}: rate-limit or API error payload leaked into evidence"
        )

    return result


def validate_results(
    results: Iterable[Any],
    rows: list[RegistryRow],
    *,
    require_merged_docs: bool = True,
) -> list[dict[str, Any]]:
    row_by_org = {row.organization.casefold(): row for row in rows}
    validated: list[dict[str, Any]] = []
    seen: set[str] = set()
    for result in results:
        if not isinstance(result, dict):
            raise ReconcileError("aggregate result contains a non-object")
        requested = result.get("requested_org")
        if not isinstance(requested, str):
            raise ReconcileError("aggregate result is missing requested_org")
        key = requested.casefold()
        if key not in row_by_org:
            raise ReconcileError(f"aggregate result has unknown organization: {requested!r}")
        if key in seen:
            raise ReconcileError(f"duplicate aggregate result for {requested}")
        seen.add(key)
        validated.append(
            validate_result(
                result,
                row_by_org[key],
                require_merged_docs=require_merged_docs,
            )
        )

    missing = sorted(set(row_by_org) - seen)
    if missing:
        raise ReconcileError(
            f"aggregate results are incomplete: {len(validated)}/{len(rows)}; "
            f"missing={missing}"
        )
    if len(validated) != len(rows):
        raise ReconcileError(
            f"aggregate results contain {len(validated)} entries for {len(rows)} rows"
        )
    return sorted(validated, key=lambda item: item["canonical_org"].casefold())


def live_verify(result: dict[str, Any], row: RegistryRow) -> None:
    canonical = result["canonical_org"]
    org_payload = gh_json(["api", f"orgs/{row.organization}"])
    live_login = org_payload.get("login") if isinstance(org_payload, dict) else None
    if not isinstance(live_login, str) or live_login.casefold() != canonical.casefold():
        raise ReconcileError(
            f"{row.organization}: live organization lookup mismatch: {live_login!r}"
        )

    repo_payload = gh_json(["api", f"repos/{canonical}/.github"])
    if repo_payload.get("visibility") != "public":
        raise ReconcileError(f"{row.organization}: .github repository is not public")
    if repo_payload.get("has_issues") is not True:
        raise ReconcileError(f"{row.organization}: .github repository issues are disabled")
    default_branch = repo_payload.get("default_branch")
    if not isinstance(default_branch, str) or not default_branch:
        raise ReconcileError(f"{row.organization}: .github repository has no default branch")

    for path in ("docs/PROJECTS.md", "profile/README.md"):
        content = gh_json(
            ["api", f"repos/{canonical}/.github/contents/{path}?ref={default_branch}"]
        )
        if not isinstance(content, dict) or content.get("type") != "file":
            raise ReconcileError(f"{row.organization}: missing organization document {path}")

    issue_number = str(result["governance_issue"]["number"])
    issue_payload = gh_json(
        ["api", f"repos/{canonical}/.github/issues/{issue_number}"]
    )
    if issue_payload.get("state") != "open":
        raise ReconcileError(
            f"{row.organization}: governance issue {issue_number} is not open"
        )

    pull_request = result["pull_request"]
    if result["documentation_action"] == "updated":
        pr_number = str(pull_request.get("number") or "")
        pr_payload = gh_json(
            ["pr", "view", pr_number, "--repo", f"{canonical}/.github", "--json", "state,mergedAt,url"]
        )
        if pr_payload.get("state") != "MERGED" or not pr_payload.get("mergedAt"):
            raise ReconcileError(
                f"{row.organization}: documentation PR {pr_number} is not merged"
            )


def is_rate_limit_error(text: str) -> bool:
    lowered = text.casefold()
    return any(pattern in lowered for pattern in RATE_LIMIT_PATTERNS)


def write_json_atomic(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def run_one(
    row: RegistryRow,
    *,
    repository_root: Path,
    evidence_dir: Path,
    run_stamp: str,
    max_wait_seconds: int,
    per_org_timeout: int,
    retries: int,
) -> dict[str, Any]:
    checkpoint_path = evidence_dir / "checkpoints" / f"{row.organization.casefold()}.json"
    if checkpoint_path.exists():
        checkpoint = json.loads(checkpoint_path.read_text(encoding="utf-8"))
        validated = validate_result(checkpoint, row)
        live_verify(validated, row)
        print(f"RESUME_VALID {row.organization}", flush=True)
        return validated

    log_path = evidence_dir / "logs" / f"{row.organization.casefold()}.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)

    for attempt in range(1, retries + 1):
        wait_for_budget(
            min_core=100,
            min_graphql=10,
            max_wait_seconds=max_wait_seconds,
        )
        with tempfile.TemporaryDirectory(prefix="org-project-docs-") as temporary:
            temporary_path = Path(temporary)
            registry_path = temporary_path / "registry.tsv"
            registry_path.write_text(
                f"organization\tlinear_url\n{row.organization}\t{row.linear_url}\n",
                encoding="utf-8",
            )
            org_evidence = temporary_path / "evidence"
            environment = os.environ.copy()
            environment.update(
                {
                    "REGISTRY_FILE": str(registry_path),
                    "EVIDENCE_DIR": str(org_evidence),
                    "RUN_STAMP": run_stamp,
                }
            )
            completed = run_command(
                ["bash", str(repository_root / "scripts/ops/sync_org_project_docs.sh")],
                env=environment,
                timeout=per_org_timeout,
                check=False,
            )
            combined = (
                f"\n===== attempt {attempt} {utc_now()} =====\n"
                f"returncode={completed.returncode}\n"
                f"stdout:\n{completed.stdout}\n"
                f"stderr:\n{completed.stderr}\n"
            )
            with log_path.open("a", encoding="utf-8") as log:
                log.write(combined)

            result_path = org_evidence / "results.json"
            if completed.returncode == 0 and result_path.exists():
                payload = json.loads(result_path.read_text(encoding="utf-8"))
                if not isinstance(payload, list) or len(payload) != 1:
                    raise ReconcileError(
                        f"{row.organization}: expected one per-org result, got {payload!r}"
                    )
                validated = validate_result(payload[0], row)
                live_verify(validated, row)
                write_json_atomic(checkpoint_path, validated)
                print(f"VALIDATED {row.organization}", flush=True)
                return validated

            if is_rate_limit_error(combined) and attempt < retries:
                print(
                    f"RETRY_RATE_LIMIT {row.organization} attempt={attempt}",
                    flush=True,
                )
                wait_for_budget(
                    min_core=100,
                    min_graphql=10,
                    max_wait_seconds=max_wait_seconds,
                )
                continue

            raise ReconcileError(
                f"{row.organization}: reconciliation failed on attempt {attempt}; "
                f"see {log_path}"
            )

    raise ReconcileError(f"{row.organization}: retry loop exhausted")


def render_markdown(results: list[dict[str, Any]]) -> str:
    lines = [
        "# Organization Project and documentation reconciliation",
        "",
        f"Generated: `{utc_now()}`",
        "",
        "| Organization | Result | Project | Repository | Documentation PR | Governance issue | Linear |",
        "|---|---|---|---|---|---|---|",
    ]
    for item in results:
        pull_request = item["pull_request"]
        issue = item["governance_issue"]
        if pull_request.get("url"):
            pr_display = (
                f"[PR #{pull_request['number']}]({pull_request['url']}) — "
                f"{pull_request['state']}"
            )
        else:
            pr_display = item["documentation_action"]
        lines.append(
            f"| `{item['canonical_org']}` | {item['status']} "
            f"| [{item['project_title']}]({item['project_url']}) "
            f"| {item['repository_action']} "
            f"| {pr_display} "
            f"| [issue #{issue['number']}]({issue['url']}) — {issue['project_item_action']} "
            f"| [Linear]({item['linear_url']}) |"
        )
    lines.extend(
        [
            "",
            "## Totals",
            "",
            f"- Processed: {len(results)}",
            f"- Successful: {len(results)}",
            "- Failed: 0",
            "",
            "Every record passed strict organization-login, Project URL, merged documentation, "
            "open governance issue, public `.github` repository, and live GitHub API verification.",
            "",
        ]
    )
    return "\n".join(lines)


def reconcile(args: argparse.Namespace) -> int:
    if not os.environ.get("GH_TOKEN"):
        raise ReconcileError("GH_TOKEN is required")
    for command in ("gh", "git", "jq", "python3"):
        if run_command(["bash", "-lc", f"command -v {command}"], check=False).returncode != 0:
            raise ReconcileError(f"required command is missing: {command}")

    root = Path(args.repository_root).resolve()
    registry = (root / args.registry).resolve()
    evidence_dir = (root / args.evidence_dir).resolve()
    evidence_dir.mkdir(parents=True, exist_ok=True)
    rows = load_registry(registry, expected_count=args.expected_count)

    wait_for_budget(
        min_core=args.min_core_start,
        min_graphql=args.min_graphql_start,
        max_wait_seconds=args.max_wait_seconds,
    )

    results: list[dict[str, Any]] = []
    failures: list[dict[str, str]] = []
    run_stamp = args.run_stamp or datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    for index, row in enumerate(rows, start=1):
        print(f"RECONCILE {index}/{len(rows)} {row.organization}", flush=True)
        try:
            result = run_one(
                row,
                repository_root=root,
                evidence_dir=evidence_dir,
                run_stamp=run_stamp,
                max_wait_seconds=args.max_wait_seconds,
                per_org_timeout=args.per_org_timeout,
                retries=args.retries,
            )
            results.append(result)
        except Exception as error:
            message = str(error)
            print(f"FAILED {row.organization}: {message}", file=sys.stderr, flush=True)
            failures.append(
                {
                    "status": "failed",
                    "requested_org": row.organization,
                    "canonical_org": "",
                    "linear_url": row.linear_url,
                    "error": message,
                    "run_stamp": run_stamp,
                }
            )

    aggregate = sorted(
        [*results, *failures],
        key=lambda item: str(item.get("canonical_org") or item["requested_org"]).casefold(),
    )
    write_json_atomic(evidence_dir / "results.json", aggregate)
    with (evidence_dir / "results.jsonl").open("w", encoding="utf-8") as handle:
        for item in aggregate:
            handle.write(json.dumps(item, sort_keys=True) + "\n")

    if failures:
        (evidence_dir / "README.md").write_text(
            "# Organization Project and documentation reconciliation\n\n"
            f"Generated: `{utc_now()}`\n\n"
            f"- Processed: {len(aggregate)}\n"
            f"- Successful: {len(results)}\n"
            f"- Failed: {len(failures)}\n\n"
            "This run is incomplete and must not be represented as fleet completion.\n",
            encoding="utf-8",
        )
        raise ReconcileError(
            f"reconciliation incomplete: {len(results)}/{len(rows)} succeeded"
        )

    validated = validate_results(results, rows)
    write_json_atomic(evidence_dir / "results.json", validated)
    (evidence_dir / "README.md").write_text(
        render_markdown(validated),
        encoding="utf-8",
    )
    print(f"COMPLETE validated={len(validated)} expected={len(rows)}", flush=True)
    return 0


def validate_only(args: argparse.Namespace) -> int:
    root = Path(args.repository_root).resolve()
    rows = load_registry(
        (root / args.registry).resolve(),
        expected_count=args.expected_count,
    )
    payload = json.loads(
        (root / args.evidence_dir / "results.json").resolve().read_text(
            encoding="utf-8"
        )
    )
    validated = validate_results(payload, rows)
    print(f"VALID evidence={len(validated)} expected={len(rows)}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Rate-aware, fail-closed organization Project/docs reconciliation"
    )
    parser.add_argument("--repository-root", default=".")
    parser.add_argument(
        "--registry",
        default="ops/portfolio/github-linear-project-registry.tsv",
    )
    parser.add_argument(
        "--evidence-dir",
        default="ops/evidence/org-project-docs-rate-aware",
    )
    parser.add_argument("--expected-count", type=int, default=64)
    parser.add_argument("--min-core-start", type=int, default=1800)
    parser.add_argument("--min-graphql-start", type=int, default=400)
    parser.add_argument("--max-wait-seconds", type=int, default=10800)
    parser.add_argument("--per-org-timeout", type=int, default=1200)
    parser.add_argument("--retries", type=int, default=3)
    parser.add_argument("--run-stamp")
    parser.add_argument("--validate-only", action="store_true")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        if args.validate_only:
            return validate_only(args)
        return reconcile(args)
    except (ReconcileError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
