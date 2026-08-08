#!/usr/bin/env python3
"""Read-only ownership registry, legacy namespace inventory, and PR ratchet."""
from __future__ import annotations

import argparse
import configparser
import json
import re
import subprocess
import sys
from collections import Counter
from dataclasses import asdict, dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Iterable, Mapping, Sequence

API_VERSION = "oresoftware.dev/v1alpha1"
REGISTRY_KIND = "NamespaceOwnerRegistry"
RULESET_KIND = "NamespaceMigrationRuleSet"
DEFAULT_REGISTRY = "catalog/namespaces/owners.json"
DEFAULT_RULES = "catalog/namespaces/migration-rules.json"
REGISTRY_SCHEMA = "./owner-registry.schema.json"
RULES_SCHEMA = "./migration-rules.schema.json"
MAX_TEXT_BYTES = 2 * 1024 * 1024
DNS_LABEL = re.compile(r"^[a-z0-9](?:[-a-z0-9]*[a-z0-9])?$")
OWNER_KINDS = {"platform", "product", "shared-service", "test"}
SYSTEMS = {"slash-namespace", "metadata-key", "host-path", "source-package", "generated-package"}
STATUSES = {"classified", "review-required", "unclassified"}
MATCH_TYPES = {"exact", "prefix", "regex"}
ALLOW_MARKER = "namespace-migration: allow-legacy"
GOVERNANCE = (
    "catalog/namespaces/",
    "tools/namespace_migration.py",
    "tools/test_namespace_migration.py",
    "docs/namespace-migration.md",
    ".github/workflows/namespace-migration-contract.yml",
    "artifacts/namespace-inventory.json",
    "artifacts/den-2926-inventory-delta.json",
)
PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "metadata-key",
        re.compile(
            r"(?<![A-Za-z0-9_.-])dd/(?:threadId|userId)"
            r"(?![A-Za-z0-9._~/-])"
        ),
    ),
    (
        "metadata-key",
        re.compile(
            r"(?<![A-Za-z0-9_.-])dd\.dev/[A-Za-z0-9][A-Za-z0-9._~-]*"
            r"(?![A-Za-z0-9._~/-])"
        ),
    ),
    (
        "source-package",
        re.compile(
            r"(?<![A-Za-z0-9_.-])github\.com/oresoftware/dd"
            r"(?:/[A-Za-z0-9._~:@%+,-]+)*"
            r"(?![A-Za-z0-9._~:@%+=-])",
            re.I,
        ),
    ),
    (
        "source-package",
        re.compile(
            r"(?<![A-Za-z0-9_.-])com\.oresoftware\.dd"
            r"(?:\.[A-Za-z0-9_]+)*"
            r"(?![A-Za-z0-9_.-])"
        ),
    ),
    (
        "generated-package",
        re.compile(
            r"(?<![A-Za-z0-9_.-])(?:dd_pg_defs|dd\.pgdefs)"
            r"(?:[A-Za-z0-9_.-]*)?"
        ),
    ),
    (
        "host-path",
        re.compile(
            r"(?:/opt/dd|/var/lib/dd|/srv/dd|"
            r"/home/[A-Za-z0-9._-]+/codes/dd)"
            r"(?:/[A-Za-z0-9._~:@%+,=-]+)*"
            r"(?![A-Za-z0-9._~@%+=-])"
        ),
    ),
    (
        "slash-namespace",
        re.compile(
            r"(?<![A-Za-z0-9_.-])dd/"
            r"[A-Za-z0-9][A-Za-z0-9._~:@%+,/=-]*"
        ),
    ),
)
TRAILING = ".,;:)]}>'\"`"


@dataclass(frozen=True)
class Diagnostic:
    rule_id: str
    message: str
    path: str = ""
    severity: str = "error"


@dataclass(frozen=True)
class Owner:
    namespace_id: str
    github_owner: str
    kind: str
    aliases: tuple[str, ...]
    description: str


@dataclass(frozen=True)
class MigrationRule:
    rule_id: str
    priority: int
    system: str
    match_type: str
    match_value: str
    owner: str
    status: str
    target_template: str | None
    environment: str | None
    workload: str | None
    consumers: tuple[str, ...]
    notes: str


