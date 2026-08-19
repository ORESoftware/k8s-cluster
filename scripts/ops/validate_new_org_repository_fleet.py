#!/usr/bin/env python3
"""Materialize and validate one representative repository for every seed role."""

from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Mapping, Sequence
from typing import Any

from new_org_repository_templates import SEED_BUILDERS, files_for_repository
from publish_new_org_repository_fleet import FleetError, validate_manifest


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=pathlib.Path,
        default=pathlib.Path(__file__).with_name("new_org_repository_fleet.json"),
    )
    parser.add_argument(
        "--workspace",
        type=pathlib.Path,
        help="Materialization directory. A temporary directory is used by default.",
    )
    parser.add_argument(
        "--static-only",
        action="store_true",
        help="Materialize and run Python contracts without external language toolchains.",
    )
    parser.add_argument("--keep-workspace", action="store_true")
    return parser.parse_args(argv)


def _representatives(
    flattened: list[tuple[Mapping[str, Any], Mapping[str, Any]]],
) -> dict[str, tuple[Mapping[str, Any], Mapping[str, Any]]]:
    representatives: dict[str, tuple[Mapping[str, Any], Mapping[str, Any]]] = {}
    for organization, repository in flattened:
        representatives.setdefault(str(repository["role"]), (organization, repository))
    if set(representatives) != set(SEED_BUILDERS):
        missing = sorted(set(SEED_BUILDERS) - set(representatives))
        extra = sorted(set(representatives) - set(SEED_BUILDERS))
        raise FleetError(f"representative role mismatch; missing={missing}, extra={extra}")
    return representatives


def _materialize(
    root: pathlib.Path,
    role: str,
    organization: Mapping[str, Any],
    repository: Mapping[str, Any],
) -> pathlib.Path:
    target = root / role
    if target.exists():
        shutil.rmtree(target)
    for relative, content in files_for_repository(organization, repository).items():
        path = target / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    return target


def _run(command: list[str], *, cwd: pathlib.Path) -> None:
    rendered = " ".join(command)
    print(json.dumps({"event": "command_start", "cwd": str(cwd), "command": rendered}), flush=True)
    subprocess.run(command, cwd=cwd, check=True)
    print(json.dumps({"event": "command_complete", "cwd": str(cwd), "command": rendered}), flush=True)


def _run_python_contracts(target: pathlib.Path) -> None:
    tests = target / "tests"
    if tests.is_dir():
        _run(
            [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py", "-v"],
            cwd=target,
        )


def _run_cargo(target: pathlib.Path, *, manifest: pathlib.Path) -> None:
    manifest_arg = str(manifest.relative_to(target))
    _run(["cargo", "fmt", "--manifest-path", manifest_arg, "--", "--check"], cwd=target)
    _run(
        ["cargo", "clippy", "--manifest-path", manifest_arg, "--all-targets", "--", "-D", "warnings"],
        cwd=target,
    )
    _run(["cargo", "test", "--manifest-path", manifest_arg, "--all-targets"], cwd=target)


def _run_external_toolchains(targets: Mapping[str, pathlib.Path]) -> None:
    for role in ("cli", "sync", "server", "mcp"):
        _run_cargo(targets[role], manifest=targets[role] / "Cargo.toml")

    clients = targets["clients"]
    _run_cargo(clients, manifest=clients / "clients/rust/Cargo.toml")
    _run(["go", "test", "./..."], cwd=clients / "clients/go")
    _run(["mvn", "-q", "test"], cwd=clients / "clients/java")
    _run(
        [
            "npx",
            "--yes",
            "--package",
            "typescript@5.9.2",
            "tsc",
            "--project",
            "tsconfig.json",
        ],
        cwd=clients / "clients/typescript",
    )


def validate(workspace: pathlib.Path, manifest_path: pathlib.Path, *, static_only: bool) -> dict[str, Any]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    flattened = validate_manifest(manifest)
    representatives = _representatives(flattened)
    targets: dict[str, pathlib.Path] = {}
    for role, (organization, repository) in sorted(representatives.items()):
        target = _materialize(workspace, role, organization, repository)
        targets[role] = target
        _run_python_contracts(target)

    if not static_only:
        _run_external_toolchains(targets)

    summary = {
        "status": "ok",
        "static_only": static_only,
        "roles": sorted(targets),
        "materialized_repositories": {
            role: f"{representatives[role][0]['owner']}/{representatives[role][1]['name']}"
            for role in sorted(representatives)
        },
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return summary


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    temporary: tempfile.TemporaryDirectory[str] | None = None
    try:
        if args.workspace is None:
            temporary = tempfile.TemporaryDirectory(prefix="new-org-fleet-")
            workspace = pathlib.Path(temporary.name)
        else:
            workspace = args.workspace.resolve()
            workspace.mkdir(parents=True, exist_ok=True)
        validate(workspace, args.manifest.resolve(), static_only=args.static_only)
        if args.keep_workspace:
            if temporary is not None:
                retained = pathlib.Path.cwd() / "new-org-fleet-validation"
                if retained.exists():
                    shutil.rmtree(retained)
                shutil.copytree(workspace, retained)
                print(f"retained materialized workspace at {retained}")
            else:
                print(f"retained materialized workspace at {workspace}")
        return 0
    except (FleetError, OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"new-org fleet validation failed: {error}", file=sys.stderr)
        return 1
    finally:
        if temporary is not None:
            temporary.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
