#!/usr/bin/env python3
"""Fail-closed validation for the GitHub/Linear/Project fleet registry."""

from __future__ import annotations

import argparse
import csv
import re
from pathlib import Path
from urllib.parse import urlsplit

DEFAULT_REGISTRY = Path(__file__).with_name("github-linear-project-registry.tsv")
EXPECTED_COLUMNS = (
    "organization",
    "github_project_title",
    "github_project_url",
    "linear_url",
)
EXPECTED_ORGANIZATION_COUNT = 64
PROJECT_NUMBER_EXCEPTIONS = {"dancing-dragons": 4}
ORG_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")


class RegistryError(ValueError):
    """The registry violates a reviewed identity or formatting invariant."""


def fail(message: str) -> None:
    raise RegistryError(message)


def exact_https_url(value: str, *, host: str, path: str, label: str) -> None:
    parsed = urlsplit(value)
    if (
        parsed.scheme != "https"
        or parsed.hostname != host
        or parsed.username is not None
        or parsed.password is not None
        or parsed.port is not None
        or parsed.path != path
        or parsed.query
        or parsed.fragment
    ):
        fail(f"{label} must be exactly https://{host}{path}: {value!r}")


def validate(registry: Path) -> tuple[int, int, int]:
    raw = registry.read_bytes()
    if b"\r" in raw:
        fail("registry must use LF line endings")
    if not raw.endswith(b"\n"):
        fail("registry must end with one newline")
    if b"\x00" in raw:
        fail("registry contains a NUL byte")

    with registry.open(newline="", encoding="utf-8") as stream:
        reader = csv.DictReader(stream, delimiter="\t", strict=True)
        if tuple(reader.fieldnames or ()) != EXPECTED_COLUMNS:
            fail(
                f"header must be {EXPECTED_COLUMNS!r}; observed {reader.fieldnames!r}"
            )
        rows = list(reader)

    if len(rows) != EXPECTED_ORGANIZATION_COUNT:
        fail(
            f"expected {EXPECTED_ORGANIZATION_COUNT} organizations, observed {len(rows)}"
        )

    organizations: set[str] = set()
    folded_organizations: set[str] = set()
    project_titles: set[str] = set()
    project_urls: set[str] = set()
    linear_urls: set[str] = set()

    for number, row in enumerate(rows, start=2):
        if None in row:
            fail(f"line {number} has extra columns")
        if any(value is None or not value or value != value.strip() for value in row.values()):
            fail(f"line {number} contains an empty or padded field")

        organization = row["organization"]
        project_title = row["github_project_title"]
        project_url = row["github_project_url"]
        linear_url = row["linear_url"]

        if not ORG_RE.fullmatch(organization):
            fail(f"line {number} has invalid organization login {organization!r}")
        folded = organization.casefold()
        if organization in organizations or folded in folded_organizations:
            fail(f"line {number} duplicates organization {organization!r}")
        organizations.add(organization)
        folded_organizations.add(folded)

        expected_title = f"{organization}-project"
        if project_title != expected_title:
            fail(
                f"line {number} project title must be {expected_title!r}, "
                f"observed {project_title!r}"
            )
        if project_title in project_titles:
            fail(f"line {number} duplicates project title {project_title!r}")
        project_titles.add(project_title)

        project_number = PROJECT_NUMBER_EXCEPTIONS.get(organization, 1)
        exact_https_url(
            project_url,
            host="github.com",
            path=f"/orgs/{organization}/projects/{project_number}",
            label=f"line {number} GitHub Project URL",
        )
        if project_url in project_urls:
            fail(f"line {number} duplicates GitHub Project URL {project_url!r}")
        project_urls.add(project_url)

        parsed_linear = urlsplit(linear_url)
        if (
            parsed_linear.scheme != "https"
            or parsed_linear.hostname != "linear.app"
            or parsed_linear.username is not None
            or parsed_linear.password is not None
            or parsed_linear.port is not None
            or not parsed_linear.path.startswith("/denman/project/")
            or parsed_linear.path.count("/") != 3
            or parsed_linear.query
            or parsed_linear.fragment
        ):
            fail(f"line {number} has invalid Linear project URL {linear_url!r}")
        if linear_url in linear_urls:
            fail(f"line {number} duplicates Linear URL {linear_url!r}")
        linear_urls.add(linear_url)

    if not set(PROJECT_NUMBER_EXCEPTIONS).issubset(organizations):
        fail("project-number exception references an organization outside the registry")

    return len(rows), len(project_urls), len(linear_urls)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "registry",
        nargs="?",
        type=Path,
        default=DEFAULT_REGISTRY,
        help="TSV registry to validate",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    try:
        organizations, projects, linear_projects = validate(args.registry)
    except (OSError, csv.Error, UnicodeError, RegistryError) as error:
        raise SystemExit(f"registry validation failed: {error}") from None
    print(
        "registry=ok "
        f"organizations={organizations} "
        f"projects={projects} "
        f"linear_projects={linear_projects}"
    )


if __name__ == "__main__":
    main()