@dataclass(frozen=True)
class Reference:
    system: str
    value: str
    column: int


@dataclass(frozen=True)
class Occurrence:
    path: str
    line: int
    column: int
    scope: str
    system: str
    reference: str
    rule_id: str
    owner: str
    status: str
    target_preview: str | None


@dataclass(frozen=True)
class ValidationResult:
    owners: tuple[Owner, ...]
    rules: tuple[MigrationRule, ...]
    diagnostics: tuple[Diagnostic, ...]

    @property
    def valid(self) -> bool:
        return not any(item.severity == "error" for item in self.diagnostics)


def mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, dict) else {}


def string(value: Any) -> str:
    return value if isinstance(value, str) else ""


def strings(value: Any) -> tuple[str, ...]:
    return tuple(value) if isinstance(value, list) and all(isinstance(item, str) for item in value) else ()


def fields(value: Mapping[str, Any], allowed: set[str], where: str) -> list[Diagnostic]:
    return [Diagnostic("catalog.unknown-field", f"{where} contains unsupported field {name!r}", where) for name in sorted(set(value) - allowed)]


def read_json(path: Path) -> tuple[Any, list[Diagnostic]]:
    try:
        return json.loads(path.read_text(encoding="utf-8")), []
    except (OSError, json.JSONDecodeError) as error:
        return None, [Diagnostic("catalog.read", f"cannot read valid JSON: {error}", path.as_posix())]


def safe_relative(value: str) -> bool:
    path = PurePosixPath(value)
    return bool(value) and "\\" not in value and not path.is_absolute() and all(part not in {"", ".", ".."} for part in path.parts)


def normalize_repo(value: str) -> str:
    value = value.strip().replace("\\", "/").lower()
    for prefix in ("git@github.com:", "ssh://git@github.com/", "https://github.com/", "http://github.com/", "git://github.com/"):
        if value.startswith(prefix):
            value = "github.com/" + value.removeprefix(prefix)
            break
    value = value.rstrip("/")
    return value[:-4] if value.endswith(".git") else value


def github_owner(value: str) -> str | None:
    parts = normalize_repo(value).split("/")
    return parts[1] if len(parts) >= 3 and parts[0] == "github.com" else None


