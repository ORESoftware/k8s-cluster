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
MANIFESTS_BY_ADOPTER = {
    "apps/fiducia-node.rs": ("formal/fm.toml",),
    "apps/fiducia-ai-agent-bridge.rs": ("formal/fm.toml",),
    "apps/fiducia-brain.rs": (
        "formal/fm.toml",
        "formal/fm-reconfiguration.toml",
    ),
    "apps/fiducia-ai-agent-control-plane": ("formal/fm.toml",),
}
PUBLIC_ADOPTERS = (
    "apps/fiducia-node.rs",
    "apps/fiducia-brain.rs",
)
VERSION_TOKEN = re.compile(r"^[A-Za-z0-9.+_-]+$")
QUINT_IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


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
    name = data.get("model")
    require(isinstance(name, str) and name.strip(), "model must be non-empty")
    model_fields = {
        key: data[key]
        for key in (
            "language",
            "spec",
            "main",
            "init",
            "step",
            "invariants",
            "witnesses",
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
    allowed = {"quint", "java", "node", "rust", "npx"}
    require(
        set(toolchain) <= allowed,
        f"toolchain has unsupported fields: {sorted(set(toolchain) - allowed)}",
    )
    for tool in ("quint", "java"):
        version = toolchain.get(tool)
        require(
            isinstance(version, str) and VERSION_TOKEN.fullmatch(version) is not None,
            f"toolchain.{tool} must be a pinned version token",
        )
    for tool in ("node", "rust"):
        version = toolchain.get(tool)
        if version is not None:
            require(
                isinstance(version, str) and VERSION_TOKEN.fullmatch(version) is not None,
                f"toolchain.{tool} must be a pinned version token",
            )
    npx = toolchain.get("npx", "npx")
    require(
        isinstance(npx, str) and npx and not any(character.isspace() for character in npx),
        "toolchain.npx must be one executable token",
    )


def validate_execution(data: dict[str, Any]) -> None:
    execution = require_table(data.get("execution"), "execution")
    allowed = {"timeout_seconds", "max_output_bytes", "artifacts_dir"}
    require(
        set(execution) <= allowed,
        f"execution has unsupported fields: {sorted(set(execution) - allowed)}",
    )
    require_positive_int(execution.get("timeout_seconds"), "execution.timeout_seconds")
    max_output_bytes = require_positive_int(
        execution.get("max_output_bytes"),
        "execution.max_output_bytes",
    )
    require(max_output_bytes >= 1024, "execution.max_output_bytes must be at least 1024")
    artifact_directory = execution.get("artifacts_dir")
    require(
        isinstance(artifact_directory, str) and artifact_directory,
        "execution.artifacts_dir must be non-empty",
    )
    artifact_path = Path(artifact_directory)
    require(
        not artifact_path.is_absolute() and ".." not in artifact_path.parts,
        "execution.artifacts_dir must stay inside the repository",
    )


def validate_model(repo: Path, model: Model) -> None:
    prefix = f"model {model.name}"
    data = model.data
    require(data.get("language") == "quint", f"{prefix}.language must be quint")
    resolve_repo_file(repo, data.get("spec"), f"{prefix}.spec")
    for field in ("main", "init", "step"):
        value = data.get(field)
        require(
            isinstance(value, str) and QUINT_IDENTIFIER.fullmatch(value) is not None,
            f"{prefix}.{field} must be a Quint-compatible identifier",
        )
    require_nonempty_strings(data.get("invariants"), f"{prefix}.invariants")
    require_nonempty_strings(data.get("witnesses"), f"{prefix}.witnesses")

    simulation = require_table(data.get("simulation"), f"{prefix}.simulation")
    simulation_allowed = {"backend", "max_samples", "max_steps"}
    require(
        set(simulation) <= simulation_allowed,
        f"{prefix}.simulation has unsupported fields: "
        f"{sorted(set(simulation) - simulation_allowed)}",
    )
    require(
        simulation.get("backend") in {"typescript", "rust"},
        f"{prefix}.simulation.backend must be typescript or rust",
    )
    require_positive_int(simulation.get("max_samples"), f"{prefix}.simulation.max_samples")
    require_positive_int(simulation.get("max_steps"), f"{prefix}.simulation.max_steps")

    verification = require_table(data.get("verification"), f"{prefix}.verification")
    verification_allowed = {"backend", "exhaustive_finite_model", "max_steps"}
    require(
        set(verification) <= verification_allowed,
        f"{prefix}.verification has unsupported fields: "
        f"{sorted(set(verification) - verification_allowed)}",
    )
    require(
        verification.get("backend") in {"tlc", "apalache"},
        f"{prefix}.verification.backend must be tlc or apalache",
    )
    if "exhaustive_finite_model" in verification:
        require(
            isinstance(verification["exhaustive_finite_model"], bool),
            f"{prefix}.verification.exhaustive_finite_model must be boolean",
        )
    if verification.get("backend") == "tlc":
        require(
            "max_steps" not in verification,
            f"{prefix}.verification.max_steps is only valid for Apalache",
        )
    elif "max_steps" in verification:
        require_positive_int(
            verification["max_steps"],
            f"{prefix}.verification.max_steps",
        )

    traces = require_table(data.get("traces"), f"{prefix}.traces")
    trace_allowed = {
        "format",
        "model_based_testing_metadata",
        "backend",
        "seed",
        "count",
        "max_steps",
        "max_samples",
        "required_actions",
    }
    require(
        set(traces) <= trace_allowed,
        f"{prefix}.traces has unsupported fields: "
        f"{sorted(set(traces) - trace_allowed)}",
    )
    require(traces.get("format") == "itf", f"{prefix}.traces.format must be itf")
    require(
        isinstance(traces.get("model_based_testing_metadata", False), bool),
        f"{prefix}.traces.model_based_testing_metadata must be boolean",
    )
    if "backend" in traces:
        require(
            traces["backend"] in {"typescript", "rust"},
            f"{prefix}.traces.backend must be typescript or rust",
        )
    if "seed" in traces:
        require(
            isinstance(traces["seed"], str)
            and VERSION_TOKEN.fullmatch(traces["seed"]) is not None,
            f"{prefix}.traces.seed must be one deterministic token",
        )
    require_positive_int(traces.get("count"), f"{prefix}.traces.count")
    require_positive_int(traces.get("max_steps"), f"{prefix}.traces.max_steps")
    if "max_samples" in traces:
        require_positive_int(traces["max_samples"], f"{prefix}.traces.max_samples")
    if "required_actions" in traces:
        require_nonempty_strings(
            traces["required_actions"],
            f"{prefix}.traces.required_actions",
        )

    adapters = require_table(data.get("adapters"), f"{prefix}.adapters")
    require(adapters, f"{prefix}.adapters must not be empty")
    for language, raw_adapter in adapters.items():
        adapter = require_table(raw_adapter, f"{prefix}.adapters.{language}")
        adapter_allowed = {
            "strategy",
            "target",
            "implementation",
            "observable_state",
            "issue",
            "status",
            "command",
            "working_directory",
            "environment",
        }
        require(
            set(adapter) <= adapter_allowed,
            f"{prefix}.adapters.{language} has unsupported fields: "
            f"{sorted(set(adapter) - adapter_allowed)}",
        )
        status = adapter.get("status")
        require(
            status in {"active", "planned", "disabled"},
            f"{prefix}.adapters.{language}.status must be active, planned, or disabled",
        )
        strategy = adapter.get("strategy")
        require(
            isinstance(strategy, str) and strategy.strip(),
            f"{prefix}.adapters.{language}.strategy must be non-empty",
        )
        if "target" in adapter:
            resolve_repo_file(
                repo,
                adapter.get("target"),
                f"{prefix}.adapters.{language}.target",
            )
        if "observable_state" in adapter:
            require_nonempty_strings(
                adapter["observable_state"],
                f"{prefix}.adapters.{language}.observable_state",
            )
        command = adapter.get("command", [])
        require(
            isinstance(command, list)
            and all(isinstance(item, str) and item for item in command),
            f"{prefix}.adapters.{language}.command must contain non-empty strings",
        )
        if status == "active":
            require(command, f"{prefix}.adapters.{language} active adapter needs command")
        environment = adapter.get("environment", {})
        require(
            isinstance(environment, dict)
            and all(
                isinstance(key, str)
                and key
                and "=" not in key
                and isinstance(value, str)
                for key, value in environment.items()
            ),
            f"{prefix}.adapters.{language}.environment is invalid",
        )


def validate_manifest(repo: Path, data: dict[str, Any]) -> list[Model]:
    allowed = {
        "schema_version",
        "project",
        "model",
        "language",
        "spec",
        "main",
        "init",
        "step",
        "invariants",
        "witnesses",
        "toolchain",
        "execution",
        "simulation",
        "verification",
        "traces",
        "adapters",
    }
    require(
        set(data) <= allowed,
        f"manifest has unsupported fields: {sorted(set(data) - allowed)}",
    )
    require(data.get("schema_version") == 1, "schema_version must equal 1")
    project = data.get("project")
    require(isinstance(project, str) and project.strip(), "project must be non-empty")
    validate_toolchain(data)
    validate_execution(data)
    models = normalize_models(data)
    for model in models:
        validate_model(repo, model)
    return models


def validate_unique_model_names(relative_repo: str, names: list[str]) -> None:
    require(
        len(names) == len(set(names)),
        f"{relative_repo} model names must be unique across manifests",
    )


def load_and_validate(
    root: Path,
    adopters: tuple[str, ...],
) -> list[tuple[str, list[Model], dict[str, Any]]]:
    results = []
    for relative_repo in adopters:
        repo = root / relative_repo
        repository_models = []
        for relative_manifest in MANIFESTS_BY_ADOPTER[relative_repo]:
            manifest_path = repo / relative_manifest
            label = f"{relative_repo}/{relative_manifest}"
            require(
                manifest_path.is_file(),
                f"{label} is not initialized or does not exist",
            )
            with manifest_path.open("rb") as manifest_file:
                data = tomllib.load(manifest_file)
            models = validate_manifest(repo, data)
            repository_models.extend(model.name for model in models)
            results.append((label, models, data))
        validate_unique_model_names(relative_repo, repository_models)
    return results


def expect_invalid(repo: Path, data: dict[str, Any], expected: str) -> None:
    try:
        validate_manifest(repo, data)
    except ManifestError as error:
        require(expected in str(error), f"self-test expected '{expected}', got '{error}'")
    else:
        raise ManifestError(f"self-test unexpectedly accepted invalid case: {expected}")


def run_self_tests(results: list[tuple[str, list[Model], dict[str, Any]]], root: Path) -> None:
    results_by_manifest = {relative: data for relative, _, data in results}
    single_repo_name = PUBLIC_ADOPTERS[0]
    single_manifest = f"{single_repo_name}/formal/fm.toml"
    single = results_by_manifest[single_manifest]
    single_repo = root / single_repo_name

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
    invalid["execution"]["artifacts_dir"] = "/tmp/formal"
    expect_invalid(single_repo, invalid, "inside the repository")

    invalid = copy.deepcopy(single)
    invalid["outputs"] = {"formats": ["json"]}
    expect_invalid(single_repo, invalid, "unsupported fields")

    try:
        validate_unique_model_names(PUBLIC_ADOPTERS[1], ["duplicate", "duplicate"])
    except ManifestError as error:
        require("unique" in str(error), f"unexpected duplicate-model error: {error}")
    else:
        raise ManifestError("self-test unexpectedly accepted duplicate model names")


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
    parser.add_argument(
        "--scope",
        choices=("public", "fleet"),
        default="fleet",
        help="validate public adopters or the complete public/private fleet",
    )
    args = parser.parse_args()

    try:
        root = args.root.resolve()
        adopters = PUBLIC_ADOPTERS if args.scope == "public" else EXPECTED_ADOPTERS
        results = load_and_validate(root, adopters)
        if args.self_test:
            run_self_tests(results, root)
    except (ManifestError, tomllib.TOMLDecodeError, OSError) as error:
        print(f"formal-methods manifest validation failed: {error}", file=sys.stderr)
        return 1

    for relative_repo, models, _ in results:
        model_names = ", ".join(model.name for model in models)
        print(f"validated {relative_repo}: {model_names}")
    if args.scope == "public":
        private_adopters = sorted(set(EXPECTED_ADOPTERS) - set(PUBLIC_ADOPTERS))
        print(
            "deferred private adopters to token-gated fleet validation: "
            + ", ".join(private_adopters)
        )
    if args.self_test:
        print("validated fail-closed manifest self-tests")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
