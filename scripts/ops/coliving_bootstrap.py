"""Assemble per-repository co-living bootstrap files."""
from __future__ import annotations
import json
import shutil
import subprocess
import tempfile
import textwrap
from coliving_repository_specs import RepoSpec
from coliving_bootstrap_common import (
    agents_md, cargo_toml, ci_workflow, project_json, repository_readme, verifier_py, zpkg_toml,
)
from coliving_rust_templates import (
    rust_cli, rust_integrations, rust_mcp, rust_public_core, rust_sidecar, rust_web, rust_worker,
)
from coliving_test_templates import python_reference_model, python_tests



def format_rust_sources(files: dict[str, str]) -> None:
    """Format generated Rust with the installed, workflow-pinned rustfmt."""
    rustfmt = shutil.which("rustfmt")
    if rustfmt is None:
        return
    for relative in sorted(path for path in files if path.endswith(".rs")):
        with tempfile.TemporaryDirectory(prefix="coliving-rustfmt-") as temporary:
            path = Path(temporary) / Path(relative).name
            path.write_text(files[relative], encoding="utf-8")
            completed = subprocess.run(
                [rustfmt, "--edition", "2021", str(path)],
                text=True,
                capture_output=True,
            )
            if completed.returncode != 0:
                raise RuntimeError(
                    f"rustfmt failed for {relative}: {completed.stderr or completed.stdout}"
                )
            files[relative] = path.read_text(encoding="utf-8")


def docs_files() -> dict[str, str]:
    return {
        "docs/architecture/resident-operations.md": textwrap.dedent(
            """\
            # Resident operations architecture

            The canonical H/HAUS lifecycle includes guest check-in/out, room and shared-space
            reservations, kitchen and meal planning, polls and immutable result snapshots, rent and
            refunds, versioned legal acceptance, ores-chat groups, MIP-backed assignments, and
            time-bounded developer access.

            Browser checkpoints are mirrored through localStorage and IndexedDB for navigation and
            offline continuity, then through Supabase and the canonical Neon/PostgreSQL API via
            opto-sync. Only a server-issued canonical receipt is legal or financial evidence.
            """
        ),
        "docs/legal/external/README.md": textwrap.dedent(
            """\
            # External legal document set

            Version and localize resident/member agreements, guest rules, house rules, room and
            shared-space policies, payment/refund terms, privacy notices, e-sign consent, acceptable
            use, and community-governance terms. Published documents require immutable content
            hashes and must be coordinated with `github.com/ores-legal`.
            """
        ),
        "docs/legal/internal/README.md": textwrap.dedent(
            """\
            # Internal legal and governance document set

            Maintain owner/manager delegations, incident handling, developer-access policy,
            vendor/data-processing terms, records retention, payment operations, refund authority,
            and privileged-access review procedures. Internal documents are access controlled but
            still versioned and auditable.
            """
        ),
    }


def asset_files() -> dict[str, str]:
    locales = ["en", "es", "pt", "fr", "de", "it", "nl", "sv", "pl", "tr", "ar", "he", "hi", "ja", "ko", "zh-Hans", "zh-Hant"]
    return {
        "manifests/locales.json": json.dumps({"schema": "hhaus.locales/v1", "locales": locales}, indent=2) + "\n",
        "manifests/wasm-preload.json": json.dumps(
            {
                "schema": "hhaus.wasm-preload/v1",
                "entries": [
                    {"id": "ores-wasm-loaders-core", "mode": "modulepreload", "sideEffectFree": True},
                    {"id": "resident-shell", "mode": "prefetch", "sideEffectFree": True},
                ],
                "forbiddenDuringPrefetch": ["authenticate", "acceptAgreement", "submitVote", "startPayment", "checkInGuest"],
            },
            indent=2,
        ) + "\n",
        "docs/accessibility.md": "# Accessibility metadata\n\nEvery asset needs text alternatives, contrast review, locale direction, reduced-motion behavior, and semantic ownership.\n",
    }


def org_meta_files(spec: RepoSpec) -> dict[str, str]:
    return {
        "profile/README.md": f"# {spec.org}\n\nShared organization policy for the H/HAUS co-living platform fleet.\n",
        "CONTRIBUTING.md": "# Contributing\n\nUse focused branches, deterministic tests, semantic conflict resolution, and reviewed pull requests.\n",
        "SECURITY.md": "# Security\n\nReport vulnerabilities privately. Never place credentials, resident data, payment payloads, ballots, or legal documents in public issues.\n",
        ".github/ISSUE_TEMPLATE/config.yml": "blank_issues_enabled: true\ncontact_links: []\n",
    }


def bootstrap_files(spec: RepoSpec) -> dict[str, str]:
    files: dict[str, str] = {
        "README.md": repository_readme(spec),
        "AGENTS.md": agents_md(spec),
        "project.json": project_json(spec),
        ".gitignore": ".env\n.env.*\n!.env.example\n__pycache__/\n*.py[cod]\ntarget/\nnode_modules/\n.vendor/.zed/\ntmp/\n*.log\n",
    }

    importable = spec.kind in {"cli", "public-core", "mcp", "sidecar", "integrations", "worker", "web"}
    rust_binary = spec.kind == "cli"
    rust_kind = spec.kind in {"cli", "public-core", "mcp", "sidecar", "integrations", "worker", "web"}

    if importable:
        files[".zpkg.toml"] = zpkg_toml(spec)
        files["env/enc/README.md"] = "# Encrypted runtime configuration\n\nOnly ores-sops encrypted material belongs here. Plaintext decoded configuration is forbidden.\n"
    if rust_kind:
        files["Cargo.toml"] = cargo_toml(spec, rust_binary)
        if spec.kind == "cli":
            files["src/main.rs"] = rust_cli()
        elif spec.kind == "public-core":
            files["src/lib.rs"] = rust_public_core()
        elif spec.kind == "mcp":
            files["src/lib.rs"] = rust_mcp()
        elif spec.kind == "sidecar":
            files["src/lib.rs"] = rust_sidecar()
        elif spec.kind == "integrations":
            files["src/lib.rs"] = rust_integrations()
        elif spec.kind == "worker":
            files["src/lib.rs"] = rust_worker()
        elif spec.kind == "web":
            files["src/lib.rs"] = rust_web(spec)
    elif spec.kind == "test":
        files["src/reference_model.py"] = python_reference_model()
        files["tests/test_resident_operations.py"] = python_tests(spec)
    elif spec.kind == "docs":
        files.update(docs_files())
    elif spec.kind == "assets":
        files.update(asset_files())
    elif spec.kind == "org-meta":
        files.update(org_meta_files(spec))
    else:
        raise AssertionError(f"unknown repository kind: {spec.kind}")

    format_rust_sources(files)

    required = list(files)
    required.extend(["scripts/verify_repository.py", ".github/workflows/ci.yml"])
    files["scripts/verify_repository.py"] = verifier_py(spec, required)
    files[".github/workflows/ci.yml"] = ci_workflow(spec)
    return dict(sorted(files.items()))

