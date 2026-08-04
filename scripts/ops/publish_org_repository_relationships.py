#!/usr/bin/env python3
"""Publish privacy-safe relationship maps to the fixed org `.github` fleet."""

from __future__ import annotations

import argparse
import base64
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import sys
from typing import Any
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE)) if str(HERE) not in sys.path else None
import bootstrap_org_dotgithub_repositories as base  # noqa: E402

ORGANIZATIONS = base.ORGANIZATIONS
SCHEMA_VERSION = "1.0.0"
JSON_PATH = "architecture/repository-relationships.json"
SCHEMA_PATH = "architecture/repository-relationships.schema.json"
MD_PATH = "architecture/REPOSITORY_RELATIONSHIPS.md"
READMES = ("README.md", "profile/README.md")
BEGIN = "<!-- BEGIN MANAGED REPOSITORY RELATIONSHIPS v1 -->"
END = "<!-- END MANAGED REPOSITORY RELATIONSHIPS v1 -->"

ROLES = (
    "organization_governance interfaces client_sdk api_service domain_service worker "
    "sync_service mcp_server web_bff application browser_extension cli site "
    "infrastructure end_to_end_tests composition_workspace library research_or_demo uncategorized"
).split()
RUNTIME = set("api_service domain_service worker sync_service mcp_server web_bff application browser_extension cli".split())
AUTH = set("3fa-app athlet-o benefactor-cc cliptown daedalus-fab fiducia-cloud hypesiege memebank messaging-intel quaestor-ledger scintilla-run sonus-auris streempilot voxletra".split())
COORD = AUTH - {"fiducia-cloud"}
EXPLICIT = {
    "3fa-app": [("organization://cliptown", "interoperates_with", "secure clipboard-item exchange")],
    "cliptown": [("organization://3FA-app", "uses_capability", "trusted-device and step-up authentication")],
    "memebank": [
        ("organization://3FA-app", "uses_capability", "step-up authentication"),
        ("organization://cliptown", "interoperates_with", "API/SDK clipboard and media exchange"),
    ],
    "shared-auth": [("organization://3FA-app", "integrates_with", "optional second/third-factor verification")],
    "sonus-auris": [("organization://3FA-app", "uses_capability", "step-up authentication and trusted-device recovery")],
    "akrion-sim": [("organization://usa-acc", "researches_with", "simulation and control-system model exchange")],
    "usa-acc": [("organization://akrion-sim", "researches_with", "simulation and control-system model exchange")],
}


def classify(name: str, description: str = "") -> str:
    n, d = name.lower(), (description or "").lower()
    checks = (
        (n == ".github", "organization_governance"),
        (any(x in n for x in ("interface", "contract", "schema")), "interfaces"),
        ("mcp" in n and "server" in n, "mcp_server"),
        ("e2e" in n or "end-to-end" in n, "end_to_end_tests"),
        ("monorepo" in n or n.endswith("-workspace"), "composition_workspace"),
        (any(x in n for x in ("infra", "terraform", "k8s")), "infrastructure"),
        ("sync" in n, "sync_service"),
        ("client" in n or "sdk" in n, "client_sdk"),
        ("extension" in n, "browser_extension"),
        (bool(re.search(r"(^|[-_.])cli($|[-_.])", n)), "cli"),
        ("worker" in n or "job" in n, "worker"),
        ("api-server" in n or "api_server" in n or n.endswith("-api") or "backend" in n, "api_service"),
        ("web-server" in n or "web_server" in n or "bff" in n, "web_bff"),
        ("server" in n or (n.endswith(".rs") and any(x in n for x in ("node", "service", "edge", "gateway", "routing"))), "domain_service"),
        (n.endswith(".github.io") or any(x in n for x in ("marketing", "website", "site.web")), "site"),
        (any(x in n for x in ("flutter", "desktop", "mobile", "app-rs", "app.rs", "ui.dart", "web-desktop")), "application"),
        (any(x in n for x in ("demo", "poc", "example", "research", "sim")), "research_or_demo"),
        (n.endswith((".rs", ".ts", ".dart", ".go", ".gleam", ".erl")) or any(x in d for x in ("library", "crate", "package", "sdk")), "library"),
    )
    return next((value for matched, value in checks if matched), "uncategorized")


def list_repos(api: base.GitHubApi, org: str) -> list[dict[str, Any]]:
    out = []
    for page in range(1, 101):
        status, payload, _ = api.request("GET", f"/orgs/{quote(org)}/repos?type=all&sort=full_name&per_page=100&page={page}")
        if status != 200 or not isinstance(payload, list):
            raise RuntimeError(f"invalid repository inventory for {org}")
        out += [item for item in payload if isinstance(item, dict)]
        if len(payload) < 100:
            return sorted(out, key=lambda item: str(item.get("name", "")).lower())
    raise RuntimeError(f"repository inventory too large for {org}")


