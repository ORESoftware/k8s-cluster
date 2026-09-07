"""Common co-living repository metadata, policy, and CI templates."""
from __future__ import annotations
import json
import textwrap
from coliving_repository_specs import (
    BRANCH, CHECKOUT_SHA, RUST_TOOLCHAIN_SHA, SETUP_PYTHON_SHA, TRACKING, RepoSpec,
)

def repository_readme(spec: RepoSpec) -> str:
    responsibilities = {
        "org-meta": "organization-wide policy and contribution metadata",
        "cli": "operator-safe command parsing and scoped resident operations",
        "docs": "versioned product, architecture, operational, privacy, and legal documentation",
        "public-core": "dependency-light public primitives for external and isomorphic consumers",
        "mcp": "read-only inspection methods with an explicit mutation denylist",
        "sidecar": "redacted telemetry and bounded runtime-configuration integration",
        "assets": "versioned localization, accessibility, and WASM preload manifests",
        "integrations": "provider adapters with tenant, idempotency, and data-minimization boundaries",
        "worker": "retryable, deduplicated background reconciliation and orchestration",
        "test": "independent acceptance and adversarial contract evidence",
        "web": "framework-specific UI integration over the shared H/HAUS contracts",
    }[spec.kind]
    return textwrap.dedent(
        f"""\
        # {spec.name}

        {spec.description}.

        This repository owns **{responsibilities}**. It is initialized through `{TRACKING}` and
        deliberately starts with an executable policy/conformance slice rather than a placeholder.

        ## Fleet contracts

        - TypeSpec and JSON Schema Draft 2020-12 remain independently authored peer authorities.
        - Reusable packages publish `.zpkg.toml`; dependency locks are created only by a working
          zed-pkg resolver and pin immutable revisions.
        - Rust service work integrates `shared-auth`, ordered `ores-middleware`, route-specific
          `ores-rate-limit`, encrypted `ores-sops` configuration, `ores-redis-lru-cache`, and
          `ores-otel` before production admission.
        - Offline state flows through `opto-sync`; browser storage is resumable cache state and is
          never the rent ledger or legal proof.
        - `ores-chat` owns message content. Resident-operation records retain only authorized space
          references and membership receipts.
        - Solver payloads sent to `ORESoftware/mip-solver-node.rs` contain opaque candidate and
          resource identifiers, never payment, legal, chat, contact, or resident-level health data.
        - Marketing and application shells may prefetch via `ores-wasm-loaders`, but prefetching
          never authenticates a user, accepts an agreement, submits a vote, or starts a payment.

        ## Local verification

        ```sh
        python3 scripts/verify_repository.py
        ```
        """
    )


def agents_md(spec: RepoSpec) -> str:
    return textwrap.dedent(
        f"""\
        # Agent rules — {spec.name}

        - Read and follow `ORESoftware/my-ai/AGENTS.md` before changing this repository.
        - Tracking identifier: `{TRACKING}`.
        - Preserve TypeSpec and JSON Schema as independent top-level authorities; generated output
          is comparison evidence only and is never edited to manufacture parity.
        - Use focused branches and reviewed pull requests. Resolve conflicts semantically by
          preserving compatible intent from both sides; never select `ours` or `theirs` wholesale.
        - Add deterministic tests with behavior changes. Never commit credentials, customer data,
          agreement bodies, ballot identity, payment payloads, or raw provider webhooks.
        - Reusable code is consumed through zed-pkg. Do not synthesize `.zpkg.lock` while the
          resolver/registry is unavailable.
        - Rust first for systems work. Native desktop code must not use React or a webview.
        - Never apply database migrations automatically or at process startup.
        """
    )


def project_json(spec: RepoSpec) -> str:
    value = {
        "schema": "ores.coliving-repository/v1",
        "organization": spec.org,
        "repository": spec.name,
        "kind": spec.kind,
        "tracking": TRACKING,
        "status": "bootstrap",
        "contractAuthorities": ["typespec", "json-schema-draft-2020-12"],
        "requiredIntegrations": [
            "shared-auth",
            "ores-middleware",
            "ores-rate-limit",
            "ores-sops",
            "opto-sync",
            "zed-pkg",
            "ores-redis-lru-cache",
            "ores-otel",
            "ores-chat",
            "ores-wasm-loaders",
        ],
    }
    return json.dumps(value, indent=2, sort_keys=True) + "\n"


def zpkg_toml(spec: RepoSpec) -> str:
    canonical = spec.name.removesuffix(".rs")
    return textwrap.dedent(
        f"""\
        [package]
        org = "{spec.org}"
        name = "{canonical}"
        version = "0.1.0"
        description = "{spec.description}"
        keywords = ["coliving", "hhaus", "resident-operations", "zed-pkg"]

        [package.repository]
        vcs = "git"
        url = "https://github.com/{spec.full_name}"

        [dependencies]
        # Add only immutable resolver-produced dependencies.

        [publish]
        include_readme = true
        tag_format = "v{{version}}"
        exclude = [".env", ".env.*", "env/**", "target/**", "node_modules/**", "tmp/**", "*.log"]

        [install]
        dir = ".vendor/.zed"

        [targets.repository]
        dir = "."
        adapter = "none"
        """
    )


