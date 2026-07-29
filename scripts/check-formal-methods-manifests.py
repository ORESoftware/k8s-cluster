#!/usr/bin/env python3
"""Validate schema-v1 formal/fm.toml contracts at reviewed app gitlinks."""

from __future__ import annotations

import argparse
import copy
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


EXPECTED_ADOPTERS = (
    "apps/fiducia-node.rs",
    "apps/fiducia-ai-agent-bridge.rs",
    "apps/fiducia-brain.rs",
    "apps/fiducia-ai-agent-control-plane",
)
REQUIRED_OUTPUT_FORMATS = {"human", "json", "junit", "sarif", "itf"}
EXACT_VERSION = re.compile(r"^[0-9]+(?:\.[0-9]+)*$")


class ManifestError(ValueError):
    """A manifest violates the fleet contract."""


@dataclass(frozen=True)
class Model:
    name: str
    data: dict[str, Any]


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ManifestError(message)


def require_table(value: Any, field: str) -> dict[str, Any]:
    require(isinstance(value, dict), f"{field} must be a table")
    return value


def require_nonempty_strings(value: Any, field: str) -> list[str]:
    require(isinstance(value, list) and value, f"{field} must be a non-empty array")
    require(
        all(isinstance(item, str) and item.strip() for item in value),
        f"{field} must contain only non-empty strings",
    )
    require(len(set(value)) == len(value), f"{field} must not contain duplicates")
    return value


def require_positive_int(value: Any, field: str) -> int:
    require(
        isinstance(value, int) and not isinstance(value, bool) and value > 0,
        f"{field} must be a positive integer",
    )
    return value


def resolve_repo_file(repo: Path, relative: Any, field: str) -> Path:
    require(isinstance(relative, str) and relative, f"{field} must be a path")
    require(not Path(relative).is_absolute(), f"{field} must be repository-relative")
    resolved = (repo / relative).resolve()
    require(
        resolved.is_relative_to(repo.resolve()),
        f"{field} escapes the repository: {relative}",
    )
    require(resolved.is_file(), f"{field} does not exist: {relative}")
    return resolved


def normalize_models(data: dict[str, Any]) -> list[Model]:
    raw_models = data.get("models")
    if raw_models is not None:
        require(
            isinstance(raw_models, list) and raw_models,
            "models must be a non-empty array of tables",
        )
        models = []
        for index, raw_model in enumerate(raw_models):
            model = require_table(raw_model, f"models[{index}]")
            name = model.get("name")
            require(
                isinstance(name, str) and name.strip(),
                f"models[{index}].name must be non-empty",
            )
            models.append(Model(name=name, data=model))
        return models

    name = data.get("model")
    require(isinstance(name, str) and name.strip(), "model must be non-empty")
    model_fields = {
        key: data[key]
        for key in (
            "language",
            "spec",
            "tests",
            "main",
            "init",
            "step",
            "invariants",
            "witnesses",
            "bounds",
            "simulation",
            "verification",
            "traces",
            "adapters",
        )
        if key in data
    }
    return [Model(name=name, data=model_fields)]


def validate_toolchain(data: dict[str, Any]) -> None:
    toolchain = require_table(data.get("toolchain"), "toolchain")
    for tool in ("quint", "node", "java", "rust"):
        version = toolchain.get(tool)
        require(
            isinstance(version, str) and EXACT_VERSION.fullmatch(version) is not None,
            f"toolchain.{tool} must be an exact numeric version",
        )


def validate_outputs(data: dict[str, Any]) -> None:
    outputs = require_table(data.get("outputs"), "outputs")
    formats = set(require_nonempty_strings(outputs.get("formats"), "outputs.formats"))
    missing = REQUIRED_OUTPUT_FORMATS - formats
    require(not missing, f"outputs.formats is missing: {sorted(missing)}")
    artifact_directory = outputs.get("artifact_directory")
    require(
        isinstance(artifact_directory, str) and artifact_directory,
        "outputs.artifact_directory must be non-empty",
    )
    artifact_path = Path(artifact_directory)
    require(
        not artifact_path.is_absolute() and ".." not in artifact_path.parts,
        "outputs.artifact_directory must stay inside the repository",
    )


