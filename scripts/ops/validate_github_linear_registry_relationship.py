#!/usr/bin/env python3
"""Validate the relationship between the 64-org and 41-portfolio registries.

The governance registry answers "which GitHub organizations are managed and
which Linear project owns each organization?". The portfolio registry is the
richer subset used by ChatGPT, GitHub Projects v2, Linear, and Slack routing.
Every portfolio row must resolve to exactly one governance row and must reuse
its Linear project URL verbatim.
"""

from __future__ import annotations

import argparse
import csv
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlparse

DEFAULT_GOVERNANCE_REGISTRY = Path(
    "ops/portfolio/github-linear-project-registry.tsv"
)
DEFAULT_PORTFOLIO_REGISTRY = Path("ops/registries/portfolio-project-links.csv")
EXPECTED_GOVERNANCE_COUNT = 64
EXPECTED_PORTFOLIO_COUNT = 41

ORG_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
LINEAR_PROJECT_PATH_RE = re.compile(r"^/denman/project/[A-Za-z0-9._~-]+$")
PROJECT_NUMBER_EXCEPTION = {"dancing-dragons": 4}
CREDENTIAL_PATTERNS = (
    re.compile(r"ghp_[A-Za-z0-9]{20,}"),
    re.compile(r"github_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"-----BEGIN (?:RSA )?PRIVATE KEY-----"),
)


class RegistryRelationshipError(RuntimeError):
    """Raised when the two canonical registries disagree."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--governance-registry",
        type=Path,
        default=DEFAULT_GOVERNANCE_REGISTRY,
    )
    parser.add_argument(
        "--portfolio-registry",
        type=Path,
        default=DEFAULT_PORTFOLIO_REGISTRY,
    )
    parser.add_argument(
        "--expected-governance-count",
        type=int,
        default=EXPECTED_GOVERNANCE_COUNT,
    )
    parser.add_argument(
        "--expected-portfolio-count",
        type=int,
        default=EXPECTED_PORTFOLIO_COUNT,
    )
    parser.add_argument("--json-output", type=Path)
    return parser.parse_args()


def reject_credentials(path: Path, raw: str) -> None:
    for pattern in CREDENTIAL_PATTERNS:
        if pattern.search(raw):
            raise RegistryRelationshipError(
                f"{path}: credential-shaped material is forbidden"
            )


def validate_linear_url(value: str, *, context: str) -> None:
    parsed = urlparse(value)
    if (
        parsed.scheme != "https"
        or parsed.netloc != "linear.app"
        or not LINEAR_PROJECT_PATH_RE.fullmatch(parsed.path)
        or parsed.params
        or parsed.query
        or parsed.fragment
    ):
        raise RegistryRelationshipError(
            f"{context}: invalid Linear project URL: {value!r}"
        )


def load_governance_registry(
    path: Path,
    *,
    expected_count: int = EXPECTED_GOVERNANCE_COUNT,
) -> dict[str, dict[str, str]]:
    raw = path.read_text(encoding="utf-8")
    reject_credentials(path, raw)
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        if reader.fieldnames != ["organization", "linear_url"]:
            raise RegistryRelationshipError(
                f"{path}: header must be organization<TAB>linear_url"
            )
        rows = list(reader)

    if len(rows) != expected_count:
        raise RegistryRelationshipError(
            f"{path}: expected {expected_count} organizations, found {len(rows)}"
        )

    by_org: dict[str, dict[str, str]] = {}
    observed_order: list[str] = []
    for line_number, row in enumerate(rows, start=2):
        organization = (row.get("organization") or "").strip()
        linear_url = (row.get("linear_url") or "").strip()
        if not ORG_RE.fullmatch(organization) or "--" in organization:
            raise RegistryRelationshipError(
                f"{path}:{line_number}: invalid GitHub organization {organization!r}"
            )
        validate_linear_url(
            linear_url,
            context=f"{path}:{line_number} {organization}",
        )
        key = organization.casefold()
        if key in by_org:
            raise RegistryRelationshipError(
                f"{path}:{line_number}: duplicate organization {organization!r}"
            )
        by_org[key] = {
            "organization": organization,
            "linear_url": linear_url,
        }
        observed_order.append(key)

    if observed_order != sorted(observed_order):
        raise RegistryRelationshipError(
            f"{path}: organizations must be sorted case-insensitively"
        )
    return by_org


def load_portfolio_registry(
    path: Path,
    *,
    expected_count: int = EXPECTED_PORTFOLIO_COUNT,
) -> list[dict[str, str]]:
    raw = path.read_text(encoding="utf-8")
    reject_credentials(path, raw)
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        required = {
            "portfolio_key",
            "github_org",
            "github_project_number",
            "github_project_title",
            "github_project_url",
            "linear_project_id",
            "linear_project_name",
            "linear_project_url",
            "slack_workspace_id",
            "slack_channel_id",
            "slack_channel_name",
            "slack_channel_url",
        }
        if reader.fieldnames is None or not required.issubset(reader.fieldnames):
            missing = sorted(required - set(reader.fieldnames or []))
            raise RegistryRelationshipError(
                f"{path}: missing required portfolio columns: {missing}"
            )
        rows = [
            {key: (value or "").strip() for key, value in row.items()}
            for row in reader
        ]

    if len(rows) != expected_count:
        raise RegistryRelationshipError(
            f"{path}: expected {expected_count} portfolios, found {len(rows)}"
        )
    return rows


def validate_relationship(
    governance_path: Path = DEFAULT_GOVERNANCE_REGISTRY,
    portfolio_path: Path = DEFAULT_PORTFOLIO_REGISTRY,
    *,
    expected_governance_count: int = EXPECTED_GOVERNANCE_COUNT,
    expected_portfolio_count: int = EXPECTED_PORTFOLIO_COUNT,
) -> dict[str, object]:
    governance = load_governance_registry(
        governance_path,
        expected_count=expected_governance_count,
    )
    portfolio = load_portfolio_registry(
        portfolio_path,
        expected_count=expected_portfolio_count,
    )

    seen_keys: set[str] = set()
    seen_orgs: set[str] = set()
    for line_number, row in enumerate(portfolio, start=2):
        portfolio_key = row["portfolio_key"]
        github_org = row["github_org"]
        normalized_org = github_org.casefold()
        if not ORG_RE.fullmatch(github_org) or "--" in github_org:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: invalid github_org {github_org!r}"
            )
        if portfolio_key != normalized_org:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: portfolio_key {portfolio_key!r} "
                f"must equal case-folded github_org {normalized_org!r}"
            )
        if portfolio_key in seen_keys:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: duplicate portfolio_key {portfolio_key!r}"
            )
        if normalized_org in seen_orgs:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: duplicate github_org {github_org!r}"
            )
        seen_keys.add(portfolio_key)
        seen_orgs.add(normalized_org)

        governance_row = governance.get(normalized_org)
        if governance_row is None:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: {github_org} is absent from the "
                "64-organization governance registry"
            )
        if row["linear_project_url"] != governance_row["linear_url"]:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: Linear URL differs from governance "
                f"registry for {github_org}"
            )
        validate_linear_url(
            row["linear_project_url"],
            context=f"{portfolio_path}:{line_number} {github_org}",
        )

        expected_number = PROJECT_NUMBER_EXCEPTION.get(portfolio_key, 1)
        if row["github_project_number"] != str(expected_number):
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: expected GitHub Project "
                f"#{expected_number} for {github_org}"
            )
        expected_title = f"{github_org}-project"
        if row["github_project_title"] != expected_title:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: expected project title "
                f"{expected_title!r}"
            )
        expected_url = (
            f"https://github.com/orgs/{github_org}/projects/{expected_number}"
        )
        if row["github_project_url"] != expected_url:
            raise RegistryRelationshipError(
                f"{portfolio_path}:{line_number}: expected project URL {expected_url!r}"
            )

    governance_only = sorted(set(governance) - seen_orgs)
    return {
        "schema_version": 1,
        "governance_organizations": len(governance),
        "active_portfolios": len(portfolio),
        "governance_only_organizations": len(governance_only),
        "governance_only_logins": [
            governance[key]["organization"] for key in governance_only
        ],
        "relationship_valid": True,
    }


def main() -> int:
    args = parse_args()
    try:
        report = validate_relationship(
            args.governance_registry,
            args.portfolio_registry,
            expected_governance_count=args.expected_governance_count,
            expected_portfolio_count=args.expected_portfolio_count,
        )
    except (OSError, RegistryRelationshipError) as error:
        print(f"GitHub/Linear registry relationship validation failed: {error}", file=sys.stderr)
        return 1

    text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.json_output:
        args.json_output.parent.mkdir(parents=True, exist_ok=True)
        args.json_output.write_text(text, encoding="utf-8")
    print(
        "validated "
        f"{report['active_portfolios']} active portfolios within "
        f"{report['governance_organizations']} governance organizations"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
