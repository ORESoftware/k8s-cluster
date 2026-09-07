"""Allowlisted co-living repository topology."""
from __future__ import annotations
from dataclasses import dataclass
from typing import Mapping

TRACKING = "DEN-1950"
BRANCH = "agent/den-1950-bootstrap-coliving-v1"
CHECKOUT_SHA = "3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_PYTHON_SHA = "ece7cb06caefa5fff74198d8649806c4678c61a1"
RUST_TOOLCHAIN_SHA = "4360b52568e2003a75bf9bc1d59f33a8e3fc893c"

@dataclass(frozen=True)
class RepoSpec:
    org: str
    name: str
    kind: str
    description: str

    @property
    def full_name(self) -> str:
        return f"{self.org}/{self.name}"


SPECS: tuple[RepoSpec, ...] = (
    # Global H/HAUS production topology.
    RepoSpec("hhaus-org", ".github", "org-meta", "Organization-wide H/HAUS community health, security, contribution, and issue templates"),
    RepoSpec("hhaus-org", "hhaus-cli", "cli", "Operator and resident H/HAUS CLI for guesting, reservations, polls, rent, refunds, and access audits"),
    RepoSpec("hhaus-org", "hhaus-docs", "docs", "H/HAUS product, operator, architecture, privacy, legal, and resident-policy documentation"),
    RepoSpec("hhaus-org", "hhaus-pub-lib-core", "public-core", "Public dependency-light H/HAUS domain primitives shared by external clients and server boundaries"),
    RepoSpec("hhaus-org", "hhaus-mcp-server.rs", "mcp", "Read-only Rust MCP server for H/HAUS inventory, contracts, health, and audit evidence"),
    RepoSpec("hhaus-org", "hhaus-sidecar.rs", "sidecar", "Rust sidecar for redacted H/HAUS telemetry, runtime configuration, and provider-neutral process integration"),
    RepoSpec("hhaus-org", "hhaus-assets", "assets", "Versioned H/HAUS icons, localization manifests, accessibility metadata, and WASM preload descriptors"),
    RepoSpec("hhaus-org", "hhaus-integrations.rs", "integrations", "Rust integration adapters for Stripe, Shared Auth, ores-chat, opto-sync, MIP solver, Supabase, and Neon"),
    RepoSpec("hhaus-org", "hhaus-operations-worker.rs", "worker", "Rust background worker for payment reconciliation, assignments, legal receipts, chat membership, and sync repair"),
    RepoSpec("hhaus-org", "hhaus-e2e", "test", "Cross-service H/HAUS acceptance tests for resident operations and administrative workflows"),
    RepoSpec("hhaus-org", "hhaus-mash-web", "web", "MASH-based H/HAUS web surface consuming the shared resident-operations contracts"),
    RepoSpec("hhaus-org", "hhaus-leptos-web", "web", "Leptos H/HAUS web surface with resumable onboarding, reservations, and safe loader prefetch"),
    RepoSpec("hhaus-org", "hhaus-dioxus-web", "web", "Dioxus H/HAUS web surface with resident, manager, and owner interaction contracts"),
    # Medellin production additions not already present.
    RepoSpec("hacker-house-medellin", "hhm-integrations.rs", "integrations", "Medellín-specific Rust adapters for shared H/HAUS services and local operational providers"),
    RepoSpec("hacker-house-medellin", "hhm-operations-worker.rs", "worker", "Medellín resident-operations worker for payments, assignments, agreements, chat, and offline reconciliation"),
    # Global H/HAUS shadow/test topology.
    RepoSpec("hhaus-org-test", ".github", "org-meta", "Organization-wide policies and templates for the H/HAUS shadow test fleet"),
    RepoSpec("hhaus-org-test", "contract-conformance-tests", "test", "Independent TypeSpec and JSON Schema peer-authority conformance tests for H/HAUS"),
    RepoSpec("hhaus-org-test", "hhaus-platform-e2e", "test", "Global H/HAUS resident, manager, owner, and administrator platform acceptance tests"),
    RepoSpec("hhaus-org-test", "payments-e2e", "test", "Stripe rent, refund, dispute, webhook replay, and idempotency acceptance tests"),
    RepoSpec("hhaus-org-test", "sync-offline-e2e", "test", "Offline localStorage, IndexedDB, Supabase, and Neon reconciliation acceptance tests"),
    RepoSpec("hhaus-org-test", "security-boundary-tests", "test", "Shared Auth, middleware ordering, rate limiting, tenant isolation, and developer-access boundary tests"),
    RepoSpec("hhaus-org-test", "solver-assignment-e2e", "test", "Pseudonymous room and shared-space assignment tests against the MIP solver adapter"),
    RepoSpec("hhaus-org-test", "legal-onboarding-e2e", "test", "Versioned agreement, document digest, resumable onboarding, and canonical receipt tests"),
    RepoSpec("hhaus-org-test", "wasm-loader-e2e", "test", "Safe ores-wasm-loaders prefetch, service-worker activation, and no-side-effect preload tests"),
    RepoSpec("hhaus-org-test", "clients-conformance", "test", "Cross-language H/HAUS client fixture, error, idempotency, and contract compatibility tests"),
    RepoSpec("hhaus-org-test", "worker-reconciliation-e2e", "test", "Background worker retry, deduplication, dead-letter, and exactly-once-effect tests"),
    # Medellin shadow/test additions.
    RepoSpec("hacker-house-medellin-test", "solver-assignment-e2e", "test", "Medellín room and shared-space solver assignment acceptance tests"),
    RepoSpec("hacker-house-medellin-test", "legal-onboarding-e2e", "test", "Medellín resident and guest agreement onboarding acceptance tests"),
    RepoSpec("hacker-house-medellin-test", "chat-groups-e2e", "test", "House, resident, project, breakout, and manager ores-chat membership acceptance tests"),
    RepoSpec("hacker-house-medellin-test", "guest-meal-governance-e2e", "test", "Guest lifecycle, kitchen reservations, meal planning, polls, quorum, and immutable-result tests"),
    RepoSpec("hacker-house-medellin-test", "wasm-loader-e2e", "test", "Medellín landing-page and application WASM loader preload acceptance tests"),
    RepoSpec("hacker-house-medellin-test", "clients-dart-consumer", "test", "Dart and Flutter consumer compatibility tests for HHM generated clients"),
    RepoSpec("hacker-house-medellin-test", "worker-reconciliation-e2e", "test", "Medellín payment, agreement, assignment, and sync worker reconciliation tests"),
)

EXPECTED_COUNTS: Mapping[str, int] = {
    "hhaus-org": 13,
    "hacker-house-medellin": 2,
    "hhaus-org-test": 11,
    "hacker-house-medellin-test": 7,
}
