#!/usr/bin/env python3
"""Build and verify the canonical Argo CD Application catalog for DEN-630."""

from __future__ import annotations

import argparse
import configparser
import difflib
import json
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Mapping, Sequence

SCHEMA_VERSION = 1
SCHEMA_REFERENCE = "./applications.schema.json"
GOVERNING_ISSUE = "DEN-630"
MANIFEST_ROOT = "remote/argocd"
CLUSTER_REPOSITORY = "github.com/oresoftware/k8s-cluster"

YQ_EXPRESSION = """
select(.kind == "Application") |
{
  "manifest_path": filename,
  "document_index": documentIndex,
  "document": .
}
"""


def load_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def _run(
    command: Sequence[str],
    *,
    cwd: Path,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )


def tracked_manifest_paths(repo_root: Path) -> list[str]:
    try:
        output = _run(
            ["git", "ls-files", "--", MANIFEST_ROOT],
            cwd=repo_root,
        ).stdout
    except subprocess.CalledProcessError:
        manifest_root = repo_root / MANIFEST_ROOT
        return sorted(
            path.relative_to(repo_root).as_posix()
            for path in manifest_root.rglob("*")
            if path.is_file() and path.suffix in {".yaml", ".yml"}
        )
    return sorted(
        path for path in output.splitlines() if path.endswith((".yaml", ".yml"))
    )


def tracked_gitlinks(repo_root: Path) -> list[str]:
    try:
        output = _run(["git", "ls-files", "--stage"], cwd=repo_root).stdout
    except subprocess.CalledProcessError:
        parser = configparser.ConfigParser()
        parser.read(repo_root / ".gitmodules", encoding="utf-8")
        return sorted(
            parser.get(section, "path")
            for section in parser.sections()
            if parser.has_option(section, "path")
        )
    gitlinks: list[str] = []
    for line in output.splitlines():
        fields = line.split(maxsplit=3)
        if len(fields) == 4 and fields[0] == "160000":
            gitlinks.append(fields[3])
    return sorted(gitlinks)


def load_application_documents(
    repo_root: Path,
    *,
    yq_binary: str = "yq",
) -> list[dict[str, Any]]:
    manifests = tracked_manifest_paths(repo_root)
    if not manifests:
        raise ValueError(f"no tracked YAML manifests found under {MANIFEST_ROOT}")
    result = _run(
        [
            yq_binary,
            "eval-all",
            "-o=json",
            "-I=0",
            YQ_EXPRESSION,
            *manifests,
        ],
        cwd=repo_root,
    )
    documents: list[dict[str, Any]] = []
    for line_number, line in enumerate(result.stdout.splitlines(), start=1):
        if not line.strip():
            continue
        value = json.loads(line)
        if not isinstance(value, dict):
            raise ValueError(f"yq output line {line_number} is not an object")
        documents.append(value)
    return documents


def normalize_repo_url(value: str) -> str:
    normalized = value.strip().lower()
    if normalized.startswith("git@github.com:"):
        normalized = "github.com/" + normalized.removeprefix("git@github.com:")
    elif normalized.startswith("ssh://git@github.com/"):
        normalized = "github.com/" + normalized.removeprefix("ssh://git@github.com/")
    elif normalized.startswith("https://"):
        normalized = normalized.removeprefix("https://")
    elif normalized.startswith("http://"):
        normalized = normalized.removeprefix("http://")
    return normalized.removesuffix("/").removesuffix(".git")


def _string(value: Any) -> str:
    return value if isinstance(value, str) else ""


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, dict) else {}


def _sources(spec: Mapping[str, Any]) -> list[dict[str, str]]:
    raw_sources: list[Any] = []
    if isinstance(spec.get("source"), dict):
        raw_sources.append(spec["source"])
    if isinstance(spec.get("sources"), list):
        raw_sources.extend(spec["sources"])
    result: list[dict[str, str]] = []
    for raw_source in raw_sources:
        source = _mapping(raw_source)
        result.append(
            {
                "chart": _string(source.get("chart")),
                "path": _string(source.get("path")),
                "repo_url": _string(source.get("repoURL")),
                "target_revision": _string(source.get("targetRevision")),
            }
        )
    return result


