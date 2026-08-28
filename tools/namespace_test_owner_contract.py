#!/usr/bin/env python3
"""Validate the boundary between production owners and their *-test organizations."""
from __future__ import annotations

import argparse
import json
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

DEFAULT_REGISTRY = "catalog/namespaces/owners.json"
DEFAULT_RULES = "catalog/namespaces/migration-rules.json"
TEST_SUFFIX = "-test"
CANONICAL_KINDS = {"product", "shared-service"}


@dataclass(frozen=True)
class Diagnostic:
    rule_id: str
    message: str
    path: str
    severity: str = "error"


@dataclass(frozen=True)
class TestOwnerBinding:
    test_owner: str
    canonical_owner: str
    github_owner: str


def mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, dict) else {}


def strings(value: Any) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        return ()
    return tuple(value)


def read_json(path: Path) -> tuple[Any, list[Diagnostic]]:
    try:
        return json.loads(path.read_text(encoding="utf-8")), []
    except (OSError, json.JSONDecodeError) as error:
        return None, [
            Diagnostic(
                "test-owner.read",
                f"cannot read valid JSON: {error}",
                path.as_posix(),
            )
        ]


def validate_test_owner_contract(
    registry_raw: Any,
    rules_raw: Any,
    *,
    registry_path: str = DEFAULT_REGISTRY,
    rules_path: str = DEFAULT_RULES,
) -> tuple[tuple[TestOwnerBinding, ...], tuple[Diagnostic, ...]]:
    diagnostics: list[Diagnostic] = []
    registry = mapping(registry_raw)
    owners_value = mapping(registry.get("spec")).get("owners")
    if not isinstance(owners_value, list):
        return (), (
            Diagnostic(
                "test-owner.registry",
                "registry spec.owners must be an array",
                registry_path,
            ),
        )

    owners: dict[str, Mapping[str, Any]] = {}
    for index, value in enumerate(owners_value):
        where = f"{registry_path}#spec.owners[{index}]"
        if not isinstance(value, dict):
            diagnostics.append(
                Diagnostic("test-owner.owner-object", "owner must be an object", where)
            )
            continue
        namespace_id = value.get("namespaceId")
        if not isinstance(namespace_id, str) or not namespace_id:
            diagnostics.append(
                Diagnostic(
                    "test-owner.namespace-id",
                    "owner requires a non-empty namespaceId",
                    where,
                )
            )
            continue
        if namespace_id in owners:
            diagnostics.append(
                Diagnostic(
                    "test-owner.duplicate-id",
                    f"namespaceId {namespace_id!r} is duplicated",
                    where,
                )
            )
            continue
        owners[namespace_id] = value

    bindings: list[TestOwnerBinding] = []
    test_ids = {
        namespace_id
        for namespace_id, owner in owners.items()
        if owner.get("kind") == "test"
    }

    for namespace_id, owner in sorted(owners.items()):
        where = f"{registry_path}#owner={namespace_id}"
        github_owner = owner.get("githubOwner")
        kind = owner.get("kind")
        aliases = strings(owner.get("aliases", []))

        if isinstance(github_owner, str) and github_owner.lower().endswith(TEST_SUFFIX):
            if kind != "test":
                diagnostics.append(
                    Diagnostic(
                        "test-owner.github-kind",
                        "a GitHub owner ending in '-test' must use kind 'test'",
                        where,
                    )
                )

        if kind != "test":
            continue

        if not namespace_id.endswith(TEST_SUFFIX):
            diagnostics.append(
                Diagnostic(
                    "test-owner.suffix",
                    "kind 'test' requires a namespaceId ending in '-test'",
                    where,
                )
            )
            continue

        if not isinstance(github_owner, str) or github_owner.lower() != namespace_id:
            diagnostics.append(
                Diagnostic(
                    "test-owner.github-owner",
                    "a test namespace must exactly match its lowercase GitHub owner",
                    where,
                )
            )

        if aliases:
            diagnostics.append(
                Diagnostic(
                    "test-owner.aliases",
                    "test owners may not declare aliases; aliases could blur production and test roots",
                    where,
                )
            )

        canonical_id = namespace_id[: -len(TEST_SUFFIX)]
        canonical = owners.get(canonical_id)
        if canonical is None:
            diagnostics.append(
                Diagnostic(
                    "test-owner.canonical-missing",
                    f"test owner {namespace_id!r} requires canonical owner {canonical_id!r}",
                    where,
                )
            )
            continue
        if canonical.get("kind") not in CANONICAL_KINDS:
            diagnostics.append(
                Diagnostic(
                    "test-owner.canonical-kind",
                    f"canonical owner {canonical_id!r} must be product or shared-service",
                    where,
                )
            )
        bindings.append(
            TestOwnerBinding(namespace_id, canonical_id, str(github_owner or ""))
        )

    rules = mapping(rules_raw)
    rules_value = mapping(rules.get("spec")).get("rules")
    if not isinstance(rules_value, list):
        diagnostics.append(
            Diagnostic(
                "test-owner.rules",
                "rule set spec.rules must be an array",
                rules_path,
            )
        )
        return tuple(bindings), tuple(diagnostics)

    for index, value in enumerate(rules_value):
        where = f"{rules_path}#spec.rules[{index}]"
        if not isinstance(value, dict):
            continue
        owner = value.get("owner")
        target = value.get("targetTemplate")
        environment = value.get("environment")
        consumers = strings(value.get("consumers", []))

        if isinstance(owner, str) and owner in test_ids:
            if isinstance(target, str) and target and not target.startswith(f"{owner}/"):
                diagnostics.append(
                    Diagnostic(
                        "test-owner.root",
                        f"test-owned target must stay under {owner + '/'!r}",
                        where,
                    )
                )
            if environment == "prod" or (
                isinstance(target, str) and target.startswith(f"{owner}/prod/")
            ):
                diagnostics.append(
                    Diagnostic(
                        "test-owner.prod-target",
                        "test owners may not own a production-environment target",
                        where,
                    )
                )

        if isinstance(target, str):
            for test_id in sorted(test_ids):
                if target.startswith(f"{test_id}/") and owner != test_id:
                    diagnostics.append(
                        Diagnostic(
                            "test-owner.foreign-write",
                            f"target under {test_id + '/'!r} must be owned by {test_id!r}",
                            where,
                        )
                    )

        for consumer in consumers:
            if consumer in test_ids and owner == consumer:
                diagnostics.append(
                    Diagnostic(
                        "test-owner.self-consumer",
                        "a test owner does not need a cross-owner consumer grant to itself",
                        where,
                    )
                )

    return tuple(bindings), tuple(diagnostics)