def validate_registry(raw: Any, *, path: str) -> tuple[tuple[Owner, ...], list[Diagnostic]]:
    if not isinstance(raw, dict):
        return (), [Diagnostic("registry.invalid-root", "registry must be a JSON object", path)]
    diagnostics = fields(raw, {"$schema", "apiVersion", "kind", "metadata", "spec"}, path)
    if raw.get("$schema") != REGISTRY_SCHEMA:
        diagnostics.append(Diagnostic("registry.schema-reference", f"$schema must equal {REGISTRY_SCHEMA!r}", path))
    if raw.get("apiVersion") != API_VERSION:
        diagnostics.append(Diagnostic("registry.api-version", f"apiVersion must equal {API_VERSION!r}", path))
    if raw.get("kind") != REGISTRY_KIND:
        diagnostics.append(Diagnostic("registry.kind", f"kind must equal {REGISTRY_KIND!r}", path))
    metadata, spec = mapping(raw.get("metadata")), mapping(raw.get("spec"))
    diagnostics += fields(metadata, {"name"}, f"{path}#metadata")
    diagnostics += fields(spec, {"platformOwner", "allowedEnvironments", "owners"}, f"{path}#spec")
    if not DNS_LABEL.fullmatch(string(metadata.get("name"))):
        diagnostics.append(Diagnostic("registry.metadata-name", "metadata.name must be a lowercase DNS label", path))
    environments = spec.get("allowedEnvironments")
    if not isinstance(environments, list) or not environments or not all(isinstance(item, str) and DNS_LABEL.fullmatch(item) for item in environments):
        diagnostics.append(Diagnostic("registry.environments", "allowedEnvironments must be a non-empty DNS-label list", path))
    raw_owners = spec.get("owners")
    if not isinstance(raw_owners, list) or not raw_owners:
        return (), diagnostics + [Diagnostic("registry.empty", "spec.owners must be non-empty", path)]
    owners: list[Owner] = []
    ids: dict[str, int] = {}
    github: dict[str, str] = {}
    aliases: dict[str, str] = {}
    for index, item in enumerate(raw_owners):
        where = f"{path}#spec.owners[{index}]"
        if not isinstance(item, dict):
            diagnostics.append(Diagnostic("registry.owner-object", "owner must be an object", where))
            continue
        diagnostics += fields(item, {"namespaceId", "githubOwner", "kind", "aliases", "description"}, where)
        namespace_id = string(item.get("namespaceId"))
        github_slug = string(item.get("githubOwner"))
        kind = string(item.get("kind"))
        owner_aliases = strings(item.get("aliases", []))
        description = string(item.get("description"))
        if not DNS_LABEL.fullmatch(namespace_id) or len(namespace_id) > 63 or namespace_id == "dd":
            diagnostics.append(Diagnostic("registry.namespace-id", "namespaceId must be a stable lowercase DNS label other than dd", where))
        elif namespace_id in ids:
            diagnostics.append(Diagnostic("registry.duplicate-id", f"namespaceId already appears at index {ids[namespace_id]}", where))
        else:
            ids[namespace_id] = index
        normalized_github = github_slug.lower()
        if not github_slug or "/" in github_slug or github_slug.strip() != github_slug:
            diagnostics.append(Diagnostic("registry.github-owner", "githubOwner must be one GitHub owner slug", where))
        elif normalized_github in github:
            diagnostics.append(Diagnostic("registry.duplicate-github-owner", f"githubOwner is already assigned to {github[normalized_github]!r}", where))
        else:
            github[normalized_github] = namespace_id
        if kind not in OWNER_KINDS:
            diagnostics.append(Diagnostic("registry.owner-kind", f"kind must be one of {sorted(OWNER_KINDS)}", where))
        if not description:
            diagnostics.append(Diagnostic("registry.description", "description must explain the ownership boundary", where))
        for alias in owner_aliases:
            normalized = alias.lower()
            if not DNS_LABEL.fullmatch(normalized):
                diagnostics.append(Diagnostic("registry.alias", f"alias {alias!r} must be a DNS label", where))
            elif normalized in aliases:
                diagnostics.append(Diagnostic("registry.duplicate-alias", f"alias {alias!r} is already owned by {aliases[normalized]!r}", where))
            else:
                aliases[normalized] = namespace_id
        owners.append(Owner(namespace_id, github_slug, kind, owner_aliases, description))
    owner_index = {item.namespace_id: item for item in owners if item.namespace_id}
    for alias, alias_owner in aliases.items():
        if alias in owner_index:
            diagnostics.append(Diagnostic("registry.alias-collision", f"alias {alias!r} owned by {alias_owner!r} collides with a namespaceId", path))
    platform_id = string(spec.get("platformOwner"))
    platform = owner_index.get(platform_id)
    if platform_id != "ores" or platform is None:
        diagnostics.append(Diagnostic("registry.platform-owner", "platformOwner must resolve to stable namespaceId 'ores'", path))
    elif platform.kind != "platform" or platform.github_owner.lower() != "oresoftware":
        diagnostics.append(Diagnostic("registry.platform-kind", "ores must be the ORESoftware platform owner", path))
    return tuple(owners), diagnostics


