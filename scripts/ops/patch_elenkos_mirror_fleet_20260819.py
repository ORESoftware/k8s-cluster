#!/usr/bin/env python3
"""Fail-closed overlay that converts the audited Elenkos paired-test fleet into an exact mirror fleet.

The base payload remains the reviewed DEN-3786 artifact. This overlay changes only the
`elenkos-systems-test` repository identities and their consumer scenarios, then updates
count/name assertions in the protected publication wrappers.
"""
from __future__ import annotations

import hashlib
import sys
from pathlib import Path

BASE_HASHES = {
    "scripts/ops/elenkos_fleet_spec_20260819.py": "20bc13462545892f45f7f0a410385c04fed846afe8bc6aac351edaa7ff4f40d8",
    "scripts/ops/test_elenkos_fleet_20260819.py": "fc3d9b3f6e789675bb336050c806ed0ff1258f5a58bc5b82cae4b2e12e4eb2c7",
    "scripts/ops/validate_elenkos_fleet_payload_20260819.sh": "fe0e76a3b68ae54f39f3b352eb1fcfdc3c62415291a15740b43676a38442b7e0",
    "scripts/ops/run_protected_elenkos_fleet_20260819.sh": "c19a5eb34012bff9f03b798c101443131669523fbe5cd7f8f5ee1a1b60f7f376",
    "scripts/ops/dispatch_elenkos_fleet_via_ssm_20260819.sh": "3ae551460c13663547f83c1b33eb10c032fbc3265e10dd3c9a30a6a04cf3d987",
}
MARKER = "# ELENKOS_EXACT_TEST_MIRROR_OVERLAY_V1\n"

TEST_SPECS_BLOCK = '''TEST_SPECS = (
    RepositorySpec(
        TEST_ORG,
        "elenkos-interfaces",
        "Consumer contract, redaction, schema-drift, OpenAPI, and generated-type tests for Elenkos interfaces",
        "test",
        dependencies=((f"{PRODUCTION_ORG}/elenkos-interfaces", "=0.1.0"),),
        topics=("qa", "contracts", "json-schema", "openapi", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-lib-core",
        "Adversarial dual-database, blind-review, consensus, assignment, adjudication, and credit-ledger tests",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-interfaces", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-lib-core", "=0.1.0"),
        ),
        topics=("qa", "seaorm", "postgresql", "cockroachdb", "blind-review", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-sync",
        "Offline convergence, actor scoping, redaction, replay, and reconnect tests for the opto-sync wrapper",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-lib-core", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-sync", "=0.1.0"),
        ),
        topics=("qa", "opto-sync", "offline-first", "redaction", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-api-server.rs",
        "Black-box API, blind reviewer projection, consensus, adjudication, and credit command tests",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-interfaces", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-lib-core", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-api-server.rs", "=0.1.0"),
        ),
        topics=("qa", "rust", "axum", "seaorm", "e2e", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-web-server.rs",
        "Web reviewer-isolation, accessibility, report-to-consensus, and discrepancy escalation tests",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-api-server.rs", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-web-server.rs", "=0.1.0"),
        ),
        topics=("qa", "rust", "leptos", "fullstack", "e2e", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-cli",
        "End-to-end report, review, consensus, adjudication, credits, and synchronization CLI tests",
        "test",
        dependencies=((f"{PRODUCTION_ORG}/elenkos-cli", "=0.1.0"),),
        topics=("qa", "cli", "e2e", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-clients",
        "Cross-language SDK wire, error, pagination, model-version, and redaction conformance tests",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-interfaces", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-clients", "=0.1.0"),
        ),
        topics=("qa", "sdk", "polyglot", "conformance", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-flutter",
        "Flutter mobile, desktop, offline queue, reviewer isolation, and accessibility tests",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-sync", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-flutter", "=0.1.0"),
        ),
        topics=("qa", "flutter", "mobile", "desktop", "e2e", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-desktop-app.rs",
        "Native Rust desktop reviewer-isolation and Flutter parity tests",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-desktop-app.rs", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-flutter", "=0.1.0"),
        ),
        topics=("qa", "rust", "desktop", "parity", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-infra",
        "PostgreSQL and CockroachDB migration, vector-index, network-policy, rollback, and observability canaries",
        "test",
        dependencies=(
            (f"{PRODUCTION_ORG}/elenkos-api-server.rs", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-web-server.rs", "=0.1.0"),
            (f"{PRODUCTION_ORG}/elenkos-infra", "=0.1.0"),
        ),
        topics=("qa", "kubernetes", "postgresql", "cockroachdb", "canary", "zed-pkg"),
    ),
    RepositorySpec(
        TEST_ORG,
        "elenkos-monorepo",
        "Cross-repository dependency graph, immutable pin, package installation, and release orchestration tests",
        "test",
        dependencies=tuple(
            (f"{PRODUCTION_ORG}/{name}", "=0.1.0")
            for name in (
                "elenkos-interfaces",
                "elenkos-lib-core",
                "elenkos-sync",
                "elenkos-api-server.rs",
                "elenkos-web-server.rs",
                "elenkos-cli",
                "elenkos-clients",
                "elenkos-flutter",
                "elenkos-desktop-app.rs",
                "elenkos-infra",
                "elenkos-monorepo",
            )
        ),
        topics=("qa", "monorepo", "dependency-graph", "release", "zed-pkg"),
    ),
)
'''