def public_entry(repo: dict[str, Any]) -> dict[str, Any]:
    name = str(repo.get("name", ""))
    role = classify(name, repo.get("description") or "")
    lifecycle = "archived" if repo.get("archived") else ("incubating" if role == "research_or_demo" or any(x in name.lower() for x in ("demo", "poc", "test")) else "active")
    return {
        "name": name,
        "full_name": str(repo.get("full_name", "")),
        "role": role,
        "lifecycle": lifecycle,
        "default_branch": repo.get("default_branch") or "",
        "archived": bool(repo.get("archived")),
        "fork": bool(repo.get("fork")),
        "visibility": "public",
    }


def edge(a: str, b: str, kind: str, why: str, basis: str, status: str = "inferred") -> dict[str, str]:
    return {"from": a, "to": b, "kind": kind, "rationale": why, "source_basis": basis, "status": status}


def relationship_edges(org: str, repos: list[dict[str, Any]]) -> list[dict[str, str]]:
    by = {role: [r for r in repos if r["role"] == role] for role in ROLES}
    out = []

    def link(sources, targets, kind, why):
        for source in sources:
            for target in targets:
                if source["full_name"] != target["full_name"]:
                    out.append(edge(source["full_name"], target["full_name"], kind, why, "role-convention"))

    interfaces, clients, apis = by["interfaces"], by["client_sdk"], by["api_service"]
    services = apis or by["domain_service"]
    link(by["organization_governance"], repos, "governs", "organization defaults, safety, and relationship declarations")
    link(clients, interfaces, "generated_from", "SDK bindings derive from canonical contracts")
    link(apis + by["domain_service"], interfaces, "implements_contracts_from", "service boundary implements canonical contracts")
    link(sum((by[x] for x in ("web_bff", "application", "browser_extension", "cli")), []), services, "calls", "client uses the product service boundary")
    link(by["mcp_server"], clients, "uses_sdk", "agent adapter reuses the typed product SDK")
    link(by["mcp_server"], services, "calls", "agent tools use the authenticated product API")
    link(by["sync_service"], interfaces, "uses_contracts_from", "sync payloads follow canonical schemas")
    link(by["sync_service"], services, "synchronizes_with", "sync exchanges state through the product service boundary")
    link(by["infrastructure"], [r for r in repos if r["role"] in RUNTIME], "deploys", "product infrastructure declares runtime resources")
    link(by["end_to_end_tests"], [r for r in repos if r["role"] in RUNTIME | {"site"}], "tests", "black-box compatibility verification")
    link(by["composition_workspace"], [r for r in repos if r["role"] != "organization_governance"], "composes", "development workspace and release bill of materials")

    source, lowered, roles = f"organization://{org}", org.lower(), {r["role"] for r in repos}
    if roles & RUNTIME or "infrastructure" in roles:
        out.append(edge(source, "platform://ORESoftware/k8s-cluster", "deployed_via", "immutable artifacts are promoted by digest through GitOps", "platform-policy", "platform-default"))
    if roles - {"organization_governance", "site"}:
        out.append(edge(source, "platform://zed-pkg", "packaged_via", "Zed resolves artifacts while submodules compose editable source", "platform-policy", "platform-default"))
    if "sync_service" in roles and lowered != "opto-sync":
        out.append(edge(source, "platform://opto-sync", "reconciles_via", "product sync wraps the generic reconciliation engine", "platform-policy", "platform-default"))
    if "mcp_server" in roles:
        out.append(edge(source, "platform://ORESoftware/mcp-rust-libs", "uses_transport_library", "shared MCP transport and protocol hardening", "platform-policy", "platform-default"))
    if lowered in AUTH and lowered != "shared-auth":
        out.append(edge(source, "capability://shared-auth/human-identity", "authenticates_via", "platform human identity and session authority", "explicit-platform-decision", "platform-default"))
    if lowered in COORD:
        out.append(edge(source, "capability://fiducia-cloud/distributed-coordination", "coordinates_via", "locks, leases, idempotency, elections, schedules, budgets, and task claims", "explicit-platform-decision", "platform-default"))
    out += [edge(source, target, kind, why, "explicit-product-decision", "declared") for target, kind, why in EXPLICIT.get(lowered, [])]
    return sorted({(e["from"], e["to"], e["kind"]): e for e in out}.values(), key=lambda e: (e["from"].lower(), e["to"].lower(), e["kind"]))