def validate_rules(raw: Any, *, path: str, owner_index: Mapping[str, Owner]) -> tuple[tuple[MigrationRule, ...], list[Diagnostic]]:
    if not isinstance(raw, dict):
        return (), [Diagnostic("rules.invalid-root", "rule set must be a JSON object", path)]
    diagnostics = fields(raw, {"$schema", "apiVersion", "kind", "metadata", "spec"}, path)
    if raw.get("$schema") != RULES_SCHEMA:
        diagnostics.append(Diagnostic("rules.schema-reference", f"$schema must equal {RULES_SCHEMA!r}", path))
    if raw.get("apiVersion") != API_VERSION:
        diagnostics.append(Diagnostic("rules.api-version", f"apiVersion must equal {API_VERSION!r}", path))
    if raw.get("kind") != RULESET_KIND:
        diagnostics.append(Diagnostic("rules.kind", f"kind must equal {RULESET_KIND!r}", path))
    metadata, spec = mapping(raw.get("metadata")), mapping(raw.get("spec"))
    diagnostics += fields(metadata, {"name"}, f"{path}#metadata")
    diagnostics += fields(spec, {"fallbackOwner", "rules"}, f"{path}#spec")
    if not DNS_LABEL.fullmatch(string(metadata.get("name"))):
        diagnostics.append(Diagnostic("rules.metadata-name", "metadata.name must be a lowercase DNS label", path))
    if spec.get("fallbackOwner") != "unclassified":
        diagnostics.append(Diagnostic("rules.fallback-owner", "unknown values must retain fallbackOwner 'unclassified'", path))
    raw_rules = spec.get("rules")
    if not isinstance(raw_rules, list) or not raw_rules:
        return (), diagnostics + [Diagnostic("rules.empty", "spec.rules must be non-empty", path)]
    rules: list[MigrationRule] = []
    ids: set[str] = set()
    matches: set[tuple[str, str, str]] = set()
    exact_targets: dict[tuple[str, str], str] = {}
    allowed_fields = {"id", "priority", "system", "match", "owner", "status", "targetTemplate", "environment", "workload", "consumers", "notes"}
    for index, item in enumerate(raw_rules):
        where = f"{path}#spec.rules[{index}]"
        if not isinstance(item, dict):
            diagnostics.append(Diagnostic("rules.rule-object", "rule must be an object", where))
            continue
        diagnostics += fields(item, allowed_fields, where)
        match = mapping(item.get("match"))
        diagnostics += fields(match, {"type", "value"}, f"{where}.match")
        rule_id = string(item.get("id"))
        priority = item.get("priority")
        system = string(item.get("system"))
        match_type = string(match.get("type"))
        match_value = string(match.get("value"))
        owner = string(item.get("owner"))
        status = string(item.get("status"))
        target = item.get("targetTemplate") if isinstance(item.get("targetTemplate"), str) and item.get("targetTemplate") else None
        environment = item.get("environment") if isinstance(item.get("environment"), str) and item.get("environment") else None
        workload = item.get("workload") if isinstance(item.get("workload"), str) and item.get("workload") else None
        consumers = strings(item.get("consumers", []))
        notes = string(item.get("notes"))
        if not re.fullmatch(r"[a-z0-9][a-z0-9.-]*", rule_id) or rule_id in ids:
            diagnostics.append(Diagnostic("rules.id", "id must be unique lowercase dot/hyphen text", where))
        ids.add(rule_id)
        if not isinstance(priority, int) or isinstance(priority, bool) or not 0 <= priority <= 10000:
            diagnostics.append(Diagnostic("rules.priority", "priority must be an integer from 0 through 10000", where))
            priority = 0
        if system not in SYSTEMS:
            diagnostics.append(Diagnostic("rules.system", f"system must be one of {sorted(SYSTEMS)}", where))
        if match_type not in MATCH_TYPES or not match_value:
            diagnostics.append(Diagnostic("rules.match", "match requires a supported type and non-empty value", where))
        elif match_type == "regex":
            try:
                re.compile(match_value)
            except re.error as error:
                diagnostics.append(Diagnostic("rules.invalid-regex", f"invalid regex: {error}", where))
        identity = (system, match_type, match_value)
        if identity in matches:
            diagnostics.append(Diagnostic("rules.duplicate-match", "match is already declared", where))
        matches.add(identity)
        if owner != "unclassified" and owner not in owner_index:
            diagnostics.append(Diagnostic("rules.unknown-owner", f"owner {owner!r} is not registered", where))
        if status not in STATUSES:
            diagnostics.append(Diagnostic("rules.status", f"status must be one of {sorted(STATUSES)}", where))
        if status == "classified" and owner == "unclassified":
            diagnostics.append(Diagnostic("rules.classified-owner", "classified rules require a registered owner", where))
        if status == "unclassified" and owner != "unclassified":
            diagnostics.append(Diagnostic("rules.unclassified-owner", "unclassified rules must retain owner 'unclassified'", where))
        if status == "unclassified" and target is not None:
            diagnostics.append(Diagnostic("rules.unclassified-target", "unclassified rules may not invent a targetTemplate", where))
        if target and ("dd/" in target or "dd.dev/" in target):
            diagnostics.append(Diagnostic("rules.legacy-target", "targetTemplate may not preserve a legacy prefix", where))
        if target and system == "slash-namespace" and owner in owner_index and not target.startswith(f"{owner}/"):
            diagnostics.append(Diagnostic("rules.owner-root", f"slash target for {owner!r} must start with {owner + '/'!r}", where))
        if target and system == "metadata-key" and not target.startswith(("platform.oresoftware.com/", "secrets.oresoftware.com/")):
            diagnostics.append(Diagnostic("rules.metadata-authority", "metadata targets require an approved *.oresoftware.com authority", where))
        if target and system == "host-path" and not target.startswith(("/opt/ores", "/var/lib/ores", "/srv/ores")):
            diagnostics.append(Diagnostic("rules.host-root", "host targets must stay under an approved /.../ores root", where))
        if target:
            placeholders = set(re.findall(r"\{([A-Za-z][A-Za-z0-9_-]*)\}", target))
            unsupported = sorted(placeholders - {"suffix", "reference", "environment", "workload", "owner"})
            if unsupported:
                diagnostics.append(Diagnostic("rules.target-placeholder", f"unsupported placeholders: {unsupported}", where))
        if workload is not None and not DNS_LABEL.fullmatch(workload):
            diagnostics.append(Diagnostic("rules.workload", "workload must be a lowercase DNS label", where))
        if len(set(consumers)) != len(consumers):
            diagnostics.append(Diagnostic("rules.duplicate-consumer", "consumers must be unique", where))
        for consumer in consumers:
            if consumer not in owner_index:
                diagnostics.append(Diagnostic("rules.unknown-consumer", f"consumer {consumer!r} is not registered", where))
        if match_type == "exact" and target and status == "classified":
            key = (system, target)
            if key in exact_targets:
                diagnostics.append(Diagnostic("rules.target-collision", f"exact target is already produced by {exact_targets[key]!r}", where))
            exact_targets[key] = rule_id
        if not notes:
            diagnostics.append(Diagnostic("rules.notes", "notes must explain classification or remaining review", where))
        rules.append(MigrationRule(rule_id, priority, system, match_type, match_value, owner, status, target, environment, workload, consumers, notes))
    required = {("slash-namespace", "prefix", "dd/"), ("metadata-key", "prefix", "dd.dev/")}
    present = {(item.system, item.match_type, item.match_value) for item in rules}
    for system, match_type, value in sorted(required - present):
        diagnostics.append(Diagnostic("rules.missing-fallback", f"missing fail-closed fallback for {system} {match_type} {value!r}", path))
    return tuple(sorted(rules, key=lambda item: (-item.priority, item.rule_id))), diagnostics