def build_report(
    root: Path,
    *,
    registry_path: str = DEFAULT_REGISTRY,
    rules_path: str = DEFAULT_RULES,
) -> tuple[dict[str, Any], int]:
    registry_raw, registry_errors = read_json(root / registry_path)
    rules_raw, rules_errors = read_json(root / rules_path)
    bindings: tuple[TestOwnerBinding, ...] = ()
    diagnostics = registry_errors + rules_errors
    if not diagnostics:
        bindings, found = validate_test_owner_contract(
            registry_raw,
            rules_raw,
            registry_path=registry_path,
            rules_path=rules_path,
        )
        diagnostics.extend(found)

    valid = not any(item.severity == "error" for item in diagnostics)
    report = {
        "valid": valid,
        "policy": {
            "testOwnedRoot": "<test-owner>/<non-prod-environment>/<workload>/<secret>",
            "canonicalOwner": "strip the '-test' suffix and require a registered product or shared-service",
            "productionWrites": "forbidden",
            "crossOwnerReads": "explicit consumer grant only",
            "credentials": "no PAT or account-wide provider credential in pull-request CI",
        },
        "bindings": [asdict(item) for item in bindings],
        "diagnostics": [asdict(item) for item in diagnostics],
    }
    return report, 0 if valid else 2


def render_text(report: Mapping[str, Any]) -> str:
    lines = [
        f"valid: {str(bool(report.get('valid'))).lower()}",
        f"test-owner bindings: {len(report.get('bindings', []))}",
    ]
    for binding in report.get("bindings", []):
        if isinstance(binding, dict):
            lines.append(
                f"- {binding.get('test_owner')} -> {binding.get('canonical_owner')}"
            )
    for diagnostic in report.get("diagnostics", []):
        if isinstance(diagnostic, dict):
            lines.append(
                f"{diagnostic.get('severity')}: {diagnostic.get('rule_id')}: "
                f"{diagnostic.get('message')} ({diagnostic.get('path')})"
            )
    return "\n".join(lines)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Validate independent *-test owner roots and canonical production bindings."
    )
    result.add_argument("--root", default=".")
    result.add_argument("--registry", default=DEFAULT_REGISTRY)
    result.add_argument("--rules", default=DEFAULT_RULES)
    result.add_argument("--format", choices=("json", "text"), default="text")
    return result


def main(argv: Sequence[str] | None = None) -> int:
    arguments = parser().parse_args(argv)
    report, status = build_report(
        Path(arguments.root).resolve(),
        registry_path=arguments.registry,
        rules_path=arguments.rules,
    )
    if arguments.format == "json":
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(render_text(report))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
