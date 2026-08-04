#!/usr/bin/env python3
"""Credential-free contract checks for named CNC lathe support."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE = ROOT / "src/lathe_model_catalog.rs"
LIB = ROOT / "src/lib.rs"
UNIT = ROOT / "src/tests.rs"
E2E = ROOT / "src/e2e_tests.rs"
DOC = ROOT / "docs/lathe-models.md"
README = ROOT / "readme.md"
WORKFLOW = ROOT / ".github/workflows/lathe-model-contract.yml"


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def read(path: Path) -> str:
    require(path.is_file(), f"missing file: {path.relative_to(ROOT)}")
    return path.read_text(encoding="utf-8")


def require_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token in text, f"{label} missing {token!r}")


def reject_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token not in text, f"{label} contains forbidden token {token!r}")


def check_module() -> None:
    text = read(MODULE)
    require_tokens(
        text,
        (
            'model: "haas-st-20"',
            'controller: "haas-gcode"',
            "work_envelope_mm: [330.0, 572.0]",
            "spindle_speed_rpm: [1, 4_000]",
            "max_bar_capacity_mm: Some(64.0)",
            "chuck_size_mm: Some(210.0)",
            "tool_positions: Some(12)",
            'model: "tormach-15l-slant-pro"',
            'controller: "linuxcnc"',
            'control_name: "PathPilot"',
            "work_envelope_mm: [254.0, 305.0]",
            "spindle_speed_rpm: [180, 3_500]",
            "pub(crate) fn lathe_model_for_token",
            "pub(crate) fn lathe_models_json",
            "canonical_and_common_aliases_resolve",
            "model_tokens_are_unique_and_envelopes_are_positive",
        ),
        "lathe model module",
    )
    reject_tokens(
        text,
        (
            'controller: "pathpilot"',
            'machine_kind: "haas-st-20"',
            'machine_kind: "tormach-15l-slant-pro"',
            "unsafe {",
        ),
        "lathe model module",
    )


def check_runtime_hooks() -> None:
    text = read(LIB)
    require_tokens(
        text,
        (
            "mod lathe_model_catalog;",
            "lathe_model_catalog::lathe_model_for_token",
            "lathe_model_fleet_machines()",
            "machines.extend(lathe_model_fleet_machines());",
            'format!("{}-1", spec.model)',
            '"supportedLatheModelCount"',
            '"supportedLatheModels"',
            "lathe_model_catalog::lathe_models_json()",
        ),
        "runtime integration",
    )
    require(
        text.count("machines.extend(lathe_model_fleet_machines());") == 1,
        "named lathe fleet must be appended exactly once",
    )


def check_tests() -> None:
    unit = read(UNIT)
    e2e = read(E2E)
    require_tokens(
        unit,
        (
            "named_lathe_aliases_resolve_to_lathe_class",
            "named_lathe_models_join_the_default_fleet",
            "named_lathe_models_are_selectable_by_alias",
            "turning_and_lathe_catalogs_advertise_named_models",
            '"haas-st-20-1"',
            '"tormach-15l-slant-pro-1"',
            '"haas-gcode"',
            '"linuxcnc"',
        ),
        "unit/integration tests",
    )
    require_tokens(
        e2e,
        (
            "named_lathe_catalogs_are_exposed_end_to_end",
            '"/turning/catalog"',
            '"/lathe/catalog"',
            '"haas-st-20"',
            '"tormach-15l-slant-pro"',
        ),
        "HTTP e2e tests",
    )


def check_docs_and_workflow() -> None:
    docs = read(DOC)
    readme = read(README)
    workflow = read(WORKFLOW)
    require_tokens(
        docs,
        (
            "Haas ST-20",
            "Tormach 15L Slant-PRO",
            "Haas NGC",
            "PathPilot",
            "Named selection never authorizes a spindle start",
            "bar support",
        ),
        "lathe documentation",
    )
    require("docs/lathe-models.md" in readme, "README must link the named lathe guide")
    require_tokens(
        workflow,
        (
            "python3 -m py_compile scripts/test_lathe_model_contract.py",
            "python3 scripts/test_lathe_model_contract.py",
            "cargo fmt --all -- --check",
            "persist-credentials: false",
            "timeout-minutes:",
        ),
        "lathe contract workflow",
    )
    require(
        re.search(r"actions/checkout@[0-9a-f]{40}\b", workflow) is not None,
        "checkout must be pinned to a full commit SHA",
    )
    reject_tokens(
        workflow,
        ("@main", "@master", "@v1", "@v2", "@v3", "@v4", "@v5", "@v6", "@v7"),
        "lathe contract workflow",
    )


def main() -> None:
    check_module()
    check_runtime_hooks()
    check_tests()
    check_docs_and_workflow()
    print("Named CNC lathe support contract is complete and fail-closed.")


if __name__ == "__main__":
    main()