def manifest(org: str, inventory: list[dict[str, Any]]) -> dict[str, Any]:
    public = [public_entry(r) for r in inventory if not r.get("private") and r.get("visibility", "public") == "public"]
    public.sort(key=lambda r: (ROLES.index(r["role"]), r["name"].lower()))
    return {
        "$schema": "./repository-relationships.schema.json",
        "schema_version": SCHEMA_VERSION,
        "organization": org,
        "registry_repository": f"{org}/.github",
        "inventory_scope": "public-repositories-plus-role-contracts",
        "privacy": {
            "public_repository_names_published": True,
            "private_repository_names_published": False,
            "private_repository_count": len(inventory) - len(public),
            "reason": "The public .github registry intentionally withholds private repository names and edges.",
        },
        "repositories": public,
        "relationships": relationship_edges(org, public),
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
            "required_resource_attributes": ["service.name", "service.namespace", "service.version", "deployment.environment", "vcs.ref.head.revision"],
        },
    }


def schema() -> dict[str, Any]:
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Organization repository relationships",
        "type": "object",
        "required": ["schema_version", "organization", "registry_repository", "privacy", "repositories", "relationships", "composition_policy"],
        "properties": {
            "schema_version": {"const": SCHEMA_VERSION},
            "organization": {"type": "string"},
            "registry_repository": {"type": "string", "pattern": r"^[^/]+/\.github$"},
            "privacy": {"type": "object"},
            "repositories": {"type": "array"},
            "relationships": {"type": "array"},
            "composition_policy": {"type": "object"},
        },
    }


def markdown(data: dict[str, Any]) -> str:
    lines = [
        f"# `{data['organization']}` repository relationships", "",
        "Generated from reviewed policy and the current **public** repository inventory.", "",
        f"- Public repositories declared: **{len(data['repositories'])}**",
        f"- Private repository names withheld: **{data['privacy']['private_repository_count']}**",
        f"- Relationship edges: **{len(data['relationships'])}**", "",
        "## Repository roles", "", "| Repository | Role | Lifecycle |", "|---|---|---|",
    ]
    lines += [f"| [`{r['name']}`](https://github.com/{r['full_name']}) | `{r['role']}` | `{r['lifecycle']}` |" for r in data["repositories"]]
    lines += ["", "## Declared edges", "", "| From | Relationship | To | Status/basis |", "|---|---|---|---|"]
    lines += [f"| `{e['from']}` | `{e['kind']}` | `{e['to']}` | `{e['status']}` / `{e['source_basis']}`: {e['rationale']} |" for e in data["relationships"]]
    lines += [
        "", "## Composition, service, and observability contract", "",
        "Git submodules compose editable source; Zed packages resolve packages/artifacts; dual-managed commits must match. Production deploys immutable image digests, not runtime source builds. Cross-service access uses APIs/SDKs/events rather than another service database. MCP uses the product API/SDK. Services emit OpenTelemetry traces, bounded metrics, and correlated structured logs.",
        "", "## Privacy boundary", "",
        "This public registry deliberately omits private repository names and edges; the count above makes the boundary explicit.", "",
    ]
    return "\n".join(lines)


def readme_body(org: str) -> str:
    return f"""## Repository relationship registry

`{org}` declares repository roles, dependency edges, cross-organization capabilities, deployment ownership, and the git-submodule/Zed-package contract:

- [Human-readable map]({MD_PATH})
- [Machine-readable manifest]({JSON_PATH})
- [JSON Schema]({SCHEMA_PATH})

The public registry withholds private repository names and edges.
"""


def merge_block(existing: str | None, body: str) -> str:
    block = f"{BEGIN}\n{body.rstrip()}\n{END}"
    if not existing:
        return block + "\n"
    start, end = existing.find(BEGIN), existing.find(END)
    if start >= 0 or end >= 0:
        if start < 0 or end < 0 or end < start:
            raise ValueError("malformed relationship markers")
        return (existing[:start].rstrip() + "\n\n" + block + existing[end + len(END):]).rstrip() + "\n"
    return existing.rstrip() + "\n\n" + block + "\n"


def managed(existing: str | None, org: str) -> str:
    return merge_block(existing, readme_body(org))


def write(api, org, path, branch, content, existing):
    body = {"message": f"docs: reconcile repository relationships in {path}", "content": base64.b64encode(content.encode()).decode(), "branch": branch}
    if existing:
        body["sha"] = existing.sha
    status, payload, _ = api.request("PUT", f"/repos/{quote(org)}/.github/contents/{quote(path, safe='/')}", body)
    if status not in (200, 201) or not isinstance(payload, dict):
        raise RuntimeError(f"failed to write relationship registry for {org}")