def declaration_from_document(value: Mapping[str, Any]) -> dict[str, Any]:
    document = _mapping(value.get("document"))
    metadata = _mapping(document.get("metadata"))
    spec = _mapping(document.get("spec"))
    destination = _mapping(spec.get("destination"))
    sync_policy = _mapping(spec.get("syncPolicy"))
    automated = _mapping(sync_policy.get("automated"))
    sync_options = sync_policy.get("syncOptions")
    if not isinstance(sync_options, list):
        sync_options = []
    normalized_options = sorted(
        option for option in sync_options if isinstance(option, str)
    )
    return {
        "application_namespace": _string(metadata.get("namespace")),
        "destination": {
            "name": _string(destination.get("name")),
            "namespace": _string(destination.get("namespace")),
            "server": _string(destination.get("server")),
        },
        "document_index": value.get("document_index", 0),
        "manifest_path": _string(value.get("manifest_path")),
        "project": _string(spec.get("project")),
        "sources": _sources(spec),
        "sync_policy": {
            "automated": bool(automated),
            "create_namespace": "CreateNamespace=true" in normalized_options,
            "prune": automated.get("prune") is True,
            "self_heal": automated.get("selfHeal") is True,
            "sync_options": normalized_options,
        },
    }


def _application_name(value: Mapping[str, Any]) -> str:
    document = _mapping(value.get("document"))
    metadata = _mapping(document.get("metadata"))
    return _string(metadata.get("name"))


def find_gitlink_render_violations(
    applications: Sequence[Mapping[str, Any]],
    gitlinks: Sequence[str],
) -> list[dict[str, Any]]:
    violations: list[dict[str, Any]] = []
    for application in applications:
        for declaration in application["declarations"]:
            for source_index, source in enumerate(declaration["sources"]):
                if normalize_repo_url(source["repo_url"]) != CLUSTER_REPOSITORY:
                    continue
                path = source["path"].strip("/")
                for gitlink in gitlinks:
                    if path == gitlink or path.startswith(f"{gitlink}/"):
                        violations.append(
                            {
                                "application": application["name"],
                                "document_index": declaration["document_index"],
                                "gitlink": gitlink,
                                "manifest_path": declaration["manifest_path"],
                                "source_index": source_index,
                                "source_path": source["path"],
                            }
                        )
    return sorted(
        violations,
        key=lambda item: (
            item["application"],
            item["manifest_path"],
            item["document_index"],
            item["source_index"],
        ),
    )


def build_catalog(
    documents: Sequence[Mapping[str, Any]],
    *,
    gitlinks: Sequence[str],
) -> dict[str, Any]:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for value in documents:
        name = _application_name(value)
        if not name:
            raise ValueError(
                f"Application in {value.get('manifest_path', '<unknown>')} "
                "has no metadata.name"
            )
        grouped[name].append(declaration_from_document(value))

    applications: list[dict[str, Any]] = []
    for name in sorted(grouped):
        declarations = sorted(
            grouped[name],
            key=lambda item: (item["manifest_path"], item["document_index"]),
        )
        applications.append(
            {
                "declaration_count": len(declarations),
                "declarations": declarations,
                "duplicate_name": len(declarations) > 1,
                "name": name,
            }
        )

    violations = find_gitlink_render_violations(applications, gitlinks)
    declarations = [
        declaration
        for application in applications
        for declaration in application["declarations"]
    ]
    duplicate_names = sum(application["duplicate_name"] for application in applications)
    return {
        "$schema": SCHEMA_REFERENCE,
        "applications": applications,
        "policy_violations": {
            "gitlink_render_paths": violations,
        },
        "schema_version": SCHEMA_VERSION,
        "scope": {
            "governing_issue": GOVERNING_ISSUE,
            "manifest_root": MANIFEST_ROOT,
            "source": "tracked Argo CD Application manifests",
        },
        "summary": {
            "application_documents": len(declarations),
            "applications": len(applications),
            "default_destination_namespace_declarations": sum(
                declaration["destination"]["namespace"] == "default"
                for declaration in declarations
            ),
            "default_project_declarations": sum(
                declaration["project"] == "default" for declaration in declarations
            ),
            "duplicate_names": duplicate_names,
            "gitlink_render_path_violations": len(violations),
            "in_repo_source_declarations": sum(
                any(
                    normalize_repo_url(source["repo_url"]) == CLUSTER_REPOSITORY
                    for source in declaration["sources"]
                )
                for declaration in declarations
            ),
        },
    }


