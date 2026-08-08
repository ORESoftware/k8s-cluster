#!/usr/bin/env python3
"""Generate and validate the read-only DEN-2786 namespace migration manifest."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections import Counter
from dataclasses import asdict, dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Mapping, Sequence

API_VERSION = "oresoftware.dev/v1alpha1"
KIND = "NamespaceMigrationManifest"
SCHEMA_REF = "./migration-manifest.schema.json"
DEFAULT_INVENTORY = "artifacts/namespace-inventory.json"
DEFAULT_REGISTRY = "catalog/namespaces/owners.json"
DEFAULT_RULES = "catalog/namespaces/migration-rules.json"
DEFAULT_MANIFEST = "catalog/namespaces/migration-manifest.json"
DEFAULT_SCHEMA = "catalog/namespaces/migration-manifest.schema.json"
GENERATED_BY = "tools/namespace_manifest.py"
IDENTITY_FIELDS = ("path", "line", "column", "system", "current")
SYSTEMS = {
    "slash-namespace",
    "metadata-key",
    "host-path",
    "source-package",
    "generated-package",
}
SCOPES = {"active", "documentation", "test"}
CLASSIFICATION_STATUSES = {"classified", "review-required", "unclassified"}
REVIEW_STATES = {"classified", "review-required", "blocked"}
MIGRATION_MODES = {
    "copy-verify-cutover",
    "dual-write-cutover",
    "service-specific-move",
    "dependent-update",
    "regenerate",
    "manual-review",
}
MODE_BY_SYSTEM = {
    "slash-namespace": "copy-verify-cutover",
    "metadata-key": "dual-write-cutover",
    "host-path": "service-specific-move",
    "source-package": "dependent-update",
    "generated-package": "regenerate",
}
OWNER_KINDS = {"platform", "product", "shared-service", "test"}
NON_PLATFORM_KINDS = {"product", "shared-service", "test"}
ALLOWED_ENVIRONMENTS = {"dev", "staging", "prod", "shared"}
DNS_LABEL = re.compile(r"^[a-z0-9](?:[-a-z0-9]*[a-z0-9])?$")
UNRESOLVED = re.compile(r"\{[A-Za-z][A-Za-z0-9_-]*\}")
ENTRY_FIELDS = {
    "id",
    "path",
    "line",
    "column",
    "scope",
    "system",
    "current",
    "ruleId",
    "classificationStatus",
    "owner",
    "target",
    "targetTemplate",
    "environment",
    "workload",
    "consumers",
    "consumerGrants",
    "migrationMode",
    "reviewState",
    "verification",
    "rollback",
    "platformTargetException",
    "destructiveCleanupAllowed",
    "notes",
}


@dataclass(frozen=True)
class Diagnostic:
    rule_id: str
    message: str
    path: str = ""
    severity: str = "error"


@dataclass(frozen=True)
class LoadedInputs:
    inventory: Mapping[str, Any]
    registry: Mapping[str, Any]
    rules: Mapping[str, Any]
    diagnostics: tuple[Diagnostic, ...]


@dataclass(frozen=True)
class BuildResult:
    manifest: Mapping[str, Any] | None
    diagnostics: tuple[Diagnostic, ...]

    @property
    def valid(self) -> bool:
        return self.manifest is not None and not any(
            item.severity == "error" for item in self.diagnostics
        )


def mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, dict) else {}


def strings(value: Any) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        return ()
    return tuple(value)


def safe_relative(value: str) -> bool:
    path = PurePosixPath(value)
    return (
        bool(value)
        and "\\" not in value
        and not path.is_absolute()
        and all(part not in {"", ".", ".."} for part in path.parts)
    )


def canonical_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_path(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def read_json(path: Path, rule_id: str) -> tuple[Mapping[str, Any], list[Diagnostic]]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return {}, [
            Diagnostic(rule_id, f"cannot read valid JSON: {error}", path.as_posix())
        ]
    if not isinstance(value, dict):
        return {}, [Diagnostic(rule_id, "root must be a JSON object", path.as_posix())]
    return value, []


def load_inputs(
    root: Path,
    *,
    inventory_path: str = DEFAULT_INVENTORY,
    registry_path: str = DEFAULT_REGISTRY,
    rules_path: str = DEFAULT_RULES,
) -> LoadedInputs:
    diagnostics: list[Diagnostic] = []
    for relative, rule_id in (
        (inventory_path, "manifest.inventory-path"),
        (registry_path, "manifest.registry-path"),
        (rules_path, "manifest.rules-path"),
    ):
        if not safe_relative(relative):
            diagnostics.append(
                Diagnostic(rule_id, "path must be repository-relative", relative)
            )
    inventory, found = read_json(root / inventory_path, "manifest.inventory-read")
    diagnostics.extend(found)
    registry, found = read_json(root / registry_path, "manifest.registry-read")
    diagnostics.extend(found)
    rules, found = read_json(root / rules_path, "manifest.rules-read")
    diagnostics.extend(found)
    return LoadedInputs(
        inventory,
        registry,
        rules,
        tuple(diagnostics),
    )


def owner_index(registry: Mapping[str, Any]) -> tuple[dict[str, Mapping[str, Any]], list[Diagnostic]]:
    diagnostics: list[Diagnostic] = []
    spec = mapping(registry.get("spec"))
    raw_owners = spec.get("owners")
    if not isinstance(raw_owners, list):
        return {}, [
            Diagnostic(
                "manifest.registry-owners",
                "registry spec.owners must be an array",
                DEFAULT_REGISTRY,
            )
        ]
    result: dict[str, Mapping[str, Any]] = {}
    for index, value in enumerate(raw_owners):
        where = f"{DEFAULT_REGISTRY}#spec.owners[{index}]"
        if not isinstance(value, dict):
            diagnostics.append(
                Diagnostic("manifest.registry-owner", "owner must be an object", where)
            )
            continue
        namespace_id = value.get("namespaceId")
        kind = value.get("kind")
        if not isinstance(namespace_id, str) or not DNS_LABEL.fullmatch(namespace_id):
            diagnostics.append(
                Diagnostic(
                    "manifest.registry-owner-id",
                    "namespaceId must be a lowercase DNS label",
                    where,
                )
            )
            continue
        if namespace_id in result:
            diagnostics.append(
                Diagnostic(
                    "manifest.registry-owner-duplicate",
                    f"owner {namespace_id!r} is duplicated",
                    where,
                )
            )
            continue
        if kind not in OWNER_KINDS:
            diagnostics.append(
                Diagnostic(
                    "manifest.registry-owner-kind",
                    f"kind must be one of {sorted(OWNER_KINDS)}",
                    where,
                )
            )
        result[namespace_id] = value
    allowed = spec.get("allowedEnvironments")
    if isinstance(allowed, list) and all(isinstance(item, str) for item in allowed):
        global ALLOWED_ENVIRONMENTS
        ALLOWED_ENVIRONMENTS = set(allowed)
    return result, diagnostics


def rule_index(rules: Mapping[str, Any]) -> tuple[dict[str, Mapping[str, Any]], list[Diagnostic]]:
    diagnostics: list[Diagnostic] = []
    raw_rules = mapping(rules.get("spec")).get("rules")
    if not isinstance(raw_rules, list):
        return {}, [
            Diagnostic(
                "manifest.rules-array",
                "rule set spec.rules must be an array",
                DEFAULT_RULES,
            )
        ]
    result: dict[str, Mapping[str, Any]] = {}
    for index, value in enumerate(raw_rules):
        where = f"{DEFAULT_RULES}#spec.rules[{index}]"
        if not isinstance(value, dict):
            diagnostics.append(
                Diagnostic("manifest.rule-object", "rule must be an object", where)
            )
            continue
        rule_id = value.get("id")
        if not isinstance(rule_id, str) or not rule_id:
            diagnostics.append(
                Diagnostic("manifest.rule-id", "rule requires a non-empty id", where)
            )
            continue
        if rule_id in result:
            diagnostics.append(
                Diagnostic(
                    "manifest.rule-duplicate",
                    f"rule {rule_id!r} is duplicated",
                    where,
                )
            )
            continue
        result[rule_id] = value
    return result, diagnostics


def occurrence_identity(value: Mapping[str, Any]) -> tuple[str, int, int, str, str] | None:
    path = value.get("path")
    line = value.get("line")
    column = value.get("column")
    system = value.get("system")
    current = value.get("current", value.get("reference"))
    if not (
        isinstance(path, str)
        and safe_relative(path)
        and isinstance(line, int)
        and not isinstance(line, bool)
        and line > 0
        and isinstance(column, int)
        and not isinstance(column, bool)
        and column > 0
        and isinstance(system, str)
        and system in SYSTEMS
        and isinstance(current, str)
        and current
    ):
        return None
    return path, line, column, system, current


def identity_id(identity: tuple[str, int, int, str, str]) -> str:
    payload = dict(zip(IDENTITY_FIELDS, identity, strict=True))
    return "nsocc-" + sha256_bytes(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    )


def matches_rule(rule: Mapping[str, Any], system: str, current: str) -> bool:
    if rule.get("system") != system:
        return False
    match = mapping(rule.get("match"))
    match_type = match.get("type")
    match_value = match.get("value")
    if not isinstance(match_value, str):
        return False
    if match_type == "exact":
        return current == match_value
    if match_type == "prefix":
        return current.startswith(match_value)
    if match_type == "regex":
        try:
            return re.search(match_value, current) is not None
        except re.error:
            return False
    return False


def expected_target(rule: Mapping[str, Any], current: str) -> str | None:
    template = rule.get("targetTemplate")
    if template is None:
        return None
    if not isinstance(template, str) or not template:
        return None
    match = mapping(rule.get("match"))
    match_type = match.get("type")
    match_value = match.get("value")
    suffix = current
    if match_type == "prefix" and isinstance(match_value, str):
        suffix = current[len(match_value) :]
    owner = rule.get("owner") if isinstance(rule.get("owner"), str) else ""
    result = template.replace("{suffix}", suffix.lstrip("/_.-"))
    result = result.replace("{reference}", current).replace("{owner}", owner)
    environment = rule.get("environment")
    workload = rule.get("workload")
    if isinstance(environment, str) and environment:
        result = result.replace("{environment}", environment)
    if isinstance(workload, str) and workload:
        result = result.replace("{workload}", workload)
    return result


def verification_procedure(system: str, review_state: str) -> str:
    if review_state == "blocked":
        return (
            "Keep the current reference unchanged. Resolve its owner, environment, workload, "
            "target, consumers, and non-secret success criteria before any migration attempt."
        )
    return {
        "slash-namespace": (
            "Create or copy the target without logging values; compare non-secret version or "
            "checksum metadata, refresh the consumer, and prove readiness plus old-path rollback."
        ),
        "metadata-key": (
            "Dual-emit old and new keys, roll or recreate affected workloads, verify all readers, "
            "selectors, and metrics, then prove no old-only objects remain."
        ),
        "host-path": (
            "Back up state, stop the owning service when required, migrate or install a compatibility "
            "link, restart, and verify integrity and readiness before cleanup."
        ),
        "source-package": (
            "Update the canonical import or package authority, run dependent builds and tests, and "
            "verify every known consumer plus lockfile or submodule pin."
        ),
        "generated-package": (
            "Change generator configuration, regenerate deterministically, and compile and test all "
            "known consumers against the regenerated output."
        ),
    }[system]


def rollback_procedure(system: str, review_state: str) -> str:
    if review_state == "blocked":
        return (
            "No migration is authorized. Preserve the current reference and restore any accidental "
            "change immediately while classification remains blocked."
        )
    return {
        "slash-namespace": (
            "Point consumers back to the current path, refresh them, and verify readiness; retain the "
            "current object through the documented grace period."
        ),
        "metadata-key": (
            "Restore legacy key emission and readers or selectors, roll affected workloads, and verify "
            "the fleet returns to the pre-migration key set."
        ),
        "host-path": (
            "Stop the service, restore the backup or compatibility path, restart, and verify state "
            "integrity before resuming traffic."
        ),
        "source-package": (
            "Revert import and package changes together with dependent pins, then rebuild and retest "
            "the affected dependency graph."
        ),
        "generated-package": (
            "Restore the previous generator configuration and generated artifacts, then rebuild and "
            "retest all known consumers."
        ),
    }[system]


def review_state_for(
    status: str, owner: str, target_template: str | None
) -> str:
    if owner == "unclassified" or status == "unclassified":
        return "blocked"
    if status == "review-required" or target_template is None:
        return "review-required"
    if UNRESOLVED.search(target_template):
        return "review-required"
    return "classified"


def build_entry(
    occurrence: Mapping[str, Any],
    rule: Mapping[str, Any],
) -> tuple[Mapping[str, Any] | None, list[Diagnostic]]:
    diagnostics: list[Diagnostic] = []
    identity = occurrence_identity(occurrence)
    if identity is None:
        return None, [
            Diagnostic(
                "manifest.inventory-identity",
                "inventory occurrence has an invalid identity",
                DEFAULT_INVENTORY,
            )
        ]
    path, line, column, system, current = identity
    where = f"{path}:{line}:{column}"
    rule_id = occurrence.get("rule_id")
    if not isinstance(rule_id, str) or not rule_id:
        diagnostics.append(
            Diagnostic("manifest.inventory-rule", "occurrence requires rule_id", where)
        )
    if not matches_rule(rule, system, current):
        diagnostics.append(
            Diagnostic(
                "manifest.inventory-rule-match",
                f"rule {rule_id!r} does not match this occurrence",
                where,
            )
        )
    owner = rule.get("owner")
    status = rule.get("status")
    if occurrence.get("owner") != owner:
        diagnostics.append(
            Diagnostic(
                "manifest.inventory-owner-drift",
                f"inventory owner {occurrence.get('owner')!r} differs from rule owner {owner!r}",
                where,
            )
        )
    if occurrence.get("status") != status:
        diagnostics.append(
            Diagnostic(
                "manifest.inventory-status-drift",
                f"inventory status {occurrence.get('status')!r} differs from rule status {status!r}",
                where,
            )
        )
    target_template = expected_target(rule, current)
    if occurrence.get("target_preview") != target_template:
        diagnostics.append(
            Diagnostic(
                "manifest.inventory-target-drift",
                "inventory target preview differs from the current rule set",
                where,
            )
        )
    if not isinstance(owner, str) or not isinstance(status, str):
        return None, diagnostics + [
            Diagnostic(
                "manifest.rule-classification",
                "matched rule requires string owner and status",
                where,
            )
        ]
    review_state = review_state_for(status, owner, target_template)
    target = (
        target_template
        if target_template is not None and UNRESOLVED.search(target_template) is None
        else None
    )
    consumers = tuple(sorted(strings(rule.get("consumers", []))))
    grants = [
        {"access": "read", "consumer": consumer, "state": "required"}
        for consumer in consumers
        if consumer != owner
    ]
    migration_mode = (
        "manual-review" if review_state == "blocked" else MODE_BY_SYSTEM[system]
    )
    environment = rule.get("environment")
    workload = rule.get("workload")
    scope = occurrence.get("scope")
    entry = {
        "id": identity_id(identity),
        "path": path,
        "line": line,
        "column": column,
        "scope": scope,
        "system": system,
        "current": current,
        "ruleId": rule_id,
        "classificationStatus": status,
        "owner": owner,
        "target": target,
        "targetTemplate": target_template,
        "environment": environment if isinstance(environment, str) else None,
        "workload": workload if isinstance(workload, str) else None,
        "consumers": list(consumers),
        "consumerGrants": grants,
        "migrationMode": migration_mode,
        "reviewState": review_state,
        "verification": {
            "procedure": verification_procedure(system, review_state),
            "state": "required",
        },
        "rollback": {
            "procedure": rollback_procedure(system, review_state),
            "state": "required",
        },
        "platformTargetException": None,
        "destructiveCleanupAllowed": False,
        "notes": rule.get("notes") if isinstance(rule.get("notes"), str) else "",
    }
    return entry, diagnostics


def manifest_summary(entries: Sequence[Mapping[str, Any]]) -> Mapping[str, Any]:
    return {
        "total": len(entries),
        "byScope": dict(sorted(Counter(item["scope"] for item in entries).items())),
        "bySystem": dict(sorted(Counter(item["system"] for item in entries).items())),
        "byClassificationStatus": dict(
            sorted(Counter(item["classificationStatus"] for item in entries).items())
        ),
        "byReviewState": dict(
            sorted(Counter(item["reviewState"] for item in entries).items())
        ),
        "byOwner": dict(sorted(Counter(item["owner"] for item in entries).items())),
        "concreteTargets": sum(item["target"] is not None for item in entries),
        "unresolvedTargetTemplates": sum(
            isinstance(item["targetTemplate"], str)
            and UNRESOLVED.search(item["targetTemplate"]) is not None
            for item in entries
        ),
        "crossOwnerGrants": sum(len(item["consumerGrants"]) for item in entries),
        "destructiveCleanupAllowed": sum(
            bool(item["destructiveCleanupAllowed"]) for item in entries
        ),
    }


def build_manifest(
    root: Path,
    *,
    inventory_path: str = DEFAULT_INVENTORY,
    registry_path: str = DEFAULT_REGISTRY,
    rules_path: str = DEFAULT_RULES,
) -> BuildResult:
    root = root.resolve()
    loaded = load_inputs(
        root,
        inventory_path=inventory_path,
        registry_path=registry_path,
        rules_path=rules_path,
    )
    diagnostics = list(loaded.diagnostics)
    owners, found = owner_index(loaded.registry)
    diagnostics.extend(found)
    rules, found = rule_index(loaded.rules)
    diagnostics.extend(found)
    raw_occurrences = loaded.inventory.get("occurrences")
    if not isinstance(raw_occurrences, list):
        diagnostics.append(
            Diagnostic(
                "manifest.inventory-occurrences",
                "inventory occurrences must be an array",
                inventory_path,
            )
        )
        return BuildResult(None, tuple(diagnostics))
    if loaded.inventory.get("valid") is not True:
        diagnostics.append(
            Diagnostic(
                "manifest.inventory-valid",
                "inventory must declare valid=true",
                inventory_path,
            )
        )
    entries: list[Mapping[str, Any]] = []
    seen: set[tuple[str, int, int, str, str]] = set()
    sortable: list[tuple[tuple[str, int, int, str, str], Mapping[str, Any]]] = []
    for index, occurrence in enumerate(raw_occurrences):
        where = f"{inventory_path}#occurrences[{index}]"
        if not isinstance(occurrence, dict):
            diagnostics.append(
                Diagnostic("manifest.inventory-object", "occurrence must be an object", where)
            )
            continue
        identity = occurrence_identity(occurrence)
        if identity is None:
            diagnostics.append(
                Diagnostic(
                    "manifest.inventory-identity",
                    "occurrence identity is invalid",
                    where,
                )
            )
            continue
        if identity in seen:
            diagnostics.append(
                Diagnostic(
                    "manifest.inventory-duplicate",
                    f"inventory identity is duplicated: {identity!r}",
                    where,
                )
            )
            continue
        seen.add(identity)
        sortable.append((identity, occurrence))
    for identity, occurrence in sorted(sortable, key=lambda item: item[0]):
        rule_id = occurrence.get("rule_id")
        rule = rules.get(rule_id) if isinstance(rule_id, str) else None
        where = f"{identity[0]}:{identity[1]}:{identity[2]}"
        if rule is None:
            diagnostics.append(
                Diagnostic(
                    "manifest.unknown-rule",
                    f"inventory rule {rule_id!r} is not registered",
                    where,
                )
            )
            continue
        entry, found = build_entry(occurrence, rule)
        diagnostics.extend(found)
        if entry is not None:
            entries.append(entry)
    if any(item.severity == "error" for item in diagnostics):
        return BuildResult(None, tuple(diagnostics))
    inventory_file = root / inventory_path
    registry_file = root / registry_path
    rules_file = root / rules_path
    manifest = {
        "$schema": SCHEMA_REF,
        "apiVersion": API_VERSION,
        "kind": KIND,
        "metadata": {
            "name": "den-2786-phase-0",
            "phase": "phase-0",
            "generatedBy": GENERATED_BY,
            "inventory": inventory_path,
            "inventorySha256": sha256_path(inventory_file),
            "registry": registry_path,
            "registrySha256": sha256_path(registry_file),
            "rules": rules_path,
            "rulesSha256": sha256_path(rules_file),
            "entryCount": len(entries),
        },
        "spec": {
            "executionAuthorized": False,
            "identityFields": list(IDENTITY_FIELDS),
            "summary": manifest_summary(entries),
            "entries": entries,
        },
    }
    semantic = validate_manifest_semantics(
        manifest,
        loaded.inventory,
        owners,
        inventory_path=inventory_path,
    )
    diagnostics.extend(semantic)
    return BuildResult(manifest, tuple(diagnostics))


def fields(
    value: Mapping[str, Any], allowed: set[str], where: str
) -> list[Diagnostic]:
    return [
        Diagnostic(
            "manifest.unknown-field",
            f"unsupported field {name!r}",
            where,
        )
        for name in sorted(set(value) - allowed)
    ]


def validate_manifest_semantics(
    manifest: Mapping[str, Any],
    inventory: Mapping[str, Any],
    owners: Mapping[str, Mapping[str, Any]],
    *,
    inventory_path: str = DEFAULT_INVENTORY,
) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    if manifest.get("$schema") != SCHEMA_REF:
        diagnostics.append(
            Diagnostic("manifest.schema-reference", f"$schema must equal {SCHEMA_REF!r}")
        )
    if manifest.get("apiVersion") != API_VERSION:
        diagnostics.append(
            Diagnostic("manifest.api-version", f"apiVersion must equal {API_VERSION!r}")
        )
    if manifest.get("kind") != KIND:
        diagnostics.append(Diagnostic("manifest.kind", f"kind must equal {KIND!r}"))
    metadata = mapping(manifest.get("metadata"))
    spec = mapping(manifest.get("spec"))
    entries = spec.get("entries")
    if not isinstance(entries, list):
        return diagnostics + [
            Diagnostic("manifest.entries", "spec.entries must be an array", DEFAULT_MANIFEST)
        ]
    if spec.get("executionAuthorized") is not False:
        diagnostics.append(
            Diagnostic(
                "manifest.execution-authorized",
                "Phase 0 manifest must keep executionAuthorized=false",
                DEFAULT_MANIFEST,
            )
        )
    if spec.get("identityFields") != list(IDENTITY_FIELDS):
        diagnostics.append(
            Diagnostic(
                "manifest.identity-fields",
                f"identityFields must equal {list(IDENTITY_FIELDS)!r}",
                DEFAULT_MANIFEST,
            )
        )
    if metadata.get("entryCount") != len(entries):
        diagnostics.append(
            Diagnostic(
                "manifest.entry-count",
                "metadata.entryCount must equal the number of entries",
                DEFAULT_MANIFEST,
            )
        )
    raw_inventory = inventory.get("occurrences")
    inventory_identities: list[tuple[str, int, int, str, str]] = []
    if isinstance(raw_inventory, list):
        for value in raw_inventory:
            if isinstance(value, dict):
                identity = occurrence_identity(value)
                if identity is not None:
                    inventory_identities.append(identity)
    entry_identities: list[tuple[str, int, int, str, str]] = []
    seen_ids: set[str] = set()
    target_index: dict[str, tuple[str, str, str]] = {}
    for index, value in enumerate(entries):
        where = f"{DEFAULT_MANIFEST}#spec.entries[{index}]"
        if not isinstance(value, dict):
            diagnostics.append(
                Diagnostic("manifest.entry-object", "entry must be an object", where)
            )
            continue
        diagnostics.extend(fields(value, ENTRY_FIELDS, where))
        missing = sorted(ENTRY_FIELDS - set(value))
        if missing:
            diagnostics.append(
                Diagnostic(
                    "manifest.required-field",
                    f"entry is missing required fields: {missing}",
                    where,
                )
            )
        identity = occurrence_identity(value)
        if identity is None:
            diagnostics.append(
                Diagnostic("manifest.entry-identity", "entry identity is invalid", where)
            )
            continue
        entry_identities.append(identity)
        expected_id = identity_id(identity)
        entry_id = value.get("id")
        if entry_id != expected_id:
            diagnostics.append(
                Diagnostic(
                    "manifest.entry-id",
                    "entry id does not match its canonical identity hash",
                    where,
                )
            )
        if isinstance(entry_id, str):
            if entry_id in seen_ids:
                diagnostics.append(
                    Diagnostic("manifest.duplicate-id", f"id {entry_id!r} is duplicated", where)
                )
            seen_ids.add(entry_id)
        owner = value.get("owner")
        owner_record = owners.get(owner) if isinstance(owner, str) else None
        if owner != "unclassified" and owner_record is None:
            diagnostics.append(
                Diagnostic(
                    "manifest.unknown-owner",
                    f"owner {owner!r} is not registered",
                    where,
                )
            )
        system = value.get("system")
        scope = value.get("scope")
        status = value.get("classificationStatus")
        review_state = value.get("reviewState")
        mode = value.get("migrationMode")
        target = value.get("target")
        target_template = value.get("targetTemplate")
        environment = value.get("environment")
        workload = value.get("workload")
        consumers = value.get("consumers")
        grants = value.get("consumerGrants")
        exception = value.get("platformTargetException")
        if system not in SYSTEMS:
            diagnostics.append(
                Diagnostic("manifest.system", f"unsupported system {system!r}", where)
            )
        if scope not in SCOPES:
            diagnostics.append(
                Diagnostic("manifest.scope", f"unsupported scope {scope!r}", where)
            )
        if status not in CLASSIFICATION_STATUSES:
            diagnostics.append(
                Diagnostic(
                    "manifest.classification-status",
                    f"unsupported classification status {status!r}",
                    where,
                )
            )
        if review_state not in REVIEW_STATES:
            diagnostics.append(
                Diagnostic(
                    "manifest.review-state",
                    f"unsupported review state {review_state!r}",
                    where,
                )
            )
        if mode not in MIGRATION_MODES:
            diagnostics.append(
                Diagnostic(
                    "manifest.migration-mode",
                    f"unsupported migration mode {mode!r}",
                    where,
                )
            )
        if target is not None and (not isinstance(target, str) or not target):
            diagnostics.append(
                Diagnostic("manifest.target", "target must be null or non-empty string", where)
            )
        if isinstance(target, str):
            if UNRESOLVED.search(target):
                diagnostics.append(
                    Diagnostic(
                        "manifest.unresolved-target",
                        "concrete target may not contain unresolved placeholders",
                        where,
                    )
                )
            if "dd/" in target or "dd.dev/" in target:
                diagnostics.append(
                    Diagnostic(
                        "manifest.legacy-target",
                        "target may not retain a legacy namespace prefix",
                        where,
                    )
                )
            collision_key = (
                str(system),
                str(value.get("current")),
                str(owner),
            )
            previous = target_index.get(target)
            if previous is not None and previous != collision_key:
                diagnostics.append(
                    Diagnostic(
                        "manifest.target-collision",
                        f"target {target!r} is shared by distinct migrations {previous!r} and {collision_key!r}",
                        where,
                    )
                )
            else:
                target_index[target] = collision_key
        if target_template is not None and (
            not isinstance(target_template, str) or not target_template
        ):
            diagnostics.append(
                Diagnostic(
                    "manifest.target-template",
                    "targetTemplate must be null or non-empty string",
                    where,
                )
            )
        if isinstance(target_template, str) and (
            "dd/" in target_template or "dd.dev/" in target_template
        ):
            diagnostics.append(
                Diagnostic(
                    "manifest.legacy-target-template",
                    "targetTemplate may not retain a legacy namespace prefix",
                    where,
                )
            )
        if environment is not None and (
            not isinstance(environment, str) or environment not in ALLOWED_ENVIRONMENTS
        ):
            diagnostics.append(
                Diagnostic(
                    "manifest.environment",
                    f"environment must be null or one of {sorted(ALLOWED_ENVIRONMENTS)}",
                    where,
                )
            )
        if workload is not None and (
            not isinstance(workload, str) or not DNS_LABEL.fullmatch(workload)
        ):
            diagnostics.append(
                Diagnostic(
                    "manifest.workload",
                    "workload must be null or a lowercase DNS label",
                    where,
                )
            )
        if not isinstance(consumers, list) or not all(
            isinstance(item, str) for item in consumers
        ):
            diagnostics.append(
                Diagnostic("manifest.consumers", "consumers must be a string array", where)
            )
            consumers = []
        if len(set(consumers)) != len(consumers):
            diagnostics.append(
                Diagnostic("manifest.duplicate-consumer", "consumers must be unique", where)
            )
        for consumer in consumers:
            if consumer not in owners:
                diagnostics.append(
                    Diagnostic(
                        "manifest.unknown-consumer",
                        f"consumer {consumer!r} is not registered",
                        where,
                    )
                )
        if not isinstance(grants, list):
            diagnostics.append(
                Diagnostic(
                    "manifest.consumer-grants",
                    "consumerGrants must be an array",
                    where,
                )
            )
            grants = []
        grant_consumers: list[str] = []
        for grant_index, grant in enumerate(grants):
            grant_where = f"{where}.consumerGrants[{grant_index}]"
            if not isinstance(grant, dict):
                diagnostics.append(
                    Diagnostic(
                        "manifest.consumer-grant-object",
                        "grant must be an object",
                        grant_where,
                    )
                )
                continue
            consumer = grant.get("consumer")
            grant_consumers.append(consumer if isinstance(consumer, str) else "")
            if consumer not in owners:
                diagnostics.append(
                    Diagnostic(
                        "manifest.unknown-grant-consumer",
                        f"grant consumer {consumer!r} is not registered",
                        grant_where,
                    )
                )
            if grant.get("access") != "read":
                diagnostics.append(
                    Diagnostic(
                        "manifest.grant-access",
                        "Phase 0 cross-owner grants must be read-only",
                        grant_where,
                    )
                )
            if grant.get("state") not in {"required", "approved", "revoked"}:
                diagnostics.append(
                    Diagnostic(
                        "manifest.grant-state",
                        "grant state must be required, approved, or revoked",
                        grant_where,
                    )
                )
        expected_grants = sorted(
            consumer for consumer in consumers if consumer != owner
        )
        if sorted(grant_consumers) != expected_grants:
            diagnostics.append(
                Diagnostic(
                    "manifest.cross-owner-grant",
                    "every cross-owner consumer must have exactly one explicit grant",
                    where,
                )
            )
        if owner == "unclassified":
            if target is not None:
                diagnostics.append(
                    Diagnostic(
                        "manifest.unclassified-target",
                        "unclassified entries must keep target=null",
                        where,
                    )
                )
            if review_state != "blocked":
                diagnostics.append(
                    Diagnostic(
                        "manifest.unclassified-review",
                        "unclassified entries must be blocked",
                        where,
                    )
                )
            if mode != "manual-review":
                diagnostics.append(
                    Diagnostic(
                        "manifest.unclassified-mode",
                        "unclassified entries require migrationMode=manual-review",
                        where,
                    )
                )
            if consumers or grants:
                diagnostics.append(
                    Diagnostic(
                        "manifest.unclassified-consumers",
                        "unclassified entries may not invent consumers or grants",
                        where,
                    )
                )
        owner_kind = owner_record.get("kind") if owner_record is not None else None
        needs_exception = (
            isinstance(target, str)
            and target.startswith("ores/")
            and owner_kind in NON_PLATFORM_KINDS
        )
        if needs_exception:
            if not (
                isinstance(exception, dict)
                and exception.get("approved") is True
                and isinstance(exception.get("reason"), str)
                and bool(exception.get("reason"))
                and isinstance(exception.get("approvedBy"), str)
                and bool(exception.get("approvedBy"))
                and isinstance(exception.get("issue"), str)
                and bool(exception.get("issue"))
            ):
                diagnostics.append(
                    Diagnostic(
                        "manifest.product-to-platform",
                        "non-platform owners may target ores/ only with an explicit approved exception",
                        where,
                    )
                )
        elif exception is not None:
            diagnostics.append(
                Diagnostic(
                    "manifest.unneeded-platform-exception",
                    "platformTargetException must be null when no ores/ exception is needed",
                    where,
                )
            )
        if (
            system == "slash-namespace"
            and isinstance(target, str)
            and owner_record is not None
            and not needs_exception
        ):
            expected_root = "ores/" if owner_kind == "platform" else f"{owner}/"
            if not target.startswith(expected_root):
                diagnostics.append(
                    Diagnostic(
                        "manifest.owner-root",
                        f"slash target must stay under {expected_root!r}",
                        where,
                    )
                )
        if system == "metadata-key" and isinstance(target, str) and not target.startswith(
            ("platform.oresoftware.com/", "secrets.oresoftware.com/")
        ):
            diagnostics.append(
                Diagnostic(
                    "manifest.metadata-authority",
                    "metadata targets require an approved *.oresoftware.com authority",
                    where,
                )
            )
        if system == "host-path" and isinstance(target, str) and not target.startswith(
            ("/opt/ores", "/var/lib/ores", "/srv/ores")
        ):
            diagnostics.append(
                Diagnostic(
                    "manifest.host-root",
                    "host targets must stay under an approved /.../ores root",
                    where,
                )
            )
        for field_name in ("verification", "rollback"):
            plan = value.get(field_name)
            if not isinstance(plan, dict):
                diagnostics.append(
                    Diagnostic(
                        f"manifest.{field_name}",
                        f"{field_name} must be an object",
                        where,
                    )
                )
                continue
            if plan.get("state") not in {"required", "defined", "verified"}:
                diagnostics.append(
                    Diagnostic(
                        f"manifest.{field_name}-state",
                        f"{field_name}.state must be required, defined, or verified",
                        where,
                    )
                )
            if not isinstance(plan.get("procedure"), str) or not plan.get("procedure"):
                diagnostics.append(
                    Diagnostic(
                        f"manifest.{field_name}-procedure",
                        f"{field_name}.procedure must be non-empty",
                        where,
                    )
                )
        if value.get("destructiveCleanupAllowed") is not False:
            diagnostics.append(
                Diagnostic(
                    "manifest.destructive-cleanup",
                    "Phase 0 forbids destructive cleanup",
                    where,
                )
            )
        if not isinstance(value.get("notes"), str) or not value.get("notes"):
            diagnostics.append(
                Diagnostic("manifest.notes", "notes must be non-empty", where)
            )
    if len(entry_identities) != len(set(entry_identities)):
        diagnostics.append(
            Diagnostic(
                "manifest.duplicate-identity",
                "manifest contains duplicate occurrence identities",
                DEFAULT_MANIFEST,
            )
        )
    inventory_counter = Counter(inventory_identities)
    manifest_counter = Counter(entry_identities)
    missing = inventory_counter - manifest_counter
    extra = manifest_counter - inventory_counter
    if missing:
        diagnostics.append(
            Diagnostic(
                "manifest.missing-identity",
                f"manifest is missing {sum(missing.values())} inventory occurrence identities",
                inventory_path,
            )
        )
    if extra:
        diagnostics.append(
            Diagnostic(
                "manifest.extra-identity",
                f"manifest contains {sum(extra.values())} identities absent from inventory",
                DEFAULT_MANIFEST,
            )
        )
    if entry_identities != sorted(entry_identities):
        diagnostics.append(
            Diagnostic(
                "manifest.order",
                "entries must be sorted by the canonical occurrence identity",
                DEFAULT_MANIFEST,
            )
        )
    expected_summary = manifest_summary(
        [item for item in entries if isinstance(item, dict)]
    )
    if spec.get("summary") != expected_summary:
        diagnostics.append(
            Diagnostic(
                "manifest.summary",
                "spec.summary does not match the entries",
                DEFAULT_MANIFEST,
            )
        )
    return diagnostics


def check_manifest(
    root: Path,
    *,
    inventory_path: str = DEFAULT_INVENTORY,
    registry_path: str = DEFAULT_REGISTRY,
    rules_path: str = DEFAULT_RULES,
    manifest_path: str = DEFAULT_MANIFEST,
    schema_path: str = DEFAULT_SCHEMA,
) -> tuple[Mapping[str, Any], int]:
    root = root.resolve()
    expected = build_manifest(
        root,
        inventory_path=inventory_path,
        registry_path=registry_path,
        rules_path=rules_path,
    )
    diagnostics = list(expected.diagnostics)
    actual, found = read_json(root / manifest_path, "manifest.read")
    diagnostics.extend(found)
    schema, found = read_json(root / schema_path, "manifest.schema-read")
    diagnostics.extend(found)
    loaded = load_inputs(
        root,
        inventory_path=inventory_path,
        registry_path=registry_path,
        rules_path=rules_path,
    )
    owners, found = owner_index(loaded.registry)
    diagnostics.extend(found)
    if actual:
        diagnostics.extend(
            validate_manifest_semantics(
                actual,
                loaded.inventory,
                owners,
                inventory_path=inventory_path,
            )
        )
    if schema and schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
        diagnostics.append(
            Diagnostic(
                "manifest.schema-draft",
                "manifest schema must use JSON Schema draft 2020-12",
                schema_path,
            )
        )
    if expected.manifest is not None and actual:
        expected_text = canonical_json(expected.manifest)
        actual_text = canonical_json(actual)
        try:
            committed_text = (root / manifest_path).read_text(encoding="utf-8")
        except OSError as error:
            committed_text = ""
            diagnostics.append(
                Diagnostic("manifest.read", f"cannot read manifest bytes: {error}", manifest_path)
            )
        if actual_text != expected_text:
            diagnostics.append(
                Diagnostic(
                    "manifest.stale",
                    "committed manifest content differs from deterministic generation",
                    manifest_path,
                )
            )
        if committed_text != actual_text:
            diagnostics.append(
                Diagnostic(
                    "manifest.noncanonical-json",
                    "committed manifest is not canonical sorted two-space JSON with a trailing newline",
                    manifest_path,
                )
            )
    encoded = sorted(
        path.relative_to(root).as_posix()
        for path in (root / "catalog/namespaces").glob(".den-2786-bundle-*.hex")
        if path.is_file()
    )
    if encoded:
        diagnostics.append(
            Diagnostic(
                "manifest.encoded-staging",
                f"opaque encoded staging files must be removed: {encoded}",
                "catalog/namespaces",
            )
        )
    valid = not any(item.severity == "error" for item in diagnostics)
    entry_count = len(mapping(actual.get("spec")).get("entries", [])) if actual else 0
    report = {
        "valid": valid,
        "manifest": manifest_path,
        "schema": schema_path,
        "entryCount": entry_count,
        "inventoryEntryCount": len(loaded.inventory.get("occurrences", []))
        if isinstance(loaded.inventory.get("occurrences"), list)
        else 0,
        "manifestSha256": sha256_path(root / manifest_path)
        if (root / manifest_path).is_file()
        else None,
        "inventorySha256": sha256_path(root / inventory_path)
        if (root / inventory_path).is_file()
        else None,
        "encodedStagingFiles": encoded,
        "diagnostics": [asdict(item) for item in diagnostics],
    }
    return report, 0 if valid else 2


def generate_manifest(
    root: Path,
    *,
    inventory_path: str = DEFAULT_INVENTORY,
    registry_path: str = DEFAULT_REGISTRY,
    rules_path: str = DEFAULT_RULES,
    output_path: str = DEFAULT_MANIFEST,
) -> tuple[Mapping[str, Any], int]:
    result = build_manifest(
        root,
        inventory_path=inventory_path,
        registry_path=registry_path,
        rules_path=rules_path,
    )
    diagnostics = list(result.diagnostics)
    if result.manifest is not None and not any(
        item.severity == "error" for item in diagnostics
    ):
        destination = root.resolve() / output_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(canonical_json(result.manifest), encoding="utf-8")
    valid = result.manifest is not None and not any(
        item.severity == "error" for item in diagnostics
    )
    report = {
        "valid": valid,
        "output": output_path,
        "entryCount": len(mapping(result.manifest or {}).get("spec", {}).get("entries", [])),
        "diagnostics": [asdict(item) for item in diagnostics],
    }
    return report, 0 if valid else 2


def render_text(report: Mapping[str, Any]) -> str:
    lines = [f"valid: {str(bool(report.get('valid'))).lower()}"]
    for key in (
        "manifest",
        "schema",
        "output",
        "entryCount",
        "inventoryEntryCount",
        "manifestSha256",
        "inventorySha256",
    ):
        if key in report:
            lines.append(f"{key}: {report.get(key)}")
    diagnostics = report.get("diagnostics", [])
    lines.append(f"diagnostics: {len(diagnostics) if isinstance(diagnostics, list) else 0}")
    if isinstance(diagnostics, list):
        for item in diagnostics:
            if isinstance(item, dict):
                lines.append(
                    f"- [{item.get('severity', 'error')}] {item.get('rule_id')}: "
                    f"{item.get('message')} ({item.get('path', '')})"
                )
    return "\n".join(lines) + "\n"


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)

    def common(command: argparse.ArgumentParser) -> None:
        command.add_argument("--root", type=Path, default=Path("."))
        command.add_argument("--inventory", default=DEFAULT_INVENTORY)
        command.add_argument("--registry", default=DEFAULT_REGISTRY)
        command.add_argument("--rules", default=DEFAULT_RULES)

    generate = commands.add_parser("generate")
    common(generate)
    generate.add_argument("--output", default=DEFAULT_MANIFEST)
    generate.add_argument("--format", choices=("json", "text"), default="text")

    check = commands.add_parser("check")
    common(check)
    check.add_argument("--manifest", default=DEFAULT_MANIFEST)
    check.add_argument("--schema", default=DEFAULT_SCHEMA)
    check.add_argument("--format", choices=("json", "text"), default="text")

    render = commands.add_parser("render")
    common(render)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    root = args.root.resolve()
    if args.command == "render":
        result = build_manifest(
            root,
            inventory_path=args.inventory,
            registry_path=args.registry,
            rules_path=args.rules,
        )
        if result.manifest is not None:
            print(canonical_json(result.manifest), end="")
        else:
            print(
                canonical_json(
                    {
                        "valid": False,
                        "diagnostics": [asdict(item) for item in result.diagnostics],
                    }
                ),
                end="",
            )
        return 0 if result.valid else 2
    if args.command == "generate":
        report, status = generate_manifest(
            root,
            inventory_path=args.inventory,
            registry_path=args.registry,
            rules_path=args.rules,
            output_path=args.output,
        )
    else:
        report, status = check_manifest(
            root,
            inventory_path=args.inventory,
            registry_path=args.registry,
            rules_path=args.rules,
            manifest_path=args.manifest,
            schema_path=args.schema,
        )
    if args.format == "json":
        print(canonical_json(report), end="")
    else:
        print(render_text(report), end="")
    return status


if __name__ == "__main__":
    sys.exit(main())
