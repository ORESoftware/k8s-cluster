#!/usr/bin/env python3
"""Render and validate the GitHub/Linear project directory from the canonical TSV."""

from __future__ import annotations

import argparse
import csv
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

BEGIN_MARKER = "<!-- BEGIN GENERATED ORGANIZATION DIRECTORY -->"
END_MARKER = "<!-- END GENERATED ORGANIZATION DIRECTORY -->"
DEFAULT_REGISTRY = Path("ops/portfolio/github-linear-project-registry.tsv")
DEFAULT_DOCUMENT = Path("docs/portfolio/github-linear-projects-by-org.md")
ORG_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
CREDENTIAL_RE = re.compile(
    r"(?i)(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|(?:token|password|secret)=)"
)


class DirectoryError(ValueError):
    """The registry or generated directory violates its public contract."""


@dataclass(frozen=True)
class OrganizationLink:
    organization: str
    linear_url: str

    @property
    def project_number(self) -> int:
        return 4 if self.organization == "dancing-dragons" else 1

    @property
    def organization_url(self) -> str:
        return f"https://github.com/{self.organization}"

    @property
    def project_title(self) -> str:
        return f"{self.organization}-project"

    @property
    def project_url(self) -> str:
        return (
            f"https://github.com/orgs/{self.organization}/projects/"
            f"{self.project_number}"
        )


def _validate_url(url: str, *, host: str, label: str) -> None:
    if CREDENTIAL_RE.search(url):
        raise DirectoryError(f"{label} contains credential-shaped material")
    parsed = urlsplit(url)
    if (
        parsed.scheme != "https"
        or parsed.hostname != host
        or parsed.username is not None
        or parsed.password is not None
        or parsed.port is not None
        or parsed.query
        or parsed.fragment
    ):
        raise DirectoryError(f"{label} is not a canonical credential-free HTTPS URL")


def load_registry(path: Path) -> list[OrganizationLink]:
    try:
        with path.open(newline="", encoding="utf-8") as handle:
            reader = csv.DictReader(handle, delimiter="\t")
            if reader.fieldnames != ["organization", "linear_url"]:
                raise DirectoryError(
                    "registry header must be exactly organization and linear_url"
                )
            rows = [OrganizationLink(**row) for row in reader]
    except OSError as exc:
        raise DirectoryError(f"cannot read registry {path}: {exc}") from exc

    if not rows:
        raise DirectoryError("registry must contain at least one organization")
    organizations = [row.organization for row in rows]
    if organizations != sorted(organizations, key=str.casefold):
        raise DirectoryError("registry organizations must be case-insensitively sorted")
    if len({value.casefold() for value in organizations}) != len(organizations):
        raise DirectoryError("registry contains duplicate organization ownership")
    if len({row.linear_url for row in rows}) != len(rows):
        raise DirectoryError("registry contains duplicate Linear project URLs")

    for row in rows:
        if not ORG_RE.fullmatch(row.organization):
            raise DirectoryError(f"invalid GitHub organization login: {row.organization!r}")
        _validate_url(
            row.organization_url,
            host="github.com",
            label=f"GitHub organization URL for {row.organization}",
        )
        _validate_url(
            row.project_url,
            host="github.com",
            label=f"GitHub Project URL for {row.organization}",
        )
        _validate_url(
            row.linear_url,
            host="linear.app",
            label=f"Linear project URL for {row.organization}",
        )
        if not urlsplit(row.linear_url).path.startswith("/denman/project/"):
            raise DirectoryError(
                f"Linear project URL for {row.organization} has the wrong workspace path"
            )
    return rows


def render_directory(rows: list[OrganizationLink]) -> str:
    lines = [
        BEGIN_MARKER,
        "",
        "| GitHub organization | Canonical GitHub Project | Linear project |",
        "| --- | --- | --- |",
    ]
    for row in rows:
        lines.append(
            f"| [`{row.organization}`]({row.organization_url}) "
            f"| [`{row.project_title}` #{row.project_number}]({row.project_url}) "
            f"| [Linear project]({row.linear_url}) |"
        )
    lines.extend(["", END_MARKER])
    return "\n".join(lines)


def replace_generated_block(document: str, rendered: str) -> str:
    if document.count(BEGIN_MARKER) != 1 or document.count(END_MARKER) != 1:
        raise DirectoryError("document must contain exactly one generated directory block")
    before, remainder = document.split(BEGIN_MARKER, 1)
    _, after = remainder.split(END_MARKER, 1)
    return f"{before}{rendered}{after}"


def validate_document(registry: Path, document: Path) -> None:
    rows = load_registry(registry)
    try:
        content = document.read_text(encoding="utf-8")
    except OSError as exc:
        raise DirectoryError(f"cannot read document {document}: {exc}") from exc
    expected = render_directory(rows)
    if content.count(BEGIN_MARKER) != 1 or content.count(END_MARKER) != 1:
        raise DirectoryError("document must contain exactly one generated directory block")
    actual = BEGIN_MARKER + content.split(BEGIN_MARKER, 1)[1].split(END_MARKER, 1)[0] + END_MARKER
    if actual != expected:
        raise DirectoryError(
            "generated organization directory drifted; run this script with --write"
        )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry", type=Path, default=DEFAULT_REGISTRY)
    parser.add_argument("--document", type=Path, default=DEFAULT_DOCUMENT)
    parser.add_argument("--write", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        rows = load_registry(args.registry)
        if args.write:
            content = args.document.read_text(encoding="utf-8")
            updated = replace_generated_block(content, render_directory(rows))
            args.document.write_text(updated, encoding="utf-8")
        validate_document(args.registry, args.document)
    except (DirectoryError, OSError) as exc:
        print(f"GitHub/Linear organization directory validation failed: {exc}", file=sys.stderr)
        return 2
    print(f"validated {len(rows)} GitHub/Linear organization mappings")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
