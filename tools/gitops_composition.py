#!/usr/bin/env python3
"""Validate and render the submodule-backed GitOps composition contract.

The existing DEN-630 application catalog inventories Argo CD Application
declarations. This companion contract proves that an independently owned
application repository is:

1. pinned as an exact gitlink in this superproject; and
2. rendered by Argo CD from the same upstream repository at that exact commit.

Submodule worktrees are not required. The validator reads .gitmodules and the
superproject index, so CI can remain credential-free and keep Argo CD
repo-server submodule initialization disabled.
"""

from __future__ import annotations

import argparse
import configparser
import json
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Iterable, Mapping, Sequence

API_VERSION = "oresoftware.dev/v1alpha1"
KIND = "GitOpsApplication"
SCHEMA_REFERENCE = "../application.schema.json"
DEFAULT_CATALOG_GLOB = "catalog/gitops/apps/*.json"
CLUSTER_REPOSITORY = "github.com/oresoftware/k8s-cluster"
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
DNS_LABEL_PATTERN = re.compile(r"^[a-z0-9](?:[-a-z0-9]*[a-z0-9])?$")

TOP_LEVEL_FIELDS = {"$schema", "apiVersion", "kind", "metadata", "spec"}
METADATA_FIELDS = {"name"}
SPEC_FIELDS = {"owner", "inventory", "source", "argo", "migration"}
INVENTORY_FIELDS = {"mode", "path", "repository", "revision"}
SOURCE_FIELDS = {"mode", "repository", "targetRevision", "path", "renderer"}
ARGO_FIELDS = {
    "project",
    "namespace",
    "destinationServer",
    "automated",
    "prune",
    "selfHeal",
}
MIGRATION_FIELDS = {"phase", "staticApplication"}
MIGRATION_PHASES = {"pilot-inert", "migration-ready", "active", "retired"}
RENDERERS = {"kustomize", "helm", "jsonnet", "plain-yaml"}


@dataclass(frozen=True)
class Diagnostic:
    rule_id: str
    message: str
    path: str
    application: str = ""
    severity: str = "error"


@dataclass(frozen=True)
class Report:
    valid: bool
    records: int
    errors: int
    warnings: int
    diagnostics: list[Diagnostic]

    def to_json(self) -> dict[str, Any]:
        return {
            "valid": self.valid,
            "records": self.records,
            "errors": self.errors,
            "warnings": self.warnings,
            "diagnostics": [asdict(item) for item in self.diagnostics],
        }


def normalize_repo_url(value: str) -> str:
    """Return a protocol-independent lower-case repository identity."""
    normalized = value.strip().replace("\\", "/").lower()
    prefixes = (
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
        "http://github.com/",
        "git://github.com/",
    )
    for prefix in prefixes:
        if normalized.startswith(prefix):
            normalized = "github.com/" + normalized.removeprefix(prefix)
            break
    normalized = normalized.rstrip("/")
    if normalized.endswith(".git"):
        normalized = normalized[:-4]
    return normalized


def safe_relative_path(value: str) -> bool:
    if not value or "\\" in value:
        return False
    path = PurePosixPath(value)
    return (
        not path.is_absolute()
        and all(part not in {"", ".", ".."} for part in path.parts)
        and ":" not in path.parts[0]
    )


def _run(command: Sequence[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )


def load_gitmodules(root: Path) -> dict[str, str]:
    path = root / ".gitmodules"
    if not path.is_file():
        return {}
    parser = configparser.ConfigParser(interpolation=None)
    parser.optionxform = str
    with path.open("r", encoding="utf-8") as handle:
        parser.read_file(handle)

    result: dict[str, str] = {}
    for section in parser.sections():
        if not section.startswith('submodule "') or not section.endswith('"'):
            continue
        module_path = parser.get(section, "path", fallback="").strip()
        repository = parser.get(section, "url", fallback="").strip()
        if module_path:
            result[module_path] = repository
    return dict(sorted(result.items()))


def tracked_gitlinks(root: Path) -> dict[str, str]:
    try:
        output = _run(["git", "ls-files", "--stage"], cwd=root).stdout
    except (OSError, subprocess.CalledProcessError):
        return {}

    result: dict[str, str] = {}
    for line in output.splitlines():
        fields = line.split(maxsplit=3)
        if len(fields) != 4 or fields[0] != "160000":
            continue
        sha = fields[1].lower()
        path = fields[3]
        if SHA_PATTERN.fullmatch(sha):
            result[path] = sha
    return dict(sorted(result.items()))


def load_records(root: Path, catalog_glob: str) -> list[tuple[Path, Any]]:
    paths = sorted(root.glob(catalog_glob))
    result: list[tuple[Path, Any]] = []
    for path in paths:
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            result.append((path, error))
        else:
            result.append((path, value))
    return result


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, dict) else {}


