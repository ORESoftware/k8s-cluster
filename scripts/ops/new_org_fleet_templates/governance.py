#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from .core import (
    MCP_RUST_TOOLCHAIN,
    RUST_TOOLCHAIN,
    _all_repository_entries,
    common_files,
    json_text,
    python_ci,
    relationship_document,
    rust_ci,
    rust_ident,
    rust_type,
    simple_cargo_lock,
    slug,
)


def governance_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    document = relationship_document(org)
    rows = "\n".join(
        f"| `{item['full_name']}` | `{item['role']}` | {item['managed_by']} |"
        for item in document["repositories"]
    )
    profile = f"""# {org['product']}

{org['summary']}

## Repository map

| Repository | Responsibility | Lifecycle |
| --- | --- | --- |
{rows}

The machine-readable source of truth is [`ORG_REPOSITORIES.yaml`](../ORG_REPOSITORIES.yaml). Product repositories are private unless the fleet manifest explicitly declares them public. Existing histories are preserved and are never force-pushed by the fleet publisher.
"""
    files = common_files(org, repo)
    files.update(
        {
            "README.md": (
                f"# {org['owner']} organization governance\n\n"
                "Organization-wide community health files, security policy, and the canonical repository relationship map.\n"
            ),
            "profile/README.md": profile,
            "ORG_REPOSITORIES.yaml": json_text(document),
            "SECURITY.md": (
                "# Security policy\n\n"
                "Do not disclose credentials or vulnerabilities in public issues. Report suspected vulnerabilities "
                "privately to the organization owners with affected repository, impact, reproduction steps, and mitigation ideas. "
                "Rotate any credential that may have entered logs, chat, commits, or build artifacts.\n"
            ),
            "CONTRIBUTING.md": (
                "# Contributing\n\n"
                "Open a focused issue or pull request. Include tests, document compatibility effects, avoid unrelated formatting churn, "
                "and resolve conflicts semantically against the current architecture rather than choosing one side wholesale.\n"
            ),
            "CODE_OF_CONDUCT.md": (
                "# Code of conduct\n\n"
                "Be respectful, specific, and constructive. Harassment, threats, and disclosure of private information are not tolerated.\n"
            ),
            ".github/CODEOWNERS": "* @ORESoftware\n",
            ".github/ISSUE_TEMPLATE/bug_report.yml": f"""name: Bug report
description: Report a reproducible defect in {org['product']}
title: "bug: "
body:
  - type: textarea
    id: observed
    attributes:
      label: Observed behavior
    validations:
      required: true
  - type: textarea
    id: expected
    attributes:
      label: Expected behavior
    validations:
      required: true
  - type: textarea
    id: reproduction
    attributes:
      label: Minimal reproduction
    validations:
      required: true
  - type: textarea
    id: environment
    attributes:
      label: Environment
    validations:
      required: true
""",
            ".github/PULL_REQUEST_TEMPLATE.md": (
                "## Summary\n\n## Validation\n\n## Compatibility and security impact\n\n## Repository relationship changes\n"
            ),
            ".github/workflows/ci.yml": python_ci(),
            "tests/test_governance.py": """import json
import pathlib
import unittest


class GovernanceContractTest(unittest.TestCase):
    def test_relationship_map_is_well_formed(self) -> None:
        document = json.loads(pathlib.Path("ORG_REPOSITORIES.yaml").read_text(encoding="utf-8"))
        self.assertEqual(document["schema_version"], 1)
        repositories = document["repositories"]
        self.assertGreaterEqual(len(repositories), 8)
        names = [item["full_name"] for item in repositories]
        self.assertEqual(len(names), len(set(names)))
        for item in repositories:
            self.assertNotIn(item["full_name"], item["depends_on"])


if __name__ == "__main__":
    unittest.main()
""",
        }
    )
    return files


def interfaces_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    package = slug(str(repo["name"]))
    relationship_schema = {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"https://github.com/{org['owner']}/{repo['name']}/schemas/repository-relationship.schema.json",
        "title": f"{org['product']} repository relationship",
        "type": "object",
        "additionalProperties": False,
        "required": [
            "schema_version",
            "organization",
            "repository",
            "role",
            "depends_on",
            "used_by",
        ],
        "properties": {
            "schema_version": {"const": 1},
            "organization": {"type": "string", "minLength": 1},
            "repository": {"type": "string", "pattern": "^[^/]+/[^/]+$"},
            "role": {"type": "string", "minLength": 1},
            "depends_on": {"type": "array", "uniqueItems": True, "items": {"type": "string"}},
            "used_by": {"type": "array", "uniqueItems": True, "items": {"type": "string"}},
            "governance_repository": {"type": "string"},
            "fleet_id": {"type": "string"},
        },
    }
    health_schema = {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": f"https://github.com/{org['owner']}/{repo['name']}/schemas/health.schema.json",
        "title": f"{org['product']} health response",
        "type": "object",
        "additionalProperties": False,
        "required": ["status", "service", "version"],
        "properties": {
            "status": {"enum": ["ok", "degraded"]},
            "service": {"type": "string", "minLength": 1},
            "version": {"type": "string", "pattern": "^[0-9]+\\.[0-9]+\\.[0-9]+$"},
        },
    }
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Versioned JSON Schemas and protocol contracts for {org['product']}.

Breaking changes require a new schema version. Consumers should pin a released Zed package or immutable commit.
""",
            ".zpkg.toml": f"""[package]
name = "{package}"
version = "0.1.0"
license = "UNLICENSED"

[exports]
repository_relationship = "schemas/repository-relationship.schema.json"
health = "schemas/health.schema.json"
""",
            ".zpkg.lock": "version = 1\npackages = []\n",
            "schemas/repository-relationship.schema.json": json_text(relationship_schema),
            "schemas/health.schema.json": json_text(health_schema),
            ".github/workflows/ci.yml": python_ci(),
            "tests/test_schemas.py": """import json
import pathlib
import unittest


class SchemaContractTest(unittest.TestCase):
    def test_every_schema_is_json_and_closed(self) -> None:
        schemas = sorted(pathlib.Path("schemas").glob("*.schema.json"))
        self.assertGreaterEqual(len(schemas), 2)
        for path in schemas:
            document = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(document["$schema"], "https://json-schema.org/draft/2020-12/schema")
            self.assertEqual(document["type"], "object")
            self.assertFalse(document["additionalProperties"])
            self.assertTrue(document["required"])


if __name__ == "__main__":
    unittest.main()
""",
        }
    )
    return files
