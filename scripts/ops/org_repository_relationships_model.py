"""Privacy-safe manifest and JSON Schema for organization relationship registries."""

from __future__ import annotations

from typing import Any

from org_repository_relationships_graph import repository_relationships
from org_repository_relationships_roles import ROLES, public_repository_entry

SCHEMA_VERSION = "1.0.0"
JSON_PATH = "architecture/repository-relationships.json"
SCHEMA_PATH = "architecture/repository-relationships.schema.json"
MD_PATH = "architecture/REPOSITORY_RELATIONSHIPS.md"


def build_manifest(
    organization: str,
    inventory: list[dict[str, Any]],
) -> dict[str, Any]:
    public_repositories = [
        public_repository_entry(repository)
        for repository in inventory
        if not repository.get("private")
        and repository.get("visibility", "public") == "public"
    ]
    public_repositories.sort(
        key=lambda repository: (
            ROLES.index(repository["role"]),
            repository["name"].lower(),
        )
    )
    return {
        "$schema": "./repository-relationships.schema.json",
        "schema_version": SCHEMA_VERSION,
        "organization": organization,
        "registry_repository": f"{organization}/.github",
        "inventory_scope": "public-repositories-plus-role-contracts",
        "privacy": {
            "public_repository_names_published": True,
            "private_repository_names_published": False,
            "private_repository_count": len(inventory) - len(public_repositories),
            "reason": (
                "The public .github registry intentionally withholds private "
                "repository names and edges."
            ),
        },
        "repositories": public_repositories,
        "relationships": repository_relationships(organization, public_repositories),
        "role_contracts": {
            "interfaces": "Own OpenAPI, events, JSON Schema, compatibility fixtures, and DTO inputs.",
            "client_sdk": "Depends on interfaces and exposes generated clients plus thin wrappers.",
            "api_service": "Owns public APIs, webhooks, authorization, validation, and orchestration.",
            "web_bff": "Owns browser sessions, CSRF, HTML/SSR, and calls the product API.",
            "sync_service": "Owns product conflict policy/adapters and wraps Opto reconciliation.",
            "mcp_server": "Calls product SDKs/APIs; no parallel domain persistence.",
            "infrastructure": "Owns product cloud resources; promotion remains GitOps-controlled.",
            "composition_workspace": "Composes source repositories and records the release BOM.",
            "end_to_end_tests": "Runs black-box compatibility and user-flow checks.",
        },
        "composition_policy": {
            "editable_source_composition": "git-submodules",
            "package_and_artifact_resolution": "zed-pkg",
            "dual_managed_invariant": "gitlink SHA must equal the source commit in the Zed lock entry",
            "production_deployment_resolution": "immutable OCI image digest or signed artifact",
            "production_source_checkout": "forbidden as a runtime deployment mechanism",
        },
        "service_boundary_policy": {
            "cross_service_database_access": "forbidden-by-default",
            "preferred_cross_service_calls": "typed API/SDK, event, or owned replicated read model",
            "mcp_database_access": "forbidden-by-default; MCP calls the product API/SDK",
        },
        "observability_policy": {
            "traces": "OpenTelemetry with W3C trace context",
            "metrics": "Prometheus-compatible bounded-cardinality metrics",
            "logs": "structured output with trace_id/span_id correlation",
            "required_resource_attributes": [
                "service.name",
                "service.namespace",
                "service.version",
                "deployment.environment",
                "vcs.ref.head.revision",
            ],
        },
    }


def relationship_schema() -> dict[str, Any]:
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Organization repository relationships",
        "type": "object",
        "required": [
            "schema_version",
            "organization",
            "registry_repository",
            "privacy",
            "repositories",
            "relationships",
            "composition_policy",
        ],
        "properties": {
            "schema_version": {"const": SCHEMA_VERSION},
            "organization": {"type": "string"},
            "registry_repository": {
                "type": "string",
                "pattern": r"^[^/]+/\.github$",
            },
            "privacy": {"type": "object"},
            "repositories": {"type": "array"},
            "relationships": {"type": "array"},
            "composition_policy": {"type": "object"},
        },
    }
