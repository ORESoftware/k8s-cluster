#!/usr/bin/env python3
"""Credential-free source contract for named CNC turning-center support."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def read(relative: str) -> str:
    path = ROOT / relative
    require(path.is_file(), f"missing {relative}")
    return path.read_text(encoding="utf-8")


def require_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token in text, f"{label} missing {token!r}")


def require_once(text: str, token: str, label: str) -> None:
    count = text.count(token)
    require(count == 1, f"{label} expected once, found {count}: {token!r}")


def main() -> None:
    catalog = read("src/turning_model_catalog.rs")
    lib = read("src/lib.rs")
    e2e = read("src/e2e_tests.rs")
    docs = read("docs/turning-models.md")
    workflow = read(".github/workflows/ci.yml")

    require_tokens(
        catalog,
        (
            'model: "haas-st-20"',
            'controller: "haas-gcode"',
            "work_envelope_mm: [330.0, 572.0]",
            "bar_capacity_mm: Some(64.0)",
            "max_spindle_speed_rpm: 4_000",
            'model: "dn-solutions-lynx-2100b-fanuc"',
            'controller: "fanuc-gcode"',
            'controller_options: &["fanuc-gcode", "siemens-sinumerik"]',
            "work_envelope_mm: [350.0, 330.0]",
            "max_spindle_speed_rpm: 4_500",
            "requires_controller_confirmation: true",
            "turning_machine_model_for_token",
            "turning_machine_models_json",
        ),
        "turning catalog",
    )
    require(catalog.count("TurningMachineModelSpec {") == 3,
            "catalog must contain exactly two model entries plus the struct declaration")

    require_tokens(
        lib,
        (
            "mod turning_model_catalog;",
            "turning_model_catalog::turning_machine_model_for_token(&token)",
            "machines.extend(turning_model_fleet_machines());",
            "fn turning_model_fleet_machines() -> Vec<MachineProfile>",
            'languages.insert("haas-gcode".to_string());',
            'languages.insert("siemens-sinumerik".to_string());',
            'languages.insert("fanuc-gcode".to_string());',
            '"supportedTurningModelCount": turning_model_catalog::TURNING_MACHINE_MODEL_SPECS.len()',
            '"supportedTurningModels": turning_model_catalog::turning_machine_models_json()',
        ),
        "lib integration",
    )
    require_once(lib, "machines.extend(turning_model_fleet_machines());", "fleet extension")
    require_once(lib, "fn turning_model_fleet_machines() -> Vec<MachineProfile>", "fleet constructor")
    require(lib.count('"supportedTurningModelCount": turning_model_catalog::TURNING_MACHINE_MODEL_SPECS.len()') == 2,
            "turning and lathe catalogs must both advertise the named model count")
    require(lib.count('"supportedTurningModels": turning_model_catalog::turning_machine_models_json()') == 2,
            "turning and lathe catalogs must both advertise the named models")

    require_tokens(
        e2e,
        (
            "turning_catalog_advertises_named_lathe_profiles_over_http",
            'get_auth(&app, "/turning/catalog")',
            'get_auth(&app, "/lathe/catalog")',
            '"haas-st-20-1"',
            '"dn-solutions-lynx-2100b-fanuc-1"',
            'language == "haas-gcode"',
            'language == "fanuc-gcode"',
        ),
        "HTTP e2e",
    )

    require_tokens(
        docs,
        (
            "Haas ST-20",
            "DN Solutions Lynx 2100B",
            "requiresControllerConfirmation",
            "Siemens-equipped machine",
            "first-piece inspection",
            "Named support is **not** remote-control certification",
        ),
        "turning documentation",
    )
    require_tokens(
        workflow,
        (
            "Validate named turning model integration",
            "python3 scripts/ci/check_turning_model_integration.py",
        ),
        "permanent CI",
    )
    require(
        not (ROOT / ".github/workflows/temporary-turning-source-extract.yml").exists(),
        "temporary turning workflow must be removed before final validation",
    )

    print("named turning model integration contract: OK")


if __name__ == "__main__":
    main()