def cargo_toml(spec: RepoSpec, binary: bool) -> str:
    package_name = spec.name.replace(".rs", "").replace(".", "-")
    return textwrap.dedent(
        f"""\
        [package]
        name = "{package_name}"
        version = "0.1.0"
        edition = "2021"
        license = "MIT"
        publish = false

        {'' if binary else '[lib]\npath = "src/lib.rs"'}
        """
    ).strip() + "\n"


def verifier_py(spec: RepoSpec, required: list[str]) -> str:
    required_literal = json.dumps(sorted(required), separators=(",", ":"))
    rust_kind = spec.kind in {"cli", "public-core", "mcp", "sidecar", "integrations", "worker", "web"}
    test_kind = spec.kind == "test"
    return textwrap.dedent(
        f"""\
        #!/usr/bin/env python3
        from __future__ import annotations

        import json
        import re
        from pathlib import Path

        ROOT = Path(__file__).resolve().parents[1]
        REQUIRED = set({required_literal})
        missing = sorted(path for path in REQUIRED if not (ROOT / path).exists())
        if missing:
            raise SystemExit(f"missing required paths: {{missing}}")

        metadata = json.loads((ROOT / "project.json").read_text(encoding="utf-8"))
        expected = {{"organization": {spec.org!r}, "repository": {spec.name!r}, "kind": {spec.kind!r}, "tracking": {TRACKING!r}}}
        for key, value in expected.items():
            if metadata.get(key) != value:
                raise SystemExit(f"project metadata mismatch: {{key}}={{metadata.get(key)!r}} expected {{value!r}}")

        marker = re.compile(r"^(<{{7}}|={{7}}|>{{7}})", re.MULTILINE)
        credential = re.compile(r"gh[pousr]_[A-Za-z0-9]{{20,}}|github_pat_[A-Za-z0-9_]{{20,}}|lin_api_[A-Za-z0-9]{{20,}}|BEGIN [A-Z ]*PRIVATE KEY")
        for path in ROOT.rglob("*"):
            if not path.is_file() or ".git" in path.parts or path.stat().st_size > 1_000_000:
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                continue
            if marker.search(text):
                raise SystemExit(f"unresolved conflict marker: {{path.relative_to(ROOT)}}")
            if credential.search(text):
                raise SystemExit(f"credential-shaped source: {{path.relative_to(ROOT)}}")

        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        if "permissions:\\n  contents: read" not in workflow or "pull_request_target" in workflow:
            raise SystemExit("unsafe workflow permission boundary")
        action_pattern = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[0-9a-f]{{40}}$")
        actions = [line.split("uses:", 1)[1].strip() for line in workflow.splitlines() if "uses:" in line]
        if not actions or any(not action_pattern.fullmatch(action) for action in actions):
            raise SystemExit(f"workflow actions are not immutably pinned: {{actions}}")

        if {rust_kind!r}:
            cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
            if 'edition = "2021"' not in cargo:
                raise SystemExit("Rust edition policy missing")
        if {test_kind!r} and not list((ROOT / "tests").glob("test_*.py")):
            raise SystemExit("test repository has no executable tests")

        agents = (ROOT / "AGENTS.md").read_text(encoding="utf-8")
        for phrase in ("TypeSpec", "JSON Schema", "semantically", "ours", "theirs", "Never commit credentials"):
            if phrase not in agents:
                raise SystemExit(f"agent policy missing phrase: {{phrase}}")

        print("validated {spec.full_name} kind={spec.kind}")
        """
    )


def ci_workflow(spec: RepoSpec) -> str:
    rust_kind = spec.kind in {"cli", "public-core", "mcp", "sidecar", "integrations", "worker", "web"}
    test_kind = spec.kind == "test"
    lines = [
        "name: CI",
        "on:",
        "  pull_request:",
        "  push:",
        "    branches: [main]",
        "permissions:",
        "  contents: read",
        "concurrency:",
        "  group: ${{ github.workflow }}-${{ github.ref }}",
        "  cancel-in-progress: true",
        "jobs:",
        "  verify:",
        "    runs-on: ubuntu-24.04",
        "    timeout-minutes: 15",
        "    steps:",
        f"      - uses: actions/checkout@{CHECKOUT_SHA}",
        "        with:",
        "          persist-credentials: false",
        f"      - uses: actions/setup-python@{SETUP_PYTHON_SHA}",
        "        with:",
        '          python-version: "3.12"',
        '          cache: ""',
        "      - name: Verify repository policy",
        "        run: python3 scripts/verify_repository.py",
    ]
    if rust_kind:
        lines.extend(
            [
                f"      - uses: dtolnay/rust-toolchain@{RUST_TOOLCHAIN_SHA}",
                "        with:",
                "          toolchain: stable",
                "          components: rustfmt",
                "      - name: Test Rust bootstrap",
                "        run: |",
                "          cargo fmt --all -- --check",
                "          cargo test --all-targets",
            ]
        )
    if test_kind:
        lines.extend(
            [
                "      - name: Run deterministic resident-operation tests",
                "        env:",
                "          PYTHONPATH: src",
                "        run: python -m unittest discover -s tests -v",
            ]
        )
    return "\n".join(lines) + "\n"


