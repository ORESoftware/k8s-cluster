"""Repository-role classification and explicit organization relationships."""

from __future__ import annotations

import re
from typing import Any

ROLES = (
    "organization_governance",
    "interfaces",
    "client_sdk",
    "api_service",
    "domain_service",
    "worker",
    "sync_service",
    "mcp_server",
    "web_bff",
    "application",
    "browser_extension",
    "cli",
    "site",
    "infrastructure",
    "end_to_end_tests",
    "composition_workspace",
    "library",
    "research_or_demo",
    "uncategorized",
)
RUNTIME_ROLES = frozenset(
    {
        "api_service",
        "domain_service",
        "worker",
        "sync_service",
        "mcp_server",
        "web_bff",
        "application",
        "browser_extension",
        "cli",
    }
)
AUTH_ORGANIZATIONS = frozenset(
    {
        "3fa-app",
        "athlet-o",
        "benefactor-cc",
        "cliptown",
        "daedalus-fab",
        "fiducia-cloud",
        "hypesiege",
        "memebank",
        "messaging-intel",
        "quaestor-ledger",
        "scintilla-run",
        "sonus-auris",
        "streempilot",
        "voxletra",
    }
)
COORDINATION_ORGANIZATIONS = AUTH_ORGANIZATIONS - {"fiducia-cloud"}
EXPLICIT_RELATIONSHIPS: dict[str, tuple[tuple[str, str, str], ...]] = {
    "3fa-app": (
        (
            "organization://cliptown",
            "interoperates_with",
            "secure clipboard-item exchange",
        ),
    ),
    "cliptown": (
        (
            "organization://3FA-app",
            "uses_capability",
            "trusted-device and step-up authentication",
        ),
    ),
    "memebank": (
        (
            "organization://3FA-app",
            "uses_capability",
            "step-up authentication",
        ),
        (
            "organization://cliptown",
            "interoperates_with",
            "API/SDK clipboard and media exchange",
        ),
    ),
    "shared-auth": (
        (
            "organization://3FA-app",
            "integrates_with",
            "optional second/third-factor verification",
        ),
    ),
    "sonus-auris": (
        (
            "organization://3FA-app",
            "uses_capability",
            "step-up authentication and trusted-device recovery",
        ),
    ),
    "akrion-sim": (
        (
            "organization://usa-acc",
            "researches_with",
            "simulation and control-system model exchange",
        ),
    ),
    "usa-acc": (
        (
            "organization://akrion-sim",
            "researches_with",
            "simulation and control-system model exchange",
        ),
    ),
}


def classify_repository(name: str, description: str = "") -> str:
    """Classify a repository using stable naming and description conventions."""
    normalized = name.lower()
    detail = (description or "").lower()
    checks = (
        (normalized == ".github", "organization_governance"),
        (any(token in normalized for token in ("interface", "contract", "schema")), "interfaces"),
        ("mcp" in normalized and "server" in normalized, "mcp_server"),
        ("e2e" in normalized or "end-to-end" in normalized, "end_to_end_tests"),
        ("monorepo" in normalized or normalized.endswith("-workspace"), "composition_workspace"),
        (any(token in normalized for token in ("infra", "terraform", "k8s")), "infrastructure"),
        ("sync" in normalized, "sync_service"),
        ("client" in normalized or "sdk" in normalized, "client_sdk"),
        ("extension" in normalized, "browser_extension"),
        (bool(re.search(r"(^|[-_.])cli($|[-_.])", normalized)), "cli"),
        ("worker" in normalized or "job" in normalized, "worker"),
        (
            "api-server" in normalized
            or "api_server" in normalized
            or normalized.endswith("-api")
            or "backend" in normalized,
            "api_service",
        ),
        (
            "web-server" in normalized or "web_server" in normalized or "bff" in normalized,
            "web_bff",
        ),
        (
            "server" in normalized
            or (
                normalized.endswith(".rs")
                and any(token in normalized for token in ("node", "service", "edge", "gateway", "routing"))
            ),
            "domain_service",
        ),
        (
            normalized.endswith(".github.io")
            or any(token in normalized for token in ("marketing", "website", "site.web")),
            "site",
        ),
        (
            any(
                token in normalized
                for token in ("flutter", "desktop", "mobile", "app-rs", "app.rs", "ui.dart", "web-desktop")
            ),
            "application",
        ),
        (any(token in normalized for token in ("demo", "poc", "example", "research", "sim")), "research_or_demo"),
        (
            normalized.endswith((".rs", ".ts", ".dart", ".go", ".gleam", ".erl"))
            or any(token in detail for token in ("library", "crate", "package", "sdk")),
            "library",
        ),
    )
    return next((role for matched, role in checks if matched), "uncategorized")


def public_repository_entry(repository: dict[str, Any]) -> dict[str, Any]:
    """Return the privacy-safe subset used by a public organization registry."""
    name = str(repository.get("name", ""))
    role = classify_repository(name, repository.get("description") or "")
    incubating = role == "research_or_demo" or any(
        token in name.lower() for token in ("demo", "poc", "test")
    )
    lifecycle = "archived" if repository.get("archived") else "incubating" if incubating else "active"
    return {
        "name": name,
        "full_name": str(repository.get("full_name", "")),
        "role": role,
        "lifecycle": lifecycle,
        "default_branch": repository.get("default_branch") or "",
        "archived": bool(repository.get("archived")),
        "fork": bool(repository.get("fork")),
        "visibility": "public",
    }