def _string(value: Any) -> str:
    return value if isinstance(value, str) else ""


def _bool(value: Any) -> bool | None:
    return value if isinstance(value, bool) else None


def _unexpected_fields(
    value: Mapping[str, Any],
    allowed: set[str],
    *,
    identity: str,
    path: str,
    app: str,
) -> list[Diagnostic]:
    return [
        Diagnostic(
            "catalog.unknown-field",
            f"{identity} contains unsupported field {field!r}",
            path,
            app,
        )
        for field in sorted(set(value) - allowed)
    ]


def _repository_slug(repository: str) -> str:
    normalized = normalize_repo_url(repository)
    return normalized.rsplit("/", 1)[-1] if "/" in normalized else normalized


def _path_exists_without_symlink_escape(root: Path, relative: str) -> bool:
    if not safe_relative_path(relative):
        return False
    candidate = root / relative
    if not candidate.is_file() or candidate.is_symlink():
        return False
    try:
        resolved_root = root.resolve(strict=True)
        resolved_parent = candidate.parent.resolve(strict=True)
    except OSError:
        return False
    return resolved_parent == resolved_root or resolved_root in resolved_parent.parents


def validate_records(
    loaded: Iterable[tuple[Path, Any]],
    *,
    root: Path,
    gitmodules: Mapping[str, str],
    gitlinks: Mapping[str, str],
    strict: bool = True,
) -> Report:
    diagnostics: list[Diagnostic] = []
    seen_names: dict[str, str] = {}
    seen_paths: dict[str, str] = {}
    record_count = 0

    for absolute_path, raw in loaded:
        record_count += 1
        try:
            relative_path = absolute_path.relative_to(root).as_posix()
        except ValueError:
            relative_path = absolute_path.as_posix()

        if isinstance(raw, Exception):
            diagnostics.append(
                Diagnostic(
                    "catalog.invalid-json",
                    f"cannot parse JSON: {raw}",
                    relative_path,
                )
            )
            continue
        if not isinstance(raw, dict):
            diagnostics.append(
                Diagnostic(
                    "catalog.invalid-root",
                    "catalog record must be a JSON object",
                    relative_path,
                )
            )
            continue

        metadata = _mapping(raw.get("metadata"))
        spec = _mapping(raw.get("spec"))
        inventory = _mapping(spec.get("inventory"))
        source = _mapping(spec.get("source"))
        argo = _mapping(spec.get("argo"))
        migration = _mapping(spec.get("migration"))
        app = _string(metadata.get("name"))

        if strict:
            diagnostics.extend(
                _unexpected_fields(
                    raw,
                    TOP_LEVEL_FIELDS,
                    identity="record",
                    path=relative_path,
                    app=app,
                )
            )
            diagnostics.extend(
                _unexpected_fields(
                    metadata,
                    METADATA_FIELDS,
                    identity="metadata",
                    path=relative_path,
                    app=app,
                )
            )
            diagnostics.extend(
                _unexpected_fields(
                    spec,
                    SPEC_FIELDS,
                    identity="spec",
                    path=relative_path,
                    app=app,
                )
            )
            diagnostics.extend(
                _unexpected_fields(
                    inventory,
                    INVENTORY_FIELDS,
                    identity="spec.inventory",
                    path=relative_path,
                    app=app,
                )
            )
            diagnostics.extend(
                _unexpected_fields(
                    source,
                    SOURCE_FIELDS,
                    identity="spec.source",
                    path=relative_path,
                    app=app,
                )
            )
            diagnostics.extend(
                _unexpected_fields(
                    argo,
                    ARGO_FIELDS,
                    identity="spec.argo",
                    path=relative_path,
                    app=app,
                )
            )
            diagnostics.extend(
                _unexpected_fields(
                    migration,
                    MIGRATION_FIELDS,
                    identity="spec.migration",
                    path=relative_path,
                    app=app,
                )
            )

        expected_header = {
            "$schema": SCHEMA_REFERENCE,
            "apiVersion": API_VERSION,
            "kind": KIND,
        }
        for field, expected in expected_header.items():
            if raw.get(field) != expected:
                diagnostics.append(
                    Diagnostic(
                        "catalog.header",
                        f"{field} must equal {expected!r}",
                        relative_path,
                        app,
                    )
                )

        if not app or not DNS_LABEL_PATTERN.fullmatch(app):
            diagnostics.append(
                Diagnostic(
                    "catalog.application-name",
                    "metadata.name must be a non-empty DNS label",
                    relative_path,
                    app,
                )
            )
        elif absolute_path.stem != app:
            diagnostics.append(
                Diagnostic(
                    "catalog.filename",
                    f"file stem must equal metadata.name ({app})",
                    relative_path,
                    app,
                )
            )
        if app:
            previous = seen_names.get(app)
            if previous:
                diagnostics.append(
                    Diagnostic(
                        "catalog.duplicate-application",
                        f"application is already declared in {previous}",
                        relative_path,
                        app,
                    )
                )
            else:
                seen_names[app] = relative_path

        owner = _string(spec.get("owner"))
        if not owner or "/" in owner or owner.strip() != owner:
            diagnostics.append(
                Diagnostic(
                    "catalog.owner",
                    "spec.owner must be one non-empty repository-owner slug",
                    relative_path,
                    app,
                )
            )

        inventory_mode = _string(inventory.get("mode"))
        inventory_path = _string(inventory.get("path"))
        inventory_repository = _string(inventory.get("repository"))
        inventory_revision = _string(inventory.get("revision")).lower()
        if inventory_mode != "git-submodule":
            diagnostics.append(
                Diagnostic(
                    "inventory.mode",
                    "spec.inventory.mode must equal 'git-submodule'",
                    relative_path,
                    app,
                )
            )
        if not safe_relative_path(inventory_path) or not inventory_path.startswith(
            "remote/deployments/"
        ):
            diagnostics.append(
                Diagnostic(
                    "inventory.path",
                    "inventory path must be a safe path under remote/deployments/",
                    relative_path,
                    app,
                )
            )
        elif inventory_path in seen_paths:
            diagnostics.append(
                Diagnostic(
                    "inventory.duplicate-path",
                    f"inventory path is already owned by {seen_paths[inventory_path]}",
                    relative_path,
                    app,
                )
            )
        else:
            seen_paths[inventory_path] = app or relative_path

        if not inventory_repository or not normalize_repo_url(inventory_repository).startswith(
            "github.com/"
        ):
            diagnostics.append(
                Diagnostic(
                    "inventory.repository",
                    "inventory repository must be an explicit GitHub repository URL",
                    relative_path,
                    app,
                )
            )
        if not SHA_PATTERN.fullmatch(inventory_revision):
            diagnostics.append(
                Diagnostic(
                    "inventory.revision",
                    "inventory revision must be an exact lowercase 40-hex commit",
                    relative_path,
                    app,
                )
            )

        configured_repository = gitmodules.get(inventory_path)
        if configured_repository is None:
            diagnostics.append(
                Diagnostic(
                    "inventory.gitmodules-entry",
                    f"{inventory_path!r} is not declared in .gitmodules",
                    relative_path,
                    app,
                )
            )
        elif normalize_repo_url(configured_repository) != normalize_repo_url(
            inventory_repository
        ):
            diagnostics.append(
                Diagnostic(
                    "inventory.repository-drift",
                    ".gitmodules URL and catalog inventory repository differ",
                    relative_path,
                    app,
                )
            )

        indexed_revision = gitlinks.get(inventory_path)
        if indexed_revision is None:
            diagnostics.append(
                Diagnostic(
                    "inventory.gitlink",
                    f"{inventory_path!r} is not an indexed gitlink",
                    relative_path,
                    app,
                )
            )
        elif inventory_revision and indexed_revision != inventory_revision:
            diagnostics.append(
                Diagnostic(
                    "inventory.gitlink-drift",
                    f"catalog revision {inventory_revision} does not match gitlink {indexed_revision}",
                    relative_path,
                    app,
                )
            )

        source_mode = _string(source.get("mode"))
        source_repository = _string(source.get("repository"))
        target_revision = _string(source.get("targetRevision")).lower()
        source_path = _string(source.get("path"))
        renderer = _string(source.get("renderer"))
        if source_mode != "direct-repository":
            diagnostics.append(
                Diagnostic(
                    "source.mode",
                    "spec.source.mode must equal 'direct-repository'",
                    relative_path,
                    app,
                )
            )
        if normalize_repo_url(source_repository) == CLUSTER_REPOSITORY:
            diagnostics.append(
                Diagnostic(
                    "source.cluster-repository",
                    "Argo CD must render the upstream app repository, not a path inside k8s-cluster",
                    relative_path,
                    app,
                )
            )
        if normalize_repo_url(source_repository) != normalize_repo_url(
            inventory_repository
        ):
            diagnostics.append(
                Diagnostic(
                    "source.repository-drift",
                    "Argo source repository must equal the submodule upstream repository",
                    relative_path,
                    app,
                )
            )
        if not SHA_PATTERN.fullmatch(target_revision):
            diagnostics.append(
                Diagnostic(
                    "source.target-revision",
                    "source.targetRevision must be an exact lowercase 40-hex commit",
                    relative_path,
                    app,
                )
            )
        elif target_revision != inventory_revision:
            diagnostics.append(
                Diagnostic(
                    "source.pin-drift",
                    "source.targetRevision must equal the inventory gitlink revision",
                    relative_path,
                    app,
                )
            )
        if not safe_relative_path(source_path):
            diagnostics.append(
                Diagnostic(
                    "source.path",
                    "source.path must be a safe non-empty repository-relative path",
                    relative_path,
                    app,
                )
            )
        if renderer not in RENDERERS:
            diagnostics.append(
                Diagnostic(
                    "source.renderer",
                    f"source.renderer must be one of {sorted(RENDERERS)}",
                    relative_path,
                    app,
                )
            )

        repository_slug = _repository_slug(inventory_repository)
        if (
            app.endswith("-infra")
            or inventory_path.rstrip("/").rsplit("/", 1)[-1].endswith("-infra")
            or repository_slug.endswith("-infra")
        ):
            diagnostics.append(
                Diagnostic(
                    "policy.infra-is-not-app",
                    "*-infra repositories cannot be classified as deployable app submodules",
                    relative_path,
                    app,
                )
            )

        project = _string(argo.get("project"))
        namespace = _string(argo.get("namespace"))
        destination_server = _string(argo.get("destinationServer"))
        automated = _bool(argo.get("automated"))
        prune = _bool(argo.get("prune"))
        self_heal = _bool(argo.get("selfHeal"))
        if not project or project == "default":
            diagnostics.append(
                Diagnostic(
                    "argo.project",
                    "Argo project must be explicit and cannot be 'default'",
                    relative_path,
                    app,
                )
            )
        if not namespace or namespace == "default":
            diagnostics.append(
                Diagnostic(
                    "argo.namespace",
                    "destination namespace must be explicit and cannot be 'default'",
                    relative_path,
                    app,
                )
            )
        if not destination_server:
            diagnostics.append(
                Diagnostic(
                    "argo.destination",
                    "destinationServer must be a non-empty string",
                    relative_path,
                    app,
                )
            )
        for field, value in (
            ("automated", automated),
            ("prune", prune),
            ("selfHeal", self_heal),
        ):
            if value is None:
                diagnostics.append(
                    Diagnostic(
                        "argo.boolean",
                        f"spec.argo.{field} must be a boolean",
                        relative_path,
                        app,
                    )
                )

        phase = _string(migration.get("phase"))
        static_application = _string(migration.get("staticApplication"))
        if phase not in MIGRATION_PHASES:
            diagnostics.append(
                Diagnostic(
                    "migration.phase",
                    f"migration.phase must be one of {sorted(MIGRATION_PHASES)}",
                    relative_path,
                    app,
                )
            )
        if phase == "pilot-inert" and any(
            value is True for value in (automated, prune, self_heal)
        ):
            diagnostics.append(
                Diagnostic(
                    "migration.inert-sync",
                    "pilot-inert records must disable automated sync, prune, and self-heal",
                    relative_path,
                    app,
                )
            )
        if not safe_relative_path(static_application):
            diagnostics.append(
                Diagnostic(
                    "migration.static-application",
                    "staticApplication must be a safe repository-relative path",
                    relative_path,
                    app,
                )
            )
        elif not _path_exists_without_symlink_escape(root, static_application):
            diagnostics.append(
                Diagnostic(
                    "migration.static-application-missing",
                    f"static Application path does not exist: {static_application}",
                    relative_path,
                    app,
                )
            )

    if record_count == 0:
        diagnostics.append(
            Diagnostic(
                "catalog.empty",
                "no catalog records matched the configured catalog glob",
                "catalog/gitops/apps",
            )
        )

    diagnostics.sort(
        key=lambda item: (
            item.severity != "error",
            item.path,
            item.application,
            item.rule_id,
            item.message,
        )
    )
    errors = sum(item.severity == "error" for item in diagnostics)
    warnings = sum(item.severity == "warning" for item in diagnostics)
    return Report(
        valid=errors == 0,
        records=record_count,
        errors=errors,
        warnings=warnings,
        diagnostics=diagnostics,
    )


