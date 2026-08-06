#!/usr/bin/env python3
"""Publish the hardened `.github` baseline to every approved organization.

This extension keeps the reviewed 36-organization publisher intact and expands
its allowlist to the complete connected organization fleet. The paired `*-test`
organizations deliberately share the corresponding production Linear project,
so planning remains canonical instead of creating duplicate project records.

Mutation remains dry-run by default and still requires the protected runner to
supply `GH_TOKEN` plus the explicit `--execute` flag.
"""

from __future__ import annotations

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sys

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import bootstrap_org_dotgithub_repositories as base  # noqa: E402
import bootstrap_org_dotgithub_repositories_hardened as hard  # noqa: E402


ADDITIONAL_ORGANIZATIONS: tuple[str, ...] = (
    "fanwaave",
    "agent-pontifex",
    "fifa-math",
    "gha-indie-worker",
    "apostille-me",
    "embedded-alerts",
    "evento-globolo",
    "hacker-house-medellin",
    "3fa-app-test",
    "claritas-viz-test",
    "cliptown-test",
    "declarative-migrations-test",
    "embedded-alerts-test",
    "evento-globolo-test",
    "fiducia-cloud-test",
    "memebank-test",
    "opto-sync-test",
    "quaestor-ledger-test",
    "sonus-auris-test",
    "messaging-intel-test",
    "scintilla-run-test",
    "file-tunnel-test",
    "shared-auth-test",
    "hypesiege-test",
    "streempilot-test",
)

TARGET_ORGANIZATIONS: tuple[str, ...] = base.ORGANIZATIONS + ADDITIONAL_ORGANIZATIONS
EXCLUDED_ORGANIZATIONS = frozenset({"r2g", "r2g-test"})

ADDITIONAL_LINEAR_PROJECTS: dict[str, str] = {
    "fanwaave": "https://linear.app/denman/project/githubcomfanwaave-6ba038b59dbc",
    "agent-pontifex": "https://linear.app/denman/project/githubcomagent-pontifex-1d2deb2be3c7",
    "fifa-math": "https://linear.app/denman/project/githubcomfifa-math-06015d19054a",
    "gha-indie-worker": "https://linear.app/denman/project/githubcomgha-indie-worker-941d4102f7dc",
    "apostille-me": "https://linear.app/denman/project/githubcomapostille-me-c884fbdbd637",
    "embedded-alerts": "https://linear.app/denman/project/githubcomembedded-alerts-2fb7392497ab",
    "evento-globolo": "https://linear.app/denman/project/githubcomevento-globolo-4daaf1952e29",
    "hacker-house-medellin": "https://linear.app/denman/project/githubcomhacker-house-medellin-d4043553c2b4",
}

PAIRED_TEST_PROJECTS: dict[str, str] = {
    "3fa-app-test": "3FA-app",
    "claritas-viz-test": "claritas-viz",
    "cliptown-test": "cliptown",
    "declarative-migrations-test": "declarative-migrations",
    "embedded-alerts-test": "embedded-alerts",
    "evento-globolo-test": "evento-globolo",
    "fiducia-cloud-test": "fiducia-cloud",
    "memebank-test": "memebank",
    "opto-sync-test": "opto-sync",
    "quaestor-ledger-test": "quaestor-ledger",
    "sonus-auris-test": "sonus-auris",
    "messaging-intel-test": "messaging-intel",
    "scintilla-run-test": "scintilla-run",
    "file-tunnel-test": "file-tunnel",
    "shared-auth-test": "shared-auth",
    "hypesiege-test": "hypesiege",
    "streempilot-test": "StreemPilot",
}

LINEAR_PROJECTS: dict[str, str] = dict(hard.LINEAR_PROJECTS)
LINEAR_PROJECTS.update(ADDITIONAL_LINEAR_PROJECTS)
for test_organization, production_organization in PAIRED_TEST_PROJECTS.items():
    LINEAR_PROJECTS[test_organization] = LINEAR_PROJECTS[production_organization]


def validate_registry() -> None:
    target_lower = {name.lower() for name in TARGET_ORGANIZATIONS}
    mapped_lower = {name.lower() for name in LINEAR_PROJECTS}

    if len(TARGET_ORGANIZATIONS) != 61 or len(target_lower) != 61:
        raise RuntimeError("all-organization allowlist must contain exactly 61 unique logins")
    if target_lower & EXCLUDED_ORGANIZATIONS:
        raise RuntimeError("r2g and r2g-test must remain excluded from publication")
    if target_lower != mapped_lower:
        missing = sorted(target_lower - mapped_lower)
        extra = sorted(mapped_lower - target_lower)
        raise RuntimeError(f"Linear registry mismatch: missing={missing}, extra={extra}")

    for organization, url in LINEAR_PROJECTS.items():
        if not url.startswith("https://linear.app/denman/project/"):
            raise RuntimeError(f"invalid Linear project URL for {organization}: {url}")

    for test_organization, production_organization in PAIRED_TEST_PROJECTS.items():
        if LINEAR_PROJECTS[test_organization] != LINEAR_PROJECTS[production_organization]:
            raise RuntimeError(
                f"paired test organization {test_organization} must use "
                f"the {production_organization} Linear project"
            )


def install_all_organization_hardening() -> None:
    """Patch the reviewed publisher only after the expanded registry validates."""
    base.ORGANIZATIONS = TARGET_ORGANIZATIONS
    hard.LINEAR_PROJECTS = dict(LINEAR_PROJECTS)
    hard.install_hardening()


def main() -> int:
    validate_registry()
    install_all_organization_hardening()

    args = base.parse_args()
    token = os.environ.get("GH_TOKEN", "")
    if not token:
        raise SystemExit("GH_TOKEN is required for both preflight and execution")

    api = base.GitHubApi(token)
    existing = base.preflight(api)
    results = [
        base.reconcile_organization(
            api,
            organization,
            existing[organization],
            execute=args.execute,
        )
        for organization in base.ORGANIZATIONS
    ]

    report = {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "mode": "execute" if args.execute else "dry-run",
        "expected_actor": base.EXPECTED_ACTOR,
        "repository_name": base.REPOSITORY_NAME,
        "managed_paths": list(base.MANAGED_PATHS),
        "organizations": [item.as_dict() for item in results],
    }
    if args.json_report:
        args.json_report.parent.mkdir(parents=True, exist_ok=True)
        args.json_report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    rendered = base.markdown_report(results, execute=args.execute)
    if args.markdown_report:
        args.markdown_report.parent.mkdir(parents=True, exist_ok=True)
        args.markdown_report.write_text(rendered, encoding="utf-8")
    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