def validate_model(repo: Path, model: Model) -> None:
    prefix = f"model {model.name}"
    data = model.data
    require(data.get("language") == "quint", f"{prefix}.language must be quint")
    resolve_repo_file(repo, data.get("spec"), f"{prefix}.spec")
    if "tests" in data:
        resolve_repo_file(repo, data["tests"], f"{prefix}.tests")
    for field in ("main", "init", "step"):
        value = data.get(field)
        require(
            isinstance(value, str) and value.strip(),
            f"{prefix}.{field} must be non-empty",
        )
    require_nonempty_strings(data.get("invariants"), f"{prefix}.invariants")
    require_nonempty_strings(data.get("witnesses"), f"{prefix}.witnesses")

    bounds = require_table(data.get("bounds"), f"{prefix}.bounds")
    require(bounds, f"{prefix}.bounds must not be empty")
    for name, value in bounds.items():
        require_positive_int(value, f"{prefix}.bounds.{name}")

    simulation = require_table(data.get("simulation"), f"{prefix}.simulation")
    require_positive_int(simulation.get("max_samples"), f"{prefix}.simulation.max_samples")
    require_positive_int(simulation.get("max_steps"), f"{prefix}.simulation.max_steps")

    verification = require_table(data.get("verification"), f"{prefix}.verification")
    require(
        verification.get("backend") in {"tlc", "apalache"},
        f"{prefix}.verification.backend must be tlc or apalache",
    )

    traces = require_table(data.get("traces"), f"{prefix}.traces")
    require(traces.get("format") == "itf", f"{prefix}.traces.format must be itf")
    require_positive_int(traces.get("count"), f"{prefix}.traces.count")
    require_positive_int(traces.get("max_steps"), f"{prefix}.traces.max_steps")

    adapters = require_table(data.get("adapters"), f"{prefix}.adapters")
    require(adapters, f"{prefix}.adapters must not be empty")
    for language, raw_adapter in adapters.items():
        adapter = require_table(raw_adapter, f"{prefix}.adapters.{language}")
        status = adapter.get("status")
        require(
            status in {"implemented", "planned"},
            f"{prefix}.adapters.{language}.status must be implemented or planned",
        )
        strategy = adapter.get("strategy")
        require(
            isinstance(strategy, str) and strategy.strip(),
            f"{prefix}.adapters.{language}.strategy must be non-empty",
        )
        if status == "implemented":
            resolve_repo_file(
                repo,
                adapter.get("target"),
                f"{prefix}.adapters.{language}.target",
            )


def validate_manifest(repo: Path, data: dict[str, Any]) -> list[Model]:
    require(data.get("schema_version") == 1, "schema_version must equal 1")
    project = data.get("project")
    require(isinstance(project, str) and project.strip(), "project must be non-empty")
    validate_toolchain(data)
    validate_outputs(data)
    models = normalize_models(data)
    names = [model.name for model in models]
    require(len(set(names)) == len(names), "model names must be unique")
    for model in models:
        validate_model(repo, model)
    return models


def load_and_validate(root: Path) -> list[tuple[str, list[Model], dict[str, Any]]]:
    results = []
    for relative_repo in EXPECTED_ADOPTERS:
        repo = root / relative_repo
        manifest_path = repo / "formal" / "fm.toml"
        require(
            manifest_path.is_file(),
            f"{relative_repo} is not initialized or has no formal/fm.toml",
        )
        with manifest_path.open("rb") as manifest_file:
            data = tomllib.load(manifest_file)
        models = validate_manifest(repo, data)
        results.append((relative_repo, models, data))
    return results


def expect_invalid(repo: Path, data: dict[str, Any], expected: str) -> None:
    try:
        validate_manifest(repo, data)
    except ManifestError as error:
        require(expected in str(error), f"self-test expected '{expected}', got '{error}'")
    else:
        raise ManifestError(f"self-test unexpectedly accepted invalid case: {expected}")


def run_self_tests(results: list[tuple[str, list[Model], dict[str, Any]]], root: Path) -> None:
    _, _, single = results[0]
    single_repo = root / EXPECTED_ADOPTERS[0]

    invalid = copy.deepcopy(single)
    invalid["schema_version"] = 2
    expect_invalid(single_repo, invalid, "schema_version")

    invalid = copy.deepcopy(single)
    invalid["spec"] = "formal/does-not-exist.qnt"
    expect_invalid(single_repo, invalid, "does not exist")

    invalid = copy.deepcopy(single)
    invalid["adapters"]["rust"]["target"] = "tests/does-not-exist.rs"
    expect_invalid(single_repo, invalid, "does not exist")

    invalid = copy.deepcopy(single)
    invalid["outputs"]["artifact_directory"] = "/tmp/formal"
    expect_invalid(single_repo, invalid, "inside the repository")

    brain_data = copy.deepcopy(results[2][2])
    brain_data["models"].append(copy.deepcopy(brain_data["models"][0]))
    expect_invalid(
        root / EXPECTED_ADOPTERS[2],
        brain_data,
        "model names must be unique",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="fiducia-monorepo root",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="also prove malformed manifests fail closed",
    )
    args = parser.parse_args()

    try:
        root = args.root.resolve()
        results = load_and_validate(root)
        if args.self_test:
            run_self_tests(results, root)
    except (ManifestError, tomllib.TOMLDecodeError, OSError) as error:
        print(f"formal-methods manifest validation failed: {error}", file=sys.stderr)
        return 1

    for relative_repo, models, _ in results:
        model_names = ", ".join(model.name for model in models)
        print(f"validated {relative_repo}: {model_names}")
    if args.self_test:
        print("validated fail-closed manifest self-tests")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