def render_application(record: Mapping[str, Any]) -> dict[str, Any]:
    metadata = _mapping(record.get("metadata"))
    spec = _mapping(record.get("spec"))
    source = _mapping(spec.get("source"))
    argo = _mapping(spec.get("argo"))
    migration = _mapping(spec.get("migration"))
    name = _string(metadata.get("name"))
    phase = _string(migration.get("phase"))
    rendered_name = f"catalog-pilot-{name}" if phase == "pilot-inert" else name

    application: dict[str, Any] = {
        "apiVersion": "argoproj.io/v1alpha1",
        "kind": "Application",
        "metadata": {
            "name": rendered_name,
            "namespace": "argocd",
            "annotations": {
                "oresoftware.dev/composition-contract": API_VERSION,
                "oresoftware.dev/source-application": name,
            },
        },
        "spec": {
            "project": _string(argo.get("project")),
            "source": {
                "repoURL": _string(source.get("repository")),
                "targetRevision": _string(source.get("targetRevision")),
                "path": _string(source.get("path")),
            },
            "destination": {
                "server": _string(argo.get("destinationServer")),
                "namespace": _string(argo.get("namespace")),
            },
        },
    }

    if argo.get("automated") is True:
        application["spec"]["syncPolicy"] = {
            "automated": {
                "prune": argo.get("prune") is True,
                "selfHeal": argo.get("selfHeal") is True,
            }
        }
    return application


