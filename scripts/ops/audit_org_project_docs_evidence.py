#!/usr/bin/env python3
"""Audit organization Project reconciliation evidence without network access."""

from __future__ import annotations

import argparse
import collections
import json
import re
import sys
from pathlib import Path
from typing import Any
from urllib.parse import quote

ORG_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$")
INTEGER_RE = re.compile(r"^[1-9][0-9]*$")


def read_registry(path: Path) -> list[str]:
    rows: list[str] = []
    for index, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
        if not line.strip():
            continue
        parts = line.split("\t", 1)
        if index == 0 and parts[0] == "organization":
            continue
        if len(parts) != 2 or not parts[0] or not parts[1].startswith("https://linear.app/"):
            raise ValueError(f"malformed registry row {index + 1}")
        rows.append(parts[0])
    if len(rows) != len(set(value.casefold() for value in rows)):
        raise ValueError("registry contains duplicate organization identities")
    return rows


def validate_record(record: dict[str, Any]) -> list[str]:
    reasons: list[str] = []
    requested = str(record.get("requested_org", ""))
    canonical = str(record.get("canonical_org", ""))
    title = str(record.get("project_title", ""))
    number = str(record.get("project_number", ""))
    project_url = str(record.get("project_url", ""))
    status = str(record.get("status", ""))

    if status != "ok":
        reasons.append(f"status:{status or 'missing'}")
    if not ORG_RE.fullmatch(canonical):
        reasons.append("invalid-canonical-org")
    elif requested.casefold() != canonical.casefold():
        reasons.append("requested-canonical-mismatch")
    if title != f"{canonical}-project":
        reasons.append("invalid-project-title")
    if not INTEGER_RE.fullmatch(number):
        reasons.append("invalid-project-number")
    expected_url = (
        f"https://github.com/orgs/{quote(canonical, safe='')}/projects/{number}"
        if ORG_RE.fullmatch(canonical) and INTEGER_RE.fullmatch(number)
        else ""
    )
    if not expected_url or project_url.casefold() != expected_url.casefold():
        reasons.append("invalid-project-url")

    repository_action = str(record.get("repository_action", ""))
    if not repository_action or repository_action == "unknown":
        reasons.append("invalid-repository-action")

    documentation_action = str(record.get("documentation_action", ""))
    pull_request = record.get("pull_request") or {}
    pr_number = str(pull_request.get("number", ""))
    pr_url = str(pull_request.get("url", ""))
    pr_state = str(pull_request.get("state", ""))
    if documentation_action == "updated":
        if not INTEGER_RE.fullmatch(pr_number):
            reasons.append("missing-documentation-pr-number")
        if not pr_url.startswith(f"https://github.com/{canonical}/.github/pull/"):
            reasons.append("invalid-documentation-pr-url")
        if not (pr_state.startswith("merged-") or pr_state == "auto-merge-enabled"):
            reasons.append("invalid-documentation-pr-state")
    elif documentation_action == "unchanged":
        if pr_state != "not-needed":
            reasons.append("invalid-unchanged-pr-state")
    else:
        reasons.append("invalid-documentation-action")

    issue = record.get("governance_issue") or {}
    issue_number = str(issue.get("number", ""))
    issue_url = str(issue.get("url", ""))
    item_action = str(issue.get("project_item_action", ""))
    if not INTEGER_RE.fullmatch(issue_number):
        reasons.append("missing-governance-issue-number")
    if not issue_url.startswith(f"https://github.com/{canonical}/.github/issues/"):
        reasons.append("invalid-governance-issue-url")
    if item_action not in {"added", "existing"}:
        reasons.append("invalid-project-item-action")
    if str(record.get("error", "")):
        reasons.append("unexpected-error-text")
    return sorted(set(reasons))


def build_audit(registry: list[str], records: list[dict[str, Any]]) -> dict[str, Any]:
    expected = {org.casefold(): org for org in registry}
    observed_counts = collections.Counter(
        str(record.get("requested_org", "")).casefold() for record in records
    )
    invalid: list[dict[str, Any]] = []
    valid: list[dict[str, Any]] = []
    reason_counts: collections.Counter[str] = collections.Counter()

    for record in records:
        reasons = validate_record(record)
        requested = str(record.get("requested_org", ""))
        if requested.casefold() not in expected:
            reasons.append("unexpected-requested-org")
        if observed_counts[requested.casefold()] != 1:
            reasons.append("duplicate-requested-org")
        reasons = sorted(set(reasons))
        if reasons:
            invalid.append({"requested_org": requested, "reasons": reasons})
            reason_counts.update(reasons)
        else:
            valid.append(record)

    observed = set(observed_counts)
    missing = [expected[key] for key in sorted(set(expected) - observed)]
    unexpected = sorted(
        str(record.get("requested_org", ""))
        for record in records
        if str(record.get("requested_org", "")).casefold() not in expected
    )

    action_fields = {
        "project_actions": "project_action",
        "repository_actions": "repository_action",
        "documentation_actions": "documentation_action",
        "project_item_actions": None,
        "pull_request_states": None,
    }
    action_counts: dict[str, dict[str, int]] = {}
    for label, field in action_fields.items():
        if field:
            values = [str(row.get(field, "")) for row in valid]
        elif label == "project_item_actions":
            values = [str((row.get("governance_issue") or {}).get("project_item_action", "")) for row in valid]
        else:
            values = [str((row.get("pull_request") or {}).get("state", "")) for row in valid]
        action_counts[label] = dict(sorted(collections.Counter(values).items()))

    return {
        "schema_version": 1,
        "expected_records": len(registry),
        "observed_records": len(records),
        "valid_records": len(valid),
        "invalid_records": len(invalid),
        "missing_requested_orgs": missing,
        "unexpected_requested_orgs": unexpected,
        "invalid_reason_counts": dict(sorted(reason_counts.items())),
        "invalid": sorted(invalid, key=lambda row: row["requested_org"].casefold()),
        "valid_action_counts": action_counts,
        "is_valid": not invalid and not missing and not unexpected and len(records) == len(registry),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--allow-invalid", action="store_true")
    args = parser.parse_args()

    registry = read_registry(args.registry)
    records = json.loads(args.evidence.read_text(encoding="utf-8"))
    if not isinstance(records, list) or not all(isinstance(row, dict) for row in records):
        raise ValueError("evidence must be an array of objects")
    audit = build_audit(registry, records)
    payload = json.dumps(audit, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(payload, encoding="utf-8")
    print(payload, end="")
    return 0 if audit["is_valid"] or args.allow_invalid else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"evidence audit failed: {exc}", file=sys.stderr)
        raise SystemExit(2)