SCENARIO_HELPER = '''\n\ndef mirrored_test_scenarios(name: str) -> list[dict[str, object]]:
    scenarios: dict[str, list[dict[str, object]]] = {
        "elenkos-interfaces": [
            {"case": "human-review-allowlist", "forbidden": ["aiScore", "confidenceBps", "modelVersion"]},
            {"case": "openapi-json-schema-parity", "required": True},
            {"case": "generated-type-drift", "languages": list(CLIENT_LANGUAGES)},
        ],
        "elenkos-lib-core": [
            {"case": "consensus-boundary", "ai": 82, "human": 78, "expected": "consensus"},
            {"case": "adjudication-boundary", "ai": 92, "human": 38, "expected": "adjudication"},
            {"case": "credit-replay", "expectedUniqueEntries": 2},
            {"case": "exclude-reporter-and-conflicts", "required": True},
            {"case": "postgres-cockroach-schema-parity", "vectorDimensions": 1536},
        ],
        "elenkos-sync": [
            {"case": "offline-reviewer-redaction", "aiMaterial": False},
            {"case": "reconnect-convergence", "replicas": 3},
            {"case": "sealed-field-replay-rejection", "required": True},
        ],
        "elenkos-api-server.rs": [
            {"case": "reviewer-before-submit", "forbidden": ["aiScore", "aiBand", "confidenceBps", "modelVersion"]},
            {"case": "report-to-consensus", "services": ["api", "postgres"]},
            {"case": "report-to-adjudication", "services": ["api", "cockroachdb"]},
        ],
        "elenkos-web-server.rs": [
            {"case": "blind-review-page", "aiMaterial": False},
            {"case": "report-to-consensus", "services": ["web", "api", "postgres"]},
            {"case": "keyboard-and-screen-reader", "required": True},
        ],
        "elenkos-cli": [
            {"case": "report-review-consensus", "commands": ["report", "review", "credits"]},
            {"case": "discrepancy", "commands": ["review", "adjudication"]},
        ],
        "elenkos-clients": [
            {"case": "wire-casing", "languages": list(CLIENT_LANGUAGES)},
            {"case": "reviewer-redaction", "languages": list(CLIENT_LANGUAGES)},
            {"case": "model-version-provenance", "languages": list(CLIENT_LANGUAGES)},
        ],
        "elenkos-flutter": [
            {"case": "android-offline-review", "aiMaterial": False},
            {"case": "desktop-accessibility", "minimumSemantics": 1},
            {"case": "mobile-desktop-convergence", "required": True},
        ],
        "elenkos-desktop-app.rs": [
            {"case": "same-reviewer-fields", "surfaces": ["flutter", "rust"]},
            {"case": "same-discrepancy-state", "surfaces": ["flutter", "rust"]},
            {"case": "native-offline-redaction", "aiMaterial": False},
        ],
        "elenkos-infra": [
            {"case": "postgres-vector-index", "required": True},
            {"case": "cockroach-vector-index", "required": True},
            {"case": "network-default-deny", "required": True},
            {"case": "migration-rollback", "required": True},
        ],
        "elenkos-monorepo": [
            {"case": "exact-production-graph", "repositories": 11},
            {"case": "exact-test-mirror", "repositories": 11},
            {"case": "immutable-predecessor-pins", "required": True},
        ],
    }
    return scenarios[name]
'''

