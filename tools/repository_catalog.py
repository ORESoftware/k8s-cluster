#!/usr/bin/env python3
"""Validate and compare canonical repository catalog snapshots.

The tool intentionally uses only the Python standard library so it can run in
GitHub Actions and operator workstations without bootstrapping project-specific
dependencies.
"""
from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

REQUIRED_FIELDS = {
    "name",
    "visibility",
    "default_branch",
    "lifecycle",
    "conformance_profile",
    "canonical_location",
    "linear_project",
    "security_class",
    "review_date",
}
LIFECYCLES = {"active", "maintenance", "deprecated", "archived", "experimental"}
VISIBILITIES = {"public", "private", "internal"}
SECURITY_CLASSES = {"public", "internal", "confidential", "restricted"}


@dataclass(frozen=True)
class ValidationError:
    repository: str
    field: str
    message: str

    def render(self) -> str:
        return f"{self.repository}: {self.field}: {self.message}"


def load_catalog(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        data = json.load(handle)
    if not isinstance(data, dict):
        raise ValueError("catalog root must be an object")
    if data.get("schema_version") != 1:
        raise ValueError("schema_version must equal 1")
    repositories = data.get("repositories")
    if not isinstance(repositories, list):
        raise ValueError("repositories must be an array")
    return data


def validate_catalog(data: dict[str, Any]) -> list[ValidationError]:
    errors: list[ValidationError] = []
    seen: set[str] = set()
    for index, repo in enumerate(data["repositories"]):
        identity = f"repositories[{index}]"
        if not isinstance(repo, dict):
            errors.append(ValidationError(identity, "record", "must be an object"))
            continue
        name = repo.get("name")
        if isinstance(name, str) and name:
            identity = name
            if name in seen:
                errors.append(ValidationError(identity, "name", "duplicate repository"))
            seen.add(name)
            if "/" not in name or name.startswith("/") or name.endswith("/"):
                errors.append(ValidationError(identity, "name", "must use owner/repository form"))
        else:
            errors.append(ValidationError(identity, "name", "must be a non-empty string"))

        for field in sorted(REQUIRED_FIELDS - set(repo)):
            errors.append(ValidationError(identity, field, "required field is missing"))

        _enum_error(errors, identity, repo, "visibility", VISIBILITIES)
        _enum_error(errors, identity, repo, "lifecycle", LIFECYCLES)
        _enum_error(errors, identity, repo, "security_class", SECURITY_CLASSES)

        dependencies = repo.get("dependencies", [])
        if not isinstance(dependencies, list):
            errors.append(ValidationError(identity, "dependencies", "must be an array"))
        else:
            for dep_index, dependency in enumerate(dependencies):
                _validate_dependency(errors, identity, dep_index, dependency)
    return errors


def _enum_error(
    errors: list[ValidationError],
    identity: str,
    repo: dict[str, Any],
    field: str,
    allowed: set[str],
) -> None:
    value = repo.get(field)
    if value is not None and value not in allowed:
        errors.append(ValidationError(identity, field, f"must be one of {sorted(allowed)}"))


def _validate_dependency(
    errors: list[ValidationError],
    identity: str,
    index: int,
    dependency: Any,
) -> None:
    field = f"dependencies[{index}]"
    if not isinstance(dependency, dict):
        errors.append(ValidationError(identity, field, "must be an object"))
        return
    for required in ("target", "kind", "evidence"):
        if not dependency.get(required):
            errors.append(ValidationError(identity, f"{field}.{required}", "is required"))
    if dependency.get("verified") is True and not dependency.get("pin"):
        errors.append(
            ValidationError(identity, f"{field}.pin", "verified dependencies require an exact pin")
        )


def index_repositories(data: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {repo["name"]: repo for repo in data["repositories"] if isinstance(repo, dict) and repo.get("name")}


def diff_catalogs(baseline: dict[str, Any], current: dict[str, Any]) -> dict[str, Any]:
    old = index_repositories(baseline)
    new = index_repositories(current)
    added = sorted(new.keys() - old.keys())
    removed = sorted(old.keys() - new.keys())
    changed: list[dict[str, Any]] = []
    tracked_fields = (
        "visibility",
        "default_branch",
        "lifecycle",
        "conformance_profile",
        "canonical_location",
        "linear_project",
        "release_state",
        "security_class",
        "dependencies",
        "exemptions",
    )
    for name in sorted(old.keys() & new.keys()):
        fields = {
            field: {"before": old[name].get(field), "after": new[name].get(field)}
            for field in tracked_fields
            if old[name].get(field) != new[name].get(field)
        }
        if fields:
            changed.append({"name": name, "fields": fields})
    return {"added": added, "removed": removed, "changed": changed}


def render_markdown(report: dict[str, Any]) -> str:
    lines = ["# Repository catalog drift", ""]
    lines.extend(_render_names("Added", report["added"]))
    lines.extend(_render_names("Removed", report["removed"]))
    lines.append("## Changed")
    if not report["changed"]:
        lines.append("- None")
    for change in report["changed"]:
        lines.append(f"- **{change['name']}**")
        for field, values in change["fields"].items():
            before = json.dumps(values["before"], sort_keys=True)
            after = json.dumps(values["after"], sort_keys=True)
            lines.append(f"  - `{field}`: `{before}` → `{after}`")
    lines.append("")
    return "\n".join(lines)


def _render_names(title: str, names: Iterable[str]) -> list[str]:
    values = list(names)
    lines = [f"## {title}"]
    lines.extend([f"- {name}" for name in values] or ["- None"])
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("catalog", type=Path, help="current catalog JSON")
    parser.add_argument("--baseline", type=Path, help="optional baseline catalog JSON")
    parser.add_argument("--json-output", type=Path, help="write machine-readable drift report")
    parser.add_argument("--markdown-output", type=Path, help="write human-readable drift report")
    args = parser.parse_args(argv)

    try:
        current = load_catalog(args.catalog)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"catalog load failed: {exc}", file=sys.stderr)
        return 2

    errors = validate_catalog(current)
    if errors:
        for error in errors:
            print(error.render(), file=sys.stderr)
        return 1

    if not args.baseline:
        print(f"validated {len(current['repositories'])} repository records")
        return 0

    try:
        baseline = load_catalog(args.baseline)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"baseline load failed: {exc}", file=sys.stderr)
        return 2

    report = diff_catalogs(baseline, current)
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    markdown = render_markdown(report)
    if args.json_output:
        args.json_output.parent.mkdir(parents=True, exist_ok=True)
        args.json_output.write_text(encoded, encoding="utf-8")
    else:
        print(encoded, end="")
    if args.markdown_output:
        args.markdown_output.parent.mkdir(parents=True, exist_ok=True)
        args.markdown_output.write_text(markdown, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