def load_contract(root: Path, *, registry_path: str = DEFAULT_REGISTRY, rules_path: str = DEFAULT_RULES) -> ValidationResult:
    registry_raw, registry_errors = read_json(root / registry_path)
    rules_raw, rules_errors = read_json(root / rules_path)
    diagnostics = registry_errors + rules_errors
    owners: tuple[Owner, ...] = ()
    rules: tuple[MigrationRule, ...] = ()
    if not registry_errors:
        owners, found = validate_registry(registry_raw, path=registry_path)
        diagnostics += found
    if not rules_errors:
        rules, found = validate_rules(rules_raw, path=rules_path, owner_index={item.namespace_id: item for item in owners})
        diagnostics += found
    return ValidationResult(owners, rules, tuple(diagnostics))


def scan_line(line: str) -> list[Reference]:
    found: list[Reference] = []
    occupied: list[tuple[int, int]] = []
    for system, pattern in PATTERNS:
        for match in pattern.finditer(line):
            start, end = match.span()
            if any(start < right and end > left for left, right in occupied):
                continue
            value = match.group(0).rstrip(TRAILING)
            if value:
                found.append(Reference(system, value, start + 1))
                occupied.append((start, start + len(value)))
    return sorted(found, key=lambda item: item.column)


def classify_reference(reference: Reference, rules: Sequence[MigrationRule]) -> tuple[MigrationRule | None, str | None]:
    for rule in rules:
        if rule.system != reference.system:
            continue
        matched = reference.value == rule.match_value if rule.match_type == "exact" else reference.value.startswith(rule.match_value) if rule.match_type == "prefix" else re.search(rule.match_value, reference.value) is not None
        if not matched:
            continue
        target = rule.target_template
        if target:
            suffix = reference.value[len(rule.match_value):] if rule.match_type == "prefix" else reference.value
            target = target.replace("{suffix}", suffix.lstrip("/_.-")).replace("{reference}", reference.value).replace("{owner}", rule.owner)
            if rule.environment:
                target = target.replace("{environment}", rule.environment)
            if rule.workload:
                target = target.replace("{workload}", rule.workload)
        return rule, target
    return None, None