def render_records(loaded: Iterable[tuple[Path, Any]]) -> list[dict[str, Any]]:
    values = [raw for _, raw in loaded if isinstance(raw, dict)]
    values.sort(key=lambda value: _string(_mapping(value.get("metadata")).get("name")))
    return [render_application(value) for value in values]


def print_human(report: Report) -> None:
    state = "valid" if report.valid else "invalid"
    print(
        f"GitOps composition contract: {state} "
        f"({report.records} records, {report.errors} errors, "
        f"{report.warnings} warnings)"
    )
    for item in report.diagnostics:
        app = f" [{item.application}]" if item.application else ""
        print(
            f"{item.severity}: {item.path}{app}: "
            f"{item.rule_id}: {item.message}"
        )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate exact submodule pins against direct Argo CD sources."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    check = subparsers.add_parser("check", help="validate catalog records")
    check.add_argument("--root", type=Path, default=Path("."))
    check.add_argument("--catalog-glob", default=DEFAULT_CATALOG_GLOB)
    check.add_argument("--format", choices=("human", "json"), default="human")
    check.add_argument(
        "--no-strict",
        action="store_true",
        help="allow unknown catalog fields during a staged schema migration",
    )

    render = subparsers.add_parser(
        "render",
        help="render deterministic preview Applications without applying them",
    )
    render.add_argument("--root", type=Path, default=Path("."))
    render.add_argument("--catalog-glob", default=DEFAULT_CATALOG_GLOB)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    root = args.root.resolve()
    loaded = load_records(root, args.catalog_glob)
    report = validate_records(
        loaded,
        root=root,
        gitmodules=load_gitmodules(root),
        gitlinks=tracked_gitlinks(root),
        strict=not getattr(args, "no_strict", False),
    )

    if args.command == "render":
        if not report.valid:
            print_human(report)
            return 2
        print(
            json.dumps(
                {
                    "apiVersion": API_VERSION,
                    "kind": "GitOpsApplicationPreviewList",
                    "items": render_records(loaded),
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    if args.format == "json":
        print(json.dumps(report.to_json(), indent=2, sort_keys=True))
    else:
        print_human(report)
    return 0 if report.valid else 2


if __name__ == "__main__":
    sys.exit(main())