def validate_catalog(value: Any) -> list[str]:
    errors: list[str] = []
    if not isinstance(value, dict):
        return ["catalog root must be an object"]
    if value.get("$schema") != SCHEMA_REFERENCE:
        errors.append(f"$schema must equal {SCHEMA_REFERENCE}")
    if value.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"schema_version must equal {SCHEMA_VERSION}")

    scope = value.get("scope")
    if not isinstance(scope, dict):
        errors.append("scope must be an object")
    else:
        expected_scope = {
            "governing_issue": GOVERNING_ISSUE,
            "manifest_root": MANIFEST_ROOT,
            "source": "tracked Argo CD Application manifests",
        }
        if scope != expected_scope:
            errors.append("scope does not match the DEN-630 contract")

    applications = value.get("applications")
    if not isinstance(applications, list):
        errors.append("applications must be an array")
        applications = []
    names: list[str] = []
    declarations: list[Mapping[str, Any]] = []
    for index, application in enumerate(applications):
        identity = f"applications[{index}]"
        if not isinstance(application, dict):
            errors.append(f"{identity} must be an object")
            continue
        name = application.get("name")
        if not isinstance(name, str) or not name:
            errors.append(f"{identity}.name must be a non-empty string")
        else:
            names.append(name)
        app_declarations = application.get("declarations")
        if not isinstance(app_declarations, list) or not app_declarations:
            errors.append(f"{identity}.declarations must be a non-empty array")
            continue
        declarations.extend(app_declarations)
        if application.get("declaration_count") != len(app_declarations):
            errors.append(f"{identity}.declaration_count is inconsistent")
        if application.get("duplicate_name") is not (len(app_declarations) > 1):
            errors.append(f"{identity}.duplicate_name is inconsistent")
        for declaration_index, declaration in enumerate(app_declarations):
            errors.extend(
                _validate_declaration(
                    declaration,
                    f"{identity}.declarations[{declaration_index}]",
                )
            )

    if names != sorted(names):
        errors.append("applications must be sorted by name")
    duplicates = [name for name, count in Counter(names).items() if count > 1]
    if duplicates:
        errors.append(
            "applications contain repeated registry records: "
            + ", ".join(sorted(duplicates))
        )

    policy_violations = value.get("policy_violations")
    if not isinstance(policy_violations, dict) or not isinstance(
        policy_violations.get("gitlink_render_paths"), list
    ):
        errors.append("policy_violations.gitlink_render_paths must be an array")
        violations: list[Any] = []
    else:
        violations = policy_violations["gitlink_render_paths"]

    expected_summary = {
        "application_documents": len(declarations),
        "applications": len(applications),
        "default_destination_namespace_declarations": sum(
            _nested_string(declaration, "destination", "namespace") == "default"
            for declaration in declarations
        ),
        "default_project_declarations": sum(
            declaration.get("project") == "default" for declaration in declarations
        ),
        "duplicate_names": sum(
            application.get("duplicate_name") is True
            for application in applications
            if isinstance(application, dict)
        ),
        "gitlink_render_path_violations": len(violations),
        "in_repo_source_declarations": sum(
            _declaration_uses_cluster_repo(declaration) for declaration in declarations
        ),
    }
    if value.get("summary") != expected_summary:
        errors.append("summary does not match application records")
    return errors


def _validate_declaration(value: Any, identity: str) -> list[str]:
    if not isinstance(value, dict):
        return [f"{identity} must be an object"]
    errors: list[str] = []
    for field in ("application_namespace", "manifest_path", "project"):
        if not isinstance(value.get(field), str):
            errors.append(f"{identity}.{field} must be a string")
    if not isinstance(value.get("document_index"), int):
        errors.append(f"{identity}.document_index must be an integer")
    destination = value.get("destination")
    if not isinstance(destination, dict) or any(
        not isinstance(destination.get(field), str)
        for field in ("name", "namespace", "server")
    ):
        errors.append(f"{identity}.destination must contain string fields")
    sources = value.get("sources")
    if not isinstance(sources, list) or not sources:
        errors.append(f"{identity}.sources must be a non-empty array")
    else:
        for source_index, source in enumerate(sources):
            if not isinstance(source, dict) or any(
                not isinstance(source.get(field), str)
                for field in ("chart", "path", "repo_url", "target_revision")
            ):
                errors.append(
                    f"{identity}.sources[{source_index}] must contain string fields"
                )
    sync_policy = value.get("sync_policy")
    if not isinstance(sync_policy, dict):
        errors.append(f"{identity}.sync_policy must be an object")
    else:
        for field in ("automated", "create_namespace", "prune", "self_heal"):
            if not isinstance(sync_policy.get(field), bool):
                errors.append(f"{identity}.sync_policy.{field} must be a boolean")
        options = sync_policy.get("sync_options")
        if not isinstance(options, list) or any(
            not isinstance(option, str) for option in options
        ):
            errors.append(f"{identity}.sync_policy.sync_options must be string array")
    return errors


def _nested_string(
    value: Mapping[str, Any],
    parent: str,
    field: str,
) -> str:
    nested = value.get(parent)
    if not isinstance(nested, dict):
        return ""
    result = nested.get(field)
    return result if isinstance(result, str) else ""