def is_governance(path: str) -> bool:
    return any(path == prefix or path.startswith(prefix) for prefix in GOVERNANCE)


def path_scope(path: str) -> str:
    lower = path.lower()
    if is_governance(path):
        return "governance"
    if path.startswith("docs/") or lower.endswith((".md", ".rst", ".txt")):
        return "documentation"
    if "/test" in lower or lower.startswith(("test/", "tests/", "scripts/test", "scripts/tests")) or lower.endswith(("_test.py", ".test.mjs", ".spec.ts", ".spec.js")):
        return "test"
    return "active"


def tracked_paths(root: Path) -> list[Path]:
    try:
        output = subprocess.run(["git", "ls-files", "-z"], cwd=root, check=True, capture_output=True).stdout
        return sorted(root / raw.decode("utf-8") for raw in output.split(b"\0") if raw and safe_relative(raw.decode("utf-8")))
    except (OSError, subprocess.CalledProcessError, UnicodeDecodeError):
        return sorted(path for path in root.rglob("*") if path.is_file())


def scan_repository(root: Path, rules: Sequence[MigrationRule], *, include_governance: bool = False) -> tuple[list[Occurrence], list[Diagnostic]]:
    occurrences: list[Occurrence] = []
    diagnostics: list[Diagnostic] = []
    for absolute in tracked_paths(root):
        try:
            relative = absolute.relative_to(root).as_posix()
            if (not include_governance and is_governance(relative)) or absolute.is_symlink() or not absolute.is_file() or absolute.stat().st_size > MAX_TEXT_BYTES:
                continue
            raw = absolute.read_bytes()
            if b"\0" in raw:
                continue
            text = raw.decode("utf-8")
        except (OSError, UnicodeDecodeError) as error:
            diagnostics.append(Diagnostic("inventory.read", f"cannot inspect file: {error}", str(absolute), "warning"))
            continue
        scope = path_scope(relative)
        for line_number, line in enumerate(text.splitlines(), 1):
            for reference in scan_line(line):
                rule, target = classify_reference(reference, rules)
                occurrences.append(Occurrence(relative, line_number, reference.column, scope, reference.system, reference.value, rule.rule_id if rule else "fallback.unclassified", rule.owner if rule else "unclassified", rule.status if rule else "unclassified", target))
    return occurrences, diagnostics


def load_gitmodules(root: Path) -> dict[str, str]:
    path = root / ".gitmodules"
    if not path.is_file():
        return {}
    parser = configparser.ConfigParser(interpolation=None)
    parser.optionxform = str
    try:
        parser.read(path, encoding="utf-8")
    except configparser.Error:
        return {}
    return {parser.get(section, "path"): parser.get(section, "url") for section in parser.sections() if section.startswith('submodule "') and parser.has_option(section, "path") and parser.has_option(section, "url")}


