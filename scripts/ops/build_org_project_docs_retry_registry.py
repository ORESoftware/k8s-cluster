#!/usr/bin/env python3
"""Build a canonical retry registry from fail-closed fleet reconciliation evidence."""

from __future__ import annotations

import argparse
import csv
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ORG_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
LINEAR_RE = re.compile(r"^https://linear\.app/[A-Za-z0-9_-]+/project/[A-Za-z0-9._~-]+$")


class RetryPlanError(RuntimeError):
    """Raised when the registry or audit cannot produce trustworthy retries."""


@dataclass(frozen=True)
class RegistryRow:
    organization: str
    linear_url: str


def load_registry(path: Path, *, expected_count: int | None = None) -> list[RegistryRow]:
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        if reader.fieldnames != ["organization", "linear_url"]:
            raise RetryPlanError(
                "registry header must be organization<TAB>linear_url, "
                f"got {reader.fieldnames!r}"
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
        raise RetryPlanError(
            f"registry contains {len(rows)} organizations, expected {expected_count}"
        )

    seen: set[str] = set()
    for row in rows:
        if not ORG_RE.fullmatch(row.organization):
            raise RetryPlanError(f"invalid organization login: {row.organization!r}")
        key = row.organization.casefold()
        if key in seen:
            raise RetryPlanError(f"duplicate organization login: {row.organization}")
        seen.add(key)
        if not LINEAR_RE.fullmatch(row.linear_url):
            raise RetryPlanError(
                f"invalid Linear project URL for {row.organization}: {row.linear_url!r}"
            )
    return rows


def load_audit(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise RetryPlanError("audit must be a JSON object")
    if payload.get("schema_version") != 1:
        raise RetryPlanError(
            f"unsupported audit schema_version: {payload.get('schema_version')!r}"
        )
    if not isinstance(payload.get("is_valid"), bool):
        raise RetryPlanError("audit is_valid must be boolean")
    return payload


def _string_list(payload: dict[str, Any], field: str) -> list[str]:
    value = payload.get(field, [])
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise RetryPlanError(f"audit {field} must be an array of organization strings")
    return value


def build_retry_plan(
    rows: list[RegistryRow], audit: dict[str, Any]
) -> tuple[list[RegistryRow], dict[str, Any]]:
    expected_records = audit.get("expected_records")
    if expected_records != len(rows):
        raise RetryPlanError(
            "audit expected_records does not match registry: "
            f"{expected_records!r} != {len(rows)}"
        )

    unexpected = _string_list(audit, "unexpected_requested_orgs")
    if unexpected:
        raise RetryPlanError(
            "audit contains unexpected requested organizations: "
            + ", ".join(sorted(unexpected, key=str.casefold))
        )

    invalid = audit.get("invalid", [])
    if not isinstance(invalid, list):
        raise RetryPlanError("audit invalid must be an array")

    invalid_orgs: list[str] = []
    for index, item in enumerate(invalid):
        if not isinstance(item, dict):
            raise RetryPlanError(f"audit invalid[{index}] must be an object")
        requested = item.get("requested_org")
        if not isinstance(requested, str) or not requested:
            raise RetryPlanError(f"audit invalid[{index}] is missing requested_org")
        invalid_orgs.append(requested)

    missing_orgs = _string_list(audit, "missing_requested_orgs")
    candidates = [
        *(("invalid", organization) for organization in invalid_orgs),
        *(("missing", organization) for organization in missing_orgs),
    ]

    registry_by_key = {row.organization.casefold(): row for row in rows}
    reasons_by_key: dict[str, set[str]] = {}
    seen_by_source: set[tuple[str, str]] = set()
    for source, organization in candidates:
        if not ORG_RE.fullmatch(organization):
            raise RetryPlanError(
                f"audit {source} organization is malformed: {organization!r}"
            )
        key = organization.casefold()
        source_key = (source, key)
        if source_key in seen_by_source:
            raise RetryPlanError(f"audit repeats {source} organization: {organization}")
        seen_by_source.add(source_key)
        if key not in registry_by_key:
            raise RetryPlanError(
                f"audit {source} organization is not in the registry: {organization}"
            )
        reasons_by_key.setdefault(key, set()).add(source)

    audit_is_valid = bool(audit["is_valid"])
    if audit_is_valid and reasons_by_key:
        raise RetryPlanError("valid audit cannot contain invalid or missing organizations")
    if not audit_is_valid and not reasons_by_key:
        raise RetryPlanError("invalid audit produced an empty retry set")

    retry_rows = [row for row in rows if row.organization.casefold() in reasons_by_key]
    retry_keys = {row.organization.casefold() for row in retry_rows}
    if retry_keys != set(reasons_by_key):
        raise RetryPlanError("retry set and canonical registry selection diverged")

    summary = {
        "schema_version": 1,
        "audit_is_valid": audit_is_valid,
        "registry_records": len(rows),
        "retry_records": len(retry_rows),
        "retry_organizations": [row.organization for row in retry_rows],
        "reason_counts": {
            "invalid": sum("invalid" in reasons for reasons in reasons_by_key.values()),
            "missing": sum("missing" in reasons for reasons in reasons_by_key.values()),
        },
    }
    return retry_rows, summary


def write_registry(path: Path, rows: list[RegistryRow]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle, delimiter="\t", lineterminator="\n")
        writer.writerow(["organization", "linear_url"])
        writer.writerows((row.organization, row.linear_url) for row in rows)
    temporary.replace(path)


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Build a canonical retry registry from org-project-docs audit evidence"
    )
    parser.add_argument(
        "--registry",
        default="ops/portfolio/github-linear-project-registry.tsv",
    )
    parser.add_argument("--audit", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--summary")
    parser.add_argument("--expected-count", type=int, default=64)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        rows = load_registry(Path(args.registry), expected_count=args.expected_count)
        audit = load_audit(Path(args.audit))
        retry_rows, summary = build_retry_plan(rows, audit)
        write_registry(Path(args.output), retry_rows)
        if args.summary:
            write_json(Path(args.summary), summary)
        print(
            "RETRY_REGISTRY "
            f"selected={len(retry_rows)} fleet={len(rows)} "
            f"audit_valid={str(audit['is_valid']).lower()}"
        )
        return 0
    except (RetryPlanError, OSError, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