EXPECTED_TEST_NAMES = [
    "elenkos-api-server.rs",
    "elenkos-cli",
    "elenkos-clients",
    "elenkos-desktop-app.rs",
    "elenkos-flutter",
    "elenkos-infra",
    "elenkos-interfaces",
    "elenkos-lib-core",
    "elenkos-monorepo",
    "elenkos-sync",
    "elenkos-web-server.rs",
]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one source match, found {count}")
    return text.replace(old, new, 1)


def patch_spec(text: str) -> str:
    start = text.index("TEST_SPECS = (\n")
    end = text.index("\nALL_SPECS = PRODUCTION_SPECS + TEST_SPECS", start)
    text = text[:start] + TEST_SPECS_BLOCK + text[end:]
    helper_anchor = "\ndef test_repo_files(spec: RepositorySpec, pins: Mapping[str, str], mode: str) -> dict[str, str]:\n"
    text = replace_once(text, helper_anchor, SCENARIO_HELPER + helper_anchor, "scenario helper insertion")
    text = replace_once(
        text,
        '            "scenarios": scenario_templates[spec.name],\n',
        '            "scenarios": mirrored_test_scenarios(spec.name),\n',
        "scenario selection",
    )
    text = replace_once(
        text,
        '    if EXPECTED_TOTAL_REPOSITORIES != 23:\n        raise ValueError(f"expected 23 repositories, observed {EXPECTED_TOTAL_REPOSITORIES}")\n',
        '    if EXPECTED_TOTAL_REPOSITORIES != 22:\n        raise ValueError(f"expected 22 repositories, observed {EXPECTED_TOTAL_REPOSITORIES}")\n',
        "fleet count invariant",
    )
    mirror_anchor = '    if production_names != required_names:\n        raise ValueError(\n            f"production repository set drift: expected {sorted(required_names)}, observed {sorted(production_names)}"\n        )\n'
    mirror_check = mirror_anchor + '    test_names = set(EXPECTED_TEST_REPOSITORIES)\n    if test_names != required_names:\n        raise ValueError(\n            f"test mirror repository set drift: expected {sorted(required_names)}, observed {sorted(test_names)}"\n        )\n'
    text = replace_once(text, mirror_anchor, mirror_check, "test mirror invariant")
    return MARKER + text


def patch_tests(text: str) -> str:
    text = replace_once(
        text,
        "        self.assertEqual(len(spec_module.EXPECTED_TEST_REPOSITORIES), 12)\n        self.assertEqual(spec_module.EXPECTED_TOTAL_REPOSITORIES, 23)\n",
        "        self.assertEqual(\n            set(spec_module.EXPECTED_TEST_REPOSITORIES),\n            set(spec_module.EXPECTED_PRODUCTION_REPOSITORIES),\n        )\n        self.assertEqual(len(spec_module.EXPECTED_TEST_REPOSITORIES), 11)\n        self.assertEqual(spec_module.EXPECTED_TOTAL_REPOSITORIES, 22)\n",
        "test inventory assertion",
    )
    text = text.replace('manifest["repository_count"], 23', 'manifest["repository_count"], 22')
    text = text.replace('manifest["test_repository_count"], 12', 'manifest["test_repository_count"], 11')
    text = text.replace('len(repository_roots), 23', 'len(repository_roots), 22')
    return MARKER + text


