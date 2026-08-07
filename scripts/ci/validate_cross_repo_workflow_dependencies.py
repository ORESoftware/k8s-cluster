#!/usr/bin/env python3
"""Validate cross-repository GitHub Actions refs and temporary exceptions.

The implementation intentionally uses only the Python standard library. It is a
bounded workflow-contract parser, not a general YAML parser: it recognizes
`actions/checkout` steps, their `with.repository`/`with.ref` inputs, and
feature-branch defaults that require dated Linear-owned exceptions.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import json
import re
import sys
from pathlib import Path
from typing import Any

CHECKOUT_RE = re.compile(
    r"^(?P<indent>\s*)(?:-\s+)?uses:\s+actions/checkout@[^\s#]+"
)
KEY_VALUE_RE = re.compile(r"^(?P<indent>\s*)(?P<key>[A-Za-z0-9_-]+):\s*(?P<value>.*)$")
FEATURE_REF_RE = re.compile(r"(?<![A-Za-z0-9_.-])(agent/[A-Za-z0-9._/-]+)")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
ISSUE_RE = re.compile(r"^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$")
ALLOWED_REF_POLICIES = {
    "immutable_commit",
    "canonical_main",
    "default_branch",
    "feature_branch",
}


class ValidationError(Exception):
    """The ledger or workflow contract is structurally invalid."""


@dataclasses.dataclass(frozen=True)
class CheckoutBlock:
    workflow: str
    repository: str | None
    ref_value: str | None
    start_line: int


@dataclasses.dataclass(frozen=True)
class Finding:
    severity: str
    code: str
    message: str
    workflow: str | None = None

    def as_dict(self) -> dict[str, Any]:
        return dataclasses.asdict(self)


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValidationError(f"cannot load ledger {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValidationError("ledger root must be an object")
    return value


def parse_date(value: str, field: str) -> dt.date:
    try:
        return dt.date.fromisoformat(value)
    except ValueError as exc:
        raise ValidationError(f"{field} must be an ISO date") from exc


def parse_checkout_blocks(text: str, workflow: str) -> list[CheckoutBlock]:
    """Return checkout inputs without allowing one step to consume the next.

    GitHub Actions supports both `- uses:` and a named step where `uses:` is a
    sibling of `name:`. The `with:` key is consequently either deeper than or
    equal to the `uses:` indentation. We track the exact `with:` indentation
    and stop at the next sibling property or step.
    """

    lines = text.splitlines()
    blocks: list[CheckoutBlock] = []
    for index, line in enumerate(lines):
        checkout = CHECKOUT_RE.match(line)
        if not checkout:
            continue

        uses_indent = len(checkout.group("indent"))
        with_indent: int | None = None
        repository: str | None = None
        ref_value: str | None = None
        cursor = index + 1

        while cursor < len(lines):
            candidate = lines[cursor]
            stripped = candidate.lstrip(" ")
            indent = len(candidate) - len(stripped)

            if not stripped or stripped.startswith("#"):
                cursor += 1
                continue
            if indent < uses_indent:
                break
            if stripped.startswith("- ") and indent <= uses_indent:
                break
            if stripped == "with:" and indent >= uses_indent:
                with_indent = indent
                cursor += 1
                continue
            if with_indent is None:
                if indent == uses_indent:
                    break
                cursor += 1
                continue
            if indent <= with_indent:
                break

            match = KEY_VALUE_RE.match(candidate)
            if match:
                key = match.group("key")
                value = match.group("value").strip().strip("'\"")
                if key == "repository":
                    repository = value
                elif key == "ref":
                    ref_value = value
            cursor += 1

        blocks.append(CheckoutBlock(workflow, repository, ref_value, index + 1))
    return blocks


def feature_refs_in_workflow(text: str) -> set[str]:
    refs: set[str] = set()
    for line in text.splitlines():
        match = KEY_VALUE_RE.match(line)
        if not match:
            continue
        key = match.group("key")
        if key not in {"ref", "default"} and not key.upper().endswith("_REF"):
            continue
        refs.update(FEATURE_REF_RE.findall(match.group("value")))
    return refs


def validate_ledger_shape(ledger: dict[str, Any]) -> None:
    if ledger.get("schema_version") != 1:
        raise ValidationError("schema_version must equal 1")
    if not isinstance(ledger.get("dependencies"), list):
        raise ValidationError("dependencies must be an array")
    if not isinstance(ledger.get("feature_ref_exceptions"), list):
        raise ValidationError("feature_ref_exceptions must be an array")


def workflow_text(repo_root: Path, relative: str) -> str:
    path = repo_root / relative
    if not path.is_file():
        raise ValidationError(f"workflow dependency path does not exist: {relative}")
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ValidationError(f"cannot read {relative}: {exc}") from exc


def dependency_key(row: dict[str, Any]) -> tuple[str, str]:
    workflow = row.get("workflow")
    repository = row.get("repository")
    if not isinstance(workflow, str) or not workflow:
        raise ValidationError("dependency.workflow must be a non-empty string")
    if not isinstance(repository, str) or not REPOSITORY_RE.fullmatch(repository):
        raise ValidationError(f"dependency.repository is invalid for {workflow!r}")
    return workflow, repository


def validate_dependency(
    row: dict[str, Any], repo_root: Path, as_of: dt.date, findings: list[Finding]
) -> None:
    workflow, repository = dependency_key(row)
    policy = row.get("ref_policy")
    if policy not in ALLOWED_REF_POLICIES:
        raise ValidationError(f"{workflow}: unsupported ref_policy {policy!r}")
    issue = row.get("owning_issue")
    if not isinstance(issue, str) or not ISSUE_RE.fullmatch(issue):
        raise ValidationError(f"{workflow}: owning_issue must be a Linear identifier")

    text = workflow_text(repo_root, workflow)
    matching = [
        block
        for block in parse_checkout_blocks(text, workflow)
        if block.repository == repository
    ]
    if len(matching) != 1:
        findings.append(
            Finding(
                "error",
                "checkout-count",
                f"expected exactly one checkout for {repository}, found {len(matching)}",
                workflow,
            )
        )
        return

    block = matching[0]
    expected_ref = row.get("expected_ref")
    if policy == "default_branch":
        if block.ref_value is not None:
            findings.append(
                Finding(
                    "error",
                    "default-branch-has-ref",
                    f"default_branch dependency unexpectedly sets ref {block.ref_value!r}",
                    workflow,
                )
            )
        return

    if policy == "canonical_main":
        if expected_ref != "main":
            raise ValidationError(f"{workflow}: canonical_main expected_ref must equal 'main'")
        if block.ref_value != "main":
            findings.append(
                Finding(
                    "error",
                    "canonical-main-drift",
                    f"expected ref 'main', found {block.ref_value!r}",
                    workflow,
                )
            )
        return

    if policy == "immutable_commit":
        if not isinstance(expected_ref, str) or not SHA_RE.fullmatch(expected_ref):
            raise ValidationError(
                f"{workflow}: immutable expected_ref must be a lowercase 40-char SHA"
            )
        source = row.get("ref_source")
        if source == "literal":
            if block.ref_value != expected_ref:
                findings.append(
                    Finding(
                        "error",
                        "immutable-checkout-drift",
                        f"expected immutable ref {expected_ref}, found {block.ref_value!r}",
                        workflow,
                    )
                )
            return
        if isinstance(source, str) and source.startswith("env:"):
            variable = source.removeprefix("env:")
            env_re = re.compile(
                rf"^\s*{re.escape(variable)}:\s*{re.escape(expected_ref)}\s*$",
                re.MULTILINE,
            )
            expected_checkout_ref = f"${{{{ env.{variable} }}}}"
            if not env_re.search(text):
                findings.append(
                    Finding(
                        "error",
                        "immutable-env-drift",
                        f"{variable} no longer pins {expected_ref}",
                        workflow,
                    )
                )
            if block.ref_value != expected_checkout_ref:
                findings.append(
                    Finding(
                        "error",
                        "immutable-checkout-drift",
                        f"checkout ref should use {expected_checkout_ref!r}, found {block.ref_value!r}",
                        workflow,
                    )
                )
            return
        raise ValidationError(f"{workflow}: unsupported ref_source {source!r}")

    if not isinstance(expected_ref, str) or not expected_ref.startswith("agent/"):
        raise ValidationError(
            f"{workflow}: feature_branch expected_ref must start with agent/"
        )
    if block.ref_value != expected_ref:
        findings.append(
            Finding(
                "error",
                "feature-ref-drift",
                f"expected temporary ref {expected_ref!r}, found {block.ref_value!r}",
                workflow,
            )
        )
    owning_pr = row.get("owning_pr")
    if not isinstance(owning_pr, int) or owning_pr <= 0:
        raise ValidationError(f"{workflow}: feature_branch requires positive owning_pr")
    expires_on = row.get("expires_on")
    if not isinstance(expires_on, str):
        raise ValidationError(f"{workflow}: feature_branch requires expires_on")
    expiry = parse_date(expires_on, f"{workflow}.expires_on")
    if expiry < as_of:
        findings.append(
            Finding(
                "error",
                "feature-ref-expired",
                f"temporary dependency {expected_ref} expired on {expiry.isoformat()}",
                workflow,
            )
        )


def validate_feature_ref_exceptions(
    ledger: dict[str, Any], repo_root: Path, as_of: dt.date, findings: list[Finding]
) -> None:
    exceptions_by_path: dict[str, dict[str, Any]] = {}
    for row in ledger["feature_ref_exceptions"]:
        if not isinstance(row, dict):
            raise ValidationError("feature_ref_exceptions entries must be objects")
        path = row.get("workflow")
        issue = row.get("owning_issue")
        expires_on = row.get("expires_on")
        reason = row.get("reason")
        if not isinstance(path, str) or not path.startswith(".github/workflows/"):
            raise ValidationError(
                "feature ref exception workflow must be under .github/workflows"
            )
        if path in exceptions_by_path:
            raise ValidationError(f"duplicate feature ref exception for {path}")
        if not isinstance(issue, str) or not ISSUE_RE.fullmatch(issue):
            raise ValidationError(f"{path}: exception owning_issue is invalid")
        if not isinstance(reason, str) or not reason.strip():
            raise ValidationError(f"{path}: exception reason is required")
        if not isinstance(expires_on, str):
            raise ValidationError(f"{path}: exception expires_on is required")
        expiry = parse_date(expires_on, f"{path}.expires_on")
        if expiry < as_of:
            findings.append(
                Finding(
                    "error",
                    "exception-expired",
                    f"feature-ref exception expired on {expiry}",
                    path,
                )
            )
        refs = feature_refs_in_workflow(workflow_text(repo_root, path))
        if not refs:
            findings.append(
                Finding(
                    "error",
                    "exception-stale",
                    "exception remains but workflow no longer contains a feature-branch ref",
                    path,
                )
            )
        exceptions_by_path[path] = row

    scan_root = repo_root / ".github/workflows"
    if not scan_root.is_dir():
        raise ValidationError(".github/workflows directory is missing")
    for path in sorted(scan_root.glob("*.y*ml")):
        relative = path.relative_to(repo_root).as_posix()
        refs = feature_refs_in_workflow(path.read_text(encoding="utf-8"))
        if refs and relative not in exceptions_by_path:
            findings.append(
                Finding(
                    "error",
                    "unapproved-feature-ref",
                    f"workflow defaults to temporary refs: {', '.join(sorted(refs))}",
                    relative,
                )
            )


def render_markdown(report: dict[str, Any]) -> str:
    summary = report["summary"]
    lines = [
        "# Cross-repository workflow dependency report",
        "",
        f"- As of: `{report['as_of']}`",
        f"- Dependencies checked: `{summary['dependencies_checked']}`",
        f"- Feature-ref exceptions: `{summary['feature_ref_exceptions']}`",
        f"- Errors: `{summary['errors']}`",
        f"- Warnings: `{summary['warnings']}`",
        "",
    ]
    findings = report["findings"]
    if not findings:
        lines.append("No drift detected in the currently governed slice.")
    else:
        lines.extend(["## Findings", ""])
        for item in findings:
            location = f" — `{item['workflow']}`" if item.get("workflow") else ""
            lines.append(
                f"- **{item['severity'].upper()} `{item['code']}`**{location}: "
                f"{item['message']}"
            )
    lines.append("")
    return "\n".join(lines)


def build_report(
    ledger: dict[str, Any], repo_root: Path, as_of: dt.date
) -> tuple[dict[str, Any], int]:
    validate_ledger_shape(ledger)
    findings: list[Finding] = []
    seen: set[tuple[str, str]] = set()
    for row in ledger["dependencies"]:
        if not isinstance(row, dict):
            raise ValidationError("dependency entries must be objects")
        key = dependency_key(row)
        if key in seen:
            raise ValidationError(f"duplicate dependency entry for {key[0]} -> {key[1]}")
        seen.add(key)
        validate_dependency(row, repo_root, as_of, findings)
    validate_feature_ref_exceptions(ledger, repo_root, as_of, findings)
    errors = sum(1 for finding in findings if finding.severity == "error")
    warnings = sum(1 for finding in findings if finding.severity == "warning")
    report = {
        "schema_version": 1,
        "as_of": as_of.isoformat(),
        "summary": {
            "dependencies_checked": len(ledger["dependencies"]),
            "feature_ref_exceptions": len(ledger["feature_ref_exceptions"]),
            "errors": errors,
            "warnings": warnings,
        },
        "findings": [finding.as_dict() for finding in findings],
    }
    return report, 1 if errors else 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ledger", type=Path, required=True)
    parser.add_argument("--repo-root", type=Path, default=Path("."))
    parser.add_argument("--as-of", type=str)
    parser.add_argument("--report-json", type=Path)
    parser.add_argument("--report-markdown", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    as_of = (
        parse_date(args.as_of, "--as-of")
        if args.as_of
        else dt.datetime.now(dt.timezone.utc).date()
    )
    try:
        ledger = load_json(args.ledger)
        report, status = build_report(ledger, args.repo_root.resolve(), as_of)
    except ValidationError as exc:
        report = {
            "schema_version": 1,
            "as_of": as_of.isoformat(),
            "summary": {
                "dependencies_checked": 0,
                "feature_ref_exceptions": 0,
                "errors": 1,
                "warnings": 0,
            },
            "findings": [Finding("error", "ledger-invalid", str(exc)).as_dict()],
        }
        status = 2

    json_text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    markdown_text = render_markdown(report)
    if args.report_json:
        args.report_json.parent.mkdir(parents=True, exist_ok=True)
        args.report_json.write_text(json_text, encoding="utf-8")
    else:
        sys.stdout.write(json_text)
    if args.report_markdown:
        args.report_markdown.parent.mkdir(parents=True, exist_ok=True)
        args.report_markdown.write_text(markdown_text, encoding="utf-8")
    return status


if __name__ == "__main__":
    raise SystemExit(main())