def build_plan(api, org, dotgithub, inventory):
    branch = dotgithub.get("default_branch")
    if not branch:
        raise RuntimeError(f"missing default branch for {org}/.github")
    data = manifest(org, inventory)
    private_names = {str(r.get("name")) for r in inventory if r.get("private") or r.get("visibility") == "private"}
    rendered, md = json.dumps(data, indent=2, sort_keys=True) + "\n", markdown(data)
    if any(name and name in rendered + md for name in private_names):
        raise RuntimeError(f"privacy preflight failed for {org}/.github")
    desired = {JSON_PATH: rendered, SCHEMA_PATH: json.dumps(schema(), indent=2, sort_keys=True) + "\n", MD_PATH: md}
    files = {path: (content, base.fetch_file(api, org, path, branch)) for path, content in desired.items()}
    for path in READMES:
        existing = base.fetch_file(api, org, path, branch)
        files[path] = (managed(existing.content if existing else None, org), existing)
    result = {"organization": org, "public_repository_count": len(data["repositories"]), "private_repository_count": data["privacy"]["private_repository_count"], "relationship_count": len(data["relationships"]), "changed_files": [], "unchanged_files": [], "verified": False}
    return branch, files, private_names, result


def run_plan(api, plan, execute):
    org, branch, files, private_names, result = plan
    for path, (desired, existing) in files.items():
        target = result["unchanged_files"] if existing and existing.content == desired else result["changed_files"]
        target.append(path)
        if execute and target is result["changed_files"]:
            write(api, org, path, branch, desired, existing)
            print(f"UPDATED {org}/.github:{path}")
    if execute:
        for path, (desired, _) in files.items():
            observed = base.fetch_file(api, org, path, branch)
            if not observed or observed.content != desired or any(name and name in observed.content for name in private_names):
                raise RuntimeError(f"relationship verification failed for {org}/.github")
        result["verified"] = True
        print(f"VERIFIED {org}/.github relationships")


def report(results, execute):
    lines = [
        "# Organization repository-relationship publication", "",
        f"- Mode: **{'executed' if execute else 'dry-run'}**",
        f"- Organizations: **{len(results)}**",
        f"- Public repositories declared: **{sum(r['public_repository_count'] for r in results)}**",
        f"- Private repository names withheld: **{sum(r['private_repository_count'] for r in results)}**",
        f"- Relationship edges: **{sum(r['relationship_count'] for r in results)}**",
        f"- Organizations verified: **{sum(bool(r['verified']) for r in results)}**", "",
        "| Organization | Public | Private names withheld | Edges | Changed | Verified |", "|---|---:|---:|---:|---:|---:|",
    ]
    lines += [f"| `{r['organization']}` | {r['public_repository_count']} | {r['private_repository_count']} | {r['relationship_count']} | {len(r['changed_files'])} | {'yes' if r['verified'] else 'no'} |" for r in results]
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--json-report", type=Path)
    parser.add_argument("--markdown-report", type=Path)
    args = parser.parse_args()
    api = base.GitHubApi(os.environ.get("GH_TOKEN", ""))
    dotgithub = base.preflight(api)
    if any(repo is None for repo in dotgithub.values()):
        raise RuntimeError("all organization .github repositories must exist before relationship publication")
    inventories = {org: list_repos(api, org) for org in ORGANIZATIONS}
    plans = []
    for org in ORGANIZATIONS:
        branch, files, private_names, result = build_plan(api, org, dotgithub[org], inventories[org])
        plans.append((org, branch, files, private_names, result))
    for plan in plans:
        run_plan(api, plan, args.execute)
    results = [p[-1] for p in plans]
    payload = {"mode": "execute" if args.execute else "dry-run", "generated_at": datetime.now(timezone.utc).isoformat(), "organizations": results}
    text = report(results, args.execute)
    if args.json_report:
        args.json_report.parent.mkdir(parents=True, exist_ok=True); args.json_report.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    if args.markdown_report:
        args.markdown_report.parent.mkdir(parents=True, exist_ok=True); args.markdown_report.write_text(text)
    print(text)
    return 0


# Stable helper names for focused tests.
classify_repository = classify
public_repository_entry = public_entry
build_internal_relationships = lambda _org, repos: [e for e in relationship_edges("example", repos) if not e["from"].startswith("organization://")]
build_external_relationships = lambda org, repos: [e for e in relationship_edges(org, repos) if e["from"].startswith("organization://")]
build_manifest = manifest
relationship_schema = schema
render_markdown = markdown
relationship_readme_block = readme_body
merge_managed_block = merge_block
BEGIN_MARKER, END_MARKER = BEGIN, END

if __name__ == "__main__":
    raise SystemExit(main())