def _declaration_uses_cluster_repo(value: Mapping[str, Any]) -> bool:
    sources = value.get("sources")
    if not isinstance(sources, list):
        return False
    return any(
        isinstance(source, dict)
        and normalize_repo_url(_string(source.get("repo_url"))) == CLUSTER_REPOSITORY
        for source in sources
    )


def render_report(catalog: Mapping[str, Any]) -> str:
    summary = catalog["summary"]
    duplicate_names = [
        application["name"]
        for application in catalog["applications"]
        if application["duplicate_name"]
    ]
    lines = [
        "# Argo CD application catalog",
        "",
        f"- Governing issue: {GOVERNING_ISSUE}",
        f"- Application documents: {summary['application_documents']}",
        f"- Distinct application names: {summary['applications']}",
        f"- Duplicate names: {summary['duplicate_names']}",
        (
            "- In-repository source declarations: "
            f"{summary['in_repo_source_declarations']}"
        ),
        (
            "- Gitlink render-path violations: "
            f"{summary['gitlink_render_path_violations']}"
        ),
        "",
        "## Duplicate application names",
        "",
    ]
    lines.extend(f"- `{name}`" for name in duplicate_names)
    if not duplicate_names:
        lines.append("- None")
    violations = catalog["policy_violations"]["gitlink_render_paths"]
    lines.extend(["", "## Gitlink render-path violations", ""])
    if violations:
        lines.extend(
            (
                f"- `{item['application']}`: `{item['source_path']}` "
                f"is inside `{item['gitlink']}`"
            )
            for item in violations
        )
    else:
        lines.append("- None")
    return "\n".join(lines) + "\n"


def current_catalog(
    repo_root: Path,
    *,
    yq_binary: str,
) -> dict[str, Any]:
    return build_catalog(
        load_application_documents(repo_root, yq_binary=yq_binary),
        gitlinks=tracked_gitlinks(repo_root),
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--yq-binary", default="yq")
    subparsers = parser.add_subparsers(dest="command", required=True)

    generate = subparsers.add_parser("generate", help="write the current catalog")
    generate.add_argument("output", type=Path)

    validate = subparsers.add_parser("validate", help="validate a catalog snapshot")
    validate.add_argument("catalog", type=Path)

    check = subparsers.add_parser(
        "check",
        help="validate and compare a committed catalog with tracked manifests",
    )
    check.add_argument("catalog", type=Path)

    report = subparsers.add_parser(
        "report",
        help="render a Markdown summary from a catalog snapshot",
    )
    report.add_argument("catalog", type=Path)
    report.add_argument("--output", type=Path)

    args = parser.parse_args(argv)
    try:
        if args.command == "generate":
            catalog = current_catalog(args.repo_root, yq_binary=args.yq_binary)
            errors = validate_catalog(catalog)
            if errors:
                raise ValueError("; ".join(errors))
            write_json(args.output, catalog)
            print(
                f"wrote {catalog['summary']['applications']} application records "
                f"from {catalog['summary']['application_documents']} documents"
            )
            return 0

        catalog = load_json(args.catalog)
        errors = validate_catalog(catalog)
        if errors:
            for error in errors:
                print(error, file=sys.stderr)
            return 1

        if args.command == "validate":
            print(f"validated {catalog['summary']['applications']} application records")
            return 0

        if args.command == "check":
            expected = current_catalog(
                args.repo_root,
                yq_binary=args.yq_binary,
            )
            if catalog != expected:
                before = json.dumps(catalog, indent=2, sort_keys=True).splitlines()
                after = json.dumps(expected, indent=2, sort_keys=True).splitlines()
                print(
                    "\n".join(
                        difflib.unified_diff(
                            before,
                            after,
                            fromfile=str(args.catalog),
                            tofile="tracked manifests",
                            lineterm="",
                        )
                    ),
                    file=sys.stderr,
                )
                print(
                    "application catalog is stale; regenerate it with "
                    f"tools/application_catalog.py generate {args.catalog}",
                    file=sys.stderr,
                )
                return 1
            violations = catalog["policy_violations"]["gitlink_render_paths"]
            if violations:
                for violation in violations:
                    print(
                        f"{violation['application']}: {violation['source_path']} "
                        f"is inside gitlink {violation['gitlink']}",
                        file=sys.stderr,
                    )
                return 1
            print(
                f"application catalog matches "
                f"{catalog['summary']['application_documents']} tracked documents"
            )
            return 0

        if args.command == "report":
            text = render_report(catalog)
            if args.output:
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(text, encoding="utf-8")
            else:
                print(text, end="")
            return 0
    except (
        OSError,
        ValueError,
        json.JSONDecodeError,
        subprocess.CalledProcessError,
    ) as exc:
        print(f"application catalog command failed: {exc}", file=sys.stderr)
        return 2
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
