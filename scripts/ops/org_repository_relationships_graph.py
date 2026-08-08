"""Deterministic internal and cross-organization repository relationship graph."""

from __future__ import annotations

from typing import Any

from org_repository_relationships_roles import (
    AUTH_ORGANIZATIONS,
    COORDINATION_ORGANIZATIONS,
    EXPLICIT_RELATIONSHIPS,
    ROLES,
    RUNTIME_ROLES,
)


def relationship_edge(
    source: str,
    target: str,
    kind: str,
    rationale: str,
    source_basis: str,
    status: str = "inferred",
) -> dict[str, str]:
    return {
        "from": source,
        "to": target,
        "kind": kind,
        "rationale": rationale,
        "source_basis": source_basis,
        "status": status,
    }


def _link(
    relationships: list[dict[str, str]],
    sources: list[dict[str, Any]],
    targets: list[dict[str, Any]],
    kind: str,
    rationale: str,
) -> None:
    for source in sources:
        for target in targets:
            if source["full_name"] == target["full_name"]:
                continue
            relationships.append(
                relationship_edge(
                    source["full_name"],
                    target["full_name"],
                    kind,
                    rationale,
                    "role-convention",
                )
            )


def _internal_relationships(repositories: list[dict[str, Any]]) -> list[dict[str, str]]:
    by_role = {
        role: [repository for repository in repositories if repository["role"] == role]
        for role in ROLES
    }
    relationships: list[dict[str, str]] = []
    interfaces = by_role["interfaces"]
    clients = by_role["client_sdk"]
    api_services = by_role["api_service"]
    service_boundary = api_services or by_role["domain_service"]
    clients_and_apps = sum(
        (by_role[role] for role in ("web_bff", "application", "browser_extension", "cli")),
        [],
    )
    runtimes = [repository for repository in repositories if repository["role"] in RUNTIME_ROLES]
    test_targets = [
        repository for repository in repositories if repository["role"] in RUNTIME_ROLES | {"site"}
    ]

    _link(
        relationships,
        by_role["organization_governance"],
        repositories,
        "governs",
        "organization defaults, safety, and relationship declarations",
    )
    _link(relationships, clients, interfaces, "generated_from", "SDK bindings derive from canonical contracts")
    _link(
        relationships,
        api_services + by_role["domain_service"],
        interfaces,
        "implements_contracts_from",
        "service boundary implements canonical contracts",
    )
    _link(relationships, clients_and_apps, service_boundary, "calls", "client uses the product service boundary")
    _link(relationships, by_role["mcp_server"], clients, "uses_sdk", "agent adapter reuses the typed product SDK")
    _link(
        relationships,
        by_role["mcp_server"],
        service_boundary,
        "calls",
        "agent tools use the authenticated product API",
    )
    _link(
        relationships,
        by_role["sync_service"],
        interfaces,
        "uses_contracts_from",
        "sync payloads follow canonical schemas",
    )
    _link(
        relationships,
        by_role["sync_service"],
        service_boundary,
        "synchronizes_with",
        "sync exchanges state through the product service boundary",
    )
    _link(
        relationships,
        by_role["infrastructure"],
        runtimes,
        "deploys",
        "product infrastructure declares runtime resources",
    )
    _link(
        relationships,
        by_role["end_to_end_tests"],
        test_targets,
        "tests",
        "black-box compatibility verification",
    )
    _link(
        relationships,
        by_role["composition_workspace"],
        [repository for repository in repositories if repository["role"] != "organization_governance"],
        "composes",
        "development workspace and release bill of materials",
    )
    return relationships


def _external_relationships(
    organization: str,
    repositories: list[dict[str, Any]],
) -> list[dict[str, str]]:
    source = f"organization://{organization}"
    normalized = organization.lower()
    roles = {repository["role"] for repository in repositories}
    relationships: list[dict[str, str]] = []

    def platform(target: str, kind: str, rationale: str, basis: str) -> None:
        relationships.append(
            relationship_edge(source, target, kind, rationale, basis, "platform-default")
        )

    if roles & RUNTIME_ROLES or "infrastructure" in roles:
        platform(
            "platform://ORESoftware/k8s-cluster",
            "deployed_via",
            "immutable artifacts are promoted by digest through GitOps",
            "platform-policy",
        )
    if roles - {"organization_governance", "site"}:
        platform(
            "platform://zed-pkg",
            "packaged_via",
            "Zed resolves artifacts while submodules compose editable source",
            "platform-policy",
        )
    if "sync_service" in roles and normalized != "opto-sync":
        platform(
            "platform://opto-sync",
            "reconciles_via",
            "product sync wraps the generic reconciliation engine",
            "platform-policy",
        )
    if "mcp_server" in roles:
        platform(
            "platform://ORESoftware/mcp-rust-libs",
            "uses_transport_library",
            "shared MCP transport and protocol hardening",
            "platform-policy",
        )
    if normalized in AUTH_ORGANIZATIONS and normalized != "shared-auth":
        platform(
            "capability://shared-auth/human-identity",
            "authenticates_via",
            "platform human identity and session authority",
            "explicit-platform-decision",
        )
    if normalized in COORDINATION_ORGANIZATIONS:
        platform(
            "capability://fiducia-cloud/distributed-coordination",
            "coordinates_via",
            "locks, leases, idempotency, elections, schedules, budgets, and task claims",
            "explicit-platform-decision",
        )
    relationships.extend(
        relationship_edge(
            source,
            target,
            kind,
            rationale,
            "explicit-product-decision",
            "declared",
        )
        for target, kind, rationale in EXPLICIT_RELATIONSHIPS.get(normalized, ())
    )
    return relationships


def repository_relationships(
    organization: str,
    repositories: list[dict[str, Any]],
) -> list[dict[str, str]]:
    """Build deterministic internal and external relationship edges."""
    relationships = _internal_relationships(repositories)
    relationships.extend(_external_relationships(organization, repositories))
    unique = {(item["from"], item["to"], item["kind"]): item for item in relationships}
    return sorted(
        unique.values(),
        key=lambda item: (item["from"].lower(), item["to"].lower(), item["kind"]),
    )


def build_internal_relationships(
    _organization: str,
    repositories: list[dict[str, Any]],
) -> list[dict[str, str]]:
    return _internal_relationships(repositories)


def build_external_relationships(
    organization: str,
    repositories: list[dict[str, Any]],
) -> list[dict[str, str]]:
    return _external_relationships(organization, repositories)