def discover_owners(root: Path) -> dict[str, set[str]]:
    discovered: dict[str, set[str]] = {}
    def add(owner: str | None, source: str) -> None:
        if owner:
            discovered.setdefault(owner.lower(), set()).add(source)
    for path, repository in load_gitmodules(root).items():
        add(github_owner(repository), f".gitmodules:{path}")
    for path in sorted(root.glob("catalog/gitops/apps/*.json")):
        raw, errors = read_json(path)
        if not errors:
            add(string(mapping(raw).get("spec") and mapping(mapping(raw).get("spec")).get("owner")), path.relative_to(root).as_posix())
    return discovered


def discovered_owner_diagnostics(root: Path, owners: Sequence[Owner]) -> list[Diagnostic]:
    known = {item.github_owner.lower() for item in owners} | {alias.lower() for item in owners for alias in item.aliases}
    return [Diagnostic("registry.discovered-owner", f"GitHub owner {owner!r} is referenced but not registered; sources: {', '.join(sorted(sources)[:5])}", DEFAULT_REGISTRY, "warning") for owner, sources in sorted(discover_owners(root).items()) if owner not in known]


def inventory_summary(items: Sequence[Occurrence]) -> dict[str, Any]:
    return {
        "total": len(items),
        "byScope": dict(sorted(Counter(item.scope for item in items).items())),
        "bySystem": dict(sorted(Counter(item.system for item in items).items())),
        "byStatus": dict(sorted(Counter(item.status for item in items).items())),
        "byOwner": dict(sorted(Counter(item.owner for item in items).items())),
        "distinctReferences": len({(item.system, item.reference) for item in items}),
        "unclassifiedActive": sum(item.scope == "active" and item.status == "unclassified" for item in items),
    }


def build_check_report(root: Path, *, registry_path: str, rules_path: str, strict_unclassified: bool) -> tuple[dict[str, Any], int]:
    contract = load_contract(root, registry_path=registry_path, rules_path=rules_path)
    diagnostics = list(contract.diagnostics) + discovered_owner_diagnostics(root, contract.owners)
    occurrences, scan_diagnostics = scan_repository(root, contract.rules) if contract.rules else ([], [])
    diagnostics += scan_diagnostics
    if strict_unclassified:
        for item in [item for item in occurrences if item.scope == "active" and item.status == "unclassified"][:100]:
            diagnostics.append(Diagnostic("inventory.unclassified-active", f"active legacy reference {item.reference!r} has no approved owner target", f"{item.path}:{item.line}:{item.column}"))
    valid = not any(item.severity == "error" for item in diagnostics)
    report = {"valid": valid, "contract": {"apiVersion": API_VERSION, "registryKind": REGISTRY_KIND, "ruleSetKind": RULESET_KIND, "owners": len(contract.owners), "rules": len(contract.rules)}, "inventory": inventory_summary(occurrences), "diagnostics": [asdict(item) for item in diagnostics]}
    return report, 0 if valid else 2


def added_lines_from_diff(diff: str) -> Iterable[tuple[str, int, str]]:
    path, new_line, in_hunk = "", 0, False
    hunk_pattern = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")
    for raw in diff.splitlines():
        if raw.startswith("+++ "):
            candidate = raw[4:]
            path = candidate[2:] if candidate.startswith("b/") else candidate
            in_hunk = False
            continue
        hunk = hunk_pattern.match(raw)
        if hunk:
            new_line, in_hunk = int(hunk.group(1)), True
            continue
        if not in_hunk or not path or path == "/dev/null":
            continue
        if raw.startswith("+") and not raw.startswith("+++"):
            yield path, new_line, raw[1:]
            new_line += 1
        elif not raw.startswith("-"):
            new_line += 1


def ratchet_report(root: Path, base_ref: str, head_ref: str) -> tuple[dict[str, Any], int]:
    try:
        diff = subprocess.run(["git", "diff", "--no-ext-diff", "--no-renames", "--unified=0", "--no-color", f"{base_ref}...{head_ref}", "--"], cwd=root, check=True, capture_output=True, text=True).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        report = {"valid": False, "baseRef": base_ref, "headRef": head_ref, "violations": [], "diagnostics": [asdict(Diagnostic("ratchet.diff", f"cannot compute exact diff: {error}"))]}
        return report, 2
    violations: list[dict[str, Any]] = []
    for path, line_number, line in added_lines_from_diff(diff):
        if is_governance(path) or ALLOW_MARKER in line:
            continue
        for reference in scan_line(line):
            violations.append({"path": path, "line": line_number, "column": reference.column, "system": reference.system, "reference": reference.value, "ruleId": "ratchet.new-legacy-reference", "message": f"new legacy references are prohibited; use the registered owner target or a reviewed line-scoped {ALLOW_MARKER!r} exception"})
    report = {"valid": not violations, "baseRef": base_ref, "headRef": head_ref, "violations": violations, "diagnostics": []}
    return report, 0 if report["valid"] else 2


