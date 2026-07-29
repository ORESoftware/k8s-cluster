#!/usr/bin/env python3
"""Build, validate, enrich, compare, and render repository catalog snapshots.

The implementation is intentionally dependency-free. Runtime dependencies are
provided by the repository's locked Nix development shell.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import subprocess
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence

SCHEMA_VERSION = 2
RECORD_SCOPES = {"fixture", "public", "full"}
VISIBILITIES = {"public", "private", "internal"}
LIFECYCLES = {
    "production",
    "active",
    "experimental",
    "fixture",
    "legacy",
    "archive-candidate",
    "archived",
    "empty-reserved",
    "unknown",
}
PROFILES = {
    "deployed service or worker",
    "shared library/SDK/tool",
    "interface/schema/specification",
    "client/mobile/desktop/web application",
    "sync/local-first engine",
    "infrastructure/GitOps repository",
    "integration monorepo",
    "MCP/automation boundary",
    "E2E/conformance fixture",
    "website/documentation",
    "research/demo",
    "upstream fork",
    "unknown",
}
EVIDENCE_STATES = {
    "verified",
    "present-unverified",
    "missing",
    "not-applicable",
    "proposed",
    "blocked",
    "unknown",
}
DEPENDENCY_STATES = {"verified", "present-unverified", "proposed", "mention"}
DEPENDENCY_KINDS = {
    "gitlink",
    "package",
    "generated-artifact",
    "http",
    "grpc",
    "nats",
    "database-schema",
    "secret-reference",
    "deployment-pin",
    "other",
}
CONFORMANCE_STATES = {"conformant", "gap-owned", "exempt", "blocked", "unknown"}
REVIEW_STATES = {"reviewed", "needs-review", "blocked"}
ZED_STATES = {"conformant", "gap-owned", "exempt", "not-applicable", "unknown"}
RELEASE_STATES = {
    "published",
    "continuous-delivery",
    "internal",
    "not-released",
    "not-applicable",
    "unknown",
}
DEPLOYMENT_STATES = {
    "production",
    "staging",
    "active",
    "not-deployed",
    "not-applicable",
    "unknown",
}
SECURITY_CLASSES = {"public", "internal", "confidential", "restricted", "unknown"}
CONFORMANCE_RANK = {
    "unknown": 0,
    "blocked": 1,
    "gap-owned": 2,
    "exempt": 4,
    "conformant": 4,
}
DEN369_ISSUE = "DEN-369"
DEN637_ISSUE = "DEN-637"


@dataclass(frozen=True)
class ValidationError:
    repository: str
    field: str
    message: str

    def render(self) -> str:
        return f"{self.repository}: {self.field}: {self.message}"


def load_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def load_catalog(path: Path) -> dict[str, Any]:
    data = load_json(path)
    if not isinstance(data, dict):
        raise ValueError("catalog root must be an object")
    if data.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(f"schema_version must equal {SCHEMA_VERSION}")
    if not isinstance(data.get("repositories"), list):
        raise ValueError("repositories must be an array")
    return data


def validate_catalog(
    data: dict[str, Any],
    *,
    public_safe: bool = False,
    repo_root: Path | None = None,
) -> list[ValidationError]:
    errors: list[ValidationError] = []
    snapshot = data.get("snapshot")
    inventory = data.get("inventory")
    imports = data.get("imports")

    if not isinstance(snapshot, dict):
        errors.append(ValidationError("catalog", "snapshot", "must be an object"))
        snapshot = {}
    if snapshot.get("record_scope") not in RECORD_SCOPES:
        errors.append(
            ValidationError(
                "catalog",
                "snapshot.record_scope",
                f"must be one of {sorted(RECORD_SCOPES)}",
            )
        )
    for field in ("id", "captured_at", "source", "governing_issue"):
        _require_nonempty(errors, "catalog", snapshot, field, prefix="snapshot.")

    if not isinstance(inventory, dict):
        errors.append(ValidationError("catalog", "inventory", "must be an object"))
        inventory = {}
    for field in (
        "repository_count",
        "public_count",
        "private_count",
        "fork_count",
        "archived_count",
        "empty_count",
        "canonical_active_count",
        "owner_count",
    ):
        if not isinstance(inventory.get(field), int) or inventory[field] < 0:
            errors.append(
                ValidationError(
                    "catalog", f"inventory.{field}", "must be a non-negative integer"
                )
            )

    baseline_deltas = inventory.get("baseline_deltas")
    if baseline_deltas is not None:
        if not isinstance(baseline_deltas, dict):
            errors.append(
                ValidationError(
                    "catalog",
                    "inventory.baseline_deltas",
                    "must be an object when present",
                )
            )
        else:
            for field, value in baseline_deltas.items():
                if not isinstance(field, str) or not isinstance(value, int):
                    errors.append(
                        ValidationError(
                            "catalog",
                            "inventory.baseline_deltas",
                            "keys must be strings and values must be integers",
                        )
                    )
                    break

    if not isinstance(imports, dict):
        errors.append(ValidationError("catalog", "imports", "must be an object"))
        imports = {}
    _validate_den369_import(errors, imports.get("den369"), repo_root)

    seen: set[str] = set()
    records = data["repositories"]
    for index, record in enumerate(records):
        identity = f"repositories[{index}]"
        if not isinstance(record, dict):
            errors.append(ValidationError(identity, "record", "must be an object"))
            continue
        name = record.get("name")
        if isinstance(name, str) and name and "/" in name:
            identity = name
            if name in seen:
                errors.append(ValidationError(identity, "name", "duplicate repository"))
            seen.add(name)
        else:
            errors.append(
                ValidationError(
                    identity, "name", "must be a non-empty owner/repository string"
                )
            )

        _validate_repository(errors, identity, record, public_safe=public_safe)

    scope = snapshot.get("record_scope")
    if scope == "full" and isinstance(inventory.get("repository_count"), int):
        if len(records) != inventory["repository_count"]:
            errors.append(
                ValidationError(
                    "catalog",
                    "repositories",
                    "full catalogs must contain inventory.repository_count records",
                )
            )
    if scope == "public" and isinstance(inventory.get("public_count"), int):
        if len(records) != inventory["public_count"]:
            errors.append(
                ValidationError(
                    "catalog",
                    "repositories",
                    "public catalogs must contain inventory.public_count records",
                )
            )
    return errors


def _validate_den369_import(
    errors: list[ValidationError],
    value: Any,
    repo_root: Path | None,
) -> None:
    if not isinstance(value, dict):
        errors.append(ValidationError("catalog", "imports.den369", "must be an object"))
        return
    if value.get("issue") != DEN369_ISSUE:
        errors.append(
            ValidationError(
                "catalog", "imports.den369.issue", f"must equal {DEN369_ISSUE}"
            )
        )
    for field in ("source_path", "artifact_sha256", "contract"):
        _require_nonempty(errors, "catalog", value, field, prefix="imports.den369.")
    artifact_hash = value.get("artifact_sha256")
    if isinstance(artifact_hash, str) and artifact_hash not in {"not-imported"}:
        if len(artifact_hash) != 64 or any(
            character not in "0123456789abcdef" for character in artifact_hash
        ):
            errors.append(
                ValidationError(
                    "catalog",
                    "imports.den369.artifact_sha256",
                    "must be a lowercase SHA-256 or not-imported",
                )
            )
    if repo_root and value.get("source_path") and artifact_hash != "not-imported":
        source_path = (repo_root / value["source_path"]).resolve()
        if not source_path.is_file():
            errors.append(
                ValidationError(
                    "catalog", "imports.den369.source_path", "artifact does not exist"
                )
            )
        elif sha256_file(source_path) != artifact_hash:
            errors.append(
                ValidationError(
                    "catalog",
                    "imports.den369.artifact_sha256",
                    "does not match source artifact",
                )
            )


def _validate_repository(
    errors: list[ValidationError],
    identity: str,
    record: dict[str, Any],
    *,
    public_safe: bool,
) -> None:
    for field in (
        "repository_id",
        "visibility",
        "fork",
        "archived",
        "empty",
        "default_branch",
        "canonical_location",
        "classification",
        "ownership",
        "release",
        "consumers",
        "dependencies",
        "security",
        "exemptions",
        "review",
        "conformance",
        "nix_oci",
        "zed",
    ):
        if field not in record:
            errors.append(ValidationError(identity, field, "required field is missing"))

    _enum(errors, identity, record, "visibility", VISIBILITIES)
    if public_safe and record.get("visibility") != "public":
        errors.append(
            ValidationError(
                identity,
                "visibility",
                "public-safe catalogs may only name public repositories",
            )
        )
    for field in ("fork", "archived", "empty"):
        if not isinstance(record.get(field), bool):
            errors.append(ValidationError(identity, field, "must be a boolean"))
    if record.get("default_branch") is not None and not isinstance(
        record.get("default_branch"), str
    ):
        errors.append(
            ValidationError(identity, "default_branch", "must be a string or null")
        )
    _require_nonempty(errors, identity, record, "canonical_location")

    classification = _object(errors, identity, record, "classification")
    _enum(
        errors,
        identity,
        classification,
        "lifecycle",
        LIFECYCLES,
        prefix="classification.",
    )
    _enum(
        errors, identity, classification, "profile", PROFILES, prefix="classification."
    )
    _enum(
        errors,
        identity,
        classification,
        "evidence_state",
        EVIDENCE_STATES,
        prefix="classification.",
    )
    _require_nonempty(
        errors, identity, classification, "source", prefix="classification."
    )

    ownership = _object(errors, identity, record, "ownership")
    for field in ("linear_project", "linear_issue", "linear_issue_url"):
        _require_nonempty(errors, identity, ownership, field, prefix="ownership.")

    release = _object(errors, identity, record, "release")
    _enum(errors, identity, release, "state", RELEASE_STATES, prefix="release.")
    _enum(
        errors,
        identity,
        release,
        "deployment_state",
        DEPLOYMENT_STATES,
        prefix="release.",
    )

    if not isinstance(record.get("consumers"), list):
        errors.append(ValidationError(identity, "consumers", "must be an array"))
    dependencies = record.get("dependencies")
    if not isinstance(dependencies, list):
        errors.append(ValidationError(identity, "dependencies", "must be an array"))
    else:
        for index, dependency in enumerate(dependencies):
            _validate_dependency(errors, identity, index, dependency)

    security = _object(errors, identity, record, "security")
    _enum(
        errors,
        identity,
        security,
        "security_class",
        SECURITY_CLASSES,
        prefix="security.",
    )
    _require_nonempty(errors, identity, security, "data_class", prefix="security.")

    exemptions = record.get("exemptions")
    if not isinstance(exemptions, list):
        errors.append(ValidationError(identity, "exemptions", "must be an array"))
    else:
        for index, exemption in enumerate(exemptions):
            if not isinstance(exemption, dict):
                errors.append(
                    ValidationError(
                        identity, f"exemptions[{index}]", "must be an object"
                    )
                )
                continue
            for field in ("control", "reason", "evidence", "review_date"):
                _require_nonempty(
                    errors,
                    identity,
                    exemption,
                    field,
                    prefix=f"exemptions[{index}].",
                )

    review = _object(errors, identity, record, "review")
    _enum(errors, identity, review, "status", REVIEW_STATES, prefix="review.")
    for field in ("issue", "review_date", "reason"):
        _require_nonempty(errors, identity, review, field, prefix="review.")
    if (
        classification.get("lifecycle") == "unknown"
        or classification.get("profile") == "unknown"
    ) and review.get("status") != "needs-review":
        errors.append(
            ValidationError(
                identity,
                "review.status",
                "unknown classifications must remain in the review queue",
            )
        )

    conformance = _object(errors, identity, record, "conformance")
    _enum(
        errors,
        identity,
        conformance,
        "state",
        CONFORMANCE_STATES,
        prefix="conformance.",
    )
    for field in ("issue", "evidence_state"):
        _require_nonempty(errors, identity, conformance, field, prefix="conformance.")

    nix_oci = _object(errors, identity, record, "nix_oci")
    for field in (
        "classification",
        "evidence_state",
        "issue",
        "source_artifact_sha256",
    ):
        _require_nonempty(errors, identity, nix_oci, field, prefix="nix_oci.")
    if nix_oci.get("issue") != DEN369_ISSUE:
        errors.append(
            ValidationError(identity, "nix_oci.issue", f"must equal {DEN369_ISSUE}")
        )

    zed = _object(errors, identity, record, "zed")
    if not isinstance(zed.get("applicable"), bool):
        errors.append(ValidationError(identity, "zed.applicable", "must be a boolean"))
    _enum(errors, identity, zed, "state", ZED_STATES, prefix="zed.")
    if zed.get("applicable"):
        if zed.get("issue") != DEN637_ISSUE:
            errors.append(
                ValidationError(identity, "zed.issue", f"must equal {DEN637_ISSUE}")
            )
        for field in ("manifest", "lock", "source_pin", "ci_gate"):
            _enum(
                errors,
                identity,
                zed,
                field,
                EVIDENCE_STATES,
                prefix="zed.",
            )
    elif zed.get("state") != "not-applicable":
        errors.append(
            ValidationError(
                identity,
                "zed.state",
                "non-applicable repositories must use not-applicable",
            )
        )


def _validate_dependency(
    errors: list[ValidationError],
    identity: str,
    index: int,
    dependency: Any,
) -> None:
    field = f"dependencies[{index}]"
    if not isinstance(dependency, dict):
        errors.append(ValidationError(identity, field, "must be an object"))
        return
    for required in ("target", "kind", "state", "source_evidence"):
        if required not in dependency:
            errors.append(
                ValidationError(identity, f"{field}.{required}", "is required")
            )
    _enum(errors, identity, dependency, "kind", DEPENDENCY_KINDS, prefix=f"{field}.")
    _enum(errors, identity, dependency, "state", DEPENDENCY_STATES, prefix=f"{field}.")
    _require_nonempty(errors, identity, dependency, "target", prefix=f"{field}.")
    evidence = dependency.get("source_evidence")
    if not isinstance(evidence, dict):
        errors.append(
            ValidationError(identity, f"{field}.source_evidence", "must be an object")
        )
    else:
        for required in ("repository", "path", "immutable_ref"):
            _require_nonempty(
                errors,
                identity,
                evidence,
                required,
                prefix=f"{field}.source_evidence.",
            )
    if dependency.get("state") == "verified" and not dependency.get("pin"):
        errors.append(
            ValidationError(
                identity, f"{field}.pin", "verified dependencies require an exact pin"
            )
        )


def _object(
    errors: list[ValidationError],
    identity: str,
    parent: dict[str, Any],
    field: str,
) -> dict[str, Any]:
    value = parent.get(field)
    if not isinstance(value, dict):
        errors.append(ValidationError(identity, field, "must be an object"))
        return {}
    return value


def _require_nonempty(
    errors: list[ValidationError],
    identity: str,
    parent: Mapping[str, Any],
    field: str,
    *,
    prefix: str = "",
) -> None:
    if not isinstance(parent.get(field), str) or not parent[field].strip():
        errors.append(
            ValidationError(identity, f"{prefix}{field}", "must be a non-empty string")
        )


def _enum(
    errors: list[ValidationError],
    identity: str,
    parent: Mapping[str, Any],
    field: str,
    allowed: set[str],
    *,
    prefix: str = "",
) -> None:
    value = parent.get(field)
    if value not in allowed:
        errors.append(
            ValidationError(
                identity,
                f"{prefix}{field}",
                f"must be one of {sorted(allowed)}",
            )
        )


def index_repositories(data: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {
        record["name"]: record
        for record in data["repositories"]
        if isinstance(record, dict) and isinstance(record.get("name"), str)
    }


def diff_catalogs(baseline: dict[str, Any], current: dict[str, Any]) -> dict[str, Any]:
    old = index_repositories(baseline)
    new = index_repositories(current)
    shared = sorted(old.keys() & new.keys())
    report: dict[str, Any] = {
        "schema_version": 1,
        "baseline_snapshot": baseline.get("snapshot", {}).get("id", "unknown"),
        "current_snapshot": current.get("snapshot", {}).get("id", "unknown"),
        "added": sorted(new.keys() - old.keys()),
        "removed": sorted(old.keys() - new.keys()),
        "ownership_moves": [],
        "default_branch_changes": [],
        "pin_drift": [],
        "conformance_regressions": [],
        "classification_changes": [],
        "zed_drift": [],
        "inventory_changes": [],
    }
    baseline_inventory = baseline.get("inventory", {})
    current_inventory = current.get("inventory", {})
    for field in (
        "repository_count",
        "public_count",
        "private_count",
        "fork_count",
        "archived_count",
        "empty_count",
        "canonical_active_count",
        "owner_count",
    ):
        before_value = baseline_inventory.get(field)
        after_value = current_inventory.get(field)
        if before_value != after_value:
            report["inventory_changes"].append(
                {
                    "name": field,
                    "before": before_value,
                    "after": after_value,
                    "delta": (
                        after_value - before_value
                        if isinstance(before_value, int)
                        and isinstance(after_value, int)
                        else None
                    ),
                }
            )
    for name in shared:
        before = old[name]
        after = new[name]
        if before.get("canonical_location") != after.get(
            "canonical_location"
        ) or before.get("ownership") != after.get("ownership"):
            report["ownership_moves"].append(
                {
                    "name": name,
                    "before": {
                        "canonical_location": before.get("canonical_location"),
                        "ownership": before.get("ownership"),
                    },
                    "after": {
                        "canonical_location": after.get("canonical_location"),
                        "ownership": after.get("ownership"),
                    },
                }
            )
        if before.get("default_branch") != after.get("default_branch"):
            report["default_branch_changes"].append(
                {
                    "name": name,
                    "before": before.get("default_branch"),
                    "after": after.get("default_branch"),
                }
            )
        if before.get("classification") != after.get("classification"):
            report["classification_changes"].append(
                {
                    "name": name,
                    "before": before.get("classification"),
                    "after": after.get("classification"),
                }
            )
        report["pin_drift"].extend(_dependency_drift(name, before, after))
        before_state = before.get("conformance", {}).get("state", "unknown")
        after_state = after.get("conformance", {}).get("state", "unknown")
        if CONFORMANCE_RANK.get(after_state, 0) < CONFORMANCE_RANK.get(before_state, 0):
            report["conformance_regressions"].append(
                {"name": name, "before": before_state, "after": after_state}
            )
        if before.get("zed") != after.get("zed"):
            report["zed_drift"].append(
                {"name": name, "before": before.get("zed"), "after": after.get("zed")}
            )
    report["summary"] = {
        key: len(value) for key, value in report.items() if isinstance(value, list)
    }
    return report


def _dependency_drift(
    repository: str,
    before: dict[str, Any],
    after: dict[str, Any],
) -> list[dict[str, Any]]:
    def dependency_index(
        record: dict[str, Any],
    ) -> dict[tuple[str, str], dict[str, Any]]:
        return {
            (item.get("kind", ""), item.get("target", "")): item
            for item in record.get("dependencies", [])
            if isinstance(item, dict)
        }

    old = dependency_index(before)
    new = dependency_index(after)
    changes: list[dict[str, Any]] = []
    for key in sorted(old.keys() | new.keys()):
        old_value = old.get(key)
        new_value = new.get(key)
        old_pin = old_value.get("pin") if old_value else None
        new_pin = new_value.get("pin") if new_value else None
        old_state = old_value.get("state") if old_value else None
        new_state = new_value.get("state") if new_value else None
        if old_pin != new_pin or old_state != new_state:
            changes.append(
                {
                    "name": repository,
                    "kind": key[0],
                    "target": key[1],
                    "before": {"pin": old_pin, "state": old_state},
                    "after": {"pin": new_pin, "state": new_state},
                }
            )
    return changes


def render_drift_markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Repository catalog drift",
        "",
        f"- Baseline: `{report['baseline_snapshot']}`",
        f"- Current: `{report['current_snapshot']}`",
        "",
    ]
    sections = (
        ("Added", "added"),
        ("Removed", "removed"),
        ("Ownership moves", "ownership_moves"),
        ("Default branch changes", "default_branch_changes"),
        ("Pin drift", "pin_drift"),
        ("Conformance regressions", "conformance_regressions"),
        ("Classification changes", "classification_changes"),
        ("Zed package drift", "zed_drift"),
        ("Inventory changes", "inventory_changes"),
    )
    for title, key in sections:
        lines.append(f"## {title}")
        values = report[key]
        if not values:
            lines.append("- None")
        else:
            for value in values:
                if isinstance(value, str):
                    lines.append(f"- `{value}`")
                else:
                    lines.append(
                        f"- `{value['name']}`: `{json.dumps(value, sort_keys=True)}`"
                    )
        lines.append("")
    return "\n".join(lines)


def build_dashboard(catalog: dict[str, Any]) -> dict[str, Any]:
    groups: dict[tuple[str, str, str], list[dict[str, Any]]] = defaultdict(list)
    actions: list[dict[str, str]] = []
    for record in catalog["repositories"]:
        owner = record["name"].split("/", 1)[0]
        ownership = record["ownership"]
        key = (
            owner,
            ownership["linear_project"],
            ownership["linear_issue"],
        )
        groups[key].append(record)
        conformance_state = record["conformance"]["state"]
        review_state = record["review"]["status"]
        zed_state = record["zed"]["state"]
        if (
            conformance_state not in {"conformant", "exempt"}
            or review_state != "reviewed"
        ):
            actions.append(
                {
                    "repository": record["name"],
                    "issue": ownership["linear_issue"],
                    "issue_url": ownership["linear_issue_url"],
                    "reason": record["review"]["reason"],
                }
            )
        if record["zed"]["applicable"] and zed_state not in {"conformant", "exempt"}:
            actions.append(
                {
                    "repository": record["name"],
                    "issue": DEN637_ISSUE,
                    "issue_url": "https://linear.app/denman/issue/DEN-637",
                    "reason": "client/SDK Zed package evidence is incomplete",
                }
            )

    owners: list[dict[str, Any]] = []
    for (owner, project, issue), records in sorted(groups.items()):
        owners.append(
            {
                "owner": owner,
                "linear_project": project,
                "linear_issue": issue,
                "repositories": len(records),
                "conformance": dict(
                    sorted(
                        Counter(
                            item["conformance"]["state"] for item in records
                        ).items()
                    )
                ),
                "needs_review": sum(
                    item["review"]["status"] != "reviewed" for item in records
                ),
                "zed_gaps": sum(
                    item["zed"]["applicable"]
                    and item["zed"]["state"] not in {"conformant", "exempt"}
                    for item in records
                ),
            }
        )
    return {
        "schema_version": 1,
        "snapshot": catalog["snapshot"]["id"],
        "record_scope": catalog["snapshot"]["record_scope"],
        "inventory": catalog["inventory"],
        "owners": owners,
        "actions": sorted(
            actions, key=lambda item: (item["issue"], item["repository"])
        ),
    }


def render_dashboard_markdown(dashboard: dict[str, Any]) -> str:
    lines = [
        "# Repository conformance dashboard",
        "",
        f"Snapshot: `{dashboard['snapshot']}` ({dashboard['record_scope']} records)",
        "",
        f"Inventory total: `{dashboard['inventory']['repository_count']}`; "
        f"public records: `{dashboard['inventory']['public_count']}`; "
        f"private aggregate: `{dashboard['inventory']['private_count']}`",
        "",
        "| Owner | Linear project | Repositories | Needs review | Zed gaps | Conformance |",
        "|---|---|---:|---:|---:|---|",
    ]
    for owner in dashboard["owners"]:
        conformance = ", ".join(
            f"{state}: {count}" for state, count in owner["conformance"].items()
        )
        issue_url = f"https://linear.app/denman/issue/{owner['linear_issue']}"
        lines.append(
            f"| {owner['owner']} | {owner['linear_project']} "
            f"([{owner['linear_issue']}]({issue_url})) | {owner['repositories']} | "
            f"{owner['needs_review']} | {owner['zed_gaps']} | {conformance} |"
        )
    lines.extend(["", "## Actionable gaps", ""])
    if not dashboard["actions"]:
        lines.append("- None")
    else:
        for action in dashboard["actions"]:
            lines.append(
                f"- `{action['repository']}` — [{action['issue']}]({action['issue_url']}): "
                f"{action['reason']}"
            )
    lines.append("")
    return "\n".join(lines)


def merge_den369(
    catalog: dict[str, Any],
    report: Sequence[dict[str, Any]],
    *,
    source_path: str,
    artifact_sha256: str,
) -> dict[str, Any]:
    result = copy.deepcopy(catalog)
    indexed = {
        item.get("repository"): item
        for item in report
        if isinstance(item, dict) and isinstance(item.get("repository"), str)
    }
    result.setdefault("imports", {})["den369"] = {
        "issue": DEN369_ISSUE,
        "contract": "nix-fleet-audit/report.json@v1",
        "source_path": source_path,
        "artifact_sha256": artifact_sha256,
    }
    for record in result["repositories"]:
        imported = indexed.get(record["name"])
        if imported:
            record["nix_oci"] = {
                "issue": DEN369_ISSUE,
                "classification": imported.get("classification", "unknown"),
                "evidence_state": "present-unverified",
                "reason": imported.get("reason", ""),
                "nix": imported.get("nix", {}),
                "container": imported.get("container", {}),
                "source_artifact_sha256": artifact_sha256,
            }
        else:
            record["nix_oci"] = {
                "issue": DEN369_ISSUE,
                "classification": "not-collected",
                "evidence_state": "missing",
                "reason": "DEN-369 artifact has no record for this repository",
                "nix": {},
                "container": {},
                "source_artifact_sha256": artifact_sha256,
            }
    return result


def load_den369_report(path: Path) -> list[dict[str, Any]]:
    value = load_json(path)
    if isinstance(value, dict):
        value = value.get("repositories")
    if not isinstance(value, list) or not all(isinstance(item, dict) for item in value):
        raise ValueError(
            "DEN-369 report must be an array or an object with repositories"
        )
    return value


def load_owner_contract(path: Path) -> dict[str, Any]:
    value = load_json(path)
    if not isinstance(value, dict) or not isinstance(value.get("owners"), list):
        raise ValueError("owner contract must be an object with an owners array")
    return value


def collect_catalog(
    owner_contract: dict[str, Any],
    *,
    captured_at: str,
    visibility_mode: str,
    gh_binary: str = "gh",
) -> dict[str, Any]:
    owner_index = {
        item["owner"]: item
        for item in owner_contract["owners"]
        if isinstance(item, dict) and isinstance(item.get("owner"), str)
    }
    command = [
        gh_binary,
        "api",
        "--paginate",
        "--slurp",
        "--method",
        "GET",
        "/user/repos?per_page=100&affiliation=owner,collaborator,organization_member"
        "&sort=full_name&direction=asc",
    ]
    completed = subprocess.run(
        command,
        check=True,
        capture_output=True,
        text=True,
    )
    pages = json.loads(completed.stdout)
    if not isinstance(pages, list):
        raise ValueError("GitHub response must be an array of pages")
    raw_records = [
        repository
        for page in pages
        if isinstance(page, list)
        for repository in page
        if isinstance(repository, dict)
        and repository.get("owner", {}).get("login") in owner_index
    ]
    raw_records.sort(key=lambda item: item["full_name"].casefold())
    records = [
        _catalog_record(
            repository, owner_index[repository["owner"]["login"]], captured_at
        )
        for repository in raw_records
        if visibility_mode == "full" or repository.get("visibility") == "public"
    ]
    total = len(raw_records)
    public_count = sum(item.get("visibility") == "public" for item in raw_records)
    private_count = total - public_count
    fork_count = sum(bool(item.get("fork", False)) for item in raw_records)
    archived_count = sum(bool(item.get("archived", False)) for item in raw_records)
    empty_count = sum(item.get("size", 0) == 0 for item in raw_records)
    canonical_active_count = sum(
        not item.get("fork", False)
        and not item.get("archived", False)
        and item.get("size", 0) != 0
        for item in raw_records
    )
    baseline = owner_contract["baseline"]
    baseline_deltas = {
        "repository_count": total - baseline["repository_count"],
        "public_count": public_count - baseline["public_count"],
        "private_count": private_count - baseline["private_count"],
        "fork_count": fork_count - baseline["fork_count"],
        "archived_count": archived_count - baseline["archived_count"],
        "empty_count": empty_count - baseline["empty_count"],
        "canonical_active_count": (
            canonical_active_count - baseline["canonical_active_count"]
        ),
        "owner_count": len(owner_index) - len(owner_contract["owners"]),
    }
    return {
        "schema_version": SCHEMA_VERSION,
        "snapshot": {
            "id": captured_at[:10],
            "captured_at": captured_at,
            "record_scope": visibility_mode,
            "source": "read-only GitHub user repository API filtered to the 24 installed owners",
            "governing_issue": "DEN-627",
        },
        "inventory": {
            "repository_count": total,
            "public_count": public_count,
            "private_count": private_count,
            "fork_count": fork_count,
            "archived_count": archived_count,
            "empty_count": empty_count,
            "canonical_active_count": canonical_active_count,
            "owner_count": len(owner_index),
            "baseline_repository_count": baseline["repository_count"],
            "baseline_delta": total - baseline["repository_count"],
            "baseline_deltas": baseline_deltas,
        },
        "imports": {
            "den369": {
                "issue": DEN369_ISSUE,
                "contract": "nix-fleet-audit/report.json@v1",
                "source_path": "not-imported",
                "artifact_sha256": "not-imported",
            }
        },
        "repositories": records,
    }


def _catalog_record(
    repository: dict[str, Any],
    owner: dict[str, Any],
    captured_at: str,
) -> dict[str, Any]:
    name = repository["full_name"]
    repo_name = repository["name"]
    visibility = repository.get("visibility", "private")
    fork = bool(repository.get("fork", False))
    archived = bool(repository.get("archived", False))
    empty = repository.get("size", 0) == 0
    lifecycle, profile, source = _classify(
        repo_name, fork=fork, archived=archived, empty=empty
    )
    client_sdk = _is_client_sdk(repo_name, profile)
    review_reason = "metadata-derived classification requires owner review; no behavior is marked verified"
    return {
        "name": name,
        "repository_id": repository.get("id", "unknown"),
        "visibility": visibility,
        "fork": fork,
        "archived": archived,
        "empty": empty,
        "default_branch": repository.get("default_branch"),
        "canonical_location": name,
        "classification": {
            "lifecycle": lifecycle,
            "profile": profile,
            "evidence_state": "present-unverified",
            "source": source,
        },
        "ownership": {
            "linear_project": owner["linear_project"],
            "linear_issue": owner["linear_issue"],
            "linear_issue_url": f"https://linear.app/denman/issue/{owner['linear_issue']}",
        },
        "release": {
            "state": "unknown",
            "deployment_state": "unknown",
        },
        "consumers": [],
        "dependencies": [],
        "security": {
            "security_class": "public" if visibility == "public" else "confidential",
            "data_class": "unknown-needs-owner-review",
        },
        "exemptions": [],
        "review": {
            "status": "needs-review",
            "issue": owner["linear_issue"],
            "review_date": captured_at[:10],
            "reason": review_reason,
        },
        "conformance": {
            "state": "gap-owned",
            "issue": owner["linear_issue"],
            "evidence_state": "present-unverified",
        },
        "nix_oci": {
            "issue": DEN369_ISSUE,
            "classification": "not-collected",
            "evidence_state": "missing",
            "reason": "awaiting DEN-369 artifact import",
            "nix": {},
            "container": {},
            "source_artifact_sha256": "not-imported",
        },
        "zed": {
            "applicable": client_sdk,
            "state": "gap-owned" if client_sdk else "not-applicable",
            "issue": DEN637_ISSUE if client_sdk else "",
            **(
                {
                    "manifest": "unknown",
                    "lock": "unknown",
                    "source_pin": "unknown",
                    "ci_gate": "unknown",
                }
                if client_sdk
                else {}
            ),
        },
    }


def _classify(
    name: str,
    *,
    fork: bool,
    archived: bool,
    empty: bool,
) -> tuple[str, str, str]:
    lowered = name.casefold()
    if archived:
        return "archived", "unknown", "GitHub archived metadata"
    if empty:
        return "empty-reserved", "unknown", "GitHub size metadata; tree review required"
    if fork:
        return "active", "upstream fork", "GitHub fork metadata"

    profile_patterns: tuple[tuple[tuple[str, ...], str], ...] = (
        (("mcp-server",), "MCP/automation boundary"),
        (
            ("-infra", "gitops", "k8s", "kubernetes", "terraform"),
            "infrastructure/GitOps repository",
        ),
        (("monorepo",), "integration monorepo"),
        (("interface", "schema", "specification"), "interface/schema/specification"),
        (("-sync", "syncer"), "sync/local-first engine"),
        (("-e2e", "conformance", "fixture"), "E2E/conformance fixture"),
        (
            (".github.io", "website", "-site", "-docs", "marketing"),
            "website/documentation",
        ),
        (
            (
                "-client",
                "clients",
                "-ui.",
                "flutter",
                "desktop",
                "mobile",
                "chrome-extension",
            ),
            "client/mobile/desktop/web application",
        ),
        (("server", "backend", "worker", "service"), "deployed service or worker"),
        (("library", "lib", "sdk", "cli", "tool"), "shared library/SDK/tool"),
        (("demo", "example", "poc", "benchmark"), "research/demo"),
    )
    for patterns, profile in profile_patterns:
        if any(pattern in lowered for pattern in patterns):
            return "active", profile, f"repository-name heuristic: {patterns}"
    return "active", "unknown", "unclassified repository name; owner review required"


def _is_client_sdk(name: str, profile: str) -> bool:
    lowered = name.casefold()
    return (
        any(token in lowered for token in ("client", "sdk"))
        or profile == "client/mobile/desktop/web application"
    )


def _private_output_is_safe(output: Path, repo_root: Path) -> bool:
    try:
        output.resolve().relative_to(repo_root.resolve())
    except ValueError:
        return True
    return False


def _default_captured_at() -> str:
    return (
        datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def _write_text(path: Path | None, text: str) -> None:
    if path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    else:
        print(text, end="")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate", help="validate a catalog")
    validate.add_argument("catalog", type=Path)
    validate.add_argument("--public-safe", action="store_true")
    validate.add_argument("--repo-root", type=Path)

    diff = subparsers.add_parser("diff", help="compare catalog snapshots")
    diff.add_argument("baseline", type=Path)
    diff.add_argument("current", type=Path)
    diff.add_argument("--json-output", type=Path)
    diff.add_argument("--markdown-output", type=Path)

    dashboard = subparsers.add_parser(
        "dashboard", help="render owner/project dashboard"
    )
    dashboard.add_argument("catalog", type=Path)
    dashboard.add_argument("--json-output", type=Path)
    dashboard.add_argument("--markdown-output", type=Path)

    merge = subparsers.add_parser("merge-den369", help="consume a DEN-369 report")
    merge.add_argument("catalog", type=Path)
    merge.add_argument("report", type=Path)
    merge.add_argument("--source-path", required=True)
    merge.add_argument("--output", required=True, type=Path)

    collect = subparsers.add_parser("collect", help="collect read-only GitHub metadata")
    collect.add_argument("--owners", required=True, type=Path)
    collect.add_argument("--output", required=True, type=Path)
    collect.add_argument("--visibility", choices=("public", "full"), default="public")
    collect.add_argument("--allow-private-output", action="store_true")
    collect.add_argument("--repo-root", type=Path, default=Path.cwd())
    collect.add_argument("--captured-at", default=_default_captured_at())
    collect.add_argument("--gh-binary", default="gh")

    args = parser.parse_args(argv)
    try:
        if args.command == "validate":
            catalog = load_catalog(args.catalog)
            errors = validate_catalog(
                catalog,
                public_safe=args.public_safe,
                repo_root=args.repo_root,
            )
            if errors:
                for error in errors:
                    print(error.render(), file=sys.stderr)
                return 1
            print(f"validated {len(catalog['repositories'])} repository records")
            return 0

        if args.command == "diff":
            report = diff_catalogs(
                load_catalog(args.baseline), load_catalog(args.current)
            )
            if args.json_output:
                write_json(args.json_output, report)
            else:
                print(json.dumps(report, indent=2, sort_keys=True))
            _write_text(args.markdown_output, render_drift_markdown(report))
            return 0

        if args.command == "dashboard":
            value = build_dashboard(load_catalog(args.catalog))
            if args.json_output:
                write_json(args.json_output, value)
            else:
                print(json.dumps(value, indent=2, sort_keys=True))
            _write_text(args.markdown_output, render_dashboard_markdown(value))
            return 0

        if args.command == "merge-den369":
            report = load_den369_report(args.report)
            value = merge_den369(
                load_catalog(args.catalog),
                report,
                source_path=args.source_path,
                artifact_sha256=sha256_file(args.report),
            )
            write_json(args.output, value)
            return 0

        if args.command == "collect":
            if args.visibility == "full":
                if not args.allow_private_output:
                    raise ValueError("full collection requires --allow-private-output")
                if not _private_output_is_safe(args.output, args.repo_root):
                    raise ValueError(
                        "full collection output must be outside the repository working tree"
                    )
            value = collect_catalog(
                load_owner_contract(args.owners),
                captured_at=args.captured_at,
                visibility_mode=args.visibility,
                gh_binary=args.gh_binary,
            )
            errors = validate_catalog(value, public_safe=args.visibility == "public")
            if errors:
                raise ValueError("; ".join(error.render() for error in errors))
            write_json(args.output, value)
            print(
                f"wrote {len(value['repositories'])} {args.visibility} records; "
                f"inventory total={value['inventory']['repository_count']}",
                file=sys.stderr,
            )
            return 0
    except (
        OSError,
        ValueError,
        json.JSONDecodeError,
        subprocess.CalledProcessError,
    ) as exc:
        print(f"repository catalog command failed: {exc}", file=sys.stderr)
        return 2
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
