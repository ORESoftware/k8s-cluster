#!/usr/bin/env python3
"""Deterministically generate the five remaining organization MCP servers.

This module performs no network I/O and never reads credentials.
"""
from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any

from remaining_mcp_fleet_data import (
    MAX_OUTPUT_BYTES, MONOREPO_SPECS, MSRV, RMCP_VERSION, RUST_VERSION,
    SERVER_SPECS, SHARED_REPOSITORY, SHARED_REVISION, STABLE_PROTOCOL,
    TEMPLATE_VERSION, MonorepoSpec, RepositorySpec,
)
from remaining_mcp_fleet_templates import (
    AGENTS_TEMPLATE, CARGO_TEMPLATE, CI_TEMPLATE, DOMAIN_TEMPLATE, LICENSE_TEXT,
    LIB_TEMPLATE, MAIN_TEMPLATE, PROCESS_TEST_TEMPLATE, README_TEMPLATE,
    RUNTIME_TEMPLATE, SECURITY_TEXT, SERVER_TEMPLATE,
)

def spec_by_full_name(full_name: str) -> RepositorySpec:
    for spec in SERVER_SPECS:
        if spec.full_name == full_name:
            return spec
    raise KeyError(full_name)


def _json(value: Any) -> str:
    return json.dumps(value, indent=4, sort_keys=True)


def template_digest() -> str:
    payload = {
        "template_version": TEMPLATE_VERSION,
        "rmcp": RMCP_VERSION,
        "shared_revision": SHARED_REVISION,
        "servers": [
            {
                "full_name": spec.full_name,
                "visibility": spec.visibility,
                "issue": spec.issue,
                "validator": spec.validator_tool,
                "repositories": spec.repositories,
                "valid_arguments": spec.valid_arguments,
            }
            for spec in SERVER_SPECS
        ],
        "monorepos": [spec.__dict__ for spec in MONOREPO_SPECS],
    }
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()



def _tool_names(spec: RepositorySpec) -> list[str]:
    return sorted(["project_overview", "repository_map", "domain_contract", spec.validator_tool, "safety_contract"])


def render_server_files(spec: RepositorySpec) -> dict[str, str]:
    mapping = {
        "binary_name": spec.binary_name,
        "crate_name": spec.crate_name,
        "server_type": spec.server_type,
        "server_title": spec.server_title,
        "description": spec.description,
        "owner": spec.owner,
        "full_name": spec.full_name,
        "issue": spec.issue,
        "validator_tool": spec.validator_tool,
        "validator_request": spec.validator_request,
        "rust_types": spec.rust_types.strip(),
        "rust_validation": spec.rust_validation.strip(),
        "rmcp_version": RMCP_VERSION,
        "rust_version": RUST_VERSION,
        "msrv": MSRV,
        "protocol": STABLE_PROTOCOL,
        "shared_repository": SHARED_REPOSITORY,
        "shared_revision": SHARED_REVISION,
        "max_output_bytes": str(MAX_OUTPUT_BYTES),
        "template_digest": template_digest(),
        "repositories": _json([{"name": name, "url": url} for name, url in spec.repositories]),
        "domain_contract": _json(spec.domain_contract),
        "valid_arguments": _json(spec.valid_arguments),
        "forbidden_key": spec.forbidden_argument[0],
        "forbidden_value": json.dumps(spec.forbidden_argument[1]),
        "tool_names": "vec![" + ", ".join(json.dumps(name) + ".to_owned()" for name in _tool_names(spec)) + "]",
    }
    marker = {
        "schema_version": 1,
        "full_name": spec.full_name,
        "tracking_issue": spec.issue,
        "template_version": TEMPLATE_VERSION,
        "template_digest": template_digest(),
        "rmcp": RMCP_VERSION,
        "shared_revision": SHARED_REVISION,
        "visibility": spec.visibility,
        "read_only": True,
    }
    return {
        ".github/workflows/ci.yml": CI_TEMPLATE.substitute(mapping),
        ".gitignore": "/target\n.env\n.env.*\n.DS_Store\n",
        ".mcp-bootstrap.json": json.dumps(marker, indent=2, sort_keys=True) + "\n",
        "AGENTS.md": AGENTS_TEMPLATE.substitute(mapping),
        "Cargo.toml": CARGO_TEMPLATE.substitute(mapping),
        "LICENSE": LICENSE_TEXT,
        "README.md": README_TEMPLATE.substitute(mapping),
        "SECURITY.md": SECURITY_TEXT,
        "src/domain.rs": DOMAIN_TEMPLATE.substitute(mapping),
        "src/lib.rs": LIB_TEMPLATE,
        "src/main.rs": MAIN_TEMPLATE.substitute(mapping),
        "src/runtime.rs": RUNTIME_TEMPLATE.substitute(mapping),
        "src/server.rs": SERVER_TEMPLATE.substitute(mapping),
        "tests/stdio_protocol.rs": PROCESS_TEST_TEMPLATE.substitute(mapping),
    }


def write_server_tree(spec: RepositorySpec, root: Path) -> dict[str, str]:
    files = render_server_files(spec)
    for relative, content in files.items():
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
    return files


def validate_request_manifest(manifest: dict[str, Any]) -> None:
    if manifest.get("schema_version") != 1 or manifest.get("execute") is not True:
        raise ValueError("request manifest must be schema v1 with execute=true")
    if not re.fullmatch(r"DEN-162-166-[0-9]{4}-[0-9]{2}-[0-9]{2}", str(manifest.get("request_id", ""))):
        raise ValueError("request_id is invalid")
    expected_servers = [{"full_name": s.full_name, "visibility": s.visibility, "issue": s.issue} for s in SERVER_SPECS]
    expected_monorepos = [{"full_name": s.full_name, "visibility": s.visibility, "issue": s.issue} for s in MONOREPO_SPECS]
    if manifest.get("servers") != expected_servers:
        raise ValueError("server target list differs from reviewed constants")
    if manifest.get("monorepos") != expected_monorepos:
        raise ValueError("monorepo target list differs from reviewed constants")
    if manifest.get("template_digest") != template_digest():
        raise ValueError("template digest mismatch")


def request_manifest() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "request_id": "DEN-162-166-2026-08-04",
        "execute": True,
        "template_digest": template_digest(),
        "servers": [{"full_name": s.full_name, "visibility": s.visibility, "issue": s.issue} for s in SERVER_SPECS],
        "monorepos": [{"full_name": s.full_name, "visibility": s.visibility, "issue": s.issue} for s in MONOREPO_SPECS],
    }
