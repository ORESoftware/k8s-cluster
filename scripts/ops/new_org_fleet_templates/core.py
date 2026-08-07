#!/usr/bin/env python3
"""Shared primitives for deterministic newer-organization repository seeds."""

from __future__ import annotations

import json
import re
from collections.abc import Mapping
from typing import Any

CHECKOUT = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1"
RUST_TOOLCHAIN = "1.85.0"
MCP_RUST_TOOLCHAIN = "1.88.0"

ROLE_DEPENDENCIES: dict[str, tuple[str, ...]] = {
    "interfaces": (),
    "server": ("interfaces",),
    "clients": ("interfaces", "server"),
    "cli": ("interfaces", "server"),
    "sync": ("interfaces", "server"),
    "e2e": ("server", "clients"),
    "mcp": ("interfaces", "server"),
    "infra": ("server", "mcp"),
    "monorepo": (
        "interfaces",
        "server",
        "clients",
        "cli",
        "sync",
        "e2e",
        "mcp",
        "infra",
    ),
    "governance": (),
}


def json_text(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True) + "\n"


def slug(value: str) -> str:
    normalized = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
    if not normalized:
        raise ValueError(f"cannot produce slug from {value!r}")
    return normalized


def rust_ident(value: str) -> str:
    normalized = re.sub(r"[^a-zA-Z0-9]+", "_", value).strip("_")
    if not normalized:
        raise ValueError(f"cannot produce Rust identifier from {value!r}")
    if normalized[0].isdigit():
        normalized = f"project_{normalized}"
    return normalized.lower()


def rust_type(value: str) -> str:
    parts = re.findall(r"[A-Za-z0-9]+", value)
    if not parts:
        raise ValueError(f"cannot produce Rust type from {value!r}")
    rendered = "".join(part[:1].upper() + part[1:] for part in parts)
    if rendered[0].isdigit():
        rendered = f"Project{rendered}"
    return rendered


def _all_repository_entries(org: Mapping[str, Any]) -> list[dict[str, str]]:
    owner = str(org["owner"])
    entries = [
        {
            "name": str(item["name"]),
            "role": str(item["role"]),
            "managed_by": "existing-history",
        }
        for item in org.get("existing_repositories", [])
    ]
    entries.extend(
        {
            "name": str(item["name"]),
            "role": str(item["role"]),
            "managed_by": "new-org-core-v1",
        }
        for item in org["repositories"]
    )
    for entry in entries:
        entry["full_name"] = f"{owner}/{entry['name']}"
    return entries


def relationship_document(org: Mapping[str, Any]) -> dict[str, Any]:
    entries = _all_repository_entries(org)
    role_map: dict[str, str] = {}
    for entry in entries:
        role_map.setdefault(entry["role"], entry["full_name"])

    relationships: list[dict[str, Any]] = []
    for entry in entries:
        dependencies = [
            role_map[role]
            for role in ROLE_DEPENDENCIES.get(entry["role"], ())
            if role in role_map and role_map[role] != entry["full_name"]
        ]
        relationships.append(
            {
                **entry,
                "depends_on": sorted(dict.fromkeys(dependencies)),
            }
        )

    used_by: dict[str, list[str]] = {entry["full_name"]: [] for entry in relationships}
    for entry in relationships:
        for dependency in entry["depends_on"]:
            used_by.setdefault(dependency, []).append(entry["full_name"])
    for entry in relationships:
        entry["used_by"] = sorted(dict.fromkeys(used_by[entry["full_name"]]))

    return {
        "schema_version": 1,
        "fleet_id": "new-org-core-v1",
        "organization": str(org["owner"]),
        "product": str(org["product"]),
        "summary": str(org["summary"]),
        "governance_repository": f"{org['owner']}/.github",
        "repositories": sorted(relationships, key=lambda item: item["full_name"].lower()),
    }


def repository_relationships(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, Any]:
    document = relationship_document(org)
    full_name = f"{org['owner']}/{repo['name']}"
    current = next(item for item in document["repositories"] if item["full_name"] == full_name)
    return {
        "schema_version": 1,
        "fleet_id": document["fleet_id"],
        "organization": document["organization"],
        "repository": full_name,
        "role": current["role"],
        "governance_repository": document["governance_repository"],
        "depends_on": current["depends_on"],
        "used_by": current["used_by"],
    }


def common_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    full_name = f"{org['owner']}/{repo['name']}"
    relations = repository_relationships(org, repo)
    return {
        "AGENTS.md": (
            f"# Agent guidance for {full_name}\n\n"
            "Keep public interfaces versioned, preserve backward compatibility, add tests for every behavior change, "
            "and never commit credentials. Repository boundaries and dependencies are declared in "
            "`repo-relationships.json` and the organization `.github` repository.\n"
        ),
        ".gitignore": "target/\n.DS_Store\n.env\n.env.*\n!.env.example\ncoverage/\nnode_modules/\n",
        "repo-relationships.json": json_text(relations),
    }


def python_ci(test_command: str = "python3 -m unittest discover -s tests -p 'test_*.py' -v") -> str:
    return f"""name: CI

on:
  workflow_dispatch:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

concurrency:
  group: ${{{{ github.workflow }}}}-${{{{ github.ref }}}}
  cancel-in-progress: true

jobs:
  validate:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - name: Check out repository
        uses: {CHECKOUT}
        with:
          persist-credentials: false
      - name: Compile Python tests
        run: python3 -m compileall -q tests
      - name: Run contract tests
        run: {test_command}
"""


def rust_ci(toolchain: str = RUST_TOOLCHAIN, *, locked: bool = True) -> str:
    lock = " --locked" if locked else ""
    return f"""name: CI

on:
  workflow_dispatch:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

concurrency:
  group: ${{{{ github.workflow }}}}-${{{{ github.ref }}}}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -Dwarnings
  RUSTDOCFLAGS: -Dwarnings

jobs:
  rust:
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    steps:
      - name: Check out repository
        uses: {CHECKOUT}
        with:
          persist-credentials: false
      - name: Install pinned Rust toolchain
        run: rustup toolchain install {toolchain} --profile minimal --component rustfmt,clippy
      - name: Check formatting
        run: cargo +{toolchain} fmt --all -- --check
      - name: Lint all targets
        run: cargo +{toolchain} clippy --all-targets{lock} -- -D warnings
      - name: Test all targets
        run: cargo +{toolchain} test --all-targets{lock}
      - name: Build documentation
        run: cargo +{toolchain} doc --no-deps{lock}
"""


def simple_cargo_lock(package: str, version: str = "0.1.0") -> str:
    return (
        "# This file is automatically @generated by Cargo.\n"
        "# It is not intended for manual editing.\n"
        "version = 4\n\n"
        "[[package]]\n"
        f'name = "{package}"\n'
        f'version = "{version}"\n'
    )
