#!/usr/bin/env python3
"""Bounded, allowlisted publisher for fourteen Astro organization sites."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

API = "https://api.github.com"
TOKEN = os.environ.get("GH_TOKEN", "")
if not TOKEN:
    raise SystemExit("GH_TOKEN is required")

SPECS: list[dict[str, Any]] = [{'accent': '#f59e0b',
  'accent2': '#fb7185',
  'category': 'Open agent infrastructure',
  'features': [['Typed discovery',
                'Negotiate public capabilities and protocol versions before work begins.'],
               ['Bridge + coordinator SDK',
                'Use typed clients for agent bridges, work queues, and coordinator adapters.'],
               ['Conformance first',
                'Keep community and vendor extension boundaries explicit, testable, and safe.']],
  'linear_url': 'https://linear.app/denman/project/githubcomagent-pontifex-1d2deb2be3c7',
  'org': 'agent-pontifex',
  'proof': ['Typed bridge and coordinator clients',
            'Cross-implementation discovery profiles',
            'Public capability negotiation'],
  'repo': 'agent-pontifex.github.io',
  'source_candidates': ['agent-pontifex/agent-sdk.rs', 'agent-pontifex/ai-agent-bridge.rs'],
  'summary': 'Typed discovery, negotiated capabilities, and coordinator adapters keep community protocols open while isolating vendor-specific extensions.',
  'tagline': 'One neutral control surface for bridges, coordinators, and every agent runtime.',
  'title': 'Agent Pontifex'},
 {'accent': '#22c55e',
  'accent2': '#06b6d4',
  'category': 'Independent CI runtime',
  'features': [['Workflow familiarity',
                'Preserve the shape of GitHub Actions YAML and common execution semantics.'],
               ['Owned compute',
                'Route jobs to AWS, Hetzner, Kubernetes, or development hardware you administer.'],
               ['Auditable execution',
                'Make job lifecycle, artifacts, cancellation, and failure evidence inspectable.']],
  'linear_url': 'https://linear.app/denman/project/githubcomgha-indie-worker-941d4102f7dc',
  'org': 'gha-indie-worker',
  'proof': ['Rust worker runtime', 'Clone and orchestration server', 'GitHub Actions YAML parity program'],
  'repo': 'gha-indie-worker.github.io',
  'source_candidates': ['gha-indie-worker/gha-indie-worker.rs', 'gha-indie-worker/gha-clone-server.rs'],
  'summary': 'A Rust worker and clone server for reproducible workflow execution, owned compute, clear logs, and a practical escape hatch when hosted minutes are constrained.',
  'tagline': 'Run familiar GitHub Actions workflows on infrastructure you control.',
  'title': 'GHA Indie Worker'},
 {'accent': '#38bdf8',
  'accent2': '#8b5cf6',
  'category': 'Capability-safe file transfer',
  'features': [['One-time pairing',
                'Exchange a phone pairing secret without leaking it through query parameters or logs.'],
               ['Resumable state',
                'Recover tunnel snapshots, declared uploads, and event gaps after reconnect.'],
               ['Four official SDKs',
                'Use consistent Rust, TypeScript, Dart, and Gleam client surfaces.']],
  'linear_url': 'https://linear.app/denman/project/githubcomfile-tunnel-f46884af1012',
  'org': 'file-tunnel',
  'proof': ['Rust, TypeScript, Dart, and Gleam clients',
            'Opto Sync boundary for metadata and checkpoints',
            'Idempotent tunnel and upload declarations'],
  'repo': 'file-tunnel.github.io',
  'source_candidates': ['file-tunnel/ftnl-clients'],
  'summary': 'Create a tunnel, exchange a one-time pairing secret, upload or download bytes, reconnect from snapshots, and keep bearer capabilities out of logs and persistence.',
  'tagline': 'Pair once. Move bytes securely. Resume without losing the plot.',
  'title': 'File Tunnel'},
 {'accent': '#a855f7',
  'accent2': '#ec4899',
  'category': 'Visual memory',
  'features': [['Smart ingestion',
                'Import, fingerprint, deduplicate, OCR, and tag visual assets from multiple storage providers.'],
               ['Multimodal retrieval',
                'Blend text, metadata, embeddings, and previews to recover the right image quickly.'],
               ['Bounded interoperability',
                'Delegate through Shared Auth and official SDKs instead of hidden app-to-app coupling.']],
  'linear_url': 'https://linear.app/denman/project/memebank-3db5f5cc7452',
  'org': 'memebank',
  'proof': ['OCR and image-analysis services', 'Shared Auth delegation boundary', 'ClipTown integration through reviewed APIs'],
  'repo': 'memebank.github.io',
  'source_candidates': ['memebank/memebank-clients', 'memebank/mbk-rest-api', 'memebank/mbk-pwa'],
  'summary': 'A cross-platform, image-first library for import, deduplication, OCR, vision tags, multimodal search, portable storage, and deliberate ClipTown interoperability.',
  'tagline': 'Find the exact meme, image, or visual asset when the moment arrives.',
  'title': 'MemeBank'},
 {'accent': '#f97316',
  'accent2': '#eab308',
  'category': 'Digital fabrication platform',
  'features': [['Job orchestration',
                'Model designs, materials, machine capabilities, fabrication steps, and status transitions.'],
               ['Polyglot control',
                'Integrate from native, web, mobile, BEAM, and server runtimes through one contract family.'],
               ['Offline-aware operations',
                'Coordinate UI, API, sync, infrastructure, and end-to-end evidence as one system.']],
  'linear_url': 'https://linear.app/denman/project/githubcomdaedalus-fab-6d311a6d8d19',
  'org': 'daedalus-fab',
  'proof': ['C, C++, and Zig SDK slices', 'Fourteen maintained client targets', 'Rust API and fabrication services'],
  'repo': 'daedalus-fab.github.io',
  'source_candidates': ['daedalus-fab/daedalus-clients'],
  'summary': 'Rust fabrication services, polyglot SDKs, Flutter interfaces, synchronization, and deployment tooling for production-shaped digital fabrication workflows.',
  'tagline': 'Turn designs into traceable fabrication jobs across machines, teams, and runtimes.',
  'title': 'Daedalus Fab'},
 {'accent': '#14b8a6',
  'accent2': '#3b82f6',
  'category': 'Precise data visualization',
  'features': [['Expressive marks',
                'Render line, area, bar, scatter, boxplot, and grouped chart structures.'],
               ['Defensive numerics',
                'Reject non-finite input, cap rendering work, and preserve mixed-sign geometry.'],
               ['Automation ready',
                'Expose visualization generation through Rust services and MCP workflows.']],
  'linear_url': 'https://linear.app/denman/project/githubcomclaritas-viz-09fcc5d7dd9e',
  'org': 'claritas-viz',
  'proof': ['Area and grouped boxplot SVG marks', 'Finite-number rejection and render caps', 'Focused numeric regression coverage'],
  'repo': 'claritas-viz.github.io',
  'source_candidates': ['claritas-viz/claritas-clients', 'claritas-viz/data-viz-server.rs'],
  'summary': 'A dependency-light Rust visualization service with deterministic SVG output, bounded rendering, strict numeric validation, and MCP-friendly automation.',
  'tagline': 'From validated numbers to crisp SVG—without a heavyweight rendering stack.',
  'title': 'Claritas Viz'},
 {'accent': '#6366f1',
  'accent2': '#06b6d4',
  'category': 'Simulation and systems tooling',
  'features': [['Explicit simulation time',
                'Represent events, queues, resources, state transitions, and deterministic ordering.'],
               ['Composable interfaces',
                'Keep interfaces, libraries, clients, CLI, web, API, and MCP boundaries versioned.'],
               ['Deployable /des surface',
                'Ship the simulation experience through a canonical web route and GitOps controls.']],
  'linear_url': 'https://linear.app/denman/project/githubcomdiscrete-event-systems-4a3086ae0c45',
  'org': 'discrete-event-systems',
  'proof': ['DES Zed dependency graph', 'Rust MCP server', 'Canonical /des web delivery'],
  'repo': 'discrete-event-systems.github.io',
  'source_candidates': ['discrete-event-systems/des-clients', 'discrete-event-systems/des-mcp-server.rs', 'discrete-event-systems/des-web.rs'],
  'summary': 'A Rust-first DES platform with a canonical /des web surface, MCP automation, versioned interfaces, Zed packages, and room for polyglot clients.',
  'tagline': 'Model time, causality, queues, and resources with production-shaped simulation tooling.',
  'title': 'Discrete Event Systems'},
 {'accent': '#ef4444',
  'accent2': '#f97316',
  'category': 'Offline-first synchronization',
  'features': [['Storage adapters',
                'Move between browser, mobile, desktop, Postgres, Supabase, HTTP, and WebSocket boundaries.'],
               ['Durable background work',
                'Handle service-worker and mobile wakeups, retries, cancellation, and process recovery.'],
               ['Certified adoption',
                'Install through Zed with immutable locks and thin product-owned adapters.']],
  'linear_url': 'https://linear.app/denman/project/githubcomopto-sync-de6ba65bd559',
  'org': 'opto-sync',
  'proof': ['Background worker lifecycle hardening', 'Cross-platform publication leases', 'Validated adoption manifests'],
  'repo': 'opto-sync.github.io',
  'source_candidates': ['opto-sync/opto-sync-clients'],
  'summary': 'The certified runtime SDK for IndexedDB, SQLite, Postgres, Supabase, HTTP, WebSocket, and supported background execution environments.',
  'tagline': 'Optimistic local work, durable background sync, and explicit conflict semantics.',
  'title': 'Opto Sync'},
 {'accent': '#8b5cf6',
  'accent2': '#22d3ee',
  'category': 'Speech and text workflows',
  'features': [['Transcript sessions',
                'Keep session metadata, text revisions, preferences, and checkpoints coherent.'],
               ['Provider boundaries',
                'Separate live audio, credentials, and model artifacts from portable client state.'],
               ['Cross-runtime clients',
                'Bring speech and language workflows to web, mobile, desktop, and services.']],
  'linear_url': 'https://linear.app/denman/project/githubcomvoxletra-5528d72e4a7d',
  'org': 'voxletra',
  'proof': ['Declared Opto Sync boundary', 'Transcript and session metadata contracts', 'Credential-free portable state'],
  'repo': 'voxletra.github.io',
  'source_candidates': ['voxletra/vxl-clients'],
  'summary': 'Client and synchronization foundations for transcript sessions, language workflows, checkpoints, preferences, and provider-aware speech processing.',
  'tagline': 'Capture, transcribe, refine, and synchronize spoken ideas across devices.',
  'title': 'Voxletra'},
 {'accent': '#84cc16',
  'accent2': '#14b8a6',
  'category': 'Human-supervised robotics research',
  'features': [['Human authority',
                'Keep manual control, takeover, geofencing, and immediate abort behavior explicit.'],
               ['Simulation-first research',
                'Evaluate tracking and handoff behavior without contact, payload deployment, or live-animal intervention.'],
               ['Auditable telemetry',
                'Record control transitions, confidence, safety events, and operator decisions.']],
  'linear_url': 'https://linear.app/denman/project/githubcomdrone-mngr-8ac391ac308d',
  'org': 'drone-mngr',
  'proof': ['Simulation-only wildlife observation plan', 'Manual-to-auto handoff research', 'Conservative abort and audit requirements'],
  'repo': 'drone-mngr.github.io',
  'source_candidates': ['drone-mngr/drone-mngr-clients', 'drone-mngr/drone-mngr-ctrl-server.rs'],
  'summary': 'Rust services, embedded controllers, infrastructure, and simulation-focused research for manual control, supervised autonomy, telemetry, and fail-safe behavior.',
  'tagline': 'Operate and study drone-control systems with conservative handoff, abort, and audit boundaries.',
  'title': 'Drone Manager'},
 {'accent': '#eab308',
  'accent2': '#f97316',
  'category': 'Shared identity infrastructure',
  'features': [['Isolated data planes',
                'Separate admin and customer authentication workloads, schemas, failure domains, and capacity.'],
               ['Scoped federation',
                'Let customers move across approved applications without flattening tenant or audience boundaries.'],
               ['Hardened clients',
                'Validate endpoints, identifiers, credentials, transport bounds, and delegation semantics before network work.']],
  'linear_url': 'https://linear.app/denman/project/githubcomshared-auth-acbca07bb390',
  'org': 'shared-auth',
  'proof': ['Polyglot SDK matrix', 'Go and TypeScript delegation clients', 'Protected introspection with credential isolation'],
  'repo': 'shared-auth.github.io',
  'source_candidates': ['shared-auth/shared-auth-clients'],
  'summary': 'Isolated admin and customer authentication realms, app-scoped federation, exact-audience delegation, and hardened polyglot clients.',
  'tagline': 'One policy-aware identity plane across apps—without collapsing trust boundaries.',
  'title': 'Shared Auth'},
 {'accent': '#ef4444',
  'accent2': '#a855f7',
  'category': 'Live production platform',
  'features': [['Browser studio',
                'Coordinate guests, scenes, media, recordings, overlays, and operator controls.'],
               ['Multistream preflight',
                'Plan direct and relay capacity deterministically before a live production starts.'],
               ['Polyglot production clients',
                'Keep TypeScript, Dart, Rust, and other runtime behavior aligned.']],
  'linear_url': 'https://linear.app/denman/project/githubcomstreempilot-e8b8f6dee124',
  'org': 'StreemPilot',
  'proof': ['TypeScript, Dart, and Rust multistream parity', 'WebRTC and signaling architecture', 'Browser, desktop, and mobile release train'],
  'repo': 'streempilot.github.io',
  'source_candidates': ['StreemPilot/streempilot-clients'],
  'summary': 'A StreamYard-style studio spanning WebRTC, signaling, recording, multistream routing, Rust services, responsive clients, and preflight planning.',
  'tagline': 'Produce, route, and multistream live video from browser, desktop, or mobile.',
  'title': 'StreemPilot'},
 {'accent': '#ec4899',
  'accent2': '#f43f5e',
  'category': 'Fan engagement infrastructure',
  'features': [['Provider-aware delivery',
                'Coordinate FCM, APNs, Expo, email, SMS, and channel-specific delivery semantics.'],
               ['Audience controls',
                'Segment, schedule, retry, suppress, and observe fan communication workflows.'],
               ['Operational portability',
                'Preserve repository history and deployment evidence while moving services into their product home.']],
  'linear_url': 'https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc',
  'org': 'fanwaave',
  'proof': ['Rust push-notification service migration',
            'FCM, APNs, Expo, SendGrid, and Twilio guidance',
            'Fail-closed repository publication workflow'],
  'repo': 'fanwaave.github.io',
  'source_candidates': ['fanwaave/fanwaave-clients', 'fanwaave/push-notification-server.rs', 'ORESoftware/push-notification-server.rs'],
  'summary': 'A home for provider-aware notification delivery, audience workflows, app-store interoperability, observability, and future fan-facing platform services.',
  'tagline': 'Reach fans reliably across mobile, email, and real-time product surfaces.',
  'title': 'Fanwaave'},
 {'accent': '#7c3aed',
  'accent2': '#ec4899',
  'category': 'Social publishing',
  'features': [['Publishing calendar',
                'Plan drafts, approvals, schedules, retries, and channel-specific delivery windows.'],
               ['Channel connectors',
                'Keep provider authentication, media rules, limits, and failures behind explicit adapters.'],
               ['Analytics loop',
                'Measure outcomes and turn evidence into the next publishing decision.']],
  'linear_url': 'https://linear.app/denman/project/githubcomhypesiege-12bdb95b4116',
  'org': 'hypesiege',
  'proof': ['Interfaces-aware client SDK', 'Rust and Flutter architecture', 'CLI and MCP automation boundaries'],
  'repo': 'hypesiege.github.io',
  'source_candidates': ['hypesiege/hypesiege-clients'],
  'summary': 'A Buffer-style platform combining scheduling, channel connectors, analytics, Rust services, Flutter applications, web UI, CLI, and MCP automation.',
  'tagline': 'Plan, publish, and learn across channels from one production-grade workflow.',
  'title': 'HypeSiege'}]

NETWORK = [
    {
        "title": spec["title"],
        "org": spec["org"],
        "url": f"https://{spec['org'].lower()}.github.io/",
        "category": spec["category"],
    }
    for spec in SPECS
]
ALLOWED_REPOSITORIES = {f"{spec['org']}/{spec['repo']}" for spec in SPECS}
EXPECTED_ORGS = {spec["org"] for spec in SPECS}
TEMPLATE_REPOSITORY = "file-tunnel/file-tunnel.github.io"
BRANCH_PREFIX = "agent/astro-marketing-site-20260805"
ISSUE_TITLE = "Launch Astro marketing site with polyglot code explorer"

LANGUAGE_EXTENSIONS: list[tuple[str, tuple[str, ...]]] = [
    ("TypeScript", (".ts", ".tsx", ".mts", ".cts")),
    ("Rust", (".rs",)),
    ("Python", (".py",)),
    ("Go", (".go",)),
    ("Dart", (".dart",)),
    ("Java", (".java",)),
    ("Kotlin", (".kt", ".kts")),
    ("Swift", (".swift",)),
    ("C++", (".cpp", ".cc", ".cxx", ".hpp", ".hh", ".hxx")),
    ("C", (".c", ".h")),
    ("Zig", (".zig",)),
    ("Gleam", (".gleam",)),
    ("Elixir", (".ex", ".exs")),
    ("Erlang", (".erl", ".hrl")),
    ("Ruby", (".rb",)),
    ("PHP", (".php",)),
    ("YAML", (".yml", ".yaml")),
    ("JSON", (".json",)),
    ("TOML", (".toml",)),
    ("Shell", (".sh",)),
]
SKIP_PARTS = {
    ".git", "node_modules", "vendor", ".vendor", "target", "dist", "build",
    "generated", "fixtures", "fixture", "snapshots", "snapshot", "coverage",
    ".astro", "__pycache__",
}
SENSITIVE_LITERAL = re.compile(
    r"(ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|AKIA[0-9A-Z]{16}|"
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----|"
    r"(?:client[_-]?secret|api[_-]?key|password|access[_-]?token)\s*[:=]\s*"
    r"['\"][^'\"]{8,}['\"])",
    re.IGNORECASE,
)


def run(
    args: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    check: bool = True,
) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if check and completed.returncode:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(args)}\n"
            f"{completed.stdout[-12000:]}"
        )
    return completed.stdout


def api(
    method: str,
    path: str,
    body: dict[str, Any] | None = None,
    *,
    allowed: tuple[int, ...] = (200,),
) -> tuple[int, Any]:
    payload = None if body is None else json.dumps(body).encode("utf-8")
    request = urllib.request.Request(API + path, data=payload, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {TOKEN}")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    request.add_header("User-Agent", "bounded-astro-marketing-site-publisher")
    if payload is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read()
            parsed: Any = json.loads(raw) if raw else None
            if response.status not in allowed:
                raise RuntimeError(
                    f"GitHub API unexpected status {response.status} for {method} {path}"
                )
            return response.status, parsed
    except urllib.error.HTTPError as error:
        raw = error.read(12000).decode("utf-8", errors="replace")
        if error.code in allowed:
            try:
                return error.code, json.loads(raw) if raw else None
            except json.JSONDecodeError:
                return error.code, raw
        raise RuntimeError(
            f"GitHub API {error.code} for {method} {path}: {raw}"
        ) from error


def graphql(query: str, variables: dict[str, Any]) -> dict[str, Any]:
    _, payload = api(
        "POST", "/graphql", {"query": query, "variables": variables}, allowed=(200,)
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid GraphQL response")
    if payload.get("errors"):
        raise RuntimeError(f"GitHub GraphQL errors: {payload['errors']}")
    data = payload.get("data")
    if not isinstance(data, dict):
        raise RuntimeError("GitHub GraphQL response lacks data")
    return data


def create_git_environment(work: Path) -> dict[str, str]:
    askpass = work / "git-askpass.sh"
    askpass.write_text(
        '#!/usr/bin/env sh\n'
        'case "${1:-}" in\n'
        '  *Username*) printf "%s\\n" "x-access-token" ;;\n'
        '  *Password*) printf "%s\\n" "${GH_TOKEN:?GH_TOKEN is required}" ;;\n'
        '  *) exit 1 ;;\n'
        'esac\n',
        encoding="utf-8",
    )
    askpass.chmod(0o700)
    environment = os.environ.copy()
    environment.update(
        {
            "GH_TOKEN": TOKEN,
            "GIT_ASKPASS": str(askpass),
            "GIT_ASKPASS_REQUIRE": "force",
            "GIT_TERMINAL_PROMPT": "0",
        }
    )
    return environment


def verify_identity_and_memberships() -> None:
    _, identity = api("GET", "/user", allowed=(200,))
    login = identity.get("login") if isinstance(identity, dict) else None
    if login != "ORESoftware":
        raise RuntimeError(f"unexpected publisher identity: {login!r}")
    for org in sorted(EXPECTED_ORGS, key=str.casefold):
        _, membership = api(
            "GET", f"/user/memberships/orgs/{urllib.parse.quote(org, safe='')}",
            allowed=(200,),
        )
        observed = (
            membership.get("role") if isinstance(membership, dict) else None,
            membership.get("state") if isinstance(membership, dict) else None,
        )
        if observed != ("admin", "active"):
            raise RuntimeError(f"{org} membership is {observed!r}, not owner/active")
        print(f"VERIFIED_OWNER {org}")


def repository(full_name: str) -> dict[str, Any] | None:
    status, payload = api("GET", f"/repos/{full_name}", allowed=(200, 404))
    if status == 404:
        return None
    if not isinstance(payload, dict):
        raise RuntimeError(f"invalid repository payload for {full_name}")
    return payload


def ensure_repository(spec: dict[str, Any]) -> dict[str, Any]:
    full_name = f"{spec['org']}/{spec['repo']}"
    if full_name not in ALLOWED_REPOSITORIES:
        raise RuntimeError(f"repository escaped allowlist: {full_name}")
    current = repository(full_name)
    if current is None:
        _, current = api(
            "POST",
            f"/orgs/{urllib.parse.quote(spec['org'], safe='')}/repos",
            {
                "name": spec["repo"],
                "description": f"Official Astro marketing site for {spec['title']}",
                "private": False,
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "auto_init": True,
                "allow_squash_merge": True,
                "allow_merge_commit": True,
                "allow_rebase_merge": False,
                "delete_branch_on_merge": True,
            },
            allowed=(201,),
        )
        print(f"CREATED {full_name}")
        time.sleep(2)
    if not isinstance(current, dict):
        raise RuntimeError(f"failed to reconcile {full_name}")
    patch = {
        "description": f"Official Astro marketing site for {spec['title']}",
        "has_issues": True,
        "has_wiki": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }
    if current.get("visibility") != "public":
        patch.update({"visibility": "public", "private": False})
    _, current = api("PATCH", f"/repos/{full_name}", patch, allowed=(200,))
    if not isinstance(current, dict):
        raise RuntimeError(f"invalid reconciled repository payload for {full_name}")
    default_branch = str(current.get("default_branch") or "main")
    if default_branch != "main":
        _, ref = api(
            "GET",
            f"/repos/{full_name}/git/ref/heads/{urllib.parse.quote(default_branch, safe='')}",
            allowed=(200,),
        )
        sha = ref.get("object", {}).get("sha") if isinstance(ref, dict) else None
        if not isinstance(sha, str):
            raise RuntimeError(f"cannot read {full_name} default branch")
        api(
            "POST",
            f"/repos/{full_name}/git/refs",
            {"ref": "refs/heads/main", "sha": sha},
            allowed=(201, 422),
        )
        api(
            "PATCH", f"/repos/{full_name}", {"default_branch": "main"}, allowed=(200,)
        )
        current["default_branch"] = "main"
        print(f"NORMALIZED {full_name} default branch to main")
    return current


def ensure_project(org: str) -> tuple[str, int, str]:
    query = """
      query OrganizationProjects($login: String!) {
        organization(login: $login) {
          id
          projectsV2(first: 50, orderBy: {field: UPDATED_AT, direction: DESC}) {
            nodes { id number title closed }
          }
        }
      }
    """
    data = graphql(query, {"login": org})
    organization = data.get("organization")
    if not isinstance(organization, dict):
        raise RuntimeError(f"organization not found in GraphQL: {org}")
    owner_id = organization.get("id")
    nodes = organization.get("projectsV2", {}).get("nodes", [])
    active_projects = [
        node
        for node in (nodes if isinstance(nodes, list) else [])
        if isinstance(node, dict) and not node.get("closed")
    ]
    project = next(
        (node for node in active_projects if int(node.get("number") or 0) == 1),
        active_projects[0] if active_projects else None,
    )
    if project is None:
        mutation = """
          mutation CreateProject($ownerId: ID!, $title: String!) {
            createProjectV2(input: {ownerId: $ownerId, title: $title}) {
              projectV2 { id number title }
            }
          }
        """
        created = graphql(
            mutation, {"ownerId": owner_id, "title": f"{org}-project"}
        )
        project = created.get("createProjectV2", {}).get("projectV2")
        if not isinstance(project, dict):
            raise RuntimeError(f"failed to create GitHub Project for {org}")
        print(f"CREATED_PROJECT {org} #{project.get('number')}")
    project_id = project.get("id")
    expected_title = f"{org}-project"
    if isinstance(project_id, str) and project.get("title") != expected_title:
        mutation = """
          mutation UpdateProject($projectId: ID!, $title: String!) {
            updateProjectV2(input: {projectId: $projectId, title: $title}) {
              projectV2 { id number title }
            }
          }
        """
        updated = graphql(
            mutation, {"projectId": project_id, "title": expected_title}
        )
        replacement = updated.get("updateProjectV2", {}).get("projectV2")
        if not isinstance(replacement, dict):
            raise RuntimeError(f"failed to normalize GitHub Project title for {org}")
        project = replacement
        print(f"NORMALIZED_PROJECT {org} {expected_title}")
    project_id = project.get("id")
    number = project.get("number")
    title = project.get("title")
    if not isinstance(project_id, str) or not isinstance(number, int):
        raise RuntimeError(f"invalid GitHub Project for {org}: {project!r}")
    return project_id, number, str(title or f"{org}-project")


def ensure_issue(
    full_name: str,
    spec: dict[str, Any],
    project_id: str,
    project_number: int,
) -> dict[str, Any]:
    _, issues = api(
        "GET", f"/repos/{full_name}/issues?state=all&per_page=100", allowed=(200,)
    )
    current = None
    for item in issues if isinstance(issues, list) else []:
        if (
            isinstance(item, dict)
            and "pull_request" not in item
            and item.get("title") == ISSUE_TITLE
        ):
            current = item
            break
    project_url = f"https://github.com/orgs/{spec['org']}/projects/{project_number}"
    body = (
        "## Outcome\n\n"
        f"Create or upgrade the public Astro marketing site at "
        f"https://{spec['org'].lower()}.github.io/.\n\n"
        "## Acceptance criteria\n\n"
        "- responsive, accessible, product-specific visual design;\n"
        "- code explorer with a language selector and reviewed repository excerpts;\n"
        "- canonical GitHub Pages workflow plus source-level regression tests;\n"
        "- links to every site in this 14-organization marketing network;\n"
        "- security and privacy pages document public-site boundaries;\n"
        "- planning links and delivery evidence remain synchronized.\n\n"
        f"Linear project: {spec['linear_url']}\n\n"
        f"GitHub Project: {project_url}\n"
    )
    if current is None:
        _, current = api(
            "POST",
            f"/repos/{full_name}/issues",
            {"title": ISSUE_TITLE, "body": body},
            allowed=(201,),
        )
        print(f"CREATED_ISSUE {full_name}#{current.get('number')}")
    else:
        _, current = api(
            "PATCH",
            f"/repos/{full_name}/issues/{current['number']}",
            {"body": body, "state": "open"},
            allowed=(200,),
        )
    if not isinstance(current, dict):
        raise RuntimeError(f"invalid issue for {full_name}")
    node_id = current.get("node_id")
    if isinstance(node_id, str):
        mutation = """
          mutation AddItem($project: ID!, $content: ID!) {
            addProjectV2ItemById(input: {projectId: $project, contentId: $content}) {
              item { id }
            }
          }
        """
        try:
            graphql(mutation, {"project": project_id, "content": node_id})
        except RuntimeError as error:
            if "already" not in str(error).lower():
                raise
    return current


def resolve_source_repository(spec: dict[str, Any]) -> dict[str, Any]:
    for candidate in spec["source_candidates"]:
        info = repository(candidate)
        if info is not None:
            return info
    raise RuntimeError(
        f"none of the source repositories exist for {spec['org']}: "
        f"{spec['source_candidates']}"
    )


def clone_repository(
    full_name: str,
    destination: Path,
    env: dict[str, str],
    *,
    depth: int = 1,
) -> None:
    run(
        [
            "git", "clone", f"--depth={depth}",
            f"https://github.com/{full_name}.git", str(destination),
        ],
        env=env,
    )


def safe_text(path: Path) -> str:
    try:
        if path.stat().st_size > 240_000:
            return ""
        raw = path.read_bytes()
    except OSError:
        return ""
    if b"\x00" in raw:
        return ""
    text = raw.decode("utf-8", errors="ignore")
    if SENSITIVE_LITERAL.search(text):
        return ""
    return text


def path_score(path: Path, root: Path, language: str) -> int:
    relative = path.relative_to(root)
    lower_parts = [part.lower() for part in relative.parts]
    if any(part in SKIP_PARTS for part in lower_parts):
        return -1000
    joined = "/".join(lower_parts)
    score = 0
    if "clients/" in joined or "/client" in joined or "sdk" in joined:
        score += 70
    if "example" in joined or "examples" in joined or "quickstart" in joined:
        score += 55
    if "/src/" in f"/{joined}" or "/lib/" in f"/{joined}":
        score += 35
    if path.name.lower() in {
        "client.ts", "client.rs", "client.go", "client.dart", "client.py",
        "lib.rs", "index.ts", "main.rs", "README.md",
    }:
        score += 24
    if any(token in joined for token in ("test", "spec", "mock", "fixture")):
        score -= 35
    if language.lower().replace("++", "pp") in joined.replace("c++", "cpp"):
        score += 10
    score -= min(len(relative.parts), 12)
    return score


def trim_code(text: str, language: str) -> str:
    lines = text.replace("\r\n", "\n").replace("\r", "\n").splitlines()
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    if not lines:
        return ""
    interesting = 0
    patterns = {
        "TypeScript": ("export ", "import ", "class ", "async ", "function "),
        "Rust": ("pub ", "use ", "impl ", "async fn", "fn "),
        "Python": ("class ", "def ", "async def", "import "),
        "Go": ("package ", "func ", "type ", "import "),
        "Dart": ("class ", "Future<", "import ", "final "),
        "Java": ("public ", "class ", "record ", "import "),
        "Kotlin": ("class ", "data class", "suspend fun", "fun "),
        "Swift": ("public ", "struct ", "class ", "func "),
        "C++": ("#include", "class ", "struct ", "namespace "),
        "C": ("#include", "typedef ", "struct ", "int "),
        "Zig": ("const ", "pub fn", "fn "),
        "Gleam": ("import ", "pub fn", "pub type"),
        "Elixir": ("defmodule ", "def ", "defp "),
        "Erlang": ("-module", "-export", "->"),
        "Ruby": ("class ", "module ", "def "),
        "PHP": ("<?php", "class ", "function "),
    }
    needles = patterns.get(language, ())
    for index, line in enumerate(lines):
        if any(needle in line for needle in needles):
            interesting = max(0, index - 4)
            break
    clipped = lines[interesting : interesting + 68]
    while clipped and not clipped[-1].strip():
        clipped.pop()
    code = "\n".join(clipped)
    if SENSITIVE_LITERAL.search(code):
        return ""
    return code[:7000]


def markdown_blocks(root: Path) -> list[dict[str, Any]]:
    aliases = {
        "ts": "TypeScript", "typescript": "TypeScript", "tsx": "TypeScript",
        "rs": "Rust", "rust": "Rust", "py": "Python", "python": "Python",
        "go": "Go", "dart": "Dart", "java": "Java", "kotlin": "Kotlin",
        "swift": "Swift", "cpp": "C++", "c++": "C++", "c": "C",
        "zig": "Zig", "gleam": "Gleam", "elixir": "Elixir", "erlang": "Erlang",
        "ruby": "Ruby", "rb": "Ruby", "php": "PHP", "sh": "Shell",
        "bash": "Shell", "shell": "Shell", "yaml": "YAML", "yml": "YAML",
        "json": "JSON", "toml": "TOML",
    }
    blocks: list[dict[str, Any]] = []
    for name in ("README.md", "docs/quickstart.md", "docs/getting-started.md"):
        path = root / name
        text = safe_text(path) if path.is_file() else ""
        if not text:
            continue
        for match in re.finditer(r"```([A-Za-z0-9+#_-]+)\s*\n(.*?)```", text, re.S):
            language = aliases.get(match.group(1).lower())
            code = match.group(2).strip()
            if language and 70 <= len(code) <= 7000 and not SENSITIVE_LITERAL.search(code):
                blocks.append(
                    {
                        "language": language,
                        "code": code,
                        "path": name,
                        "score": 120,
                    }
                )
    return blocks


def collect_snippets(
    source_root: Path,
    source_info: dict[str, Any],
) -> list[dict[str, str]]:
    candidates: list[dict[str, Any]] = markdown_blocks(source_root)
    for language, extensions in LANGUAGE_EXTENSIONS:
        ranked: list[tuple[int, Path, str]] = []
        for path in source_root.rglob("*"):
            if not path.is_file() or path.suffix.lower() not in extensions:
                continue
            score = path_score(path, source_root, language)
            if score < 0:
                continue
            text = safe_text(path)
            if not text:
                continue
            code = trim_code(text, language)
            if len(code) >= 80:
                ranked.append((score, path, code))
        ranked.sort(key=lambda item: (-item[0], len(item[1].as_posix())))
        if ranked:
            score, path, code = ranked[0]
            candidates.append(
                {
                    "language": language,
                    "code": code,
                    "path": path.relative_to(source_root).as_posix(),
                    "score": score,
                }
            )

    best: dict[str, dict[str, Any]] = {}
    for candidate in candidates:
        language = str(candidate["language"])
        if language not in best or candidate["score"] > best[language]["score"]:
            best[language] = candidate
    priority = [
        "TypeScript", "Rust", "Python", "Go", "Dart", "Java", "Kotlin",
        "Swift", "C++", "C", "Zig", "Gleam", "Elixir", "Erlang", "Ruby",
        "PHP", "YAML", "JSON", "TOML", "Shell",
    ]
    selected = [best[language] for language in priority if language in best][:8]
    source_private = bool(source_info.get("private"))
    full_name = str(source_info["full_name"])
    default_branch = str(source_info.get("default_branch") or "main")
    snippets: list[dict[str, str]] = []
    for candidate in selected:
        path = str(candidate["path"])
        if source_private:
            source = f"Official SDK/source · {Path(path).name}"
            source_url = ""
        else:
            source = f"{full_name}/{path}"
            source_url = (
                f"https://github.com/{full_name}/blob/{default_branch}/"
                f"{urllib.parse.quote(path, safe='/')}"
            )
        snippets.append(
            {
                "language": str(candidate["language"]),
                "slug": re.sub(
                    r"[^a-z0-9]+", "-", str(candidate["language"]).lower()
                ).strip("-"),
                "code": str(candidate["code"]),
                "source": source,
                "sourceUrl": source_url,
            }
        )
    if len(snippets) < 2:
        source_hint = (
            "nix develop --command agent-check"
            if source_private
            else (
                f"git clone https://github.com/{full_name}.git\n"
                f"cd {full_name.split('/', 1)[1]}"
            )
        )
        snippets.append(
            {
                "language": "Shell",
                "slug": "shell",
                "code": (
                    "# Validate the reviewed source and integration contracts\n"
                    + source_hint
                ),
                "source": "Repository validation workflow",
                "sourceUrl": "" if source_private else f"https://github.com/{full_name}",
            }
        )
    deduped: list[dict[str, str]] = []
    seen: set[str] = set()
    for snippet in snippets:
        if snippet["language"] in seen:
            continue
        seen.add(snippet["language"])
        deduped.append(snippet)
    return deduped[:8]


def public_repositories(org: str, site_repo: str) -> list[dict[str, str]]:
    _, payload = api(
        "GET",
        f"/orgs/{urllib.parse.quote(org, safe='')}/repos"
        "?per_page=100&sort=pushed&direction=desc&type=public",
        allowed=(200,),
    )
    repos: list[dict[str, str]] = []
    for item in payload if isinstance(payload, list) else []:
        if not isinstance(item, dict):
            continue
        name = str(item.get("name") or "")
        if (
            not name
            or name in {site_repo, ".github"}
            or name.lower().endswith("-test")
            or "-test-" in name.lower()
            or item.get("archived")
        ):
            continue
        repos.append(
            {
                "name": name,
                "description": str(item.get("description") or "Product repository"),
                "url": str(item.get("html_url") or f"https://github.com/{org}/{name}"),
            }
        )
        if len(repos) == 4:
            break
    return repos


GLOBAL_CSS = r"""
:root {
  color-scheme: dark;
  --bg: #07101f;
  --panel: rgba(10, 20, 38, 0.72);
  --panel-strong: rgba(13, 25, 45, 0.94);
  --text: #f8fafc;
  --muted: #a9b7cd;
  --line: rgba(148, 163, 184, 0.2);
  --accent: #38bdf8;
  --accent-2: #8b5cf6;
  --max: 1180px;
  --radius: 24px;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
}
* { box-sizing: border-box; }
html { scroll-behavior: smooth; background: var(--bg); }
body {
  margin: 0;
  min-width: 320px;
  color: var(--text);
  background:
    radial-gradient(circle at 12% 12%, color-mix(in srgb, var(--accent) 24%, transparent), transparent 31rem),
    radial-gradient(circle at 88% 5%, color-mix(in srgb, var(--accent-2) 22%, transparent), transparent 34rem),
    linear-gradient(180deg, #07101f 0%, #091426 47%, #050a13 100%);
  overflow-x: hidden;
}
body::before {
  content: "";
  position: fixed;
  inset: 0;
  pointer-events: none;
  z-index: -1;
  opacity: .2;
  background-image:
    linear-gradient(rgba(255,255,255,.035) 1px, transparent 1px),
    linear-gradient(90deg, rgba(255,255,255,.035) 1px, transparent 1px);
  background-size: 42px 42px;
  mask-image: linear-gradient(to bottom, black, transparent 84%);
}
a { color: inherit; text-decoration: none; }
button, select { font: inherit; }
button:focus-visible, select:focus-visible, a:focus-visible {
  outline: 3px solid color-mix(in srgb, var(--accent) 78%, white);
  outline-offset: 4px;
}
.container { width: min(var(--max), calc(100% - 2rem)); margin-inline: auto; }
.skip-link {
  position: fixed; left: 1rem; top: -5rem; z-index: 100;
  padding: .8rem 1rem; border-radius: 12px; background: white; color: #07101f;
}
.skip-link:focus { top: 1rem; }
.site-header {
  position: sticky; top: 0; z-index: 50;
  backdrop-filter: blur(18px);
  background: rgba(5, 10, 19, .68);
  border-bottom: 1px solid var(--line);
}
.nav { min-height: 70px; display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
.brand { display: inline-flex; align-items: center; gap: .72rem; font-weight: 800; letter-spacing: -.02em; }
.brand-mark {
  width: 34px; aspect-ratio: 1; border-radius: 11px; position: relative; overflow: hidden;
  background: linear-gradient(135deg, var(--accent), var(--accent-2));
  box-shadow: 0 0 34px color-mix(in srgb, var(--accent) 40%, transparent);
}
.brand-mark::after {
  content: ""; position: absolute; inset: 7px; border: 2px solid rgba(255,255,255,.84); border-radius: 7px; transform: rotate(12deg);
}
.nav-links { display: flex; align-items: center; gap: 1rem; color: var(--muted); font-size: .94rem; }
.nav-links a:hover { color: white; }
.nav-cta {
  padding: .66rem .9rem; border: 1px solid var(--line); border-radius: 999px;
  background: rgba(255,255,255,.045); color: white;
}
.hero {
  padding: clamp(5rem, 11vw, 9rem) 0 5rem;
  position: relative;
}
.hero-grid { display: grid; grid-template-columns: minmax(0, 1.14fr) minmax(300px, .86fr); gap: 4rem; align-items: center; }
.eyebrow {
  display: inline-flex; align-items: center; gap: .55rem; color: #d8e2f1; text-transform: uppercase;
  font-weight: 750; letter-spacing: .13em; font-size: .75rem;
}
.eyebrow::before { content: ""; width: 32px; height: 2px; background: linear-gradient(90deg, var(--accent), var(--accent-2)); }
h1 {
  font-size: clamp(3.3rem, 8vw, 7.4rem); line-height: .89; letter-spacing: -.075em;
  max-width: 11ch; margin: 1.4rem 0 1.6rem; text-wrap: balance;
}
.gradient-text {
  background: linear-gradient(115deg, #fff 10%, var(--accent) 52%, var(--accent-2) 92%);
  -webkit-background-clip: text; background-clip: text; color: transparent;
}
.lede { color: #c1cee0; font-size: clamp(1.06rem, 2vw, 1.28rem); line-height: 1.72; max-width: 62ch; }
.actions { display: flex; flex-wrap: wrap; gap: .85rem; margin-top: 2rem; }
.button {
  display: inline-flex; align-items: center; justify-content: center; gap: .55rem;
  min-height: 48px; padding: .8rem 1.08rem; border-radius: 14px; font-weight: 750;
  border: 1px solid transparent; transition: transform .18s ease, box-shadow .18s ease, background .18s ease;
}
.button:hover { transform: translateY(-2px); }
.button-primary {
  color: #04101d; background: linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent-2) 68%, white));
  box-shadow: 0 16px 42px color-mix(in srgb, var(--accent) 25%, transparent);
}
.button-secondary { background: rgba(255,255,255,.055); border-color: var(--line); }
.signal-card {
  position: relative; min-height: 430px; border: 1px solid var(--line); border-radius: 34px;
  overflow: hidden; background: linear-gradient(145deg, rgba(255,255,255,.07), rgba(255,255,255,.015));
  box-shadow: 0 34px 100px rgba(0,0,0,.32);
}
.signal-card::before {
  content: ""; position: absolute; inset: 16%; border-radius: 50%;
  background: radial-gradient(circle, color-mix(in srgb, var(--accent) 42%, transparent), transparent 62%);
  filter: blur(12px);
}
.orbit { position: absolute; inset: 50% auto auto 50%; translate: -50% -50%; border: 1px solid color-mix(in srgb, var(--accent) 45%, transparent); border-radius: 50%; }
.orbit.one { width: 112px; aspect-ratio: 1; }
.orbit.two { width: 218px; aspect-ratio: 1; border-color: color-mix(in srgb, var(--accent-2) 42%, transparent); animation: spin 18s linear infinite; }
.orbit.three { width: 330px; aspect-ratio: 1; border-style: dashed; animation: spin 28s linear infinite reverse; }
.orbit.two::after, .orbit.three::after {
  content: ""; position: absolute; width: 15px; aspect-ratio: 1; border-radius: 50%;
  top: -8px; left: 50%; translate: -50% 0; background: var(--accent);
  box-shadow: 0 0 24px var(--accent);
}
.orbit.three::after { background: var(--accent-2); box-shadow: 0 0 24px var(--accent-2); }
.core {
  position: absolute; left: 50%; top: 50%; translate: -50% -50%;
  width: 74px; aspect-ratio: 1; display: grid; place-items: center; border-radius: 24px;
  background: linear-gradient(135deg, var(--accent), var(--accent-2));
  color: #06101f; font-size: 1.5rem; font-weight: 950; box-shadow: 0 0 55px color-mix(in srgb, var(--accent) 42%, transparent);
}
.signal-label {
  position: absolute; left: 1.4rem; right: 1.4rem; bottom: 1.35rem;
  padding: 1rem 1.1rem; border: 1px solid var(--line); border-radius: 18px;
  background: rgba(4, 10, 20, .72); backdrop-filter: blur(12px);
}
.signal-label strong { display: block; margin-bottom: .3rem; }
.signal-label span { color: var(--muted); font-size: .9rem; }
@keyframes spin { to { transform: rotate(360deg); } }
.section { padding: 5.5rem 0; }
.section-heading { max-width: 720px; margin-bottom: 2rem; }
.section-kicker { color: var(--accent); font-weight: 800; letter-spacing: .11em; text-transform: uppercase; font-size: .76rem; }
h2 { font-size: clamp(2.1rem, 4.7vw, 4.2rem); line-height: 1; letter-spacing: -.055em; margin: .75rem 0 1rem; text-wrap: balance; }
.section-heading p { color: var(--muted); font-size: 1.07rem; line-height: 1.7; }
.bento { display: grid; grid-template-columns: repeat(12, 1fr); gap: 1rem; }
.feature-card {
  grid-column: span 4; min-height: 265px; padding: 1.5rem; border: 1px solid var(--line);
  border-radius: var(--radius); background: var(--panel); position: relative; overflow: hidden;
  transition: transform .2s ease, border-color .2s ease;
}
.feature-card:hover { transform: translateY(-5px); border-color: color-mix(in srgb, var(--accent) 46%, var(--line)); }
.feature-card::after {
  content: ""; position: absolute; width: 170px; aspect-ratio: 1; right: -80px; bottom: -110px;
  border-radius: 50%; background: radial-gradient(circle, color-mix(in srgb, var(--accent) 24%, transparent), transparent 68%);
}
.card-index { color: var(--accent); font: 700 .78rem/1 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .13em; }
.feature-card h3 { font-size: 1.35rem; margin: 4.8rem 0 .75rem; letter-spacing: -.025em; }
.feature-card p { color: var(--muted); line-height: 1.65; margin: 0; }
.proof-row { display: flex; flex-wrap: wrap; gap: .7rem; margin-top: 1.2rem; }
.proof-pill {
  border: 1px solid var(--line); background: rgba(255,255,255,.045); border-radius: 999px;
  padding: .62rem .85rem; color: #d4dfed; font-size: .88rem;
}
.code-shell {
  border: 1px solid var(--line); border-radius: 28px; overflow: hidden;
  background: #050b14; box-shadow: 0 32px 90px rgba(0,0,0,.33);
}
.code-toolbar {
  display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding: 1rem 1.15rem;
  border-bottom: 1px solid var(--line); background: rgba(255,255,255,.035);
}
.language-control { display: flex; align-items: center; gap: .7rem; }
.language-control label { color: var(--muted); font-size: .86rem; }
.language-control select {
  color: white; background: #0c1727; border: 1px solid var(--line); border-radius: 11px; padding: .65rem 2.4rem .65rem .8rem;
}
.copy-button {
  border: 1px solid var(--line); color: white; background: rgba(255,255,255,.055);
  border-radius: 11px; padding: .65rem .85rem; cursor: pointer;
}
.code-panel[hidden] { display: none; }
.code-meta {
  display: flex; justify-content: space-between; gap: 1rem; padding: .85rem 1.15rem;
  color: var(--muted); font-size: .82rem; border-bottom: 1px solid rgba(148,163,184,.12);
}
.code-meta a { color: var(--accent); }
pre {
  margin: 0; padding: 1.5rem; overflow: auto; min-height: 350px; max-height: 590px;
  color: #d7e4f5; font: 500 .9rem/1.65 ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  tab-size: 2;
}
.architecture-grid { display: grid; grid-template-columns: .85fr 1.15fr; gap: 1rem; }
.manifest, .repo-grid {
  border: 1px solid var(--line); border-radius: var(--radius); background: var(--panel); padding: 1.4rem;
}
.manifest ol { margin: 1.2rem 0 0; padding: 0; list-style: none; counter-reset: path; }
.manifest li { display: grid; grid-template-columns: 34px 1fr; gap: .75rem; padding: .82rem 0; border-top: 1px solid rgba(148,163,184,.13); color: var(--muted); }
.manifest li::before {
  counter-increment: path; content: counter(path); display: grid; place-items: center; width: 27px; height: 27px;
  border-radius: 9px; color: #07101f; background: linear-gradient(135deg, var(--accent), var(--accent-2)); font-weight: 850; font-size: .8rem;
}
.repo-list { display: grid; gap: .75rem; margin-top: 1rem; }
.repo-card {
  display: block; border: 1px solid rgba(148,163,184,.14); border-radius: 16px; padding: 1rem;
  background: rgba(255,255,255,.025); transition: background .18s ease, transform .18s ease;
}
.repo-card:hover { background: rgba(255,255,255,.06); transform: translateX(3px); }
.repo-card strong { display: block; margin-bottom: .35rem; }
.repo-card span { color: var(--muted); font-size: .87rem; line-height: 1.45; }
.network-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: .8rem; }
.network-card {
  min-height: 142px; padding: 1rem; border: 1px solid var(--line); border-radius: 18px;
  background: rgba(255,255,255,.032); display: flex; flex-direction: column; justify-content: space-between;
  transition: transform .18s ease, background .18s ease;
}
.network-card:hover { transform: translateY(-4px); background: rgba(255,255,255,.07); }
.network-card[aria-current="page"] { border-color: color-mix(in srgb, var(--accent) 55%, var(--line)); background: color-mix(in srgb, var(--accent) 10%, transparent); }
.network-card small { color: var(--muted); line-height: 1.35; }
.network-card span { color: var(--accent); font-weight: 750; font-size: .82rem; }
.cta-panel {
  display: grid; grid-template-columns: 1.1fr .9fr; gap: 2rem; align-items: center;
  border: 1px solid var(--line); border-radius: 30px; padding: clamp(1.5rem, 5vw, 3.2rem);
  background:
    linear-gradient(135deg, color-mix(in srgb, var(--accent) 12%, transparent), color-mix(in srgb, var(--accent-2) 10%, transparent)),
    var(--panel-strong);
}
.cta-panel p { color: var(--muted); line-height: 1.65; }
.cta-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: .8rem; }
footer { border-top: 1px solid var(--line); padding: 2.4rem 0 3.2rem; color: var(--muted); }
.footer-row { display: flex; justify-content: space-between; gap: 1rem; flex-wrap: wrap; }
.footer-row a { color: white; }
@media (max-width: 950px) {
  .hero-grid, .architecture-grid, .cta-panel { grid-template-columns: 1fr; }
  .signal-card { min-height: 370px; max-width: 630px; width: 100%; }
  .feature-card { grid-column: span 6; }
  .network-grid { grid-template-columns: repeat(2, 1fr); }
  .cta-actions { justify-content: flex-start; }
}
@media (max-width: 650px) {
  .nav-links a:not(.nav-cta) { display: none; }
  .hero { padding-top: 4rem; }
  h1 { font-size: clamp(3rem, 17vw, 5rem); }
  .feature-card { grid-column: 1 / -1; min-height: 230px; }
  .feature-card h3 { margin-top: 3.4rem; }
  .network-grid { grid-template-columns: 1fr; }
  .code-toolbar, .code-meta { align-items: flex-start; flex-direction: column; }
  pre { font-size: .78rem; padding: 1rem; min-height: 300px; }
}
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { scroll-behavior: auto !important; animation: none !important; transition-duration: .001ms !important; }
}
"""


def astro_config(spec: dict[str, Any]) -> str:
    hostname = spec["org"].lower()
    return (
        "import { defineConfig } from 'astro/config';\n\n"
        "export default defineConfig({\n"
        f"  site: 'https://{hostname}.github.io',\n"
        "  output: 'static',\n"
        "  trailingSlash: 'always',\n"
        "});\n"
    )


def package_json(spec: dict[str, Any]) -> str:
    payload = {
        "name": spec["repo"],
        "version": "1.0.0",
        "private": True,
        "type": "module",
        "scripts": {
            "dev": "astro dev",
            "build": "astro check && astro build",
            "check": "astro check",
            "test": "node --test tests/site.test.mjs",
            "preview": "astro preview",
        },
        "dependencies": {
            "@astrojs/check": "^0.9.10",
            "astro": "^7.1.6",
            "typescript": "^6.0.2",
        },
        "license": "MIT",
    }
    return json.dumps(payload, indent=2) + "\n"


def patch_lock(template_lock: Path, destination: Path, package_name: str) -> None:
    payload = json.loads(template_lock.read_text(encoding="utf-8"))
    if isinstance(payload, dict):
        payload["name"] = package_name
        packages = payload.get("packages")
        if isinstance(packages, dict) and isinstance(packages.get(""), dict):
            packages[""]["name"] = package_name
            packages[""]["version"] = "1.0.0"
    destination.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def page_source(
    spec: dict[str, Any],
    snippets: list[dict[str, str]],
    repos: list[dict[str, str]],
    project: dict[str, Any],
    source_info: dict[str, Any],
) -> str:
    site = {
        "title": spec["title"],
        "org": spec["org"],
        "category": spec["category"],
        "tagline": spec["tagline"],
        "summary": spec["summary"],
        "accent": spec["accent"],
        "accent2": spec["accent2"],
        "features": spec["features"],
        "proof": spec["proof"],
        "linearUrl": spec["linear_url"],
        "projectUrl": project["url"],
        "sourceName": source_info["full_name"],
        "sourceUrl": "" if source_info.get("private") else source_info["html_url"],
        "siteUrl": f"https://{spec['org'].lower()}.github.io/",
    }
    template = r'''---
import "../styles/global.css";
const site = __SITE__;
const snippets = __SNIPPETS__;
const repositories = __REPOSITORIES__;
const network = __NETWORK__;
const pageTitle = `${site.title} · ${site.category}`;
const description = site.summary;
const structuredData = {
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  name: site.title,
  applicationCategory: site.category,
  description,
  url: site.siteUrl,
  codeRepository: site.sourceUrl || `https://github.com/${site.org}`,
};
---
<!doctype html>
<html lang="en" style={`--accent:${site.accent};--accent-2:${site.accent2}`}>
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width" />
    <meta name="theme-color" content="#07101f" />
    <meta name="description" content={description} />
    <meta property="og:title" content={pageTitle} />
    <meta property="og:description" content={description} />
    <meta property="og:type" content="website" />
    <meta property="og:url" content={site.siteUrl} />
    <meta name="twitter:card" content="summary_large_image" />
    <link rel="canonical" href={site.siteUrl} />
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <title>{pageTitle}</title>
    <script type="application/ld+json" set:html={JSON.stringify(structuredData)} />
  </head>
  <body>
    <a class="skip-link" href="#main">Skip to content</a>
    <header class="site-header">
      <nav class="nav container" aria-label="Primary">
        <a class="brand" href="/" aria-label={`${site.title} home`}>
          <span class="brand-mark" aria-hidden="true"></span>
          <span>{site.title}</span>
        </a>
        <div class="nav-links">
          <a href="#platform">Platform</a>
          <a href="#code">Code</a>
          <a href="#network">Network</a>
          <a class="nav-cta" href={`https://github.com/${site.org}`}>GitHub ↗</a>
        </div>
      </nav>
    </header>

    <main id="main">
      <section class="hero">
        <div class="container hero-grid">
          <div>
            <span class="eyebrow">{site.category}</span>
            <h1><span class="gradient-text">{site.tagline}</span></h1>
            <p class="lede">{site.summary}</p>
            <div class="actions">
              <a class="button button-primary" href="#code">Explore the SDK ↓</a>
              <a class="button button-secondary" href={`https://github.com/${site.org}`}>View organization ↗</a>
            </div>
            <div class="proof-row" aria-label="Current implementation evidence">
              {site.proof.map((item) => <span class="proof-pill">{item}</span>)}
            </div>
          </div>
          <div class="signal-card" aria-label={`${site.title} system map`}>
            <span class="orbit one" aria-hidden="true"></span>
            <span class="orbit two" aria-hidden="true"></span>
            <span class="orbit three" aria-hidden="true"></span>
            <span class="core" aria-hidden="true">{site.title.slice(0, 2).toUpperCase()}</span>
            <div class="signal-label">
              <strong>Contracts at the center</strong>
              <span>Interfaces → libraries → clients → apps → evidence</span>
            </div>
          </div>
        </div>
      </section>

      <section class="section" id="platform">
        <div class="container">
          <div class="section-heading">
            <span class="section-kicker">Product system</span>
            <h2>Designed as a platform, not a demo.</h2>
            <p>Each surface is grounded in the organization’s current repositories, project plan, and recent implementation work.</p>
          </div>
          <div class="bento">
            {site.features.map(([title, body], index) => (
              <article class="feature-card">
                <span class="card-index">0{index + 1}</span>
                <h3>{title}</h3>
                <p>{body}</p>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section class="section" id="code">
        <div class="container">
          <div class="section-heading">
            <span class="section-kicker">Code explorer</span>
            <h2>One workflow. Multiple runtimes.</h2>
            <p>These reviewed previews are selected from the current official SDK or source repository. Choose a language to compare integration surfaces.</p>
          </div>
          <div class="code-shell">
            <div class="code-toolbar">
              <div class="language-control">
                <label for="language-select">Language</label>
                <select id="language-select" aria-controls="code-panels">
                  {snippets.map((snippet) => <option value={snippet.slug}>{snippet.language}</option>)}
                </select>
              </div>
              <button class="copy-button" type="button" data-copy>Copy code</button>
            </div>
            <div id="code-panels">
              {snippets.map((snippet, index) => (
                <section class="code-panel" data-language={snippet.slug} hidden={index !== 0}>
                  <div class="code-meta">
                    <span>{snippet.language}</span>
                    {snippet.sourceUrl
                      ? <a href={snippet.sourceUrl}>{snippet.source} ↗</a>
                      : <span>{snippet.source}</span>}
                  </div>
                  <pre tabindex="0"><code>{snippet.code}</code></pre>
                </section>
              ))}
            </div>
          </div>
        </div>
      </section>

      <section class="section" id="architecture">
        <div class="container">
          <div class="section-heading">
            <span class="section-kicker">Delivery graph</span>
            <h2>Open boundaries, visible ownership.</h2>
            <p>The marketing surface links directly to source, planning, and organization delivery records.</p>
          </div>
          <div class="architecture-grid">
            <article class="manifest">
              <h3>Working contract</h3>
              <ol>
                <li>Define versioned interfaces and safety boundaries.</li>
                <li>Implement SDK or service behavior in reviewed source.</li>
                <li>Exercise the behavior with repository-owned tests.</li>
                <li>Ship through pull requests, Pages CI, and project evidence.</li>
              </ol>
              <div class="actions">
                <a class="button button-secondary" href={site.linearUrl}>Linear project ↗</a>
                <a class="button button-secondary" href={site.projectUrl}>GitHub Project ↗</a>
              </div>
            </article>
            <article class="repo-grid">
              <h3>Active repositories</h3>
              <div class="repo-list">
                {repositories.map((repository) => (
                  <a class="repo-card" href={repository.url}>
                    <strong>{repository.name}</strong>
                    <span>{repository.description}</span>
                  </a>
                ))}
              </div>
            </article>
          </div>
        </div>
      </section>

      <section class="section" id="network">
        <div class="container">
          <div class="section-heading">
            <span class="section-kicker">Engineering network</span>
            <h2>Explore the connected product organizations.</h2>
            <p>Fourteen focused platforms share a consistent, source-backed Astro marketing and SDK discovery experience.</p>
          </div>
          <div class="network-grid">
            {network.map((item) => (
              <a class="network-card" href={item.url} aria-current={item.org === site.org ? "page" : undefined}>
                <small>{item.category}</small>
                <strong>{item.title}</strong>
                <span>{item.org === site.org ? "You are here" : "Visit site ↗"}</span>
              </a>
            ))}
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container cta-panel">
          <div>
            <span class="section-kicker">Build with the source</span>
            <h2>Follow the contracts, tests, and delivery trail.</h2>
            <p>Start in the official repository, compare runtime examples above, and use the project links to understand current priorities and remaining work.</p>
          </div>
          <div class="cta-actions">
            {site.sourceUrl && <a class="button button-primary" href={site.sourceUrl}>Open source ↗</a>}
            <a class="button button-secondary" href={`https://github.com/${site.org}/issues`}>Browse issues ↗</a>
          </div>
        </div>
      </section>
    </main>

    <footer>
      <div class="container footer-row">
        <span>© 2026 {site.title}. Built with Astro and GitHub Pages.</span>
        <span><a href="/security/">Security</a> · <a href="/privacy/">Privacy</a> · <a href={site.linearUrl}>Planning</a> · <a href={site.projectUrl}>Delivery</a> · <a href={`https://github.com/${site.org}`}>Source</a></span>
      </div>
    </footer>

    <script>
      const select = document.querySelector<HTMLSelectElement>("#language-select");
      const panels = Array.from(document.querySelectorAll<HTMLElement>(".code-panel"));
      const copyButton = document.querySelector<HTMLButtonElement>("[data-copy]");
      const showPanel = (slug) => {
        for (const panel of panels) {
          panel.hidden = panel.dataset.language !== slug;
        }
        const active = panels.find((panel) => !panel.hidden);
        copyButton?.setAttribute("aria-label", `Copy ${select?.selectedOptions[0]?.textContent ?? ""} code`);
        return active;
      };
      select?.addEventListener("change", () => showPanel(select.value));
      copyButton?.addEventListener("click", async () => {
        const active = showPanel(select?.value ?? "");
        const code = active?.querySelector("code")?.textContent ?? "";
        try {
          await navigator.clipboard.writeText(code);
          copyButton.textContent = "Copied";
          window.setTimeout(() => { copyButton.textContent = "Copy code"; }, 1400);
        } catch {
          copyButton.textContent = "Select and copy";
        }
      });
    </script>
  </body>
</html>
'''
    return (
        template.replace("__SITE__", json.dumps(site, ensure_ascii=False))
        .replace("__SNIPPETS__", json.dumps(snippets, ensure_ascii=False))
        .replace("__REPOSITORIES__", json.dumps(repos, ensure_ascii=False))
        .replace("__NETWORK__", json.dumps(NETWORK, ensure_ascii=False))
    )


def tests_source(spec: dict[str, Any], snippets: list[dict[str, str]]) -> str:
    network_urls = [f"https://{item['org'].lower()}.github.io/" for item in NETWORK]
    payload = {
        "title": spec["title"],
        "languages": [item["language"] for item in snippets],
        "networkUrls": network_urls,
    }
    return r'''import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(new URL("../src/pages/index.astro", import.meta.url), "utf8");
const styles = await readFile(new URL("../src/styles/global.css", import.meta.url), "utf8");
const security = await readFile(new URL("../src/pages/security.astro", import.meta.url), "utf8");
const privacy = await readFile(new URL("../src/pages/privacy.astro", import.meta.url), "utf8");
const expected = __EXPECTED__;

test("renders the product identity and source-backed code explorer", () => {
  assert.match(source, new RegExp(expected.title.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  assert.match(source, /id="language-select"/);
  assert.match(source, /data-language=/);
  assert.match(source, /navigator\.clipboard\.writeText/);
  for (const language of expected.languages) {
    assert.match(source, new RegExp(language.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
});

test("connects all fourteen organization sites", () => {
  assert.equal(expected.networkUrls.length, 14);
  for (const url of expected.networkUrls) {
    assert.match(source, new RegExp(url.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
});

test("documents public-site security and privacy boundaries", () => {
  assert.match(source, /href="\/security\//);
  assert.match(source, /href="\/privacy\//);
  assert.match(security, /Report a vulnerability/);
  assert.match(privacy, /No analytics or advertising trackers/);
});

test("keeps responsive and reduced-motion behavior", () => {
  assert.match(styles, /@media \(max-width: 650px\)/);
  assert.match(styles, /prefers-reduced-motion: reduce/);
  assert.match(styles, /\.network-grid/);
});
'''.replace("__EXPECTED__", json.dumps(payload, ensure_ascii=False))


def ci_workflow() -> str:
    return """name: CI

on:
  pull_request:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  site:
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - name: Check out repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Install Node.js
        uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0
        with:
          node-version: 22
          cache: npm
      - run: npm ci
      - run: npm test
      - run: npm run build
"""


def pages_workflow() -> str:
    return """name: Pages

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: true

jobs:
  build:
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - name: Check out repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Install Node.js
        uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0
        with:
          node-version: 22
          cache: npm
      - run: npm ci
      - run: npm test
      - run: npm run build
      - name: Configure Pages
        uses: actions/configure-pages@45bfe0192ca1faeb007ade9deae92b16b8254a0d # v6.0.0
      - name: Upload site
        uses: actions/upload-pages-artifact@fc324d3547104276b827a68afc52ff2a11cc49c9 # v5.0.0
        with:
          path: dist
  deploy:
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    needs: build
    steps:
      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@cd2ce8fcbc39b97be8ca5fce6e763baed58fa128 # v5.0.0
"""


def readme_source(
    spec: dict[str, Any],
    project: dict[str, Any],
    source_info: dict[str, Any],
    snippets: list[dict[str, str]],
) -> str:
    source_reference = (
        source_info["full_name"]
        if source_info.get("private")
        else f"[{source_info['full_name']}]({source_info['html_url']})"
    )
    languages = ", ".join(item["language"] for item in snippets)
    return f"""# {spec['title']} marketing site

Astro marketing and developer-discovery site for
[`{spec['org']}`](https://github.com/{spec['org']}).

- Live site: https://{spec['org'].lower()}.github.io/
- GitHub Project: {project['url']}
- Linear project: {spec['linear_url']}
- Source used for code examples: {source_reference}
- Code explorer languages: {languages}

## Product scope

{spec['summary']}

The page copy is grounded in the organization’s Linear plan, GitHub Project,
active repositories, and current source. Private repositories are never linked
from the public page; selected excerpts are screened for credentials and
presented only as official SDK/source examples.

## Local development

```bash
npm ci
npm test
npm run build
npm run dev
```

GitHub Pages deployment is performed by the pinned `Pages` workflow after
changes land on `main`. The site includes responsive layouts, keyboard focus
states, reduced-motion behavior, structured metadata, security and privacy
pages, and a fourteen-site engineering network.
"""


def policy_page(spec: dict[str, Any], kind: str) -> str:
    if kind == "security":
        title = "Security"
        intro = (
            "This public marketing site contains static product information and "
            "reviewed source previews. It does not process product credentials or "
            "operate the product control plane."
        )
        sections = [
            (
                "Report a vulnerability",
                f"Use the private security-reporting channel for the {spec['org']} "
                "organization or contact the maintainers through a non-public route. "
                "Do not include secrets in a public issue.",
            ),
            (
                "Code preview boundary",
                "Examples are selected from current official repositories and screened "
                "for credential-like literals. They are documentation previews, not "
                "embedded credentials or a production configuration.",
            ),
            (
                "Delivery controls",
                "Changes are reviewed through pull requests, tested with Astro and "
                "source-level checks, and deployed through pinned GitHub Pages actions.",
            ),
        ]
    elif kind == "privacy":
        title = "Privacy"
        intro = (
            "This static site is designed to minimize data collection while linking "
            "visitors to public source, planning, and delivery evidence."
        )
        sections = [
            (
                "No analytics or advertising trackers",
                "The site does not add product analytics, advertising pixels, session "
                "replay, fingerprinting, or custom tracking cookies.",
            ),
            (
                "External destinations",
                "Links to GitHub and Linear are external destinations governed by their "
                "respective privacy terms. The site does not proxy credentials to them.",
            ),
            (
                "Public content only",
                "The page exposes public product copy and reviewed code previews. Private "
                "repository URLs, access tokens, and user data are not intentionally "
                "published.",
            ),
        ]
    else:
        raise ValueError(f"unsupported policy page: {kind}")
    page = {
        "title": spec["title"],
        "org": spec["org"],
        "accent": spec["accent"],
        "accent2": spec["accent2"],
        "policyTitle": title,
        "intro": intro,
        "sections": sections,
    }
    template = r'''---
import "../styles/global.css";
const page = __PAGE__;
const title = `${page.policyTitle} · ${page.title}`;
---
<!doctype html>
<html lang="en" style={`--accent:${page.accent};--accent-2:${page.accent2}`}>
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width" />
    <meta name="description" content={page.intro} />
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <title>{title}</title>
  </head>
  <body>
    <a class="skip-link" href="#main">Skip to content</a>
    <header class="site-header">
      <nav class="nav container" aria-label="Policy">
        <a class="brand" href="/">
          <span class="brand-mark" aria-hidden="true"></span>
          <span>{page.title}</span>
        </a>
        <div class="nav-links">
          <a href="/security/">Security</a>
          <a href="/privacy/">Privacy</a>
          <a class="nav-cta" href={`https://github.com/${page.org}`}>GitHub ↗</a>
        </div>
      </nav>
    </header>
    <main id="main" class="section">
      <div class="container">
        <div class="section-heading">
          <span class="section-kicker">Public-site policy</span>
          <h1 style="font-size:clamp(3rem,9vw,6.8rem);max-width:12ch">{page.policyTitle}</h1>
          <p>{page.intro}</p>
        </div>
        <div class="bento">
          {page.sections.map(([heading, body], index) => (
            <article class="feature-card">
              <span class="card-index">0{index + 1}</span>
              <h3>{heading}</h3>
              <p>{body}</p>
            </article>
          ))}
        </div>
        <div class="actions">
          <a class="button button-primary" href="/">Return home</a>
          <a class="button button-secondary" href={`https://github.com/${page.org}/security`}>Organization security ↗</a>
        </div>
      </div>
    </main>
  </body>
</html>
'''
    return template.replace("__PAGE__", json.dumps(page, ensure_ascii=False))


def favicon_source(spec: dict[str, Any]) -> str:
    initials = "".join(word[0] for word in spec["title"].split()[:2]).upper()
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <defs><linearGradient id="g" x1="0" x2="1" y1="0" y2="1"><stop stop-color="{spec['accent']}"/><stop offset="1" stop-color="{spec['accent2']}"/></linearGradient></defs>
  <rect width="64" height="64" rx="18" fill="#07101f"/>
  <rect x="5" y="5" width="54" height="54" rx="15" fill="url(#g)"/>
  <text x="32" y="39" text-anchor="middle" font-family="system-ui,sans-serif" font-size="21" font-weight="800" fill="#07101f">{initials}</text>
</svg>
"""


def write_site(
    stage: Path,
    template_lock: Path,
    spec: dict[str, Any],
    snippets: list[dict[str, str]],
    repos: list[dict[str, str]],
    project: dict[str, Any],
    source_info: dict[str, Any],
) -> None:
    (stage / "src/pages").mkdir(parents=True, exist_ok=True)
    (stage / "src/styles").mkdir(parents=True, exist_ok=True)
    (stage / "public").mkdir(parents=True, exist_ok=True)
    (stage / "tests").mkdir(parents=True, exist_ok=True)
    (stage / ".github/workflows").mkdir(parents=True, exist_ok=True)
    (stage / "src/pages/index.astro").write_text(
        page_source(spec, snippets, repos, project, source_info), encoding="utf-8"
    )
    (stage / "src/pages/security.astro").write_text(
        policy_page(spec, "security"), encoding="utf-8"
    )
    (stage / "src/pages/privacy.astro").write_text(
        policy_page(spec, "privacy"), encoding="utf-8"
    )
    (stage / "src/styles/global.css").write_text(GLOBAL_CSS, encoding="utf-8")
    (stage / "public/favicon.svg").write_text(favicon_source(spec), encoding="utf-8")
    (stage / "tests/site.test.mjs").write_text(
        tests_source(spec, snippets), encoding="utf-8"
    )
    (stage / "astro.config.mjs").write_text(astro_config(spec), encoding="utf-8")
    (stage / "package.json").write_text(package_json(spec), encoding="utf-8")
    patch_lock(template_lock, stage / "package-lock.json", spec["repo"])
    (stage / ".github/workflows/ci.yml").write_text(ci_workflow(), encoding="utf-8")
    (stage / ".github/workflows/pages.yml").write_text(
        pages_workflow(), encoding="utf-8"
    )
    (stage / "README.md").write_text(
        readme_source(spec, project, source_info, snippets), encoding="utf-8"
    )
    (stage / ".gitignore").write_text(
        "node_modules/\ndist/\n.astro/\n.DS_Store\n", encoding="utf-8"
    )


def validate_site(stage: Path, node_modules: Path) -> None:
    link = stage / "node_modules"
    if link.exists() or link.is_symlink():
        if link.is_dir() and not link.is_symlink():
            shutil.rmtree(link)
        else:
            link.unlink()
    link.symlink_to(node_modules, target_is_directory=True)
    environment = os.environ.copy()
    environment["PATH"] = f"{node_modules / '.bin'}:{environment.get('PATH', '')}"
    try:
        run(["node", "--test", "tests/site.test.mjs"], cwd=stage, env=environment)
        run(["astro", "check"], cwd=stage, env=environment)
        run(["astro", "build"], cwd=stage, env=environment)
    finally:
        link.unlink(missing_ok=True)
        shutil.rmtree(stage / "dist", ignore_errors=True)
        shutil.rmtree(stage / ".astro", ignore_errors=True)


def copy_generated_site(stage: Path, checkout: Path) -> None:
    for relative in (
        "src",
        "tests",
        ".github/workflows",
        "astro.config.mjs",
        "package.json",
        "package-lock.json",
        "README.md",
        ".gitignore",
    ):
        target = checkout / relative
        if target.is_dir() and not target.is_symlink():
            shutil.rmtree(target)
        elif target.exists() or target.is_symlink():
            target.unlink()
    for source in stage.rglob("*"):
        relative = source.relative_to(stage)
        if any(part in {"node_modules", "dist", ".astro"} for part in relative.parts):
            continue
        destination = checkout / relative
        if source.is_dir():
            destination.mkdir(parents=True, exist_ok=True)
        elif source.is_file():
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)


def ensure_pages(full_name: str) -> dict[str, Any]:
    status, current = api("GET", f"/repos/{full_name}/pages", allowed=(200, 404))
    if status == 404:
        _, current = api(
            "POST",
            f"/repos/{full_name}/pages",
            {"build_type": "workflow"},
            allowed=(201,),
        )
        print(f"ENABLED_PAGES {full_name}")
    else:
        _, current = api(
            "PUT",
            f"/repos/{full_name}/pages",
            {"build_type": "workflow"},
            allowed=(204,),
        )
        _, current = api("GET", f"/repos/{full_name}/pages", allowed=(200,))
    if not isinstance(current, dict):
        _, current = api("GET", f"/repos/{full_name}/pages", allowed=(200,))
    if not isinstance(current, dict):
        raise RuntimeError(f"invalid Pages state for {full_name}")
    return current


def open_or_update_pull_request(
    full_name: str,
    spec: dict[str, Any],
    branch: str,
    head_sha: str,
    issue_number: int,
) -> dict[str, Any]:
    org = spec["org"]
    _, pulls = api(
        "GET",
        f"/repos/{full_name}/pulls?state=open&head="
        f"{urllib.parse.quote(org + ':' + branch, safe=':')}&per_page=20",
        allowed=(200,),
    )
    body = (
        f"Closes #{issue_number}\n\n"
        "## What changed\n\n"
        "- builds a product-specific Astro marketing experience;\n"
        "- adds a source-backed, multi-language code explorer;\n"
        "- connects Linear planning and the organization GitHub Project;\n"
        "- links the complete fourteen-site engineering network;\n"
        "- adds responsive, accessibility, regression, and Pages delivery controls.\n\n"
        "The source excerpts were selected from the current official repository and "
        "screened for credential-like literals before publication."
    )
    current = pulls[0] if isinstance(pulls, list) and pulls else None
    if current is None:
        _, current = api(
            "POST",
            f"/repos/{full_name}/pulls",
            {
                "title": "Launch polished Astro marketing site and code explorer",
                "head": branch,
                "base": "main",
                "body": body,
                "draft": False,
            },
            allowed=(201,),
        )
        print(f"CREATED_PR {full_name}#{current.get('number')}")
    else:
        _, current = api(
            "PATCH",
            f"/repos/{full_name}/pulls/{current['number']}",
            {"title": "Launch polished Astro marketing site and code explorer", "body": body},
            allowed=(200,),
        )
    if not isinstance(current, dict):
        raise RuntimeError(f"invalid pull request for {full_name}")
    observed = current.get("head", {}).get("sha")
    if observed != head_sha:
        raise RuntimeError(
            f"pull request head mismatch for {full_name}: {observed} != {head_sha}"
        )
    return current


def merge_pull_request(
    full_name: str,
    pull: dict[str, Any],
    head_sha: str,
) -> dict[str, Any]:
    number = int(pull["number"])
    last_error = ""
    for attempt in range(60):
        status, payload = api(
            "PUT",
            f"/repos/{full_name}/pulls/{number}/merge",
            {
                "merge_method": "squash",
                "sha": head_sha,
                "commit_title": "Launch polished Astro marketing site and code explorer",
                "commit_message": (
                    "Ship the product-specific Astro experience, reviewed code examples, "
                    "project links, tests, and GitHub Pages delivery workflow."
                ),
            },
            allowed=(200, 405, 409),
        )
        if status == 200 and isinstance(payload, dict) and payload.get("merged"):
            print(f"MERGED_PR {full_name}#{number} {payload.get('sha')}")
            return payload
        last_error = json.dumps(payload, sort_keys=True)[:2000]
        if attempt in {0, 5, 15, 30, 45}:
            print(f"WAITING_TO_MERGE {full_name}#{number} attempt={attempt + 1}")
        time.sleep(10)
    raise RuntimeError(
        f"unable to merge {full_name}#{number} at {head_sha}: {last_error}"
    )


def publish_site(
    root: Path,
    spec: dict[str, Any],
    git_env: dict[str, str],
    template_lock: Path,
    node_modules: Path,
) -> dict[str, Any]:
    full_name = f"{spec['org']}/{spec['repo']}"
    ensure_repository(spec)
    project_id, project_number, project_title = ensure_project(spec["org"])
    issue = ensure_issue(full_name, spec, project_id, project_number)
    source_info = resolve_source_repository(spec)

    slug = spec["org"].lower()
    source_root = root / "sources" / slug
    clone_repository(str(source_info["full_name"]), source_root, git_env)
    snippets = collect_snippets(source_root, source_info)
    if len(snippets) < 2:
        raise RuntimeError(f"{full_name} has fewer than two safe code examples")

    repos = public_repositories(spec["org"], spec["repo"])
    if not repos:
        repos = [
            {
                "name": f"{spec['org']} organization",
                "description": "Browse the organization repositories and current delivery work.",
                "url": f"https://github.com/{spec['org']}",
            }
        ]

    project = {
        "number": project_number,
        "title": project_title,
        "url": f"https://github.com/orgs/{spec['org']}/projects/{project_number}",
    }
    stage = root / "stages" / slug
    stage.mkdir(parents=True, exist_ok=True)
    write_site(stage, template_lock, spec, snippets, repos, project, source_info)
    validate_site(stage, node_modules)

    checkout = root / "sites" / slug
    clone_repository(full_name, checkout, git_env)
    run(["git", "config", "user.name", "ORESoftware publication automation"], cwd=checkout)
    run(["git", "config", "user.email", "bot@oresoftware.dev"], cwd=checkout)
    run(["git", "checkout", "main"], cwd=checkout)
    branch = f"{BRANCH_PREFIX}-{slug}"
    run(["git", "checkout", "-B", branch, "main"], cwd=checkout)
    copy_generated_site(stage, checkout)
    run(["git", "add", "-A"], cwd=checkout)
    changed = run(["git", "status", "--porcelain"], cwd=checkout).strip()

    pull_number: int | None = None
    merge_sha = run(["git", "rev-parse", "HEAD"], cwd=checkout).strip()
    if changed:
        run(
            [
                "git",
                "commit",
                "-m",
                "feat: launch polished Astro marketing site",
                "-m",
                f"Linear: {spec['linear_url']}",
                "-m",
                f"GitHub-Project: {project['url']}",
            ],
            cwd=checkout,
        )
        head_sha = run(["git", "rev-parse", "HEAD"], cwd=checkout).strip()
        run(
            ["git", "push", "--force", "origin", f"HEAD:refs/heads/{branch}"],
            cwd=checkout,
            env=git_env,
        )
        pull = open_or_update_pull_request(
            full_name, spec, branch, head_sha, int(issue["number"])
        )
        pull_number = int(pull["number"])
        merged = merge_pull_request(full_name, pull, head_sha)
        merge_sha = str(merged.get("sha") or "")
    else:
        print(f"UNCHANGED {full_name}")

    api(
        "PATCH",
        f"/repos/{full_name}/issues/{int(issue['number'])}",
        {"state": "closed"},
        allowed=(200,),
    )
    pages = ensure_pages(full_name)
    _, current = api("GET", f"/repos/{full_name}", allowed=(200,))
    if not isinstance(current, dict) or current.get("visibility") != "public":
        raise RuntimeError(f"{full_name} is not public after publication")
    return {
        "organization": spec["org"],
        "repository": full_name,
        "site": f"https://{spec['org'].lower()}.github.io/",
        "source": str(source_info["full_name"]),
        "sourcePrivate": bool(source_info.get("private")),
        "languages": [snippet["language"] for snippet in snippets],
        "githubProject": project["url"],
        "issue": f"https://github.com/{full_name}/issues/{issue['number']}",
        "pullRequest": (
            f"https://github.com/{full_name}/pull/{pull_number}"
            if pull_number is not None
            else None
        ),
        "mergeSha": merge_sha,
        "pagesHtmlUrl": str(pages.get("html_url") or ""),
        "status": "published",
    }


def main() -> int:
    verify_identity_and_memberships()
    root = Path(tempfile.mkdtemp(prefix="astro-marketing-sites-"))
    report: list[dict[str, Any]] = []
    try:
        git_env = create_git_environment(root)
        template = root / "template"
        clone_repository(TEMPLATE_REPOSITORY, template, git_env)
        run(["npm", "ci"], cwd=template)
        template_lock = template / "package-lock.json"
        node_modules = template / "node_modules"
        if not template_lock.is_file() or not node_modules.is_dir():
            raise RuntimeError("Astro template dependencies were not prepared")

        for spec in SPECS:
            print(f"BEGIN {spec['org']}/{spec['repo']}")
            result = publish_site(
                root, spec, git_env, template_lock, node_modules
            )
            report.append(result)
            print(
                f"COMPLETE {result['repository']} "
                f"languages={','.join(result['languages'])}"
            )

        expected = {f"{spec['org']}/{spec['repo']}" for spec in SPECS}
        observed = {item["repository"] for item in report}
        if observed != expected:
            raise RuntimeError(
                f"publication report mismatch: missing={sorted(expected - observed)} "
                f"unexpected={sorted(observed - expected)}"
            )
        report_path = root / "marketing-sites-report.json"
        report_path.write_text(
            json.dumps(
                {
                    "schemaVersion": 1,
                    "publishedAt": "2026-08-05",
                    "siteCount": len(report),
                    "sites": report,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        print("MARKETING_SITES_REPORT_BEGIN")
        print(report_path.read_text(encoding="utf-8"))
        print("MARKETING_SITES_REPORT_END")
        print(f"PASS published and verified {len(report)}/{len(SPECS)} Astro sites")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