def patch_validation(text: str) -> str:
    replacements = {
        ".validation_manifest.repository_count == 23": ".validation_manifest.repository_count == 22",
        ".validation_manifest.test_repository_count == 12": ".validation_manifest.test_repository_count == 11",
        ".verifiers.repository_verifiers == 23": ".verifiers.repository_verifiers == 22",
        ".verifiers.test_repository_suites == 12": ".verifiers.test_repository_suites == 11",
        'abort("expected 23 generated workflows, got #{paths.length}") unless paths.length == 23': 'abort("expected 22 generated workflows, got #{paths.length}") unless paths.length == 22',
        "VERIFIED_ELENKOS_REPOSITORIES 23/23": "VERIFIED_ELENKOS_REPOSITORIES 22/22",
        'test "$count" -eq 23': 'test "$count" -eq 22',
        "ELENKOS_PAYLOAD_VALIDATED repositories=23 production=11 test=12 zed=23": "ELENKOS_PAYLOAD_VALIDATED repositories=22 production=11 test=11 zed=22",
    }
    for old, new in replacements.items():
        if old not in text:
            raise RuntimeError(f"validation source missing: {old}")
        text = text.replace(old, new)
    return MARKER + text


def exact_full_name_block(indent: str = "    ") -> str:
    prod = [f'"elenkos-systems/{name}"' for name in EXPECTED_TEST_NAMES]
    test = [f'"elenkos-systems-test/{name}"' for name in EXPECTED_TEST_NAMES]
    return "\n".join(f"{indent}{item}," for item in prod + test)


def patch_protected(text: str) -> str:
    text = text.replace(".publication.test_repository_count == 12", ".publication.test_repository_count == 11")
    text = text.replace(".publication.repository_count == 23", ".publication.repository_count == 22")
    block_start = text.index('    "elenkos-systems/elenkos-api-server.rs",')
    block_end_token = '    "elenkos-systems-test/severity-consensus"\n'
    block_end = text.index(block_end_token, block_start) + len(block_end_token)
    text = text[:block_start] + exact_full_name_block("    ")[:-1] + "\n" + text[block_end:]
    text = text.replace("VERIFIED_ELENKOS_REPOSITORIES 23/23", "VERIFIED_ELENKOS_REPOSITORIES 22/22")
    return MARKER + text


def patch_dispatch(text: str) -> str:
    replacements = {
        "-eq 23": "-eq 22",
        "repository=elenkos-systems-test/blind-review-isolation ": "repository=elenkos-systems-test/elenkos-lib-core ",
        "repositories=23 production=11 test=12 ": "repositories=22 production=11 test=11 ",
        "VERIFIED_ELENKOS_REPOSITORIES 23/23": "VERIFIED_ELENKOS_REPOSITORIES 22/22",
    }
    for old, new in replacements.items():
        if old not in text:
            raise RuntimeError(f"dispatcher source missing: {old}")
        text = text.replace(old, new)
    return MARKER + text


PATCHERS = {
    "scripts/ops/elenkos_fleet_spec_20260819.py": patch_spec,
    "scripts/ops/test_elenkos_fleet_20260819.py": patch_tests,
    "scripts/ops/validate_elenkos_fleet_payload_20260819.sh": patch_validation,
    "scripts/ops/run_protected_elenkos_fleet_20260819.sh": patch_protected,
    "scripts/ops/dispatch_elenkos_fleet_via_ssm_20260819.sh": patch_dispatch,
}


def main(argv: list[str]) -> int:
    root = Path(argv[1] if len(argv) > 1 else ".").resolve()
    if len(argv) > 2:
        raise SystemExit("usage: patch_elenkos_mirror_fleet_20260819.py [root]")
    results: dict[str, str] = {}
    for relative, patcher in PATCHERS.items():
        path = root / relative
        if not path.is_file():
            raise RuntimeError(f"missing materialized file: {relative}")
        original = path.read_text(encoding="utf-8")
        if original.startswith(MARKER):
            results[relative] = "already-applied"
            continue
        actual = sha256(path)
        expected = BASE_HASHES[relative]
        if actual != expected:
            raise RuntimeError(f"refusing unexpected {relative}: {actual} != {expected}")
        patched = patcher(original)
        path.write_text(patched, encoding="utf-8")
        results[relative] = sha256(path)
    print("ELENKOS_EXACT_TEST_MIRROR_PATCHED files=5 repositories=22 production=11 test=11")
    for relative in sorted(results):
        print(f"ELENKOS_MIRROR_PATCH_FILE path={relative} result={results[relative]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