def inventory_report(root: Path, *, registry_path: str, rules_path: str, include_governance: bool) -> tuple[dict[str, Any], int]:
    contract = load_contract(root, registry_path=registry_path, rules_path=rules_path)
    diagnostics = list(contract.diagnostics)
    occurrences, scan_diagnostics = scan_repository(root, contract.rules, include_governance=include_governance) if contract.rules else ([], [])
    diagnostics += scan_diagnostics
    valid = not any(item.severity == "error" for item in diagnostics)
    report = {"valid": valid, "generatedFrom": {"root": ".", "registry": registry_path, "rules": rules_path}, "summary": inventory_summary(occurrences), "occurrences": [asdict(item) for item in occurrences], "diagnostics": [asdict(item) for item in diagnostics]}
    return report, 0 if valid else 2


def render_text(report: Mapping[str, Any]) -> str:
    lines = [f"valid: {str(bool(report.get('valid'))).lower()}"]
    summary = report.get("inventory") or report.get("summary")
    if isinstance(summary, dict):
        for key in ("total", "distinctReferences", "unclassifiedActive"):
            lines.append(f"{key}: {summary.get(key, 0)}")
        for key in ("byScope", "bySystem", "byStatus", "byOwner"):
            lines.append(f"{key}:")
            lines += [f"  {name}: {count}" for name, count in sorted(mapping(summary.get(key)).items())]
    violations = report.get("violations", [])
    lines.append(f"violations: {len(violations) if isinstance(violations, list) else 0}")
    for item in violations if isinstance(violations, list) else []:
        lines.append(f"  - {item.get('path')}:{item.get('line')}:{item.get('column')} {item.get('reference')}")
    diagnostics = report.get("diagnostics", [])
    lines.append(f"diagnostics: {len(diagnostics) if isinstance(diagnostics, list) else 0}")
    for item in diagnostics if isinstance(diagnostics, list) else []:
        lines.append(f"  - [{item.get('severity', 'error')}] {item.get('rule_id')}: {item.get('message')} ({item.get('path', '')})")
    return "\n".join(lines) + "\n"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    def contract_args(command: argparse.ArgumentParser) -> None:
        command.add_argument("--root", type=Path, default=Path("."))
        command.add_argument("--registry", default=DEFAULT_REGISTRY)
        command.add_argument("--rules", default=DEFAULT_RULES)
        command.add_argument("--format", choices=("json", "text"), default="text")
    check = commands.add_parser("check")
    contract_args(check)
    check.add_argument("--strict-unclassified", action="store_true")
    inventory = commands.add_parser("inventory")
    contract_args(inventory)
    inventory.add_argument("--include-governance", action="store_true")
    ratchet = commands.add_parser("ratchet")
    ratchet.add_argument("--root", type=Path, default=Path("."))
    ratchet.add_argument("--base-ref", required=True)
    ratchet.add_argument("--head-ref", default="HEAD")
    ratchet.add_argument("--format", choices=("json", "text"), default="text")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = args.root.resolve()
    if args.command == "check":
        report, status = build_check_report(root, registry_path=args.registry, rules_path=args.rules, strict_unclassified=args.strict_unclassified)
    elif args.command == "inventory":
        report, status = inventory_report(root, registry_path=args.registry, rules_path=args.rules, include_governance=args.include_governance)
    else:
        report, status = ratchet_report(root, args.base_ref, args.head_ref)
    print(json.dumps(report, indent=2, sort_keys=True) if args.format == "json" else render_text(report), end="\n" if args.format == "json" else "")
    return status


if __name__ == "__main__":
    sys.exit(main())
